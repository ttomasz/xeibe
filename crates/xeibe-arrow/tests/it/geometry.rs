//! Geometry columns read end to end (`docs/geometry.md`, "Column encoding",
//! "Several geometry columns", "Empty, invalid and degenerate geometry").

use arrow_array::Array;
use arrow_array::cast::AsArray;
use arrow_array::types::Float64Type;
use xeibe_arrow::{ColumnSpec, OnFeatureError, ReadOptions, Settings, read, scan};
use xeibe_schema::ScanExtent;
use xeibe_schema::rules::meta;
use xeibe_testkit::gml;
use xeibe_testkit::wkt::assert_wkt;

use crate::support::{Read, extension_name, read_document, read_with, sources};

fn parcel(id: &str, body: &str) -> String {
    gml::feature("Parcel", id, body)
}

fn point(x: f64, y: f64) -> String {
    format!("<gml:Point srsName=\"EPSG:2180\"><gml:pos>{x} {y}</gml:pos></gml:Point>")
}

const RING: &str =
    "<gml:exterior><gml:LinearRing><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gml:LinearRing></gml:exterior>";

/// Read with the schema of a full scan: `Auto` picks native types there,
/// where a read's own sample makes WKB.
fn read_scanned(document: &str, layer: &str) -> (Read, Settings) {
    let scan = scan(sources(document), ScanExtent::Full, &ReadOptions::default()).expect("a scan");
    let schema = scan.arrow_schema(layer).expect("a schema");
    let settings = scan.to_settings().expect("settings");
    (read_with(document, layer, Some(schema), &ReadOptions::default()), settings)
}

/// The settings-file type the scan wrote for a column.
fn scanned_type(settings: &Settings, column: &str) -> String {
    let (_, columns) = settings.layers.first().expect("a layer");
    match &columns[column] {
        ColumnSpec::Type(data_type) | ColumnSpec::Detailed { data_type, .. } => data_type.clone(),
    }
}

#[test]
fn native_columns_from_a_full_scan_hold_the_geometries() {
    let document = gml::gml32_collection(&[
        &parcel(
            "p1",
            &format!(
                "<app:geom>{}</app:geom><app:shape><gml:Polygon srsName=\"EPSG:2180\">{RING}</gml:Polygon></app:shape>",
                point(1.0, 2.0)
            ),
        ),
        &parcel(
            "p2",
            &format!(
                "<app:geom>{}</app:geom><app:shape><gml:MultiSurface srsName=\"EPSG:2180\"><gml:surfaceMember><gml:Polygon>{RING}</gml:Polygon></gml:surfaceMember></gml:MultiSurface></app:shape>",
                point(3.0, 4.0)
            ),
        ),
    ]);
    let (read, settings) = read_scanned(&document, "Parcel");
    assert_eq!(extension_name(&read.field("geom")), Some("geoarrow.point"));
    assert_eq!(scanned_type(&settings, "geom"), "geometry(Point)");
    let points = read.native_geometries("geom");
    assert_wkt(points[0].as_ref().expect("a point"), "POINT (1 2)");
    assert_wkt(points[1].as_ref().expect("a point"), "POINT (3 4)");

    // A kind and its Multi form: the Multi type, so the Polygon reads as a
    // one-part MultiPolygon (`docs/geometry.md`, "Column encoding").
    assert_eq!(extension_name(&read.field("shape")), Some("geoarrow.multipolygon"));
    let shapes = read.native_geometries("shape");
    for shape in &shapes {
        assert_wkt(shape.as_ref().expect("a shape"), "MULTIPOLYGON (((0 0,1 0,1 1,0 0)))");
    }
}

#[test]
fn several_geometry_properties_stay_separate_columns() {
    // PRG's `AD_Miejscowosc` has `geometria` and `pozycja`; each property is
    // a column of its own, with its own type and CRS (`docs/geometry.md`,
    // "Several geometry columns").
    let document = gml::gml32_collection(&[
        &parcel(
            "p1",
            &format!(
                concat!(
                    "<app:geometria><gml:Polygon srsName=\"EPSG:2180\">{}</gml:Polygon></app:geometria>",
                    "<app:pozycja><gml:Point srsName=\"EPSG:4326\"><gml:pos>52 21</gml:pos></gml:Point></app:pozycja>"
                ),
                RING
            ),
        ),
        &parcel(
            "p2",
            &format!("<app:geometria><gml:Polygon srsName=\"EPSG:2180\">{RING}</gml:Polygon></app:geometria>"),
        ),
    ]);
    let read = read_document(&document, "Parcel");
    assert_eq!(read.column_names(), ["@id", "geometria", "pozycja"]);
    let srs = |column: &str| read.field(column).metadata().get(meta::SRS_NAME).cloned();
    assert_eq!(srs("geometria").as_deref(), Some("EPSG:2180"));
    assert_eq!(srs("pozycja").as_deref(), Some("EPSG:4326"));

    let areas = read.geometries("geometria");
    assert!(areas.iter().all(Option::is_some), "{areas:?}");
    let points = read.geometries("pozycja");
    assert!(points[0].is_some());
    assert_eq!(points[1], None, "null where the feature has no such property");
}

#[test]
fn an_empty_geometry_element_is_an_empty_geometry_not_null() {
    let document = gml::gml32_collection(&[
        &parcel("p1", "<app:geom><gml:Point srsName=\"EPSG:2180\"/></app:geom>"),
        &parcel("p2", &format!("<app:geom>{}</app:geom>", point(1.0, 2.0))),
        &parcel("p3", ""),
    ]);
    let read = read_document(&document, "Parcel");
    let geometries = read.geometries("geom");
    assert_wkt(geometries[0].as_ref().expect("an empty point, not null"), "POINT EMPTY");
    assert_wkt(geometries[1].as_ref().expect("a point"), "POINT (1 2)");
    assert_eq!(geometries[2], None, "a missing property is null");
}

#[test]
fn mixed_2d_and_3d_in_one_column_is_xyz_with_a_nan_z() {
    let document = gml::gml32_collection(&[
        &parcel("p1", &format!("<app:geom>{}</app:geom>", point(1.0, 2.0))),
        &parcel(
            "p2",
            "<app:geom><gml:Point srsName=\"EPSG:2180\" srsDimension=\"3\"><gml:pos>3 4 5</gml:pos></gml:Point></app:geom>",
        ),
    ]);
    let (read, settings) = read_scanned(&document, "Parcel");
    assert_eq!(scanned_type(&settings, "geom"), "geometry(Point, XYZ)");
    let points = read.native_geometries("geom");
    let first = points[0].as_ref().expect("a point").first_vertex().expect("a vertex");
    assert_eq!(first[..2], [1.0, 2.0]);
    assert!(first[2].is_nan(), "a 2D value gets a NaN Z: {first:?}");
    assert_wkt(points[1].as_ref().expect("a point"), "POINT Z (3 4 5)");
}

#[test]
fn a_z_value_in_an_xy_column_is_a_geometry_error() {
    // Dimensions come from the column's type (`docs/geometry.md`, "Options").
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        "<app:area>1</app:area><app:geom><gml:Point srsName=\"EPSG:2180\" srsDimension=\"3\"><gml:pos>3 4 5</gml:pos></gml:Point></app:geom>",
    )]);
    let settings: Settings = serde_json::from_str(
        r#"{ "format_version": 1, "layers": { "Parcel": { "area": "bigint", "geom": "geometry(Point)" } } }"#,
    )
    .expect("settings");
    let schema = settings.schema("Parcel").expect("a schema");
    let failed = match read(sources(&document), "Parcel", Some(schema.clone()), &ReadOptions::default()) {
        Err(_) => true,
        Ok(reader) => reader.into_iter().any(|batch| batch.is_err()),
    };
    assert!(failed, "the default policy stops the read");

    let null_geometry = ReadOptions { on_feature_error: OnFeatureError::NullGeometry, ..ReadOptions::default() };
    let read = read_with(&document, "Parcel", Some(schema), &null_geometry);
    assert_eq!(read.i64s("area"), [Some(1)]);
    assert_eq!(read.native_geometries("geom"), [None]);
}

#[test]
fn gml_2_generic_geometry_properties_are_geometry_columns() {
    let document = gml::gml2_collection(&[concat!(
        "<app:Parcel fid=\"p1\">",
        "<gml:pointProperty><gml:Point srsName=\"EPSG:2180\"><gml:coordinates>1,2</gml:coordinates></gml:Point></gml:pointProperty>",
        "<gml:polygonProperty><gml:Polygon srsName=\"EPSG:2180\"><gml:outerBoundaryIs><gml:LinearRing>",
        "<gml:coordinates>0,0 1,0 1,1 0,0</gml:coordinates>",
        "</gml:LinearRing></gml:outerBoundaryIs></gml:Polygon></gml:polygonProperty>",
        "</app:Parcel>"
    )]);
    let read = read_document(&document, "Parcel");
    assert_wkt(read.geometries("pointProperty")[0].as_ref().expect("a point"), "POINT (1 2)");
    assert_wkt(
        read.geometries("polygonProperty")[0].as_ref().expect("a polygon"),
        "POLYGON ((0 0,1 0,1 1,0 0))",
    );
}

#[test]
fn a_bounded_by_box_column_holds_the_envelope() {
    let mut options = ReadOptions::default();
    options.inference.gml.bounded_by = xeibe_schema::options::BoundedBy::BoxStruct;
    let envelope = concat!(
        "<gml:boundedBy><gml:Envelope srsName=\"EPSG:2180\">",
        "<gml:lowerCorner>10 20</gml:lowerCorner><gml:upperCorner>30 40</gml:upperCorner>",
        "</gml:Envelope></gml:boundedBy>"
    );
    let document = gml::gml32_collection(&[&parcel("p1", &format!("{envelope}<app:area>1</app:area>"))]);
    let read = crate::support::read_with(&document, "Parcel", None, &options);
    assert_eq!(extension_name(&read.field("boundedBy")), Some("geoarrow.box"));
    let column = read.batches[0].column_by_name("boundedBy").expect("a box column");
    let boxes = column.as_struct();
    let value = |name: &str| boxes.column_by_name(name).expect(name).as_primitive::<Float64Type>().value(0);
    assert_eq!([value("xmin"), value("ymin"), value("xmax"), value("ymax")], [10.0, 20.0, 30.0, 40.0]);
    assert!(!boxes.is_null(0));
}

#[test]
fn a_crs_without_projjson_is_written_as_an_authority_code() {
    // PROJJSON exists for EPSG codes only; other CRSs fall back to
    // `authority:code` (`docs/geometry.md`, "CRS metadata").
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        "<app:geom><gml:Point srsName=\"urn:ogc:def:crs:OGC:1.3:CRS84\"><gml:pos>21 52</gml:pos></gml:Point></app:geom>",
    )]);
    let (read, _) = read_scanned(&document, "Parcel");
    let metadata = read.field("geom").metadata().get("ARROW:extension:metadata").cloned().unwrap_or_default();
    assert!(metadata.contains("authority_code") && metadata.contains("OGC:CRS84"), "{metadata}");

    let (read, _) = read_scanned(
        &gml::gml32_collection(&[&parcel("p1", &format!("<app:geom>{}</app:geom>", point(1.0, 2.0)))]),
        "Parcel",
    );
    let metadata = read.field("geom").metadata().get("ARROW:extension:metadata").cloned().unwrap_or_default();
    assert!(metadata.contains("projjson"), "EPSG codes get PROJJSON: {metadata}");
}

#[test]
fn an_array_property_is_read_as_the_matching_multi_geometry() {
    // `gml:pointArrayProperty`, `curveArrayProperty` and `surfaceArrayProperty`
    // hold several geometries: one value of the Multi kind, also for one part
    // (`docs/geometry.md`, "Empty, invalid and degenerate geometry").
    let line = |a: &str| format!("<gml:LineString srsName=\"EPSG:2180\"><gml:posList>{a}</gml:posList></gml:LineString>");
    let polygon = format!("<gml:Polygon srsName=\"EPSG:2180\">{RING}</gml:Polygon>");
    let document = gml::gml32_collection(&[
        &parcel(
            "p1",
            &format!(
                concat!(
                    "<gml:pointArrayProperty>{}{}{}</gml:pointArrayProperty>",
                    "<gml:curveArrayProperty>{}{}</gml:curveArrayProperty>",
                    "<gml:surfaceArrayProperty>{}{}</gml:surfaceArrayProperty>"
                ),
                point(1.0, 2.0),
                point(3.0, 4.0),
                point(5.0, 6.0),
                line("0 0 1 1"),
                line("2 2 3 3"),
                polygon,
                polygon
            ),
        ),
        &parcel("p2", &format!("<gml:pointArrayProperty>{}</gml:pointArrayProperty>", point(7.0, 8.0))),
    ]);

    // A read's own sample: WKB.
    let read = read_document(&document, "Parcel");
    let points = read.geometries("pointArrayProperty");
    assert_wkt(points[0].as_ref().expect("points"), "MULTIPOINT ((1 2),(3 4),(5 6))");
    assert_wkt(points[1].as_ref().expect("points"), "MULTIPOINT ((7 8))");
    assert_wkt(
        read.geometries("curveArrayProperty")[0].as_ref().expect("curves"),
        "MULTILINESTRING ((0 0,1 1),(2 2,3 3))",
    );
    assert_wkt(
        read.geometries("surfaceArrayProperty")[0].as_ref().expect("surfaces"),
        "MULTIPOLYGON (((0 0,1 0,1 1,0 0)),((0 0,1 0,1 1,0 0)))",
    );

    // A full scan types an array property by its Multi kind: a native Multi
    // column.
    let (read, settings) = read_scanned(&document, "Parcel");
    assert_eq!(scanned_type(&settings, "pointArrayProperty"), "geometry(MultiPoint)");
    assert_eq!(scanned_type(&settings, "curveArrayProperty"), "geometry(MultiLineString)");
    assert_eq!(scanned_type(&settings, "surfaceArrayProperty"), "geometry(MultiPolygon)");
    let points = read.native_geometries("pointArrayProperty");
    assert_wkt(points[0].as_ref().expect("points"), "MULTIPOINT ((1 2),(3 4),(5 6))");
    assert_wkt(points[1].as_ref().expect("points"), "MULTIPOINT ((7 8))");
}

#[test]
fn a_second_geometry_in_an_ordinary_property_is_a_feature_error() {
    // Only GML's array properties hold several geometries; anywhere else a
    // second one is a second value where the column holds one. It is not a
    // geometry error, so `NullGeometry` doesn't keep the feature.
    let document = gml::gml32_collection(&[
        &parcel("p1", &format!("<app:geom>{}</app:geom>", point(1.0, 2.0))),
        &parcel("p2", &format!("<app:geom>{}{}</app:geom>", point(3.0, 4.0), point(5.0, 6.0))),
    ]);
    for policy in [OnFeatureError::Error, OnFeatureError::NullGeometry] {
        let options = ReadOptions { on_feature_error: policy, ..ReadOptions::default() };
        let failed = match read(sources(&document), "Parcel", None, &options) {
            Err(_) => true,
            Ok(reader) => reader.into_iter().any(|batch| batch.is_err()),
        };
        assert!(failed, "{policy:?} stops the read");
    }
    let skipping = ReadOptions { on_feature_error: OnFeatureError::Skip, ..ReadOptions::default() };
    let read = read_with(&document, "Parcel", None, &skipping);
    assert_eq!(read.rows(), 1);
    assert!(read.report.skipped[0].1.contains("second geometry"), "{:?}", read.report.skipped);
}

#[test]
fn a_broken_part_nulls_the_whole_array() {
    // With `NullGeometry`, a geometry error nulls the column's value, never
    // leaves the parts that could be read.
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        &format!(
            "<app:area>1</app:area><gml:pointArrayProperty>{}<gml:Point srsName=\"EPSG:2180\"><gml:pos>x y</gml:pos></gml:Point></gml:pointArrayProperty>",
            point(1.0, 2.0)
        ),
    )]);
    let options = ReadOptions { on_feature_error: OnFeatureError::NullGeometry, ..ReadOptions::default() };
    let read = read_with(&document, "Parcel", None, &options);
    assert_eq!(read.i64s("area"), [Some(1)]);
    assert_eq!(read.geometries("pointArrayProperty"), [None]);
}
