//! `_overflow: Map(Utf8View → Utf8View)` for data outside the schema
//! (`OnSchemaMismatch::Overflow`, the default for every read).

use arrow_array::ArrayRef;

pub const OVERFLOW_COLUMN: &str = "_overflow";

#[derive(Debug, Default)]
pub struct OverflowCollector {
    current_row: Vec<(String, String)>,
    rows: Vec<Vec<(String, String)>>,
}

impl OverflowCollector {
    pub fn push(&mut self, path: &str, value: &str) {
        todo!()
    }

    pub fn end_row(&mut self) {
        todo!()
    }

    pub fn finish(&mut self) -> ArrayRef {
        todo!()
    }
}
