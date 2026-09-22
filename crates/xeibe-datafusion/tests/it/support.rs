//! Helpers shared by the `xeibe-datafusion` tests.

#![allow(dead_code)]

use std::path::PathBuf;

use datafusion::arrow::array::{Array, AsArray, RecordBatch};
use datafusion::arrow::datatypes::DataType;
use datafusion::prelude::SessionContext;

/// The PRG address-point sample: three layers of two features each.
pub const PRG: &str = "samples/pl/prg-address-points.gml";
pub const POINTS: &str = "AD_PunktAdresowy";

/// Absolute path of a file below `tests/data`.
pub fn path(relative: &str) -> String {
    xeibe_testkit::data_dir().join(relative).display().to_string()
}

/// A `file://` URL, read through the session's local object store.
pub fn file_url(relative: &str) -> String {
    format!("file://{}", path(relative))
}

pub fn context() -> SessionContext {
    let ctx = SessionContext::new();
    xeibe_datafusion::register(&ctx);
    ctx
}

pub async fn query(ctx: &SessionContext, sql: &str) -> Vec<RecordBatch> {
    ctx.sql(sql)
        .await
        .unwrap_or_else(|e| panic!("planning {sql}: {e}"))
        .collect()
        .await
        .unwrap_or_else(|e| panic!("running {sql}: {e}"))
}

pub fn rows(batches: &[RecordBatch]) -> usize {
    batches.iter().map(RecordBatch::num_rows).sum()
}

/// The single value of `select count(*) …`.
pub async fn count(ctx: &SessionContext, sql: &str) -> i64 {
    let batches = query(ctx, sql).await;
    let column = batches[0].column(0).as_primitive::<datafusion::arrow::datatypes::Int64Type>();
    column.value(0)
}

/// A string column of every batch, whatever its string type.
pub fn strings(batches: &[RecordBatch], column: &str) -> Vec<Option<String>> {
    let mut out = Vec::new();
    for batch in batches {
        let array = batch.column_by_name(column).unwrap_or_else(|| panic!("no column {column}"));
        let array = datafusion::arrow::compute::cast(array, &DataType::Utf8).expect("a string column");
        let array = array.as_string::<i32>();
        out.extend((0..array.len()).map(|i| array.is_valid(i).then(|| array.value(i).to_string())));
    }
    out
}

/// An empty directory below Cargo's temporary directory.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creating the temporary directory");
    dir
}
