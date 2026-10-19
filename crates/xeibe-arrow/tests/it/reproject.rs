//! Reads with a target CRS (`docs/geometry.md`, "Reprojection"): geometries
//! convert after parsing, the column takes the target's CRS, grids load
//! before the read, and what can't be converted is a feature error.

use arrow_array::cast::AsArray;
use arrow_array::types::Float64Type;
use xeibe_arrow::{GridLoader, OnFeatureError, ReadOptions, read};
use xeibe_schema::rules::meta;
use xeibe_testkit::gml;
use xeibe_testkit::wkt::{Tol, assert_wkt_tol};

use crate::support::{read_with, sources};

fn parcel(id: &str, geometry: &str) -> String {
    gml::feature("Parcel", id, &format!("<app:geom>{geometry}</app:geom>"))
}

fn point(srs: &str, pos: &str) -> String {
    format!("<gml:Point srsName=\"{srs}\"><gml:pos>{pos}</gml:pos></gml:Point>")
}

fn to(target: &str) -> ReadOptions {
    let mut options = ReadOptions::default();
    options.geometry.target_crs = Some(target.to_string());
    options
}

const MM: Tol = Tol {
    abs: 1e-6,
    rel: 0.0,
};

/// ETRS89, latitude first as the URN says, at 52.2°N 21°E.
const WARSAW_4258: &str = "52.2 21.0";
/// PROJ 9.8.1's result in ETRS89 / PL-1992 (EPSG:2180), easting first.
const WARSAW_2180: &str = "POINT (636640.467013 483432.200828)";

#[test]
fn geometries_convert_and_the_column_takes_the_target_crs() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        &point("urn:ogc:def:crs:EPSG::4258", WARSAW_4258),
    )]);
    let read = read_with(&document, "Parcel", None, &to("EPSG:2180"));
    let geometries = read.geometries("geom");
    assert_wkt_tol(geometries[0].as_ref().expect("a point"), WARSAW_2180, MM);

    let field = read.field("geom");
    assert_eq!(field.metadata()[meta::SRS_NAME], "EPSG:2180");
    let crs: serde_json::Value =
        serde_json::from_str(&field.metadata()["ARROW:extension:metadata"]).unwrap();
    assert_eq!(crs["crs"]["id"]["code"], 2180, "{crs}");

    let [reprojection] = read.report.reprojections.as_slice() else {
        panic!("one srsName: {:?}", read.report.reprojections);
    };
    assert_eq!(
        reprojection.srs_name.as_deref(),
        Some("urn:ogc:def:crs:EPSG::4258")
    );
    assert_eq!(reprojection.error, None);
    assert!(reprojection.grids.is_empty());
}

#[test]
fn a_column_of_several_crss_converts_to_one() {
    // Without a target CRS the second feature is a feature error (another
    // CRS than the column's).
    let document = gml::gml32_collection(&[
        &parcel("p1", &point("urn:ogc:def:crs:EPSG::4258", WARSAW_4258)),
        &parcel("p2", &point("EPSG:2180", "636640.467013 483432.200828")),
    ]);
    let read = read_with(&document, "Parcel", None, &to("EPSG:2180"));
    for geometry in read.geometries("geom") {
        assert_wkt_tol(geometry.as_ref().expect("a point"), WARSAW_2180, MM);
    }
    assert_eq!(read.report.reprojections.len(), 2);
}

#[test]
fn a_bounded_by_box_converts_by_its_edges() {
    let mut options = to("EPSG:2180");
    options.inference.gml.bounded_by = xeibe_schema::options::BoundedBy::BoxStruct;
    let envelope = concat!(
        "<gml:boundedBy><gml:Envelope srsName=\"urn:ogc:def:crs:EPSG::4258\">",
        "<gml:lowerCorner>52 20</gml:lowerCorner><gml:upperCorner>53 22</gml:upperCorner>",
        "</gml:Envelope></gml:boundedBy>"
    );
    let document = gml::gml32_collection(&[&gml::feature(
        "Parcel",
        "p1",
        &format!(
            "{envelope}<app:geom>{}</app:geom>",
            point("urn:ogc:def:crs:EPSG::4258", WARSAW_4258)
        ),
    )]);
    let read = read_with(&document, "Parcel", None, &options);
    let boxes = read.batches[0]
        .column_by_name("boundedBy")
        .expect("a box column")
        .as_struct()
        .clone();
    let value = |name: &str| {
        boxes
            .column_by_name(name)
            .expect(name)
            .as_primitive::<Float64Type>()
            .value(0)
    };
    let transform =
        xeibe_reproject::Transform::from_epsg(4258, 2180, xeibe_reproject::AxisOrder::EastFirst)
            .unwrap();
    let want = transform
        .convert_bounds([20.0, 52.0, 22.0, 53.0], xeibe_reproject::DEFAULT_DENSIFY)
        .unwrap();
    let got = [value("xmin"), value("ymin"), value("xmax"), value("ymax")];
    for (got, want) in got.iter().zip(want) {
        assert!((got - want).abs() < 1e-6, "{got:?} against {want:?}");
    }
    let field = read.field("boundedBy");
    assert_eq!(field.metadata()[meta::SRS_NAME], "EPSG:2180");
}

#[test]
fn arcs_convert_only_when_linearized() {
    let arc = concat!(
        "<gml:Curve srsName=\"urn:ogc:def:crs:EPSG::4258\"><gml:segments><gml:ArcString>",
        "<gml:posList>52.2 21.0 52.21 21.01 52.2 21.02</gml:posList>",
        "</gml:ArcString></gml:segments></gml:Curve>"
    );
    let document = gml::gml32_collection(&[&parcel("p1", arc)]);
    let mut options = to("EPSG:2180");
    options.on_feature_error = OnFeatureError::Skip;
    let read = read_with(&document, "Parcel", None, &options);
    assert_eq!(read.rows(), 0);
    let (_, reason) = &read.report.skipped[0];
    assert!(reason.contains("linearize"), "{reason}");

    options.geometry.curves =
        xeibe_geom::options::CurveMode::Linearize(xeibe_geom::options::LinearizeOptions::default());
    let read = read_with(&document, "Parcel", None, &options);
    assert_eq!(read.rows(), 1);
    let geometries = read.geometries("geom");
    let line = geometries[0].as_ref().expect("a line");
    assert!(
        line.to_string().starts_with("LINESTRING (636640.467"),
        "{line}"
    );
}

#[test]
fn a_geometry_without_crs_is_a_feature_error() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        "<gml:Point><gml:pos>21.0 52.2</gml:pos></gml:Point>",
    )]);
    let mut options = to("EPSG:2180");
    options.on_feature_error = OnFeatureError::Skip;
    let read = read_with(&document, "Parcel", None, &options);
    let (_, reason) = &read.report.skipped[0];
    assert!(reason.contains("without srsName"), "{reason}");

    // With `crs_override`, it is in that CRS.
    options.geometry.crs_override = Some("EPSG:4258".into());
    let read = read_with(&document, "Parcel", None, &options);
    assert_wkt_tol(
        read.geometries("geom")[0].as_ref().expect("a point"),
        WARSAW_2180,
        MM,
    );
}

#[test]
fn an_unknown_target_crs_is_an_option_error() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        &point("urn:ogc:def:crs:EPSG::4258", WARSAW_4258),
    )]);
    let error = read(sources(&document), "Parcel", None, &to("EPSG:999999"))
        .err()
        .expect("no such CRS");
    assert!(
        matches!(&error, xeibe_arrow::Error::Option { key, .. } if key == "target_crs"),
        "{error}"
    );
}

/// St. George Island (NAD27, EPSG:4138), latitude first, and PROJ 9.8.1's
/// result in NAD83 (EPSG:4269) through NADCON's grid
/// (`tests/data/reprojection/selection-grids.jsonl`).
const ST_GEORGE_4138: &str = "56.518977129636866 -169.79423565632723";
const ST_GEORGE_4269: &str = "POINT (-169.79666831384327 56.51999536461077)";

#[test]
fn grids_load_from_a_directory_before_the_read() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        &point("urn:ogc:def:crs:EPSG::4138", ST_GEORGE_4138),
    )]);
    let mut options = to("EPSG:4269");
    options
        .set(
            "grid_dir",
            crate::support::path("reprojection/grids").to_str().unwrap(),
        )
        .unwrap();
    let read = read_with(&document, "Parcel", None, &options);
    assert_wkt_tol(
        read.geometries("geom")[0].as_ref().expect("a point"),
        ST_GEORGE_4269,
        Tol {
            abs: 1e-11,
            rel: 0.0,
        },
    );
    let [reprojection] = read.report.reprojections.as_slice() else {
        panic!("one srsName: {:?}", read.report.reprojections);
    };
    assert!(!reprojection.grids.is_empty());
}

/// OSGB 1936 to ETRS89: OSTN15's grid is not committed, so no test loads it.
#[test]
fn a_grid_that_cant_be_loaded_fails_the_read_before_it_starts() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        &point("urn:ogc:def:crs:EPSG::4277", "52.5 -1.5"),
    )]);
    let mut options = to("EPSG:4258");
    let error = read(sources(&document), "Parcel", None, &options)
        .err()
        .expect("no grid loader");
    assert!(
        error
            .to_string()
            .contains("uk_os_OSTN15_NTv2_OSGBtoETRS.tif"),
        "{error}"
    );

    options.grid_loader = Some(GridLoader::new(|_| Err("offline".into())));
    let error = read(sources(&document), "Parcel", None, &options)
        .err()
        .expect("the loader fails");
    assert!(error.to_string().contains("offline"), "{error}");

    // Without grids, EPSG's Helmert transformation applies.
    options.geometry.no_grids = true;
    let read = read_with(&document, "Parcel", None, &options);
    let point = read.geometries("geom")[0].clone().expect("a point");
    assert!(point.to_string().starts_with("POINT (-1.50"), "{point}");
}
