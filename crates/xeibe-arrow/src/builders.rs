//! Column builders driven by the inferred schema's routes.

use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, SchemaRef};
use xeibe_schema::LayerSchema;

use crate::geometry_column::GeometryColumnBuilder;

/// Builds one layer's batches.
pub struct LayerBatchBuilder {
    schema: SchemaRef,
    columns: Vec<ColumnBuilder>,
    rows: usize,
}

/// A builder for one (possibly nested) column.
pub enum ColumnBuilder {
    Scalar(ScalarBuilder),
    Struct {
        children: Vec<ColumnBuilder>,
        validity: Vec<bool>,
    },
    List {
        item: Box<ColumnBuilder>,
        offsets: Vec<i32>,
        validity: Vec<bool>,
    },
    Map {
        keys: Vec<String>,
        values: Vec<String>,
        offsets: Vec<i32>,
    },
    Geometry(GeometryColumnBuilder),
    /// Raw XML fragments (mixed content, `RawXml` policies).
    RawXml(Vec<Option<String>>),
}

/// Typed scalar builder; parses text according to the target type.
pub struct ScalarBuilder {
    data_type: DataType,
    inner: Box<dyn arrow_array::builder::ArrayBuilder>,
}

impl ScalarBuilder {
    pub fn new(data_type: DataType, capacity: usize) -> Self {
        todo!()
    }

    /// Parse and append; returns `Err(text)` if the value doesn't fit the type
    /// (caller routes it to `_overflow` or errors).
    pub fn append_text(&mut self, text: &str) -> Result<(), String> {
        todo!()
    }

    pub fn append_null(&mut self) {
        todo!()
    }

    pub fn finish(&mut self) -> ArrayRef {
        todo!()
    }
}

impl LayerBatchBuilder {
    pub fn new(layer_schema: &LayerSchema, capacity: usize) -> crate::Result<Self> {
        todo!()
    }

    pub fn len(&self) -> usize {
        self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    pub fn column_mut(&mut self, field_path: &[usize]) -> &mut ColumnBuilder {
        todo!()
    }

    /// Close the current row: missing fields become null.
    pub fn end_row(&mut self) {
        todo!()
    }

    pub fn finish(&mut self) -> crate::Result<RecordBatch> {
        todo!()
    }
}
