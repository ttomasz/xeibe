use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::metadata::ParquetMetaData;
use serde_json::Value;
use xeibe_arrow::ReadOptions;
use xeibe_export::{BboxColumn, Error, ParquetOptions, ParquetWriter};
use xeibe_geom::options::{CurveMode, LinearizeOptions};

use crate::support::*;

/// Write `layer` of `sample` to memory; the file's bytes.
fn write(
    sample: &str,
    layer: &str,
    read: &ReadOptions,
    options: ParquetOptions,
) -> Result<Bytes, Error> {
    let (schema, batches, settings) = crate::support::read(sample, layer, read);
    let mut out = Vec::new();
    let mut writer = ParquetWriter::new(&mut out, schema, options, settings_metadata(settings));
    for batch in &batches {
        writer.write(batch)?;
    }
    let summary = writer.finish()?;
    assert_eq!(
        summary.rows,
        batches.iter().map(|b| b.num_rows() as u64).sum::<u64>()
    );
    Ok(Bytes::from(out))
}

fn metadata(file: &Bytes) -> std::sync::Arc<ParquetMetaData> {
    ParquetRecordBatchReaderBuilder::try_new(file.clone())
        .expect("a Parquet file")
        .metadata()
        .clone()
}

fn key(metadata: &ParquetMetaData, key: &str) -> Option<String> {
    metadata
        .file_metadata()
        .key_value_metadata()?
        .iter()
        .find(|kv| kv.key == key)
        .and_then(|kv| kv.value.clone())
}

fn geo(metadata: &ParquetMetaData) -> Value {
    serde_json::from_str(&key(metadata, "geo").expect("geo metadata")).unwrap()
}

fn columns(metadata: &ParquetMetaData) -> Vec<String> {
    let schema = metadata.file_metadata().schema_descr();
    (0..schema.num_columns())
        .map(|i| schema.column(i).path().string())
        .collect()
}

#[test]
fn points_get_geo_metadata_and_no_bbox_column() {
    let file = write(
        PRG,
        "AD_PunktAdresowy",
        &ReadOptions::default(),
        ParquetOptions::default(),
    )
    .unwrap();
    let metadata = metadata(&file);
    assert_eq!(metadata.file_metadata().num_rows(), 2);
    let geo = geo(&metadata);
    assert_eq!(geo["primary_column"], "georeferencja");
    let column = &geo["columns"]["georeferencja"];
    assert_eq!(column["encoding"], "WKB");
    assert_eq!(column["geometry_types"], serde_json::json!(["Point"]));
    assert_eq!(column["crs"]["id"]["code"], 2180);
    assert!(column.get("covering").is_none(), "{column}");
    assert!(
        !columns(&metadata)
            .iter()
            .any(|c| c.starts_with("georeferencja_bbox"))
    );
    let settings: Value =
        serde_json::from_str(&key(&metadata, "gml:settings").expect("the settings")).unwrap();
    assert!(settings["layers"].is_object(), "{settings}");
}

#[test]
fn a_bbox_column_when_asked_for() {
    let options = ParquetOptions {
        bbox_column: BboxColumn::Always,
        ..ParquetOptions::default()
    };
    let file = write(PRG, "AD_PunktAdresowy", &ReadOptions::default(), options).unwrap();
    let metadata = metadata(&file);
    let covering = &geo(&metadata)["columns"]["georeferencja"]["covering"]["bbox"];
    assert_eq!(
        covering["xmin"],
        serde_json::json!(["georeferencja_bbox", "xmin"])
    );
    assert!(columns(&metadata).contains(&"georeferencja_bbox.xmin".to_string()));
}

#[test]
fn curves_are_an_error() {
    let error = write(
        RCN_ARCS,
        "RCN_Budynek",
        &ReadOptions::default(),
        ParquetOptions::default(),
    )
    .unwrap_err();
    match error {
        Error::Curve { column, kind, .. } => {
            assert_eq!(column, "geometria");
            assert_eq!(kind, "CurvePolygon");
        }
        other => panic!("expected a curve error, got {other}"),
    }
}

#[test]
fn linearized_curves_are_written() {
    let mut read = ReadOptions::default();
    read.geometry.curves = CurveMode::Linearize(LinearizeOptions::default());
    let file = write(RCN_ARCS, "RCN_Budynek", &read, ParquetOptions::default()).unwrap();
    let types = &geo(&metadata(&file))["columns"]["geometria"]["geometry_types"];
    assert!(
        types
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t.as_str().unwrap().contains("Polygon")),
        "{types}"
    );
}

#[test]
fn a_layer_without_batches_is_still_a_file() {
    let (schema, _, settings) = read(PRG, "AD_PunktAdresowy", &ReadOptions::default());
    let mut out = Vec::new();
    let writer = ParquetWriter::new(
        &mut out,
        schema,
        ParquetOptions::default(),
        settings_metadata(settings),
    );
    assert_eq!(writer.finish().unwrap().rows, 0);
    let metadata = metadata(&Bytes::from(out));
    assert_eq!(metadata.file_metadata().num_rows(), 0);
    assert_eq!(
        geo(&metadata)["columns"]["georeferencja"]["geometry_types"],
        serde_json::json!([])
    );
}

#[test]
fn a_crs_without_projjson_is_a_warning() {
    // An srsName that names no CRS xeibe knows.
    let document = xeibe_testkit::gml::gml32_collection(&[&xeibe_testkit::gml::feature(
        "Place",
        "p1",
        "<app:pos><gml:Point srsName=\"urn:ogc:def:crs:EPSG::999999\"><gml:pos>1 2</gml:pos></gml:Point></app:pos>",
    )]);
    let (schema, batches, _) = read_document(&document, "Place");
    let mut out = Vec::new();
    let mut writer = ParquetWriter::new(&mut out, schema, ParquetOptions::default(), Vec::new());
    for batch in &batches {
        writer.write(batch).unwrap();
    }
    let summary = writer.finish().unwrap();
    assert_eq!(summary.rows, 1);
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.contains("no PROJJSON form")),
        "{:?}",
        summary.warnings
    );
}

/// Text in Arrow's view layout is written as the classic `Utf8`, in lists
/// and maps too: the Arrow schema a Parquet file keeps tells readers which
/// layout to rebuild, and pyarrow 26 can't hand `list<string_view>` to
/// pandas (`geopandas.read_parquet` fails on it).
#[test]
fn view_types_are_written_as_classic_ones() {
    use std::collections::HashMap;
    use std::sync::Arc;

    use arrow_array::builder::{ListBuilder, MapBuilder, StringViewBuilder};
    use arrow_array::cast::AsArray;
    use arrow_array::{Array, ArrayRef, Int64Array, RecordBatch, StringViewArray};
    use arrow_schema::{DataType, Field, Schema};

    let id = StringViewArray::from(vec!["p1", "p2"]);
    let mut tags = ListBuilder::new(StringViewBuilder::new());
    tags.values().append_value("a");
    tags.values().append_value("b");
    tags.append(true);
    tags.append(false);
    let tags = tags.finish();
    let mut map = MapBuilder::new(None, StringViewBuilder::new(), StringViewBuilder::new());
    map.keys().append_value("street");
    map.values().append_value("Polna");
    map.append(true).unwrap();
    map.append(false).unwrap();
    let map = map.finish();
    let count = Int64Array::from(vec![Some(1), None]);
    let path = HashMap::from([("gml:path".to_string(), "@gml:id".to_string())]);
    let schema = Arc::new(Schema::new(vec![
        Field::new("@id", DataType::Utf8View, true).with_metadata(path.clone()),
        Field::new("tags", tags.data_type().clone(), true),
        Field::new("address", map.data_type().clone(), true),
        Field::new("count", DataType::Int64, true),
    ]));
    let arrays: Vec<ArrayRef> = vec![Arc::new(id), Arc::new(tags), Arc::new(map), Arc::new(count)];
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    let mut out = Vec::new();
    let mut writer = ParquetWriter::new(&mut out, schema, ParquetOptions::default(), Vec::new());
    writer.write(&batch).unwrap();
    writer.finish().unwrap();

    // Read back as the file's own Arrow schema says.
    let read: Vec<RecordBatch> = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(out))
        .unwrap()
        .build()
        .unwrap()
        .map(|batch| batch.unwrap())
        .collect();
    let read = &read[0];
    let schema = read.schema();
    assert_eq!(schema.field(0).data_type(), &DataType::Utf8);
    assert_eq!(
        schema.field(0).metadata(),
        &path,
        "the field metadata is kept"
    );
    let DataType::List(item) = schema.field(1).data_type() else {
        panic!("{:?}", schema.field(1));
    };
    assert_eq!(item.data_type(), &DataType::Utf8);
    let DataType::Map(entries, _) = schema.field(2).data_type() else {
        panic!("{:?}", schema.field(2));
    };
    let DataType::Struct(entry) = entries.data_type() else {
        panic!("{entries:?}");
    };
    assert!(
        entry
            .iter()
            .all(|field| field.data_type() == &DataType::Utf8),
        "{entry:?}"
    );
    assert_eq!(schema.field(3).data_type(), &DataType::Int64);

    // The values are the same.
    let ids = read.column(0).as_string::<i32>();
    assert_eq!((ids.value(0), ids.value(1)), ("p1", "p2"));
    let tags = read.column(1).as_list::<i32>();
    let first = tags.value(0);
    let first = first.as_string::<i32>();
    assert_eq!((first.value(0), first.value(1)), ("a", "b"));
    assert!(tags.is_null(1));
    let map = read.column(2).as_map();
    let entries = map.value(0);
    assert_eq!(entries.column(0).as_string::<i32>().value(0), "street");
    assert_eq!(entries.column(1).as_string::<i32>().value(0), "Polna");
    assert!(map.is_null(1));
}
