//! Parallel read pipeline:
//! splitter → bounded chunk queue → workers (parse + build) → bounded batch
//! queue → optional reorder by sequence number. One layer per pipeline.
//!
//! Memory stays bounded however slow the consumer is: the splitter hands out
//! a chunk only while fewer than `queue_depth + threads` chunks are between
//! it and the consumer (queued, being parsed, or waiting to be reordered),
//! and the batch queue holds at most `queue_depth` messages.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, Mutex};

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use crossbeam_channel::{Receiver, Sender, bounded};
use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_core::{FeatureChunk, FeatureSplitter, Location, NamespaceContext, QName, SourceId, Sources, ns};
use xeibe_geom::GeometryParser;
use xeibe_geom::options::GeometryOptions;

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
    pub fn axis_report(&self) -> (Vec<(xeibe_geom::AxisKey, xeibe_geom::AxisDecision)>, Vec<Warning>) {
        let mut decisions: Vec<(xeibe_geom::AxisKey, xeibe_geom::AxisDecision)> = Vec::new();
        let mut warnings = Vec::new();
        for axis in &self.axis {
            for (key, decision) in axis.decisions() {
                if !decisions.iter().any(|(k, _)| *k == key) {
                    decisions.push((key, decision));
                }
            }
            for warning in axis.warnings() {
                if !warnings.iter().any(|w: &Warning| w.message == warning.message) {
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
        if let Some((uri, local)) = layer.strip_prefix('{').and_then(|rest| rest.split_once('}')) {
            return LayerSelector::Exact(QName::new(Some(uri), local));
        }
        match layer.split_once(':') {
            Some((prefix, local)) => LayerSelector::Prefixed { prefix: prefix.into(), local: local.into() },
            None => LayerSelector::Local(layer.into()),
        }
    }

    /// The splitter's filter entry: exact, or the local name in any namespace.
    pub fn filter(&self) -> QName {
        match self {
            LayerSelector::Exact(name) => name.clone(),
            LayerSelector::Prefixed { local, .. } | LayerSelector::Local(local) => QName::new(None, local),
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
/// A source in which the splitter finds no features (an ISO metadata member
/// of a zip, say) is skipped with a warning.
pub struct ChunkStream {
    sources: Sources,
    options: xeibe_core::SplitterOptions,
    current: Option<FeatureSplitter<Box<dyn Read + Send>>>,
    next_source: u32,
    next_seq: u64,
    contexts: SharedContexts,
    warnings: Arc<Mutex<Vec<Warning>>>,
    finished: bool,
}

impl ChunkStream {
    pub fn new(sources: Sources, options: xeibe_core::SplitterOptions, contexts: SharedContexts) -> Self {
        ChunkStream {
            sources,
            options,
            current: None,
            next_source: 0,
            next_seq: 0,
            contexts,
            warnings: Arc::new(Mutex::new(Vec::new())),
            finished: false,
        }
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
        let mut splitter = FeatureSplitter::new(source.open()?, id, self.options.clone());
        let context = match splitter.header() {
            Ok(header) => Some(xeibe_schema::scan::source_context(header)),
            Err(xeibe_core::Error::NoFeatures(name)) => {
                lock(&self.warnings).push(Warning {
                    kind: WarningKind::Other,
                    location: None,
                    message: format!("skipped {name}: no feature collection or feature member"),
                });
                None
            }
            Err(error) => return Err(error),
        };
        // One context per source id, also for a skipped source.
        lock(&self.contexts).push(context.clone().unwrap_or_default());
        if context.is_some() {
            self.current = Some(splitter);
        }
        Ok(true)
    }
}

impl Iterator for ChunkStream {
    type Item = xeibe_core::Result<FeatureChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.finished {
            if let Some(splitter) = &mut self.current {
                match splitter.next() {
                    Some(Ok(mut chunk)) => {
                        chunk.seq = self.next_seq;
                        self.next_seq += 1;
                        return Some(Ok(chunk));
                    }
                    Some(Err(error)) => {
                        self.finished = true;
                        return Some(Err(error));
                    }
                    None => {
                        let referenced = splitter.referenced_members();
                        if referenced > 0 {
                            lock(&self.warnings).push(Warning {
                                kind: WarningKind::ReferencedMember,
                                location: None,
                                message: format!("{referenced} feature members given by reference (not read)"),
                            });
                        }
                        self.current = None;
                    }
                }
            }
            match self.open_next() {
                Ok(true) => {}
                Ok(false) => self.finished = true,
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
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct Pipeline {
    options: ReadOptions,
    plan: Arc<ReadPlan>,
    report: Arc<Mutex<ReadReport>>,
}

/// A batch tagged for reordering.
pub struct SeqBatch {
    pub chunk_seq: u64,
    pub batch: RecordBatch,
}

/// A chunk's result, sent from a worker to the reorder stage.
struct ChunkResult {
    seq: u64,
    batches: crate::Result<Vec<SeqBatch>>,
}

impl Pipeline {
    pub fn new(options: ReadOptions, plan: Arc<ReadPlan>, report: Arc<Mutex<ReadReport>>) -> Self {
        Pipeline { options, plan, report }
    }

    /// Start the workers. `chunks` already contains only this layer's features
    /// (splitter layer filter), starting with any buffered sample chunks, and
    /// numbered from 0 without gaps.
    pub fn start(
        self,
        chunks: Box<dyn Iterator<Item = xeibe_core::Result<FeatureChunk>> + Send>,
    ) -> Receiver<crate::Result<RecordBatch>> {
        let threads = self.options.threads.max(1);
        let depth = self.options.queue_depth.max(1);
        let preserve_order = self.options.preserve_order;
        let (chunk_tx, chunk_rx) = bounded::<(u64, xeibe_core::Result<FeatureChunk>)>(depth);
        let (result_tx, result_rx) = bounded::<ChunkResult>(depth);
        let (out_tx, out_rx) = bounded::<crate::Result<RecordBatch>>(depth);
        // Tickets for chunks between the splitter and the consumer.
        let (ticket_tx, ticket_rx) = bounded::<()>(depth + threads);

        std::thread::Builder::new()
            .name("xeibe-split".into())
            .spawn(move || split(chunks, chunk_tx, ticket_tx))
            .expect("spawning the splitter thread");

        let pipeline = Arc::new(self);
        for index in 0..threads {
            let pipeline = pipeline.clone();
            let chunk_rx = chunk_rx.clone();
            let result_tx = result_tx.clone();
            std::thread::Builder::new()
                .name(format!("xeibe-read-{index}"))
                .spawn(move || {
                    for (seq, chunk) in chunk_rx {
                        // A panic becomes an error in its place in the stream,
                        // rather than a gap the reorder stage would wait on.
                        let batches = chunk.map_err(Into::into).and_then(|chunk| {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pipeline.process_chunk(&chunk)))
                                .unwrap_or_else(|panic| {
                                    let message = panic
                                        .downcast_ref::<&str>()
                                        .map(|s| s.to_string())
                                        .or_else(|| panic.downcast_ref::<String>().cloned())
                                        .unwrap_or_default();
                                    Err(arrow_schema::ArrowError::ComputeError(format!(
                                        "reading chunk {seq} panicked: {message}"
                                    ))
                                    .into())
                                })
                        });
                        if result_tx.send(ChunkResult { seq, batches }).is_err() {
                            break;
                        }
                    }
                })
                .expect("spawning a worker thread");
        }
        drop(result_tx);

        std::thread::Builder::new()
            .name("xeibe-order".into())
            .spawn(move || reorder(result_rx, out_tx, ticket_rx, preserve_order))
            .expect("spawning the reorder thread");
        out_rx
    }

    fn process_chunk(&self, chunk: &FeatureChunk) -> crate::Result<Vec<SeqBatch>> {
        let plan = &*self.plan;
        let axis = plan.axis.iter().map(|axis| axis.for_source(chunk.source)).collect();
        let features = FeatureReader::new(plan, GeometryParser::new(&plan.geometry), axis);
        let mut builder = LayerBatchBuilder::new(plan.schema.clone(), plan.batch_size.min(4096))?;
        let mut report = ReadReport::default();
        let mut batches = Vec::new();
        let mut reader = GmlReader::new(&chunk.bytes, &chunk.namespaces, chunk.byte_offset).with_source(chunk.source);
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
                    if features.read_feature(&mut reader, location, &mut builder, &mut report)? == FeatureOutcome::Row {
                        rows += 1;
                    }
                    if builder.len() >= plan.batch_size {
                        batches.push(SeqBatch { chunk_seq: chunk.seq, batch: builder.finish()? });
                    }
                }
                XmlEvent::Text(_) | XmlEvent::End { .. } => {}
                XmlEvent::Eof => break,
            }
        }
        if !builder.is_empty() {
            batches.push(SeqBatch { chunk_seq: chunk.seq, batch: builder.finish()? });
        }
        *report.features_per_layer.entry(plan.layer_name.clone()).or_default() += rows;
        lock(&self.report).merge(report);
        Ok(batches)
    }
}

/// The splitter stage: number the chunks and queue them, one ticket each.
fn split(
    chunks: Box<dyn Iterator<Item = xeibe_core::Result<FeatureChunk>> + Send>,
    chunk_tx: Sender<(u64, xeibe_core::Result<FeatureChunk>)>,
    ticket_tx: Sender<()>,
) {
    for (seq, chunk) in chunks.enumerate() {
        let seq = seq as u64;
        let failed = chunk.is_err();
        // Blocks while too many chunks are in flight; fails once the
        // consumer is gone.
        if ticket_tx.send(()).is_err() || chunk_tx.send((seq, chunk)).is_err() || failed {
            return;
        }
    }
}

/// The reorder stage: pass batches on (in chunk order if asked), release one
/// ticket per chunk, stop at the first error.
fn reorder(
    results: Receiver<ChunkResult>,
    out: Sender<crate::Result<RecordBatch>>,
    tickets: Receiver<()>,
    preserve_order: bool,
) {
    let mut pending: BTreeMap<u64, crate::Result<Vec<SeqBatch>>> = BTreeMap::new();
    let mut next = 0u64;
    for result in results {
        if preserve_order {
            pending.insert(result.seq, result.batches);
            while let Some(batches) = pending.remove(&next) {
                next += 1;
                let _ = tickets.try_recv();
                if !emit(&out, batches) {
                    return;
                }
            }
        } else {
            let _ = tickets.try_recv();
            if !emit(&out, result.batches) {
                return;
            }
        }
    }
}

/// Send one chunk's batches; `false` to stop (an error, or no consumer).
fn emit(out: &Sender<crate::Result<RecordBatch>>, batches: crate::Result<Vec<SeqBatch>>) -> bool {
    match batches {
        Ok(batches) => batches.into_iter().all(|batch| out.send(Ok(batch.batch)).is_ok()),
        Err(error) => {
            let _ = out.send(Err(error));
            false
        }
    }
}
