//! Read pipeline: `threads` workers take chunks of one layer from the chunk
//! stream, parse them and build batches; the consumer puts the batches back
//! in chunk order (if asked) and combines the small batches that end chunks
//! into `batch_size` ones. With `threads = 1` it all runs on the consumer's
//! thread (`xeibe_core::parallel`).
//!
//! Memory stays bounded however slow the consumer is: at most
//! `queue_depth + threads` chunks are between the chunk stream and the
//! consumer (being parsed, or their batches waiting to be reordered).

use std::collections::VecDeque;
use std::io::Read;
use std::sync::{Arc, Mutex};

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use arrow_select::coalesce::BatchCoalescer;
use xeibe_core::decode::DecodeAhead;
use xeibe_core::parallel::{Helper, MapOptions, ParallelMap, panic_message};
use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_core::{
    FeatureChunk, FeatureSplitter, Location, NamespaceContext, QName, SourceId, Sources, ns,
};
use xeibe_geom::options::GeometryOptions;
use xeibe_geom::{GeometryParser, ParseContext};

use crate::axis::{AxisDecisions, SharedContexts};
use crate::builders::LayerBatchBuilder;
use crate::feature::{FeatureOutcome, FeatureReader};
use crate::report::{ReadReport, Warning, WarningKind};
use crate::route::RouteTree;
use crate::{OnFeatureError, ReadOptions};

/// Everything a worker needs to turn chunks of one layer into batches.
pub struct ReadPlan {
    /// The layer name as the caller gave it (report key).
    pub layer_name: String,
    pub selector: LayerSelector,
    /// The output schema: the projection of the read's schema.
    pub schema: SchemaRef,
    pub routes: RouteTree,
    /// One per geometry column, in `routes.geometry_columns` order.
    pub axis: Vec<AxisDecisions>,
    pub has_geometry: bool,
    pub geometry: GeometryOptions,
    pub on_feature_error: OnFeatureError,
    pub strip_local_href_hash: bool,
    pub empty_as_null: bool,
    pub batch_size: usize,
}

impl ReadPlan {
    /// The applied axis decisions and the warnings about them.
    pub fn axis_report(
        &self,
    ) -> (
        Vec<(xeibe_geom::AxisKey, xeibe_geom::AxisDecision)>,
        Vec<Warning>,
    ) {
        let mut decisions: Vec<(xeibe_geom::AxisKey, xeibe_geom::AxisDecision)> = Vec::new();
        let mut warnings = Vec::new();
        for axis in &self.axis {
            for (key, decision) in axis.decisions() {
                if !decisions.iter().any(|(k, _)| *k == key) {
                    decisions.push((key, decision));
                }
            }
            for warning in axis.warnings() {
                if !warnings
                    .iter()
                    .any(|w: &Warning| w.message == warning.message)
                {
                    warnings.push(warning);
                }
            }
        }
        decisions.sort_by(|a, b| a.0.cmp(&b.0));
        (decisions, warnings)
    }
}

/// The layer a read asks for: a Clark name (exact), `prefix:local` (the
/// prefix resolved per chunk), or a local name (any namespace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerSelector {
    Exact(QName),
    Prefixed { prefix: String, local: String },
    Local(String),
}

impl LayerSelector {
    pub fn parse(layer: &str) -> Self {
        if let Some((uri, local)) = layer
            .strip_prefix('{')
            .and_then(|rest| rest.split_once('}'))
        {
            return LayerSelector::Exact(QName::new(Some(uri), local));
        }
        match layer.split_once(':') {
            Some((prefix, local)) => LayerSelector::Prefixed {
                prefix: prefix.into(),
                local: local.into(),
            },
            None => LayerSelector::Local(layer.into()),
        }
    }

    /// The splitter's filter entry: exact, or the local name in any namespace.
    pub fn filter(&self) -> QName {
        match self {
            LayerSelector::Exact(name) => name.clone(),
            LayerSelector::Prefixed { local, .. } | LayerSelector::Local(local) => {
                QName::new(None, local)
            }
        }
    }

    pub fn accepts(&self, name: &QName, namespaces: &NamespaceContext) -> bool {
        match self {
            LayerSelector::Exact(exact) => exact == name,
            LayerSelector::Local(local) => &*name.local == local,
            LayerSelector::Prefixed { prefix, local } => {
                &*name.local == local
                    && namespaces
                        .resolve_prefix(Some(prefix))
                        .is_none_or(|uri| name.ns.as_deref() == Some(uri))
            }
        }
    }
}

/// Chunks of one layer over every source, one source after another, with
/// sequence numbers that run on across sources.
///
/// Each source gets `SourceId(i)` in order and one axis context at index `i`.
/// A source in which the splitter finds no feature collection or feature
/// member (an ISO metadata member of a zip, say) is skipped with a warning;
/// if every source is, the stream ends with [`xeibe_core::Error::NoFeatures`].
pub struct ChunkStream {
    sources: Sources,
    options: xeibe_core::SplitterOptions,
    /// The source being split, and its name.
    current: Option<(FeatureSplitter<Box<dyn Read + Send>>, String)>,
    /// Sources split to the end, and the names of those skipped.
    read_sources: usize,
    skipped: Vec<String>,
    next_source: u32,
    next_seq: u64,
    contexts: SharedContexts,
    warnings: Arc<Mutex<Vec<Warning>>>,
    finished: bool,
    /// Decoding ahead of the source being split.
    ahead: Arc<DecodeAhead>,
}

impl ChunkStream {
    pub fn new(
        sources: Sources,
        options: xeibe_core::SplitterOptions,
        contexts: SharedContexts,
    ) -> Self {
        ChunkStream {
            sources,
            options,
            current: None,
            read_sources: 0,
            skipped: Vec::new(),
            next_source: 0,
            next_seq: 0,
            contexts,
            warnings: Arc::new(Mutex::new(Vec::new())),
            finished: false,
            ahead: Arc::default(),
        }
    }

    /// What workers waiting for the stream can do meanwhile: decode the
    /// current source ahead of the splitter.
    pub fn helper(&self) -> Helper {
        let ahead = self.ahead.clone();
        Arc::new(move || ahead.help())
    }

    /// Warnings about skipped sources, shared with the read report.
    pub fn warnings(&self) -> Arc<Mutex<Vec<Warning>>> {
        self.warnings.clone()
    }

    /// Open the next source; `Ok(false)` when there is none.
    fn open_next(&mut self) -> xeibe_core::Result<bool> {
        let Some(source) = self.sources.next() else {
            return Ok(false);
        };
        let source = source?;
        let id = SourceId(self.next_source);
        self.next_source += 1;
        let mut splitter =
            FeatureSplitter::new(source.open_ahead(&self.ahead)?, id, self.options.clone());
        let context = match splitter.header() {
            Ok(header) => Some(xeibe_schema::scan::source_context(header)),
            Err(xeibe_core::Error::NoFeatures(_)) => None,
            Err(error) => return Err(error),
        };
        // One context per source id, also for a skipped source.
        lock(&self.contexts).push(context.clone().unwrap_or_default());
        match context {
            Some(_) => self.current = Some((splitter, source.name())),
            None => self.skip(source.name()),
        }
        Ok(true)
    }

    /// A source without a feature collection or feature member.
    fn skip(&mut self, name: String) {
        lock(&self.warnings).push(Warning {
            kind: WarningKind::SkippedSource,
            location: None,
            message: format!("skipped {name}: no feature collection or feature member"),
        });
        self.skipped.push(name);
    }
}

impl Iterator for ChunkStream {
    type Item = xeibe_core::Result<FeatureChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.finished {
            if let Some((splitter, _)) = &mut self.current {
                match splitter.next() {
                    Some(Ok(mut chunk)) => {
                        chunk.seq = self.next_seq;
                        self.next_seq += 1;
                        return Some(Ok(chunk));
                    }
                    // Found at the end of the input: the root was read, but
                    // nothing in it holds features.
                    Some(Err(xeibe_core::Error::NoFeatures(_))) => {
                        let (_, name) = self.current.take().expect("a current source");
                        self.skip(name);
                    }
                    Some(Err(error)) => {
                        self.finished = true;
                        return Some(Err(error));
                    }
                    None => {
                        self.read_sources += 1;
                        let referenced = splitter.referenced_members();
                        if referenced > 0 {
                            lock(&self.warnings).push(Warning {
                                kind: WarningKind::ReferencedMember,
                                location: None,
                                message: format!(
                                    "{referenced} feature members given by reference (not read)"
                                ),
                            });
                        }
                        self.current = None;
                    }
                }
            }
            match self.open_next() {
                Ok(true) => {}
                Ok(false) => {
                    self.finished = true;
                    if self.read_sources == 0 && !self.skipped.is_empty() {
                        return Some(Err(xeibe_core::Error::no_features_in(&self.skipped)));
                    }
                }
                Err(error) => {
                    self.finished = true;
                    return Some(Err(error));
                }
            }
        }
        None
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct Pipeline {
    options: ReadOptions,
    plan: Arc<ReadPlan>,
    report: Arc<Mutex<ReadReport>>,
}

impl Pipeline {
    pub fn new(options: ReadOptions, plan: Arc<ReadPlan>, report: Arc<Mutex<ReadReport>>) -> Self {
        Pipeline {
            options,
            plan,
            report,
        }
    }

    /// Start the workers. `chunks` already contains only this layer's features
    /// (splitter layer filter), starting with any buffered sample chunks.
    /// `helper`: what a worker can do while another takes a chunk.
    pub fn start(
        self,
        chunks: Box<dyn Iterator<Item = xeibe_core::Result<FeatureChunk>> + Send>,
        helper: Option<Helper>,
    ) -> crate::Result<Batches> {
        let threads = self.options.threads.max(1);
        let options = MapOptions {
            threads,
            in_flight: self.options.queue_depth.max(1) + threads,
            preserve_order: self.options.preserve_order,
            name: "xeibe-read".into(),
            helper,
        };
        // Batches of at least half the size pass through uncopied.
        let batch_size = self.plan.batch_size;
        let coalescer = BatchCoalescer::new(self.plan.schema.clone(), batch_size)
            .with_biggest_coalesce_batch_size(Some(batch_size / 2));
        let pipeline = Arc::new(self);
        let chunks = ParallelMap::new(options, chunks, move |chunk| {
            pipeline.process_chunk(&chunk?)
        })
        .map_err(xeibe_core::Error::from)?;
        Ok(Batches {
            chunks: Some(chunks),
            coalescer,
            ready: VecDeque::new(),
            chunk_index: 0,
        })
    }

    fn process_chunk(&self, chunk: &FeatureChunk) -> crate::Result<Vec<RecordBatch>> {
        let plan = &*self.plan;
        let axis = plan
            .axis
            .iter()
            .map(|axis| axis.for_source(chunk.source))
            .collect();
        let inherited = match chunk.collection_bounded_by.as_deref() {
            Some(raw) if plan.has_geometry => xeibe_geom::parse::collection_bounded_by(raw)?.1,
            _ => ParseContext::default(),
        };
        let features = FeatureReader::new(plan, GeometryParser::new(&plan.geometry), axis)
            .with_inherited(inherited);
        let mut builder = LayerBatchBuilder::new(plan.schema.clone(), plan.batch_size.min(4096))?;
        let mut report = ReadReport::default();
        let mut batches = Vec::new();
        let mut reader = GmlReader::new(&chunk.bytes, &chunk.namespaces, chunk.byte_offset)
            .with_source(chunk.source);
        let mut index = 0u64;
        let mut rows = 0u64;
        loop {
            match reader.next_event()? {
                XmlEvent::Start { name, attrs } => {
                    if !plan.selector.accepts(&name, &chunk.namespaces) {
                        reader.skip_element()?;
                        continue;
                    }
                    let gml_id = attrs
                        .iter_raw()
                        .find(|(namespace, local, _)| {
                            *local == "id" && matches!(*namespace, Some(ns::GML | ns::GML_32))
                        })
                        .map(|(_, _, value)| value.into_owned());
                    let location = Location {
                        source: chunk.source,
                        byte_offset: reader.location().byte_offset,
                        feature_seq: Some(chunk.first_feature_seq + index),
                        gml_id,
                    };
                    index += 1;
                    if features.read_feature(&mut reader, location, &mut builder, &mut report)?
                        == FeatureOutcome::Row
                    {
                        rows += 1;
                    }
                    if builder.len() >= plan.batch_size {
                        batches.push(builder.finish()?);
                    }
                }
                XmlEvent::Text(_) | XmlEvent::End { .. } => {}
                XmlEvent::Eof => break,
            }
        }
        if !builder.is_empty() {
            batches.push(builder.finish()?);
        }
        *report
            .features_per_layer
            .entry(plan.layer_name.clone())
            .or_default() += rows;
        lock(&self.report).merge(report);
        Ok(batches)
    }
}

/// The output of a read: the chunks' batches, in chunk order if asked,
/// combined into batches of the target size. Ends after the first error; the
/// rows before it still come out first.
pub struct Batches {
    /// `None` once the read has ended (dropping it stops the workers).
    chunks: Option<ParallelMap<crate::Result<Vec<RecordBatch>>>>,
    coalescer: BatchCoalescer,
    ready: VecDeque<crate::Result<RecordBatch>>,
    /// Chunks taken so far, for messages.
    chunk_index: u64,
}

impl Batches {
    /// Queue the coalescer's completed batches, or the error of the step that
    /// completed them; `false` after an error.
    fn take_completed(&mut self, step: Result<(), arrow_schema::ArrowError>) -> bool {
        if let Err(error) = step {
            self.end_with(error.into());
            return false;
        }
        while let Some(batch) = self.coalescer.next_completed_batch() {
            self.ready.push_back(Ok(batch));
        }
        true
    }

    /// The rows before the error first, then the error.
    fn end_with(&mut self, error: crate::Error) {
        self.chunks = None;
        let step = self.coalescer.finish_buffered_batch();
        if self.take_completed(step) {
            self.ready.push_back(Err(error));
        }
    }
}

impl Iterator for Batches {
    type Item = crate::Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(batch) = self.ready.pop_front() {
                return Some(batch);
            }
            let chunks = self.chunks.as_mut()?;
            let index = self.chunk_index;
            self.chunk_index += 1;
            match chunks.next() {
                Some(Ok(Ok(batches))) => {
                    for batch in batches {
                        let step = self.coalescer.push_batch(batch);
                        if !self.take_completed(step) {
                            break;
                        }
                    }
                }
                Some(Ok(Err(error))) => self.end_with(error),
                // A panic becomes an error in its place in the stream.
                Some(Err(panic)) => self.end_with(
                    arrow_schema::ArrowError::ComputeError(format!(
                        "reading chunk {index} panicked: {}",
                        panic_message(&panic)
                    ))
                    .into(),
                ),
                None => {
                    self.chunks = None;
                    let step = self.coalescer.finish_buffered_batch();
                    self.take_completed(step);
                }
            }
        }
    }
}
