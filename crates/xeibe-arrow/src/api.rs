//! The two user-facing operations: [`scan`] and [`read`]
//! (see `docs/architecture.md#user-facing-api-scan-and-read`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use arrow_schema::{Field, Schema, SchemaRef};
use geoarrow_schema::{Crs, GeoArrowType, Metadata};
use xeibe_core::{FeatureChunk, QName, SourceId, Sources};
use xeibe_geom::axis::{AxisContext, AxisSelector, decide};
use xeibe_geom::{AxisKey, AxisOrderMode, AxisOrderOptions, CrsRef, SrsName};
use xeibe_schema::rules::meta;
use xeibe_schema::{
    DatasetObservation, ElementNode, FieldRoute, InferenceOptions, LayerSchema, Merge,
    OnSchemaMismatch, RouteValue, SampleOptions, ScanExtent, ScanOptions, Scanner, bind_schema,
    infer_schema,
};

use crate::axis::{AxisDecisions, SharedContexts};
use crate::geometry_column::geoarrow_type;
use crate::overflow::overflow_field;
use crate::pipeline::{ChunkStream, LayerSelector, Pipeline, ReadPlan, lock};
use crate::route::{FieldShape, RouteTree};
use crate::{LayerReader, ReadOptions, ReadReport, Settings};

/// List every layer with its inferred schema. Full, or the first N features of
/// the input (late layers may then be missing).
pub fn scan(sources: impl Into<Sources>, extent: ScanExtent, options: &ReadOptions) -> crate::Result<ScanResult> {
    let scanner = Scanner::new(ScanOptions {
        extent,
        limits: options.inference.limits,
        layers: None,
        splitter: options.splitter.clone(),
        threads: options.threads.max(1),
    });
    let observation = scanner.run(sources.into())?;
    Ok(ScanResult { observation, options: options.clone() })
}

/// Read one layer. With `schema: None` the schema is inferred from the first
/// `options.sample.features_per_layer` features of that layer (conservative
/// types), then frozen; the reader's schema is known once the sample is complete.
/// Data outside the schema is handled by `options.on_mismatch`.
///
/// `layer` is a local name (`AD_PunktAdresowy`), a prefixed name
/// (`prgad:AD_PunktAdresowy`) or Clark notation (`{uri}AD_PunktAdresowy`).
///
/// When the sample holds the whole layer (the input ended before the sample
/// was full), the schema describes all the data: `min_typed_values` does not
/// apply and no `_overflow` column is added, since nothing can overflow.
pub fn read(
    sources: impl Into<Sources>,
    layer: &str,
    schema: Option<SchemaRef>,
    options: &ReadOptions,
) -> crate::Result<LayerReader> {
    let mut selector = LayerSelector::parse(layer);
    let mut splitter = options.splitter.clone();
    splitter.layers = Some(vec![selector.filter()]);
    let contexts: SharedContexts = Arc::default();
    let mut stream = ChunkStream::new(sources.into(), splitter, contexts.clone());
    let source_warnings = stream.warnings();

    // A sample: for the schema, and for `Auto` axis evidence.
    // A given schema's geometry columns need one too when their CRS is to
    // come from the data.
    let has_geometry = schema.as_ref().is_none_or(|schema| schema.fields().iter().any(|f| is_geometry(f)));
    let crs_from_data = options.inference.geometry.crs_override.is_none()
        && schema.as_ref().is_some_and(|schema| schema.fields().iter().any(|f| is_geometry(f) && !has_crs(f)));
    let needs_sample = schema.is_none()
        || (has_geometry && (crs_from_data || needs_axis_evidence(&options.inference.geometry.axis)));
    let sample = if needs_sample { Some(take_sample(&mut stream, &contexts, options)?) } else { None };

    let layer_name = match &sample {
        Some(sample) => match sample.observation.layer(layer) {
            Ok((name, _)) => Some(name.clone()),
            Err(error) if schema.is_none() => return Err(error.into()),
            Err(_) => None,
        },
        None => None,
    };
    if let Some(name) = &layer_name {
        selector = LayerSelector::Exact(name.clone());
    }
    let qname = layer_name.clone().unwrap_or_else(|| selector.filter());
    let inference = options.inference.for_layer(&qname.to_clark());

    let complete_sample = schema.is_none() && sample.as_ref().is_some_and(|s| s.complete);
    let mut layer_schema = match &schema {
        None => {
            let sample = sample.as_ref().expect("a read without a schema samples");
            let sample_options = sample_options(&options.sample, sample.complete);
            infer_schema(&sample.observation, &qname, &options.inference, Some(&sample_options))?
        }
        Some(schema) => bind_schema(&qname, schema, &options.inference)?,
    };

    // Output columns: the projection, then `_overflow`.
    let base = layer_schema.schema.clone();
    let wanted = |name: &str| options.projection.as_ref().is_none_or(|names| names.iter().any(|n| n == name));
    let mut fields: Vec<Field> = Vec::new();
    let mut columns: Vec<Option<usize>> = Vec::new();
    let srs = inference.geometry.crs_override.clone().or_else(|| {
        let layer = sample.as_ref()?.observation.layers.get(&qname)?;
        let mut srs = BTreeMap::new();
        collect_srs(&layer.root, true, &mut srs);
        srs.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0))).map(|(name, _)| name)
    });
    for field in base.fields() {
        if wanted(field.name()) {
            columns.push(Some(fields.len()));
            let field = match (&schema, &srs) {
                // A given schema carries no CRS: it comes from the data.
                (Some(_), Some(srs)) if is_geometry(field) && !has_crs(field) => with_crs(field, srs)?,
                _ => field.as_ref().clone(),
            };
            fields.push(field);
        } else {
            columns.push(None);
        }
    }
    let has_overflow = layer_schema.routes.iter().any(|route| route.value == RouteValue::Overflow);
    if options.on_mismatch == OnSchemaMismatch::Overflow
        && !has_overflow
        && !complete_sample
        && wanted(crate::overflow::OVERFLOW_COLUMN)
    {
        layer_schema.routes.push(FieldRoute {
            source_path: Vec::new(),
            attribute: None,
            value: RouteValue::Overflow,
            field_path: vec![columns.len()],
        });
        columns.push(Some(fields.len()));
        fields.push(overflow_field());
    }
    let output = Arc::new(Schema::new_with_metadata(fields, base.metadata().clone()));
    let shapes = output
        .fields()
        .iter()
        .map(|field| FieldShape::of(field, inference.geometry.dimension))
        .collect::<crate::Result<Vec<_>>>()?;

    let observed = match (&schema, &sample) {
        (None, Some(sample)) => sample.observation.layers.get(&qname).map(|layer| &layer.root),
        _ => None,
    };
    let routes = RouteTree::new(&layer_schema, &columns, observed);
    let empty = DatasetObservation::default();
    let observation = sample.as_ref().map_or(&empty, |sample| &sample.observation);
    let axis = routes
        .geometry_columns
        .iter()
        .map(|(_, column)| {
            AxisDecisions::from_observation(observation, &qname.to_clark(), column, &inference.geometry.axis)
                .with_contexts(contexts.clone())
        })
        .collect();

    let mut report = ReadReport::default();
    if schema.is_none() {
        let mut settings = Settings::new(options.clone());
        settings.options.inference.geometry.axis = plain_axis(observation, &options.inference.geometry.axis);
        settings.set_schema(&display_name(observation, &qname), &layer_schema.schema)?;
        report.inferred = Some(settings);
    }
    let report = Arc::new(Mutex::new(report));

    let plan = Arc::new(ReadPlan {
        layer_name: layer.to_string(),
        selector,
        schema: output,
        shapes,
        has_geometry: !routes.geometry_columns.is_empty(),
        routes,
        axis,
        geometry: inference.geometry.clone(),
        on_mismatch: options.on_mismatch,
        on_feature_error: options.on_feature_error,
        strip_local_href_hash: inference.gml.strip_local_href_hash,
        empty_as_null: inference.types.empty_as_null,
        batch_size: options.batch_size.max(1),
    });
    let buffered = sample.map(|sample| sample.chunks).unwrap_or_default();
    let chunks: Box<dyn Iterator<Item = xeibe_core::Result<FeatureChunk>> + Send> =
        Box::new(buffered.into_iter().map(Ok).chain(stream));
    let receiver = Pipeline::new(options.clone(), plan.clone(), report.clone()).start(chunks);
    Ok(LayerReader::new(receiver, report, plan, source_warnings))
}

/// The chunks buffered before the workers start, and what they showed.
struct Sample {
    chunks: Vec<FeatureChunk>,
    observation: DatasetObservation,
    /// The input ended within the sample: it is the whole layer.
    complete: bool,
}

/// Take chunks until `sample.features_per_layer` features of the layer (or
/// `max_buffer_bytes`) are in, or the input ends.
fn take_sample(stream: &mut ChunkStream, contexts: &SharedContexts, options: &ReadOptions) -> crate::Result<Sample> {
    let scanner = Scanner::new(ScanOptions {
        extent: ScanExtent::Full,
        limits: options.inference.limits,
        layers: None,
        splitter: options.splitter.clone(),
        threads: 1,
    });
    let mut sample = Sample { chunks: Vec::new(), observation: DatasetObservation::default(), complete: false };
    let mut bytes = 0u64;
    let mut budget = options.sample.features_per_layer;
    loop {
        let Some(chunk) = stream.next() else {
            sample.complete = true;
            break;
        };
        let chunk = chunk?;
        // Only the first `features_per_layer` features count, even where a
        // chunk holds more.
        let (observation, stopped) = scanner.scan_chunk_limited(&chunk, &mut budget)?;
        sample.observation.merge(observation);
        bytes += chunk.bytes.len() as u64;
        sample.chunks.push(chunk);
        if stopped {
            break;
        }
        if budget == 0 || bytes >= options.sample.max_buffer_bytes {
            // One chunk more tells whether the sample is the whole layer.
            match stream.next() {
                None => sample.complete = true,
                Some(chunk) => sample.chunks.push(chunk?),
            }
            break;
        }
    }
    sample.observation.source_context = lock(contexts).clone();
    sample.observation.sampled = !sample.complete;
    Ok(sample)
}

fn sample_options(options: &SampleOptions, complete: bool) -> SampleOptions {
    let mut options = options.clone();
    if complete {
        // Every value of the layer was seen: there is no unseen data to be
        // careful about.
        options.min_typed_values = 0;
    }
    options
}

/// `Auto` somewhere in the axis options needs evidence from a sample.
fn needs_axis_evidence(options: &AxisOrderOptions) -> bool {
    fn auto(mode: &AxisOrderMode) -> bool {
        match mode {
            AxisOrderMode::Auto => true,
            AxisOrderMode::GmlVersion { gml2, gml3 } => auto(gml2) || auto(gml3),
            _ => false,
        }
    }
    auto(&options.mode) || options.overrides.iter().any(|(_, mode)| auto(mode))
}

/// The field has GeoArrow CRS metadata.
fn has_crs(field: &Field) -> bool {
    GeoArrowType::from_extension_field(field)
        .ok()
        .flatten()
        .is_some_and(|typ| typ.metadata().crs().crs_value().is_some())
}

/// The geometry field with the CRS of `srs`: PROJJSON for an EPSG code,
/// else `authority:code`, else the srsName as written (as the rule engine
/// writes it for inferred schemas).
fn with_crs(field: &Field, srs: &str) -> crate::Result<Field> {
    let crs = match SrsName::parse(srs).crs {
        Some(crs) => {
            let projjson = match &crs {
                CrsRef::Code { authority, code } if authority == "EPSG" => code
                    .parse::<u32>()
                    .ok()
                    .and_then(xeibe_crs::projjson)
                    .and_then(|json| serde_json::from_str(json).ok()),
                _ => None,
            };
            match projjson {
                Some(value) => Crs::from_projjson(value),
                None => Crs::from_authority_code(crs.authority_code()),
            }
        }
        None => Crs::from_unknown_crs_type(srs.to_string()),
    };
    let typ = geoarrow_type(field)?.with_metadata(Arc::new(Metadata::new(crs, None)));
    let with = typ.to_field(field.name(), field.is_nullable());
    let mut metadata = with.metadata().clone();
    for (key, value) in field.metadata() {
        if !key.starts_with("ARROW:extension:") {
            metadata.insert(key.clone(), value.clone());
        }
    }
    metadata.insert(meta::SRS_NAME.to_string(), srs.to_string());
    Ok(with.with_metadata(metadata))
}

fn is_geometry(field: &Field) -> bool {
    field
        .metadata()
        .get("ARROW:extension:name")
        .is_some_and(|name| name.starts_with("geoarrow."))
}

/// `prefix:local` with the prefix the input used, else Clark notation.
fn display_name(observation: &DatasetObservation, name: &QName) -> String {
    match &name.ns {
        Some(uri) => match observation.prefixes.get(uri) {
            Some(prefix) => format!("{prefix}:{}", name.local),
            None => name.to_clark(),
        },
        None => name.local.to_string(),
    }
}

/// The axis decisions of every geometry key as one mode, plus overrides for
/// the srsNames decided the other way (`docs/geometry.md`, "Decision key and
/// scope"). Without geometry, the options are kept as they are.
fn plain_axis(observation: &DatasetObservation, options: &AxisOrderOptions) -> AxisOrderOptions {
    // (srsName, dialect) → swaps decided for it.
    let mut decided: BTreeMap<(Option<String>, xeibe_core::Dialect), Vec<bool>> = BTreeMap::new();
    for (name, layer) in &observation.layers {
        let mut evidence = Vec::new();
        collect_geometry(&layer.root, true, &mut evidence);
        for (key, evidence) in evidence {
            let axis_key = AxisKey { source: SourceId(key.source), srs_name: key.srs_name.clone(), dialect: key.dialect };
            let context = axis_context(observation, key.source);
            let decision = decide(&axis_key, Some(&name.local), None, evidence, &context, options);
            decided.entry((key.srs_name.clone(), key.dialect)).or_default().push(decision.swap);
        }
    }
    let all: Vec<bool> = decided.values().flatten().copied().collect();
    if all.is_empty() {
        return options.clone();
    }
    let swapped = all.iter().filter(|swap| **swap).count();
    let majority = swapped * 2 > all.len();
    let mode = |swap: bool| if swap { AxisOrderMode::YX } else { AxisOrderMode::XY };
    let mut overrides = Vec::new();
    for ((srs_name, dialect), swaps) in decided {
        let Some(srs_name) = srs_name else { continue };
        let swap = swaps.iter().filter(|s| **s).count() * 2 > swaps.len();
        if swap != majority {
            overrides.push((
                AxisSelector { srs_name: Some(srs_name), dialect: Some(dialect), ..AxisSelector::default() },
                mode(swap),
            ));
        }
    }
    AxisOrderOptions {
        mode: mode(majority),
        overrides,
        auto: options.auto.clone(),
        crs_table: options.crs_table.clone(),
    }
}

fn axis_context(observation: &DatasetObservation, source: u32) -> AxisContext {
    let Some(context) = observation.source_context.get(source as usize) else {
        return AxisContext::default();
    };
    AxisContext {
        fme_produced: context.fme_produced,
        producer: context.producer.clone(),
        wfs_version: context.wfs_version.clone(),
        requested_srs: context.requested_srs.as_deref().map(SrsName::parse),
        requested_bbox: context.requested_bbox,
        source: None,
    }
}

/// Axis evidence of the geometry properties below a feature (not its `boundedBy`).
fn collect_geometry<'n>(
    node: &'n ElementNode,
    feature: bool,
    out: &mut Vec<(&'n xeibe_schema::geometry_stats::ColumnAxisKey, &'n xeibe_geom::AxisEvidence)>,
) {
    if let Some(stats) = &node.geometry {
        out.extend(stats.axis_evidence.iter());
    }
    for (name, child) in &node.children {
        if feature && name.is_gml_named("boundedBy") {
            continue;
        }
        collect_geometry(child, false, out);
    }
}

/// Result of [`scan`]. In memory; save what is worth keeping with [`Self::to_settings`].
pub struct ScanResult {
    observation: DatasetObservation,
    options: ReadOptions,
}

/// Summary of one feature type, like QGIS's sub-layer list.
#[derive(Debug, Clone)]
pub struct LayerInfo {
    pub name: QName,
    /// Features seen; a lower bound for a sampled scan.
    pub feature_count: u64,
    pub geometry_columns: Vec<String>,
    pub crs: Vec<String>,
    pub extent: Option<[f64; 4]>,
}

impl ScanResult {
    pub fn observation(&self) -> &DatasetObservation {
        &self.observation
    }

    /// `false` for a sampled scan: the layer list may be incomplete.
    pub fn is_complete(&self) -> bool {
        !self.observation.sampled
    }

    pub fn layers(&self) -> Vec<LayerInfo> {
        self.observation
            .layers
            .iter()
            .map(|(name, layer)| {
                let geometry_columns = infer_schema(&self.observation, name, &self.options.inference, self.sampled())
                    .map(|schema| {
                        schema
                            .schema
                            .fields()
                            .iter()
                            .filter(|field| is_geometry(field) && !is_box(field))
                            .map(|field| field.name().clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let mut srs: BTreeMap<String, u64> = BTreeMap::new();
                collect_srs(&layer.root, true, &mut srs);
                let mut crs: Vec<(String, u64)> = srs.into_iter().collect();
                crs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                LayerInfo {
                    name: name.clone(),
                    feature_count: layer.feature_count,
                    geometry_columns,
                    crs: crs.into_iter().map(|(name, _)| name).collect(),
                    extent: layer.extent,
                }
            })
            .collect()
    }

    /// Schema of one layer with the scan's options.
    pub fn schema(&self, layer: &str) -> crate::Result<LayerSchema> {
        self.schema_with(layer, &self.options.inference)
    }

    /// Schema of one layer with other options, without another pass.
    pub fn schema_with(&self, layer: &str, options: &InferenceOptions) -> crate::Result<LayerSchema> {
        let (name, _) = self.observation.layer(layer)?;
        Ok(infer_schema(&self.observation, name, options, self.sampled())?)
    }

    pub fn arrow_schema(&self, layer: &str) -> crate::Result<SchemaRef> {
        Ok(Arc::new(self.schema(layer)?.schema))
    }

    /// Reason and evidence for every field (`xeibe scan --explain`).
    pub fn explain(&self, layer: &str) -> crate::Result<String> {
        Ok(xeibe_schema::explain::explain(&self.schema(layer)?))
    }

    /// Options used, the axis-order decision (one mode; per-srsName overrides only
    /// for keys decided differently) and one `column → type` map per layer.
    pub fn to_settings(&self) -> crate::Result<Settings> {
        let mut settings = Settings::new(self.options.clone());
        settings.options.inference.geometry.axis =
            plain_axis(&self.observation, &self.options.inference.geometry.axis);
        for name in self.observation.layers.keys() {
            let schema = infer_schema(&self.observation, name, &self.options.inference, self.sampled())?;
            settings.set_schema(&display_name(&self.observation, name), &schema.schema)?;
        }
        Ok(settings)
    }

    /// The conservative rules, for a sampled scan.
    fn sampled(&self) -> Option<&SampleOptions> {
        self.observation.sampled.then_some(&self.options.sample)
    }
}

fn is_box(field: &Field) -> bool {
    field.metadata().get("ARROW:extension:name").is_some_and(|name| name == "geoarrow.box")
}

/// srsNames of the geometry properties below a feature, with counts.
fn collect_srs(node: &ElementNode, feature: bool, out: &mut BTreeMap<String, u64>) {
    if let Some(stats) = &node.geometry {
        for (srs, count) in &stats.srs {
            *out.entry(srs.clone()).or_default() += count;
        }
    }
    for (name, child) in &node.children {
        if feature && name.is_gml_named("boundedBy") {
            continue;
        }
        collect_srs(child, false, out);
    }
}
