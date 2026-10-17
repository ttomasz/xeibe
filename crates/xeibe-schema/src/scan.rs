//! Scan: stream chunks, build one path tree per chunk, merge.

use std::collections::BTreeMap;
use std::io::Read;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex, MutexGuard};

use xeibe_core::decode::DecodeAhead;
use xeibe_core::parallel::Streams;
use xeibe_core::reader::{Attributes, GmlReader, XmlEvent};
use xeibe_core::splitter::DocumentHeader;
use xeibe_core::version::VersionHints;
use xeibe_core::{
    FeatureChunk, FeatureSplitter, Location, NamespaceContext, QName, SourceId, Sources, ns,
};
use xeibe_geom::ParseContext;
use xeibe_geom::sniff::sniff_geometry_in;

use crate::geometry_stats::{GeometryStats, is_envelope, union_bbox};
use crate::node::{ElementNode, is_gml_id};
use crate::observation::{LayerObservation, SourceContext};
use crate::options::Limits;
use crate::{DatasetObservation, Merge, ValueStats};

/// How much of the input a scan reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanExtent {
    /// Everything: the complete layer list and full-data evidence.
    #[default]
    Full,
    /// The first N features of the input, whatever their layers. Layers that
    /// start later are missing.
    Sample { max_features: u64 },
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub extent: ScanExtent,
    pub limits: Limits,
    /// Only these layers (the sample of a read without a schema); `None` = all.
    pub layers: Option<Vec<xeibe_core::QName>>,
    pub splitter: xeibe_core::SplitterOptions,
    /// Threads the scan works on, all included; `<= 1`: the calling thread only.
    pub threads: usize,
}

impl Default for ScanOptions {
    /// A full scan of every layer, on as many threads as the machine has.
    fn default() -> Self {
        ScanOptions {
            extent: ScanExtent::Full,
            limits: Limits::default(),
            layers: None,
            splitter: xeibe_core::SplitterOptions::default(),
            threads: xeibe_core::parallel::default_threads(),
        }
    }
}

/// Builds the observation for a set of sources.
pub struct Scanner {
    options: ScanOptions,
}

impl Scanner {
    pub fn new(options: ScanOptions) -> Self {
        Scanner { options }
    }

    /// Scan every source and merge the results.
    ///
    /// Each source gets `SourceId(i)` in the order the sources come, and one
    /// [`SourceContext`] at the same index. The sources are split by
    /// whichever of the `threads` workers is free, several at once when one
    /// source can't keep the workers busy, and their chunks are scanned on
    /// all of them: there is no pause between sources. With `threads <= 1`
    /// all of it runs on the calling thread. A sampled scan splits its
    /// sources one after another and gives each chunk its share of the
    /// budget as it is split, so it is "the first N features" on any number
    /// of threads.
    ///
    /// A source without a feature collection or feature member is skipped
    /// and listed in [`DatasetObservation::skipped_sources`]; if every
    /// source is, the scan fails with [`xeibe_core::Error::NoFeatures`].
    pub fn run(&self, sources: Sources) -> crate::Result<DatasetObservation> {
        let mut splitter_options = self.options.splitter.clone();
        if self.options.layers.is_some() {
            splitter_options.layers = self.options.layers.clone();
        }
        let budget = match self.options.extent {
            ScanExtent::Full => None,
            ScanExtent::Sample { max_features } => Some(max_features),
        };
        let shared = Arc::new(ScanShared {
            ahead: DecodeAhead::default(),
            splitter_options,
            opened: Mutex::new(Vec::new()),
            budget: Mutex::new(Budget {
                left: budget,
                sampled: false,
                used_up: false,
            }),
        });
        let threads = self.options.threads;
        // A sample is the first features of the input: its sources are split
        // one after another.
        let max_open = match budget {
            None => xeibe_core::parallel::max_open_streams(threads),
            Some(_) => 1,
        };
        let stream = ScanSources {
            sources,
            shared: shared.clone(),
        };
        let partials = xeibe_core::parallel::fold(
            threads,
            max_open,
            "xeibe-scan",
            stream,
            Some(&|| shared.ahead.help()),
            Partial::default,
            |partial, item| self.scan_item(partial, item),
        )
        .map_err(xeibe_core::Error::from)?;

        // What the sources' headers say comes first: their prefixes win over
        // those of chunks.
        let opened = std::mem::take(&mut *lock(&shared.opened));
        let mut total = DatasetObservation::default();
        let mut scanned = 0usize;
        for source in &opened {
            match &source.status {
                SourceStatus::Read(header) => {
                    scanned += 1;
                    note_prefixes(&mut total, &header.namespaces);
                    total.source_context.push(header.context.clone());
                }
                SourceStatus::Skipped => {
                    total.source_context.push(SourceContext::default());
                    total.skipped_sources.push(source.name.clone());
                }
                // Only after an error, which is returned below.
                SourceStatus::Pending => total.source_context.push(SourceContext::default()),
            }
        }
        let mut hints = vec![ElementHints::default(); opened.len()];
        let mut error = None;
        for partial in partials {
            if let Some(partial_error) = partial.error {
                error.get_or_insert(partial_error);
            }
            for (source, source_hints) in partial.hints {
                if let Some(hints) = hints.get_mut(source as usize) {
                    hints.merge(source_hints);
                }
            }
            total.merge(partial.observation);
        }
        if let Some(error) = error {
            return Err(error);
        }
        if scanned == 0 && !total.skipped_sources.is_empty() {
            return Err(xeibe_core::Error::no_features_in(&total.skipped_sources).into());
        }
        // The GML version, per source: its header and the elements of its chunks.
        for (source, element_hints) in opened.iter().zip(&hints) {
            if let SourceStatus::Read(header) = &source.status {
                let mut hints = header.hints.clone();
                element_hints.apply_to(&mut hints);
                if let Some(version) = hints.detect() {
                    total.gml_versions.insert(version);
                }
            }
        }
        total.sampled = lock(&shared.budget).sampled;
        Ok(total)
    }

    /// A worker's step: scan one chunk into the worker's partial observation.
    fn scan_item(
        &self,
        partial: &mut Partial,
        item: crate::Result<(FeatureChunk, Option<u64>)>,
    ) -> ControlFlow<()> {
        let scanned = item.and_then(|(chunk, mut budget)| {
            let scanned = self.scan_chunk_inner(&chunk, &mut budget)?;
            Ok((chunk.source, scanned))
        });
        match scanned {
            Ok((source, scanned)) => {
                partial.observation.merge(scanned.observation);
                partial
                    .hints
                    .entry(source.0)
                    .or_default()
                    .merge(scanned.hints);
                ControlFlow::Continue(())
            }
            Err(error) => {
                partial.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }

    /// Scan one chunk into a partial observation (called from worker threads, and
    /// by a read that samples its layer from buffered chunks).
    ///
    /// The GML version is guessed from the chunk alone (namespaces and
    /// elements); [`Scanner::run`] also uses the document header.
    pub fn scan_chunk(&self, chunk: &FeatureChunk) -> crate::Result<DatasetObservation> {
        let scanned = self.scan_chunk_inner(chunk, &mut None)?;
        let mut observation = scanned.observation;
        let mut hints = VersionHints {
            gml_namespace: namespace_hint(&chunk.namespaces),
            gml_33: declares_gml_33(&chunk.namespaces),
            ..VersionHints::default()
        };
        scanned.hints.apply_to(&mut hints);
        if let Some(version) = hints.detect() {
            observation.gml_versions.insert(version);
        }
        Ok(observation)
    }

    /// Like [`Scanner::scan_chunk`], but scan at most `*budget` features and
    /// count them off (the sample of a read). Also returns whether features
    /// were left unscanned.
    pub fn scan_chunk_limited(
        &self,
        chunk: &FeatureChunk,
        budget: &mut u64,
    ) -> crate::Result<(DatasetObservation, bool)> {
        let mut remaining = Some(*budget);
        let scanned = self.scan_chunk_inner(chunk, &mut remaining)?;
        *budget = remaining.unwrap_or(0);
        let mut observation = scanned.observation;
        let mut hints = VersionHints {
            gml_namespace: namespace_hint(&chunk.namespaces),
            gml_33: declares_gml_33(&chunk.namespaces),
            ..VersionHints::default()
        };
        scanned.hints.apply_to(&mut hints);
        if let Some(version) = hints.detect() {
            observation.gml_versions.insert(version);
        }
        Ok((observation, scanned.stopped))
    }

    /// `budget`: features still to scan (sampled scans); `Some(0)` stops at the
    /// next feature.
    fn scan_chunk_inner(
        &self,
        chunk: &FeatureChunk,
        budget: &mut Option<u64>,
    ) -> crate::Result<ChunkScan> {
        let mut builder = TreeBuilder::new(
            self.options.limits,
            self.options.layers.as_deref(),
            chunk.source.0,
        );
        note_prefixes(&mut builder.observation, &chunk.namespaces);
        let stopped = builder.scan(chunk, budget)?;
        let hints = builder.state.hints;
        Ok(ChunkScan {
            observation: builder.finish(),
            hints,
            stopped,
        })
    }
}

/// What one worker of a scan collected.
#[derive(Default)]
struct Partial {
    observation: DatasetObservation,
    /// Per source id.
    hints: BTreeMap<u32, ElementHints>,
    error: Option<crate::Error>,
}

/// The sources of a scan, each a stream of its chunks.
struct ScanSources {
    sources: Sources,
    shared: Arc<ScanShared>,
}

/// What the streams of a scan share.
struct ScanShared {
    /// Decoding ahead of the sources being split, by waiting workers.
    ahead: DecodeAhead,
    splitter_options: xeibe_core::SplitterOptions,
    /// Every source so far, at the index of its id.
    opened: Mutex<Vec<OpenedSource>>,
    budget: Mutex<Budget>,
}

/// What a sampled scan may still take.
struct Budget {
    /// Features left; `None` in a full scan.
    left: Option<u64>,
    /// A sampled scan stopped before the end of the input.
    sampled: bool,
    /// Nothing more is taken.
    used_up: bool,
}

struct OpenedSource {
    name: String,
    status: SourceStatus,
}

enum SourceStatus {
    /// Not opened yet.
    Pending,
    /// Opened, with features.
    Read(Box<SourceHeader>),
    /// Without a feature collection or feature member.
    Skipped,
}

struct SourceHeader {
    namespaces: NamespaceContext,
    context: SourceContext,
    /// From the header; the chunks' elements add to them.
    hints: VersionHints,
}

impl Streams for ScanSources {
    type Item = crate::Result<(FeatureChunk, Option<u64>)>;
    type Stream = SourceScan;

    fn next_stream(&mut self) -> Option<SourceScan> {
        if lock(&self.shared.budget).used_up {
            return None;
        }
        let state = match self.sources.next()? {
            Ok(source) => {
                let mut opened = lock(&self.shared.opened);
                let id = SourceId(u32::try_from(opened.len()).unwrap_or(u32::MAX));
                opened.push(OpenedSource {
                    name: source.name(),
                    status: SourceStatus::Pending,
                });
                SourceState::Unopened(source, id)
            }
            Err(error) => SourceState::Failed(error.into()),
        };
        Some(SourceScan {
            shared: self.shared.clone(),
            state,
        })
    }

    fn ends_input(item: &Self::Item) -> bool {
        item.is_err()
    }
}

/// The chunks of one source, each with its share of a sampled scan's
/// budget. Opens the source on its first `next`, and records its header.
struct SourceScan {
    shared: Arc<ScanShared>,
    state: SourceState,
}

enum SourceState {
    Unopened(xeibe_core::Source, SourceId),
    Splitting(Box<FeatureSplitter<Box<dyn Read + Send>>>, SourceId),
    Failed(crate::Error),
    Done,
}

impl SourceScan {
    /// Open the source and read its header.
    fn open(&mut self, source: &xeibe_core::Source, id: SourceId) -> crate::Result<()> {
        let mut splitter = FeatureSplitter::new(
            source.open_ahead(&self.shared.ahead)?,
            id,
            self.shared.splitter_options.clone(),
        );
        let header = match splitter.header() {
            Ok(header) => {
                let context = source_context(header);
                SourceHeader {
                    namespaces: header.namespaces.clone(),
                    hints: VersionHints {
                        gml_namespace: namespace_hint(&header.namespaces),
                        gml_33: declares_gml_33(&header.namespaces),
                        schema_location: header.schema_location.clone(),
                        wfs_version: context.wfs_version.clone(),
                        ..VersionHints::default()
                    },
                    context,
                }
            }
            Err(xeibe_core::Error::NoFeatures(_)) => {
                self.set_status(id, SourceStatus::Skipped);
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        self.set_status(id, SourceStatus::Read(Box::new(header)));
        self.state = SourceState::Splitting(Box::new(splitter), id);
        Ok(())
    }

    fn set_status(&self, id: SourceId, status: SourceStatus) {
        lock(&self.shared.opened)[id.0 as usize].status = status;
    }

    /// This chunk's share of the budget; `None` when it is used up.
    fn share(&self, chunk: &FeatureChunk) -> Option<Option<u64>> {
        let mut budget = lock(&self.shared.budget);
        let Some(left) = budget.left else {
            return Some(None);
        };
        // The budget is used up, and there is another feature.
        if left == 0 {
            budget.sampled = true;
            budget.used_up = true;
            return None;
        }
        let take = chunk.features.min(left);
        budget.left = Some(left - take);
        if take < chunk.features {
            budget.sampled = true;
            budget.used_up = true;
        }
        Some(Some(take))
    }
}

impl Iterator for SourceScan {
    type Item = crate::Result<(FeatureChunk, Option<u64>)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match std::mem::replace(&mut self.state, SourceState::Done) {
                SourceState::Unopened(source, id) => {
                    if let Err(error) = self.open(&source, id) {
                        return Some(Err(error));
                    }
                }
                SourceState::Splitting(mut splitter, id) => {
                    if lock(&self.shared.budget).used_up {
                        return None;
                    }
                    match splitter.next() {
                        Some(Ok(chunk)) => {
                            let share = self.share(&chunk)?;
                            self.state = SourceState::Splitting(splitter, id);
                            return Some(Ok((chunk, share)));
                        }
                        // Found at the end of the input: the root was read,
                        // but nothing in it holds features.
                        Some(Err(xeibe_core::Error::NoFeatures(_))) => {
                            self.set_status(id, SourceStatus::Skipped);
                            return None;
                        }
                        Some(Err(error)) => return Some(Err(error.into())),
                        None => return None,
                    }
                }
                SourceState::Failed(error) => return Some(Err(error)),
                SourceState::Done => return None,
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct ChunkScan {
    observation: DatasetObservation,
    hints: ElementHints,
    /// A sampled scan used up its budget and saw another feature.
    stopped: bool,
}

/// GML version evidence from the elements of a chunk.
#[derive(Debug, Clone, Copy, Default)]
struct ElementHints {
    saw_gml: bool,
    saw_gml32: bool,
    /// An element in a GML 3.3 namespace.
    saw_gml33: bool,
    saw_gml2_elements: bool,
    saw_gml3_elements: bool,
}

/// Elements that only GML 2 has (in the namespace it shares with 3.1).
const GML2_ELEMENTS: &[&str] = &["coordinates", "coord", "outerBoundaryIs", "innerBoundaryIs"];

/// Elements that only GML 3 has.
const GML3_ELEMENTS: &[&str] = &[
    "pos",
    "posList",
    "exterior",
    "interior",
    "Curve",
    "Surface",
    "MultiCurve",
    "MultiSurface",
    "Envelope",
];

impl ElementHints {
    fn observe(&mut self, name: &QName) {
        if name.is_gml_33() {
            self.saw_gml33 = true;
        }
        match name.ns.as_deref() {
            Some(ns::GML_32) => self.saw_gml32 = true,
            Some(ns::GML) => {
                self.saw_gml = true;
                if GML2_ELEMENTS.contains(&&*name.local) {
                    self.saw_gml2_elements = true;
                } else if GML3_ELEMENTS.contains(&&*name.local) {
                    self.saw_gml3_elements = true;
                }
            }
            _ => {}
        }
    }

    fn merge(&mut self, other: ElementHints) {
        self.saw_gml |= other.saw_gml;
        self.saw_gml32 |= other.saw_gml32;
        self.saw_gml33 |= other.saw_gml33;
        self.saw_gml2_elements |= other.saw_gml2_elements;
        self.saw_gml3_elements |= other.saw_gml3_elements;
    }

    fn apply_to(&self, hints: &mut VersionHints) {
        if self.saw_gml32 {
            hints.gml_namespace = Some(ns::GML_32.to_string());
        } else if self.saw_gml && hints.gml_namespace.is_none() {
            hints.gml_namespace = Some(ns::GML.to_string());
        }
        hints.gml_33 |= self.saw_gml33;
        hints.saw_gml2_elements |= self.saw_gml2_elements;
        hints.saw_gml3_elements |= self.saw_gml3_elements;
    }
}

/// A GML 3.3 namespace is declared in scope. Documents declare the ones
/// their application schema imports, also when no element uses them.
fn declares_gml_33(namespaces: &NamespaceContext) -> bool {
    ns::GML_33.iter().any(|uri| namespaces.declares_uri(uri))
}

/// The GML namespace declared in scope, 3.2 first.
fn namespace_hint(namespaces: &NamespaceContext) -> Option<String> {
    if namespaces.declares_uri(ns::GML_32) {
        Some(ns::GML_32.to_string())
    } else if namespaces.declares_uri(ns::GML) {
        Some(ns::GML.to_string())
    } else {
        None
    }
}

fn note_prefixes(observation: &mut DatasetObservation, namespaces: &NamespaceContext) {
    for (prefix, uri) in namespaces.iter() {
        if let Some(prefix) = prefix {
            observation.note_prefix(uri, prefix);
        }
    }
}

/// Axis-order context from the document header (also used by reads, which
/// split their sources themselves).
pub fn source_context(header: &DocumentHeader) -> SourceContext {
    SourceContext {
        fme_produced: header.fme_produced,
        producer: producer_fingerprint(header),
        wfs_version: wfs_version(header),
        requested_srs: None,
        requested_bbox: None,
    }
}

/// What the root element tells about the producer, for the axis-order quirk
/// table: `FME`, the namespace URIs declared on it (MapServer declares
/// `http://mapserver.gis.umn.edu/mapserver`) and `xsi:schemaLocation` (the
/// service URL, e.g. ArcGIS's `…/WFSServer`), separated by spaces.
fn producer_fingerprint(header: &DocumentHeader) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    if header.fme_produced {
        parts.push("FME");
    }
    parts.extend(header.namespaces.iter().map(|(_, uri)| uri));
    parts.extend(header.schema_location.as_deref());
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// The WFS version of a saved response: the 2.0 namespace, or the WFS schema
/// in `xsi:schemaLocation` (1.0 and 1.1 share a namespace).
fn wfs_version(header: &DocumentHeader) -> Option<String> {
    let from_location = header.schema_location.as_deref().and_then(|location| {
        location.split_ascii_whitespace().find_map(|url| {
            let (_, rest) = url.split_once("/wfs/")?;
            ["1.0.0", "1.1.0", "2.0.0", "2.0"]
                .iter()
                .find(|version| rest.starts_with(**version))
                .map(|version| if *version == "2.0" { "2.0.0" } else { version }.to_string())
        })
    });
    from_location
        .or_else(|| (header.root.ns.as_deref() == Some(ns::WFS_20)).then(|| "2.0.0".to_string()))
}

/// GML geometry elements that can be the value of a property. The property
/// becomes a geometry leaf; the scan doesn't descend into it.
const GEOMETRY_ELEMENTS: &[&str] = &[
    "Point",
    "LineString",
    "LinearRing",
    "Polygon",
    "Curve",
    "OrientableCurve",
    "CompositeCurve",
    "Ring",
    "Surface",
    "OrientableSurface",
    "CompositeSurface",
    // Patches aren't geometries, but GDAL reads one outside a `Surface`.
    "PolygonPatch",
    "Triangle",
    "Rectangle",
    "PolyhedralSurface",
    "TriangulatedSurface",
    "Tin",
    "MultiPoint",
    "MultiLineString",
    "MultiCurve",
    "MultiPolygon",
    "MultiSurface",
    "MultiGeometry",
    "MultiSolid",
    "Solid",
    "CompositeSolid",
    "GeometricComplex",
    "Envelope",
    "Box",
    "Grid",
    "RectifiedGrid",
];

/// An envelope's corners as a 2D bbox, as written.
fn envelope_bbox(envelope: Option<&xeibe_geom::model::Envelope>) -> Option<[f64; 4]> {
    let envelope = envelope.filter(|e| e.lower.len() >= 2 && e.upper.len() >= 2)?;
    let (lower, upper) = (&envelope.lower, &envelope.upper);
    union_bbox(
        Some([lower[0], lower[1], lower[0], lower[1]]),
        Some([upper[0], upper[1], upper[0], upper[1]]),
    )
}

/// Also GML 3.3's geometry (compact encodings, TIN, referenceable grids).
fn is_geometry_element(name: &QName) -> bool {
    (name.is_gml() && GEOMETRY_ELEMENTS.contains(&&*name.local))
        || xeibe_geom::parse::is_gml_33_geometry(name)
}

/// Per-chunk tree builder: walks features, keeps per-parent child counters on
/// the parse stack, and folds them into `max_occurs` when the parent closes.
pub(crate) struct TreeBuilder<'o> {
    observation: DatasetObservation,
    state: WalkState,
    layers: Option<&'o [QName]>,
}

/// Everything the walk needs besides the node it is in.
struct WalkState {
    limits: Limits,
    source: u32,
    hints: ElementHints,
    feature: FeatureState,
    /// Namespaces declared inside features (`xmlns:x` on a property), for
    /// readable prefixes in paths: `(uri, prefix)`, first declaration wins.
    prefixes: Vec<(String, String)>,
}

/// The feature being walked.
#[derive(Default)]
struct FeatureState {
    seq: u64,
    gml_id: Option<String>,
    /// srsName and srsDimension its geometries inherit: the feature's
    /// `boundedBy`'s, else the collection's.
    srs_name: Option<String>,
    srs_dimension: Option<u8>,
    /// The feature's `boundedBy` was read.
    bounded: bool,
    extent: Option<[f64; 4]>,
}

/// One open element: counters for its children and what its content was.
#[derive(Default)]
struct Frame {
    /// `(child index, count in this instance, location of the second one)`.
    child_counts: Vec<(usize, u32, Option<Location>)>,
    had_text: bool,
    had_children: bool,
    text: String,
    /// The geometry elements of this instance (several in an array property).
    geometries: Vec<xeibe_geom::sniff::GeometrySniff>,
}

/// What an element's start tag said.
#[derive(Default)]
struct Instance {
    href: bool,
    gml_id: Option<String>,
    /// A GML array property (`gml:pointArrayProperty`, …).
    array: bool,
    /// The feature's `gml:boundedBy`.
    bounded_by: bool,
}

impl<'o> TreeBuilder<'o> {
    pub fn new(limits: Limits, layers: Option<&'o [QName]>, source: u32) -> Self {
        TreeBuilder {
            observation: DatasetObservation::default(),
            state: WalkState {
                limits,
                source,
                hints: ElementHints::default(),
                feature: FeatureState::default(),
                prefixes: Vec::new(),
            },
            layers,
        }
    }

    pub fn finish(mut self) -> DatasetObservation {
        for (uri, prefix) in std::mem::take(&mut self.state.prefixes) {
            self.observation.note_prefix(&uri, &prefix);
        }
        self.observation
    }

    /// Walk the features of a chunk. Returns `true` if the budget ran out
    /// before the chunk did.
    fn scan(&mut self, chunk: &FeatureChunk, budget: &mut Option<u64>) -> crate::Result<bool> {
        let mut reader = GmlReader::new(&chunk.bytes, &chunk.namespaces, chunk.byte_offset)
            .with_source(chunk.source);
        let inherited = match chunk.collection_bounded_by.as_deref() {
            Some(raw) => {
                let (envelope, inherited) = xeibe_geom::parse::collection_bounded_by(raw)?;
                self.observation.extent = envelope_bbox(envelope.as_ref());
                inherited
            }
            None => ParseContext::default(),
        };
        let mut index = 0u64;
        loop {
            match reader.next_event()? {
                XmlEvent::Start { name, attrs } => {
                    if self.layers.is_some_and(|layers| !layers.contains(&name)) {
                        reader.skip_element()?;
                        continue;
                    }
                    if let Some(remaining) = budget {
                        if *remaining == 0 {
                            return Ok(true);
                        }
                        *remaining -= 1;
                    }
                    let layer = self
                        .observation
                        .layers
                        .entry(name.clone())
                        .or_insert_with(|| LayerObservation {
                            feature_count: 0,
                            root: ElementNode::new(&name.local, None),
                            extent: None,
                        });
                    layer.feature_count += 1;
                    layer.root.instances += 1;
                    layer.root.parents_with += 1;
                    self.state.hints.observe(&name);
                    let instance = self.state.start(&mut layer.root, &attrs);
                    let offset = reader.location().byte_offset;
                    layer
                        .root
                        .first_seen
                        .get_or_insert((self.state.source, offset));
                    self.state.feature = FeatureState {
                        seq: chunk.first_feature_seq + index,
                        gml_id: instance.gml_id.clone(),
                        srs_name: inherited.srs_name.clone(),
                        srs_dimension: inherited.srs_dimension,
                        ..FeatureState::default()
                    };
                    index += 1;
                    self.state
                        .content(&mut reader, &mut layer.root, 0, instance)?;
                    layer.extent = union_bbox(layer.extent, self.state.feature.extent.take());
                }
                XmlEvent::Text(_) | XmlEvent::End { .. } => {}
                XmlEvent::Eof => return Ok(false),
            }
        }
    }
}

impl WalkState {
    /// Record the attributes of an element's start tag in its node.
    fn start(&mut self, node: &mut ElementNode, attrs: &Attributes<'_>) -> Instance {
        for (prefix, uri) in attrs.namespace_declarations() {
            let Some(prefix) = prefix else { continue };
            if self.prefixes.len() < crate::observation::MAX_PREFIXES
                && !self.prefixes.iter().any(|(known, _)| known == uri)
            {
                self.prefixes.push((uri.to_string(), prefix.to_string()));
            }
        }
        let mut instance = Instance::default();
        let mut nil = false;
        let mut nil_reason = None;
        for (name, value) in attrs.iter() {
            self.hints.observe(&name);
            match name.ns.as_deref() {
                Some(ns::XSI) => {
                    // `schemaLocation`, `type`: never data.
                    if &*name.local == "nil" {
                        nil = matches!(value.trim(), "true" | "1");
                    }
                    continue;
                }
                Some(ns::XLINK) if &*name.local == "href" => instance.href = true,
                _ if is_gml_id(&name) => {
                    node.has_gml_id += 1;
                    instance.gml_id = Some(value.to_string());
                }
                None if &*name.local == "nilReason" => {
                    nil_reason = Some(value);
                    continue;
                }
                _ => {}
            }
            self.attribute(node, name, &value);
        }
        if nil {
            node.nil.count += 1;
            if let Some(reason) = &nil_reason {
                node.nil.add_reason(reason.trim());
            }
        } else if let Some(reason) = nil_reason {
            self.attribute(node, QName::new(None, "nilReason"), &reason);
        }
        instance
    }

    fn attribute(&self, node: &mut ElementNode, name: QName, value: &str) {
        let distinct = self.limits.distinct_values;
        if let Some(stats) = node.attributes.get_mut(&name) {
            stats.observe(value.trim());
        } else if node.attributes.len() < self.limits.max_children as usize {
            let mut stats = ValueStats::with_capacity(distinct);
            stats.observe(value.trim());
            node.attributes.insert(name, stats);
        } else {
            node.truncated = true;
        }
    }

    /// Walk the content of an element whose start tag was just read, up to and
    /// including its end tag. `depth` is the element's depth (feature = 0).
    fn content(
        &mut self,
        reader: &mut GmlReader<'_>,
        node: &mut ElementNode,
        depth: u16,
        instance: Instance,
    ) -> crate::Result<()> {
        let mut frame = Frame::default();
        loop {
            match reader.next_event()? {
                XmlEvent::Start { name, attrs } => {
                    frame.had_children = true;
                    self.hints.observe(&name);
                    if depth >= 1 && is_geometry_element(&name) {
                        let sniff = self.geometry(reader, depth == 1 && instance.bounded_by)?;
                        frame.geometries.push(sniff);
                        continue;
                    }
                    let known = node.children.get_index_of(&name);
                    if depth >= self.limits.max_depth
                        || (known.is_none()
                            && node.children.len() >= self.limits.max_children as usize)
                    {
                        node.truncated = true;
                        reader.skip_element()?;
                        continue;
                    }
                    let index = match known {
                        Some(index) => index,
                        None => {
                            node.children
                                .insert_full(name.clone(), ElementNode::new(&name.local, None))
                                .0
                        }
                    };
                    let child = &mut node.children[index];
                    child.instances += 1;
                    let mut child_instance = self.start(child, &attrs);
                    child_instance.array = xeibe_geom::parse::is_array_property(&name);
                    child_instance.bounded_by = name.is_gml_named("boundedBy");
                    let location = reader.location();
                    child
                        .first_seen
                        .get_or_insert((self.source, location.byte_offset));
                    match frame.child_counts.iter_mut().find(|(i, _, _)| *i == index) {
                        Some((_, count, second)) => {
                            *count += 1;
                            if second.is_none() {
                                *second = Some(self.evidence(location));
                            }
                        }
                        None => frame.child_counts.push((index, 1, None)),
                    }
                    let child = &mut node.children[index];
                    self.content(reader, child, depth + 1, child_instance)?;
                }
                XmlEvent::Text(text) => {
                    if !text.trim().is_empty() {
                        frame.had_text = true;
                        if !frame.had_children {
                            frame.text.push_str(&text);
                        }
                    }
                }
                XmlEvent::End { .. } => break,
                XmlEvent::Eof => {
                    return Err(xeibe_core::Error::Xml {
                        location: reader.location(),
                        message: "unexpected end of chunk inside an element".to_string(),
                    }
                    .into());
                }
            }
        }

        if !frame.geometries.is_empty() {
            let stats = node.geometry.get_or_insert_with(GeometryStats::default);
            if instance.array {
                stats.observe_parts(self.source, &frame.geometries);
            } else {
                // A second geometry in an ordinary property is a feature error
                // when read; each one still tells its kind and position.
                for geometry in &frame.geometries {
                    stats.observe(self.source, geometry);
                }
            }
        }
        if !frame.had_text && matches!(frame.child_counts.as_slice(), [(_, 1, _)]) {
            node.single_child += 1;
        }
        for (index, count, second) in frame.child_counts {
            let child = &mut node.children[index];
            child.parents_with += 1;
            child.max_occurs = child.max_occurs.max(count);
            if count > 1 && child.first_multi.is_none() {
                child.first_multi = second;
            }
        }
        let has_content = frame.had_text || frame.had_children;
        if frame.had_text && frame.had_children {
            node.mixed = true;
        } else if frame.had_text {
            node.text
                .get_or_insert_with(|| ValueStats::with_capacity(self.limits.distinct_values))
                .observe(frame.text.trim());
        }
        match (instance.href, has_content) {
            (true, true) => node.href_and_content += 1,
            (true, false) => node.by_reference += 1,
            (false, false) => node.empty += 1,
            (false, true) => {}
        }
        Ok(())
    }

    /// A geometry element inside a property: sniff it (without building it).
    /// `feature_bounds`: the envelope of the feature's `boundedBy`.
    fn geometry(
        &mut self,
        reader: &mut GmlReader<'_>,
        feature_bounds: bool,
    ) -> crate::Result<xeibe_geom::sniff::GeometrySniff> {
        let sniff = sniff_geometry_in(
            reader,
            self.feature.srs_name.as_deref(),
            self.feature.srs_dimension,
        )?;
        self.hints.saw_gml33 |= sniff.gml_33;
        match sniff.dialect {
            Some(xeibe_core::Dialect::Gml2) => self.hints.saw_gml2_elements = true,
            Some(xeibe_core::Dialect::Gml3) if self.hints.saw_gml && !self.hints.saw_gml32 => {
                self.hints.saw_gml3_elements = true;
            }
            _ => {}
        }
        if is_envelope(&sniff) {
            // A feature's `boundedBy` hands its srsName and srsDimension down
            // to its geometries, in place of the collection's.
            if feature_bounds && !self.feature.bounded {
                self.feature.bounded = true;
                self.feature.srs_name.clone_from(&sniff.srs_name);
                if sniff.srs_dimension.is_some() {
                    self.feature.srs_dimension = sniff.srs_dimension;
                }
            }
        } else if let Some(p) = sniff.first_position.as_deref().filter(|p| p.len() >= 2) {
            self.feature.extent = union_bbox(self.feature.extent, Some([p[0], p[1], p[0], p[1]]));
        }
        Ok(sniff)
    }

    /// A location with the current feature's sequence number and `gml:id`.
    fn evidence(&self, mut location: Location) -> Location {
        location.feature_seq = Some(self.feature.seq);
        location.gml_id = self.feature.gml_id.clone();
        location
    }
}
