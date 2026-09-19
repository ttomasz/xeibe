//! The settings file: read options and/or per-layer schemas, as JSON
//! (see `docs/architecture.md#settings-file`).

use std::path::Path;

use arrow_schema::{Field, Schema, SchemaRef};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::ReadOptions;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub format_version: u32,
    /// Every key optional; callers' parameters override it.
    #[serde(default)]
    pub options: ReadOptions,
    /// Layer name (prefixed or Clark notation) → columns in order.
    #[serde(default)]
    pub layers: IndexMap<String, IndexMap<String, ColumnSpec>>,
}

/// One column: a type string, or an object for the less common cases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColumnSpec {
    /// Arrow `DataType` string (`Utf8View`, `List(Utf8View)`, …), or `Geometry` /
    /// `Geometry(<kind>[, <dims>])` for geometry columns. Nullable.
    Type(String),
    Detailed {
        #[serde(rename = "type")]
        data_type: String,
        /// XML path relative to the feature, for renamed columns (`gml:path`).
        #[serde(default)]
        path: Option<String>,
    },
}

impl Settings {
    pub const FORMAT_VERSION: u32 = 1;

    pub fn load(path: &Path) -> crate::Result<Self> {
        todo!()
    }

    pub fn save(&self, path: &Path) -> crate::Result<()> {
        todo!()
    }

    /// Arrow schema of one layer, with GeoArrow extension types on geometry columns.
    pub fn schema(&self, layer: &str) -> crate::Result<SchemaRef> {
        todo!()
    }

    /// Store an Arrow schema as `column → type` (the reverse of [`Self::schema`]).
    pub fn set_schema(&mut self, layer: &str, schema: &Schema) -> crate::Result<()> {
        todo!()
    }
}

impl ColumnSpec {
    pub fn to_field(&self, name: &str) -> crate::Result<Field> {
        todo!()
    }

    pub fn from_field(field: &Field) -> crate::Result<Self> {
        todo!()
    }
}
