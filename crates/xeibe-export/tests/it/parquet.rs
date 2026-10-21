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
