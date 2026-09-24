use xeibe_core::SplitterOptions;
use xeibe_geom::GeometryOptions;
use xeibe_schema::{InferenceOptions, OnSchemaMismatch, SampleOptions};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OnFeatureError {
    #[default]
    Error,
    Skip,
    /// Keep attributes; geometry errors only.
    NullGeometry,
}

/// Read parameters: the `options` section of a settings file, overridable by
/// CLI flags and function arguments. Schemas are passed separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadOptions {
    /// Axis order, CRS override, curves and the primary geometry column; they
    /// apply to every read (`docs/geometry.md`, "Options").
    pub geometry: GeometryOptions,
    /// Reads without a schema, and scans.
    pub inference: InferenceOptions,
    /// Reads without a schema.
    pub sample: SampleOptions,
    pub on_mismatch: OnSchemaMismatch,
    pub on_feature_error: OnFeatureError,
    pub splitter: SplitterOptions,
    pub batch_size: usize,
    pub threads: usize,
    /// Restore source order of batches (off = faster).
    pub preserve_order: bool,
    /// Bounded queue lengths (backpressure).
    pub queue_depth: usize,
    /// Only build these columns (projection pushdown); `None` = all.
    #[serde(skip)]
    pub projection: Option<Vec<String>>,
}

impl Default for ReadOptions {
    /// Default inference and sampling, `_overflow` for data outside the schema,
    /// stop at the first feature error, 8192-row batches, one worker per core,
    /// source order kept.
    fn default() -> Self {
        ReadOptions {
            geometry: GeometryOptions::default(),
            inference: InferenceOptions::default(),
            sample: SampleOptions::default(),
            on_mismatch: OnSchemaMismatch::Overflow,
            on_feature_error: OnFeatureError::Error,
            splitter: SplitterOptions::default(),
            batch_size: 8192,
            threads: std::thread::available_parallelism().map_or(1, usize::from),
            preserve_order: true,
            queue_depth: 8,
            projection: None,
        }
    }
}

impl ReadOptions {
    /// These options with the geometry options also where inference looks for
    /// them.
    pub(crate) fn effective(&self) -> ReadOptions {
        let mut options = self.clone();
        options.inference.geometry = self.geometry.clone();
        options
    }
}
