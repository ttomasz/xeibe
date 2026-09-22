//! `_overflow: Map(Utf8View → Utf8View)` for data outside the schema
//! (`OnSchemaMismatch::Overflow`, the default for every read).

use std::sync::Arc;

use arrow_array::ArrayRef;
use arrow_schema::{DataType, Field, Fields};

use crate::builders::{ColumnBuilder, Value};

pub const OVERFLOW_COLUMN: &str = xeibe_schema::bind::OVERFLOW_COLUMN;

/// Overflow entries per row: `path → raw text / XML`. A row without entries
/// is null.
#[derive(Debug, Default)]
pub struct OverflowCollector {
    current_row: Vec<(String, String)>,
    rows: Vec<Vec<(String, String)>>,
}

impl OverflowCollector {
    pub fn push(&mut self, path: &str, value: &str) {
        self.current_row.push((path.to_string(), value.to_string()));
    }

    pub fn end_row(&mut self) {
        let row = self.take_row();
        self.rows.push(row);
    }

    /// The current row's entries, for a row built elsewhere; the row starts over.
    pub fn take_row(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.current_row)
    }

    /// Forget the current row's entries (a skipped feature).
    pub fn discard_row(&mut self) {
        self.current_row.clear();
    }

    pub fn finish(&mut self) -> ArrayRef {
        let field = overflow_field();
        let mut builder = ColumnBuilder::for_field(&field, self.rows.len()).expect("a map column");
        for row in self.rows.drain(..) {
            let value = if row.is_empty() { Value::Null } else { Value::Map(row) };
            builder.append(value).expect("appending to a map column");
        }
        builder.finish().expect("a valid map array")
    }
}

/// `Map(Utf8View → Utf8View)`, with Arrow's required non-null entries and keys.
pub fn overflow_type() -> DataType {
    let entries = Field::new(
        "entries",
        DataType::Struct(Fields::from(vec![
            Field::new("key", DataType::Utf8View, false),
            Field::new("value", DataType::Utf8View, true),
        ])),
        false,
    );
    DataType::Map(Arc::new(entries), false)
}

/// The `_overflow` field a read appends.
pub fn overflow_field() -> Field {
    Field::new(OVERFLOW_COLUMN, overflow_type(), true)
}
