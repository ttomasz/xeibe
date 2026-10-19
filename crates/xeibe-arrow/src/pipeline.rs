//! Read pipeline: `threads` workers take chunks of one layer from the
//! sources (several split at once when one can't keep them busy), parse
//! them and build batches; the consumer puts the batches back
//! in chunk order (if asked) and combines the small batches that end chunks
//! into `batch_size` ones. With `threads = 1` it all runs on the consumer's
//! thread (`xeibe_core::parallel`).
//!
//! Memory stays bounded however slow the consumer is: at most
//! `queue_depth + threads` chunks are between the chunk stream and the
//! consumer (being parsed, or their batches waiting to be reordered).

use std::collections::VecDeque;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arrow_array::{ArrayRef, RecordBatch, StructArray, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Fields, SchemaRef};
use arrow_select::coalesce::BatchCoalescer;
use xeibe_core::decode::DecodeAhead;
use xeibe_core::parallel::{
    Helper, MapOptions, ParallelMap, Streams, max_open_streams, panic_message,
};
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
    /// The output schema: the projection of the read's schema, and the
    /// location column if asked for.
    pub schema: SchemaRef,
    /// The columns built from the features: the output without the
    /// location column.
    pub built: SchemaRef,
    /// Add the location column (`ReadOptions::location_column`).
    pub location: bool,
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
    /// The rows built so far as a batch of the output schema.
    fn finish(
        &self,
        builder: &mut LayerBatchBuilder,
        locations: &mut Locations,
    ) -> crate::Result<RecordBatch> {
        let batch = builder.finish()?;
        if !self.location {
            return Ok(batch);
        }
        let mut columns = batch.columns().to_vec();
        columns.push(locations.finish());
        Ok(RecordBatch::try_new(self.schema.clone(), columns)?)
    }

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

/// Warnings about the sources (skipped ones, members given by reference),
/// with the source each is about, shared with the read report.
pub type SourceWarnings = Arc<Mutex<Vec<(SourceId, Warning)>>>;

/// The sources of a read, as streams of chunks of one layer, one per source
/// ([`Streams`]), so that several sources can be split at once.
///
/// Each source gets `SourceId(i)` in order and one axis context at index `i`.
/// A source in which the splitter finds no feature collection or feature
/// member (an ISO metadata member of a zip, say) is skipped with a warning;
/// if every source is, the read ends with [`xeibe_core::Error::NoFeatures`].
pub struct ChunkStream {
    sources: Sources,
    shared: Arc<SourceShared>,
    next_source: u32,
}

/// What the streams of a read share.
struct SourceShared {
    options: xeibe_core::SplitterOptions,
    contexts: SharedContexts,
    warnings: SourceWarnings,
    /// Decoding ahead of the sources being split.
    ahead: Arc<DecodeAhead>,
    /// Sources split to the end, and those skipped.
    read: AtomicUsize,
    skipped: Mutex<Vec<(SourceId, String)>>,
}

impl ChunkStream {
    pub fn new(
        sources: Sources,
        options: xeibe_core::SplitterOptions,
        contexts: SharedContexts,
    ) -> Self {
        ChunkStream {
            sources,
            shared: Arc::new(SourceShared {
                options,
                contexts,
                warnings: Arc::default(),
                ahead: Arc::default(),
                read: AtomicUsize::new(0),
                skipped: Mutex::new(Vec::new()),
            }),
            next_source: 0,
        }
    }

    /// What workers waiting for the stream can do meanwhile: decode the
    /// sources being split ahead of their splitters.
    pub fn helper(&self) -> Helper {
        let ahead = self.shared.ahead.clone();
        Arc::new(move || ahead.help())
    }

    /// Warnings about skipped sources, shared with the read report.
    pub fn warnings(&self) -> SourceWarnings {
        self.shared.warnings.clone()
    }
}

impl Streams for ChunkStream {
    type Item = xeibe_core::Result<FeatureChunk>;
    type Stream = SourceChunks;

    fn next_stream(&mut self) -> Option<SourceChunks> {
        let state = match self.sources.next()? {
            Ok(source) => {
                let id = SourceId(self.next_source);
                self.next_source += 1;
                Split::Unopened(source, id)
            }
            Err(error) => Split::Failed(error),
        };
        Some(SourceChunks {
            shared: self.shared.clone(),
            state,
        })
    }

    fn ends_input(item: &Self::Item) -> bool {
        item.is_err()
    }

    fn finish(&mut self) -> Option<Self::Item> {
        let mut skipped = std::mem::take(&mut *lock(&self.shared.skipped));
        if self.shared.read.load(Ordering::Relaxed) > 0 || skipped.is_empty() {
            return None;
        }
        skipped.sort();
        let names: Vec<String> = skipped.into_iter().map(|(_, name)| name).collect();
        Some(Err(xeibe_core::Error::no_features_in(&names)))
    }
}

/// The chunks of one source; opens it on the first `next`.
pub struct SourceChunks {
    shared: Arc<SourceShared>,
    state: Split,
}

enum Split {
    Unopened(xeibe_core::Source, SourceId),
    /// With the source's name.
    Splitting(Box<FeatureSplitter<Box<dyn Read + Send>>>, SourceId, String),
    Failed(xeibe_core::Error),
    Done,
}

impl SourceChunks {
    /// Open the source and read its header; `Ok(None)` when it is skipped.
    fn open(
        &self,
        source: &xeibe_core::Source,
        id: SourceId,
    ) -> xeibe_core::Result<Option<FeatureSplitter<Box<dyn Read + Send>>>> {
        let mut splitter = FeatureSplitter::new(
            source.open_ahead(&self.shared.ahead)?,
            id,
            self.shared.options.clone(),
        );
        let context = match splitter.header() {
            Ok(header) => Some(xeibe_schema::scan::source_context(header)),
            Err(xeibe_core::Error::NoFeatures(_)) => None,
            Err(error) => return Err(error),
        };
        // One context per source id, also for a skipped source.
        {
            let mut contexts = lock(&self.shared.contexts);
            let index = id.0 as usize;
            if contexts.len() <= index {
                contexts.resize_with(index + 1, Default::default);
            }
            contexts[index] = context.clone().unwrap_or_default();
        }
        match context {
            Some(_) => Ok(Some(splitter)),
            None => {
                self.note_skipped(id, source.name());
                Ok(None)
            }
        }
    }

    /// A source without a feature collection or feature member.
    fn note_skipped(&self, id: SourceId, name: String) {
        lock(&self.shared.warnings).push((
            id,
            Warning {
                kind: WarningKind::SkippedSource,
                location: None,
                message: format!("skipped {name}: no feature collection or feature member"),
            },
        ));
        lock(&self.shared.skipped).push((id, name));
    }
}

impl Iterator for SourceChunks {
    type Item = xeibe_core::Result<FeatureChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        if !matches!(self.state, Split::Splitting(..)) {
            match std::mem::replace(&mut self.state, Split::Done) {
                Split::Unopened(source, id) => match self.open(&source, id) {
                    Ok(Some(splitter)) => {
                        self.state = Split::Splitting(Box::new(splitter), id, source.name());
                    }
                    Ok(None) => return None,
                    Err(error) => return Some(Err(error)),
                },
                Split::Failed(error) => return Some(Err(error)),
                Split::Splitting(..) | Split::Done => return None,
            }
        }
        let Split::Splitting(splitter, id, name) = &mut self.state else {
            return None;
        };
        let next = splitter.next();
        if let Some(Ok(chunk)) = next {
            return Some(Ok(chunk));
        }
        // The source has ended, or failed.
        let (id, name, referenced) = (*id, std::mem::take(name), splitter.referenced_members());
        self.state = Split::Done;
        match next {
            Some(Ok(_)) => unreachable!("returned above"),
            // Found at the end of the input: the root was read, but nothing
            // in it holds features.
            Some(Err(xeibe_core::Error::NoFeatures(_))) => {
                self.note_skipped(id, name);
                None
            }
            Some(Err(error)) => Some(Err(error)),
            None => {
                self.shared.read.fetch_add(1, Ordering::Relaxed);
                if referenced > 0 {
                    lock(&self.shared.warnings).push((
                        id,
                        Warning {
                            kind: WarningKind::ReferencedMember,
                            location: None,
                            message: format!(
                                "{referenced} feature members given by reference (not read)"
                            ),
                        },
                    ));
                }
                None
            }
        }
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
    /// `helper`: what a worker can do while others split.
    pub fn start(
        self,
        chunks: impl Streams<Item = xeibe_core::Result<FeatureChunk>> + 'static,
        helper: Helper,
    ) -> crate::Result<Batches> {
        let threads = self.options.threads.max(1);
        let options = MapOptions {
            threads,
            in_flight: self.options.queue_depth.max(1) + threads,
            max_open: max_open_streams(threads),
            preserve_order: self.options.preserve_order,
            name: "xeibe-read".into(),
            helper: Some(helper),
        };
        // Batches of at least half the size pass through uncopied.
        let batch_size = self.plan.batch_size;
        let coalescer = BatchCoalescer::new(self.plan.schema.clone(), batch_size)
            .with_biggest_coalesce_batch_size(Some(batch_size / 2));
        let pipeline = Arc::new(self);
        let chunks = ParallelMap::streams(options, chunks, move |chunk| {
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
        let mut builder = LayerBatchBuilder::new(plan.built.clone(), plan.batch_size.min(4096))?;
        let mut locations = Locations::default();
        let mut report = ReadReport::default();
        let mut batches = Vec::new();
        let mut reader = GmlReader::for_chunk(chunk);
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
                    let start_position = reader.last_start_position();
                    let start = reader.stream_offset(start_position);
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
                        if plan.location {
                            // A feature is as long in the chunk as in the
                            // stream; its end is where the next one starts
                            // in the chunk.
                            let end = start + (reader.position() - start_position) as u64;
                            locations.push(chunk.source.0, start, end);
                        }
                    }
                    if builder.len() >= plan.batch_size {
                        batches.push(plan.finish(&mut builder, &mut locations)?);
                    }
                }
                XmlEvent::Text(_) | XmlEvent::End { .. } => {}
                XmlEvent::Eof => break,
            }
        }
        if !builder.is_empty() {
            batches.push(plan.finish(&mut builder, &mut locations)?);
        }
        *report
            .features_per_layer
            .entry(plan.layer_name.clone())
            .or_default() += rows;
        lock(&self.report).merge(report);
        Ok(batches)
    }
}

/// The location column (`ReadOptions::location_column`).
pub(crate) fn location_field(name: &str) -> Field {
    Field::new(name, DataType::Struct(location_fields()), false)
}

fn location_fields() -> Fields {
    Fields::from(vec![
        Field::new("source", DataType::UInt32, false),
        Field::new("start", DataType::UInt64, false),
        Field::new("end", DataType::UInt64, false),
    ])
}

/// Where the rows of a batch being built are written.
#[derive(Default)]
struct Locations {
    source: Vec<u32>,
    start: Vec<u64>,
    end: Vec<u64>,
}

impl Locations {
    fn push(&mut self, source: u32, start: u64, end: u64) {
        self.source.push(source);
        self.start.push(start);
        self.end.push(end);
    }

    fn finish(&mut self) -> ArrayRef {
        let columns: Vec<ArrayRef> = vec![
            Arc::new(UInt32Array::from(std::mem::take(&mut self.source))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.start))),
            Arc::new(UInt64Array::from(std::mem::take(&mut self.end))),
        ];
        Arc::new(StructArray::new(location_fields(), columns, None))
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
