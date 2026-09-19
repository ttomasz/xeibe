//! Binding a given schema (settings file or Arrow schema) to XML paths.
//!
//! Columns are matched by name, the way spark-xml applies a user schema (see
//! `docs/architecture.md#settings-file`): XML names are mapped to column names
//! with `NamingOptions`; type-wrapper elements without a column of their own are
//! looked through; a `gml:path` field-metadata entry wins over the name. Anything
//! unmatched is routed to `_overflow` (or dropped / an error) at read time.

use arrow_schema::Schema;
use xeibe_core::QName;

use crate::{InferenceOptions, LayerSchema};

pub fn bind_schema(layer: &QName, schema: &Schema, options: &InferenceOptions) -> crate::Result<LayerSchema> {
    todo!()
}
