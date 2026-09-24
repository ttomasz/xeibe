//! Helpers shared by the `xeibe-arrow` tests.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arrow_array::cast::AsArray;
use arrow_array::types::{Date32Type, Float64Type, Int64Type, TimestampMicrosecondType};
use arrow_array::{Array, RecordBatch};
use arrow_schema::{DataType, Schema, SchemaRef};
use xeibe_arrow::{LayerReader, ReadOptions, ReadReport, ScanResult, read, scan};
use xeibe_core::{Source, Sources};
use xeibe_schema::ScanExtent;
use xeibe_testkit::wkb;
use xeibe_testkit::wkt::G;

/// One source over a document held in memory.
pub fn sources(document: &str) -> Sources {
    Sources::from(Source::reader(
        "test.gml",
        Box::new(std::io::Cursor::new(document.as_bytes().to_vec())),
    ))
}

/// One source over a file below `tests/data`.
pub fn file_sources(relative: &str) -> Sources {
    Sources::from(Source::file(path(relative)).expect("a file source"))
}

pub fn path(relative: &str) -> PathBuf {
    xeibe_testkit::data_dir().join(relative)
}

pub fn scan_document(document: &str) -> ScanResult {
    scan(sources(document), ScanExtent::Full, &ReadOptions::default())
        .expect("the document scans")
}

/// Read one layer and collect everything: batches, schema and report.
pub fn read_document(document: &str, layer: &str) -> Read {
    read_with(document, layer, None, &ReadOptions::default())
}

pub fn read_with(
    document: &str,
    layer: &str,
    schema: Option<SchemaRef>,
    options: &ReadOptions,
) -> Read {
    let reader = read(sources(document), layer, schema, options)
        .unwrap_or_else(|e| panic!("reading {layer}: {e}"));
    collect(reader)
}

pub fn collect(mut reader: LayerReader) -> Read {
    use arrow_array::RecordBatchReader;

    let schema = reader.schema();
    let mut batches = Vec::new();
    for batch in reader.by_ref() {
        batches.push(batch.expect("a batch"));
    }
    Read {
        report: reader.report(),
        schema,
        batches,
    }
}

pub struct Read {
    pub schema: SchemaRef,
    pub batches: Vec<RecordBatch>,
    pub report: ReadReport,
}

impl Read {
    pub fn rows(&self) -> usize {
        self.batches.iter().map(RecordBatch::num_rows).sum()
    }

    pub fn column_names(&self) -> Vec<String> {
        self.schema.fields().iter().map(|f| f.name().clone()).collect()
    }

    pub fn data_type(&self, name: &str) -> DataType {
        self.field(name).data_type().clone()
    }

    #[track_caller]
    pub fn field(&self, name: &str) -> arrow_schema::FieldRef {
        self.schema
            .fields()
            .iter()
            .find(|f| f.name() == name)
            .unwrap_or_else(|| panic!("no column {name:?} in {:?}", self.column_names()))
            .clone()
    }

    /// Values of a string column over every batch.
    #[track_caller]
    pub fn strings(&self, name: &str) -> Vec<Option<String>> {
        self.map_column(name, |array| match array.data_type() {
            DataType::Utf8View => {
                let array = array.as_string_view();
                (0..array.len())
                    .map(|i| (!array.is_null(i)).then(|| array.value(i).to_string()))
                    .collect()
            }
            DataType::Utf8 => {
                let array = array.as_string::<i32>();
                (0..array.len())
                    .map(|i| (!array.is_null(i)).then(|| array.value(i).to_string()))
                    .collect()
            }
            DataType::LargeUtf8 => {
                let array = array.as_string::<i64>();
                (0..array.len())
                    .map(|i| (!array.is_null(i)).then(|| array.value(i).to_string()))
                    .collect()
            }
            other => panic!("{name} is {other}, not a string column"),
        })
    }

    #[track_caller]
    pub fn i64s(&self, name: &str) -> Vec<Option<i64>> {
        self.map_column(name, |array| {
            let array = array.as_primitive::<Int64Type>();
            (0..array.len())
                .map(|i| (!array.is_null(i)).then(|| array.value(i)))
                .collect()
        })
    }

    #[track_caller]
    pub fn f64s(&self, name: &str) -> Vec<Option<f64>> {
        self.map_column(name, |array| {
            let array = array.as_primitive::<Float64Type>();
            (0..array.len())
                .map(|i| (!array.is_null(i)).then(|| array.value(i)))
                .collect()
        })
    }

    #[track_caller]
    pub fn bools(&self, name: &str) -> Vec<Option<bool>> {
        self.map_column(name, |array| {
            let array = array.as_boolean();
            (0..array.len())
                .map(|i| (!array.is_null(i)).then(|| array.value(i)))
                .collect()
        })
    }

    /// Days since the epoch.
    #[track_caller]
    pub fn dates(&self, name: &str) -> Vec<Option<i32>> {
        self.map_column(name, |array| {
            let array = array.as_primitive::<Date32Type>();
            (0..array.len())
                .map(|i| (!array.is_null(i)).then(|| array.value(i)))
                .collect()
        })
    }

    /// Microseconds since the epoch.
    #[track_caller]
    pub fn timestamps(&self, name: &str) -> Vec<Option<i64>> {
        self.map_column(name, |array| {
            let array = array.as_primitive::<TimestampMicrosecondType>();
            (0..array.len())
                .map(|i| (!array.is_null(i)).then(|| array.value(i)))
                .collect()
        })
    }

    /// Geometry values, decoded from WKB with the testkit's own reader.
    #[track_caller]
    pub fn geometries(&self, name: &str) -> Vec<Option<G>> {
        self.map_column(name, |array| match array.data_type() {
            DataType::Binary => {
                let array = array.as_binary::<i32>();
                (0..array.len())
                    .map(|i| {
                        (!array.is_null(i))
                            .then(|| wkb::decode(array.value(i)).expect("valid WKB"))
                    })
                    .collect()
            }
            DataType::LargeBinary => {
                let array = array.as_binary::<i64>();
                (0..array.len())
                    .map(|i| {
                        (!array.is_null(i))
                            .then(|| wkb::decode(array.value(i)).expect("valid WKB"))
                    })
                    .collect()
            }
            DataType::BinaryView => {
                let array = array.as_binary_view();
                (0..array.len())
                    .map(|i| {
                        (!array.is_null(i))
                            .then(|| wkb::decode(array.value(i)).expect("valid WKB"))
                    })
                    .collect()
            }
            other => panic!(
                "{name} is {other}; read with GeomEncoding::Wkb to compare geometry"
            ),
        })
    }

    /// Values of a list-of-strings column: `None` for a null list.
    #[track_caller]
    pub fn string_lists(&self, name: &str) -> Vec<Option<Vec<Option<String>>>> {
        self.map_column(name, |array| {
            let lists = array.as_list::<i32>();
            (0..lists.len())
                .map(|i| {
                    (!lists.is_null(i)).then(|| {
                        let items = lists.value(i);
                        let items = items.as_string_view();
                        (0..items.len())
                            .map(|j| (!items.is_null(j)).then(|| items.value(j).to_string()))
                            .collect()
                    })
                })
                .collect()
        })
    }

    /// Values of a `geometry[]` column (a list of WKB), decoded.
    #[track_caller]
    pub fn geometry_lists(&self, name: &str) -> Vec<Option<Vec<Option<G>>>> {
        self.map_column(name, |array| {
            let lists = array.as_list::<i32>();
            (0..lists.len())
                .map(|i| {
                    (!lists.is_null(i)).then(|| {
                        let items = lists.value(i);
                        let items = items.as_binary::<i32>();
                        (0..items.len())
                            .map(|j| {
                                (!items.is_null(j))
                                    .then(|| wkb::decode(items.value(j)).expect("valid WKB"))
                            })
                            .collect()
                    })
                })
                .collect()
        })
    }

    #[track_caller]
    fn map_column<T>(&self, name: &str, mut f: impl FnMut(&dyn Array) -> Vec<T>) -> Vec<T> {
        let mut values = Vec::new();
        for batch in &self.batches {
            let column = batch
                .column_by_name(name)
                .unwrap_or_else(|| panic!("no column {name:?} in {:?}", self.column_names()));
            values.extend(f(column.as_ref()));
        }
        values
    }
}

/// The GeoArrow extension type of a field, from its metadata.
pub fn extension_name(field: &arrow_schema::Field) -> Option<&str> {
    field
        .metadata()
        .get("ARROW:extension:name")
        .map(String::as_str)
}

/// A fresh directory for files a test writes (`target/tmp/<name>-<n>`).
/// Each call gets its own, so tests running in parallel never share one.
pub fn temp_dir(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creating the temporary directory");
    dir
}

/// An Arrow schema to pass to a read.
pub fn schema_of(fields: Vec<arrow_schema::Field>) -> SchemaRef {
    Arc::new(Schema::new(fields))
}
