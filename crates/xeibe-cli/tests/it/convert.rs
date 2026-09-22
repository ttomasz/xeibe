//! `xeibe convert` (`docs/architecture.md`, "CLI"; `docs/geometry.md`,
//! "Parquet and GeoParquet output" and "Curves").

use std::fs::File;

use arrow_array::cast::AsArray;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::LogicalType;
use parquet::file::reader::FileReader;

use crate::support::*;

const PRG_POINTS_EXTENT: [f64; 4] = [220155.39, 426441.08, 222442.2, 431274.25];

#[test]
fn convert_to_parquet_writes_wkb_with_the_geometry_logical_type() {
    let dir = out_dir("convert_parquet");
    let out = dir.join("points.parquet");
    let run = xeibe_ok(&["convert", &sample(PRG), "--layer", "AD_PunktAdresowy", "-o", path_str(&out)]);
    assert!(run.stderr.contains("2 rows written"), "{}", run.stderr);

    let reader = parquet_reader(&out);
    let metadata = reader.metadata();
    assert_eq!(metadata.file_metadata().num_rows(), 2);
    let schema = metadata.file_metadata().schema_descr();
    let index = (0..schema.num_columns())
        .find(|&i| schema.column(i).name() == "georeferencja")
        .expect("a georeferencja column");
    match schema.column(index).logical_type_ref() {
        Some(LogicalType::Geometry(geometry)) => assert_eq!(geometry.crs.as_deref(), Some("EPSG:2180")),
        other => panic!("expected the GEOMETRY logical type, got {other:?}"),
    }
    // The parquet crate computes the bbox statistics per row group.
    let statistics = metadata.row_group(0).column(index).geo_statistics().expect("geospatial statistics");
    let bbox = statistics.bounding_box().expect("a bbox");
    assert_eq!([bbox.get_xmin(), bbox.get_ymin(), bbox.get_xmax(), bbox.get_ymax()], PRG_POINTS_EXTENT);

    // The values are WKB points (x/y as written for the short EPSG:2180 form).
    let batches: Vec<_> = ParquetRecordBatchReaderBuilder::try_new(File::open(&out).unwrap())
        .unwrap()
        .build()
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let column = batches[0].column_by_name("georeferencja").unwrap();
    for wkb in column.as_binary::<i32>().iter() {
        let wkb = wkb.expect("no null geometry");
        assert_eq!(wkb.len(), 21, "a 2D WKB point");
        assert_eq!(u32::from_le_bytes(wkb[1..5].try_into().unwrap()), 1);
        let x = f64::from_le_bytes(wkb[5..13].try_into().unwrap());
        let y = f64::from_le_bytes(wkb[13..21].try_into().unwrap());
        assert!((PRG_POINTS_EXTENT[0]..=PRG_POINTS_EXTENT[2]).contains(&x), "x {x}");
        assert!((PRG_POINTS_EXTENT[1]..=PRG_POINTS_EXTENT[3]).contains(&y), "y {y}");
    }
    assert!(batches[0].column_by_name("kodPocztowy").is_some());
}

#[test]
fn convert_to_parquet_writes_geoparquet_metadata_with_projjson() {
    let dir = out_dir("convert_geo");
    let out = dir.join("streets.parquet");
    xeibe_ok(&["convert", &sample(PRG), "--layer", "prgad:AD_UlicaPlac", "-o", path_str(&out)]);

    let geo = geo_metadata(&out);
    assert_eq!(geo["version"], "1.1.0");
    assert_eq!(geo["primary_column"], "geometria");
    let column = &geo["columns"]["geometria"];
    assert_eq!(column["encoding"], "WKB");
    assert_eq!(column["geometry_types"], serde_json::json!(["Polygon"]));
    let crs = &column["crs"];
    assert!(crs.is_object(), "crs must be PROJJSON: {crs}");
    assert_eq!(crs["id"]["authority"], "EPSG");
    assert_eq!(crs["id"]["code"], 2180);
    assert_eq!(crs["type"], "ProjectedCRS");
    let bbox: Vec<f64> = serde_json::from_value(column["bbox"].clone()).expect("a bbox");
    assert_eq!(bbox.len(), 4);
    assert!(bbox[0] < bbox[2] && bbox[1] < bbox[3], "{bbox:?}");
}

#[test]
fn convert_with_a_settings_file_uses_its_schema() {
    let dir = out_dir("convert_settings");
    let settings = dir.join("prg.json");
    // Only the columns in the file are read, in its order.
    let text = r#"{
  "format_version": 1,
  "options": { "inference": { "geometry": { "axis": "XY" } } },
  "layers": {
    "prgad:AD_PunktAdresowy": {
      "kodPocztowy": "Utf8View",
      "numerPorzadkowy": "Utf8View",
      "georeferencja": "Geometry(Point)"
    }
  }
}"#;
    std::fs::write(&settings, text).unwrap();

    let out = dir.join("points.parquet");
    xeibe_ok(&[
        "convert",
        &sample(PRG),
        "--layer",
        "AD_PunktAdresowy",
        "--settings",
        path_str(&settings),
        "--on-mismatch",
        "drop",
        "-o",
        path_str(&out),
    ]);
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(&out).unwrap()).unwrap();
    let names: Vec<String> = builder.schema().fields().iter().map(|f| f.name().clone()).collect();
    assert_eq!(names, ["kodPocztowy", "numerPorzadkowy", "georeferencja"]);
    // The CRS comes from the data, not from the schema.
    assert_eq!(geo_metadata(&out)["columns"]["georeferencja"]["crs"]["id"]["code"], 2180);
}

#[test]
fn convert_to_ipc_keeps_geoarrow_types() {
    let dir = out_dir("convert_ipc");
    // A sampled read makes `Auto` geometry WKB (`docs/schema-inference.md`
    // §6.2); a scanned schema has the native type.
    let settings = dir.join("prg.json");
    xeibe_ok(&["scan", &sample(PRG), "-o", path_str(&settings)]);
    let out = dir.join("points.arrow");
    xeibe_ok(&[
        "convert",
        &sample(PRG),
        "--layer",
        "AD_PunktAdresowy",
        "--settings",
        path_str(&settings),
        "--format",
        "ipc",
        "-o",
        path_str(&out),
    ]);

    let reader = arrow_ipc::reader::FileReader::try_new(File::open(&out).unwrap(), None).unwrap();
    let schema = reader.schema();
    let field = schema.field_with_name("georeferencja").unwrap();
    assert_eq!(field.metadata().get("ARROW:extension:name").map(String::as_str), Some("geoarrow.point"));
    let crs: serde_json::Value =
        serde_json::from_str(&field.metadata()["ARROW:extension:metadata"]).expect("extension metadata");
    assert_eq!(crs["crs"]["id"]["code"], 2180);
    let rows: usize = reader.map(|batch| batch.unwrap().num_rows()).sum();
    assert_eq!(rows, 2);
}

#[test]
fn curves_are_an_error_for_parquet_without_linearize() {
    let dir = out_dir("convert_curves");
    let out = dir.join("buildings.parquet");
    let run = xeibe(&["convert", &sample(RCN_ARCS), "--layer", "RCN_Budynek", "-o", path_str(&out)]);
    assert!(!run.success, "curves must not be written to Parquet\n{}", run.stderr);
    assert!(run.stderr.contains("RCN_Budynek"), "names the layer: {}", run.stderr);
    assert!(run.stderr.contains("geometria"), "names the column: {}", run.stderr);
    assert!(run.stderr.contains("CurvePolygon"), "names the curve: {}", run.stderr);
    assert!(run.stderr.contains("--linearize"), "names the option: {}", run.stderr);
    assert!(!out.exists(), "the partial output is removed");
}

#[test]
fn linearize_makes_curves_writable_to_parquet() {
    let dir = out_dir("convert_linearize");
    let out = dir.join("buildings.parquet");
    xeibe_ok(&["convert", &sample(RCN_ARCS), "--layer", "RCN_Budynek", "--linearize", "-o", path_str(&out)]);
    assert_eq!(parquet_reader(&out).metadata().file_metadata().num_rows(), 3);
    let types = geo_metadata(&out)["columns"]["geometria"]["geometry_types"].clone();
    let types: Vec<String> = serde_json::from_value(types).unwrap();
    assert!(!types.is_empty() && types.iter().all(|t| t == "Polygon" || t == "MultiPolygon"), "{types:?}");
    let crs = &geo_metadata(&out)["columns"]["geometria"]["crs"];
    assert!(crs.is_object(), "{crs}");

    // A finer step gives more vertices.
    let fine = dir.join("fine.parquet");
    xeibe_ok(&["convert", &sample(RCN_ARCS), "--layer", "RCN_Budynek", "--linearize", "1", "-o", path_str(&fine)]);
    let size = |path: &std::path::Path| std::fs::metadata(path).unwrap().len();
    assert!(size(&fine) > size(&out), "--linearize 1 should add vertices");
}

#[test]
fn curves_are_kept_in_ipc() {
    let dir = out_dir("convert_curves_ipc");
    let out = dir.join("buildings.arrow");
    xeibe_ok(&["convert", &sample(RCN_ARCS), "--layer", "RCN_Budynek", "--format", "ipc", "-o", path_str(&out)]);
    let reader = arrow_ipc::reader::FileReader::try_new(File::open(&out).unwrap(), None).unwrap();
    let schema = reader.schema();
    let field = schema.field_with_name("geometria").unwrap();
    assert_eq!(field.metadata().get("ARROW:extension:name").map(String::as_str), Some("geoarrow.wkb"));
    let batch = reader.into_iter().next().unwrap().unwrap();
    let column = batch.column_by_name("geometria").unwrap().as_binary::<i32>().clone();
    // ISO WKB CurvePolygon (10) somewhere in the layer.
    let has_curve = column
        .iter()
        .flatten()
        .any(|wkb| u32::from_le_bytes(wkb[1..5].try_into().unwrap()) % 1000 == 10);
    assert!(has_curve, "the curves are kept");
}

#[test]
fn convert_needs_a_layer() {
    let dir = out_dir("convert_no_layer");
    let run = xeibe(&["convert", &sample(PRG), "-o", path_str(&dir.join("x.parquet"))]);
    assert!(!run.success);
    assert!(run.stderr.contains("--layer"), "{}", run.stderr);
}

#[test]
fn convert_of_an_unknown_layer_fails() {
    let dir = out_dir("convert_unknown_layer");
    let out = dir.join("x.parquet");
    let run = xeibe(&["convert", &sample(PRG), "--layer", "NoSuchLayer", "-o", path_str(&out)]);
    assert!(!run.success);
    assert!(run.stderr.contains("NoSuchLayer"), "{}", run.stderr);
}

#[test]
fn a_bad_override_is_reported() {
    let dir = out_dir("convert_bad_override");
    let run = xeibe(&[
        "convert",
        &sample(PRG),
        "--layer",
        "AD_PunktAdresowy",
        "--override",
        "*/kodPocztowy=NotAType",
        "-o",
        path_str(&dir.join("x.parquet")),
    ]);
    assert!(!run.success);
    assert!(run.stderr.contains("--override"), "{}", run.stderr);
}

#[test]
fn overrides_change_column_types() {
    let dir = out_dir("convert_override");
    let out = dir.join("points.parquet");
    xeibe_ok(&[
        "convert",
        &sample(PRG),
        "--layer",
        "AD_PunktAdresowy",
        "--override",
        "*/numerPorzadkowy=utf8",
        "-o",
        path_str(&out),
    ]);
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(&out).unwrap()).unwrap();
    let field = builder.schema().field_with_name("numerPorzadkowy").unwrap().clone();
    assert!(field.data_type().to_string().starts_with("Utf8"), "{:?}", field.data_type());
}
