//! Rule engine: one recursive walk over a layer's tree → Arrow schema.

use arrow_schema::{Field, Schema};
use xeibe_core::QName;

use crate::{DatasetObservation, ElementNode, InferenceOptions, SampleOptions};

/// Metadata keys written on fields (see `docs/type-mapping.md`).
pub mod meta {
    pub const PATH: &str = "gml:path";
    pub const NS: &str = "gml:ns";
    pub const MAX_SCALE: &str = "gml:max_scale";
    pub const ATTR_PREFIX: &str = "gml:attr:";
    pub const TZ_OFFSET: &str = "gml:tz_offset";
    pub const SRS_NAME: &str = "gml:srs_name";
    pub const AXIS_SWAPPED: &str = "gml:axis_swapped";
    pub const AXIS_DECISION: &str = "gml:axis_decision";
    /// Schema-level: the settings a read used (optional).
    pub const SETTINGS: &str = "gml:settings";
}

/// A layer's schema bound to XML paths: inferred (with the decisions behind it)
/// or given by the user and bound with [`crate::bind_schema`].
#[derive(Debug, Clone)]
pub struct LayerSchema {
    pub layer: QName,
    pub schema: Schema,
    /// Source path → field index path (for routing values to builders).
    pub routes: Vec<FieldRoute>,
    /// One entry per field, for `--explain`. Empty for a given schema.
    pub decisions: Vec<FieldDecision>,
}

#[derive(Debug, Clone)]
pub struct FieldRoute {
    pub source_path: Vec<QName>,
    /// Index path into nested struct fields.
    pub field_path: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct FieldDecision {
    pub field: String,
    pub reasons: Vec<String>,
}

/// `sampled`: the observation covers only part of the data; apply the
/// conservative rules of `SampleOptions` (docs/schema-inference.md §6.2).
pub fn infer_schema(
    observation: &DatasetObservation,
    layer: &QName,
    options: &InferenceOptions,
    sampled: Option<&SampleOptions>,
) -> crate::Result<LayerSchema> {
    todo!()
}

/// One node → field (or `None` if dropped). Recursive.
fn field_for(
    name: &QName,
    node: &ElementNode,
    parent_instances: u64,
    path: &mut Vec<QName>,
    options: &InferenceOptions,
    decisions: &mut Vec<FieldDecision>,
) -> crate::Result<Option<Field>> {
    todo!()
}
