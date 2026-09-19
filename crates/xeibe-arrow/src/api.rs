//! The two user-facing operations: [`scan`] and [`read`]
//! (see `docs/architecture.md#user-facing-api-scan-and-read`).

use arrow_schema::SchemaRef;
use xeibe_core::{QName, Sources};
use xeibe_schema::{DatasetObservation, InferenceOptions, LayerSchema, ScanExtent};

use crate::{LayerReader, ReadOptions, Settings};

/// List every layer with its inferred schema. Full, or the first N features of
/// the input (late layers may then be missing).
pub fn scan(sources: impl Into<Sources>, extent: ScanExtent, options: &ReadOptions) -> crate::Result<ScanResult> {
    todo!()
}

/// Read one layer. With `schema: None` the schema is inferred from the first
/// `options.sample.features_per_layer` features of that layer (conservative
/// types), then frozen; the reader's schema is known once the sample is complete.
/// Data outside the schema is handled by `options.on_mismatch`.
pub fn read(
    sources: impl Into<Sources>,
    layer: &str,
    schema: Option<SchemaRef>,
    options: &ReadOptions,
) -> crate::Result<LayerReader> {
    todo!()
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
        todo!()
    }

    pub fn layers(&self) -> Vec<LayerInfo> {
        todo!()
    }

    /// Schema of one layer with the scan's options.
    pub fn schema(&self, layer: &str) -> crate::Result<LayerSchema> {
        todo!()
    }

    /// Schema of one layer with other options, without another pass.
    pub fn schema_with(&self, layer: &str, options: &InferenceOptions) -> crate::Result<LayerSchema> {
        todo!()
    }

    pub fn arrow_schema(&self, layer: &str) -> crate::Result<SchemaRef> {
        todo!()
    }

    /// Reason and evidence for every field (`xeibe scan --explain`).
    pub fn explain(&self, layer: &str) -> crate::Result<String> {
        todo!()
    }

    /// Options used, the axis-order decision (one mode; per-srsName overrides only
    /// for keys decided differently) and one `column → type` map per layer.
    pub fn to_settings(&self) -> crate::Result<Settings> {
        todo!()
    }
}
