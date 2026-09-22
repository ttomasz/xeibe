use xeibe_core::SplitterOptions;
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
    /// Sampled schemas; `naming` also maps XML names to a given schema's columns;
    /// `geometry` (axis order, CRS override, curves) applies to every read.
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
