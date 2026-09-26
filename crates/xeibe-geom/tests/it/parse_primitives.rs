//! `Point`, `LineString`, `LinearRing`, the coordinate carriers in context,
//! srsName/srsDimension inheritance and axis swapping
//! (`docs/geometry.md`, "Mapping table", "Coordinates", "srsName inheritance").

use xeibe_core::Dialect;
use xeibe_geom::model::GeomKind;
use xeibe_geom::parse::ParseContext;
use xeibe_geom::{GeometryOptions, Error};
use xeibe_testkit::wkt::assert_wkt;

use crate::support::{
    FixedAxis, RecordingAxis, assert_geometry, g, g31, parse, parse_gml31, parse_in_context,
    parse_with, warnings,
};

#[test]
fn point_from_pos_and_coordinates() {
    assert_geometry("<gml:Point><gml:pos>1 2</gml:pos></gml:Point>", "POINT (1 2)");
    assert_geometry(
        "<gml:Point srsDimension=\"3\"><gml:pos>31 29 16</gml:pos></gml:Point>",
        "POINT Z (31 29 16)",
    );
    // A `pos` with three values is 3D even without srsDimension (rule 4).
    assert_geometry(
        "<gml:Point><gml:pos>31 29 16</gml:pos></gml:Point>",
        "POINT Z (31 29 16)",
    );
    assert_wkt(
        &g31("<gml:Point><gml:coordinates>1,2</gml:coordinates></gml:Point>"),
        "POINT (1 2)",
    );
    // GML 2 `coord` (deprecated, still common).
    assert_wkt(
        &g31("<gml:Point><gml:coord><gml:X>1</gml:X><gml:Y>2</gml:Y></gml:coord></gml:Point>"),
        "POINT (1 2)",
    );
    assert_wkt(
        &g31(concat!(
            "<gml:Point><gml:coord><gml:X>1</gml:X><gml:Y>2</gml:Y>",
            "<gml:Z>3</gml:Z></gml:coord></gml:Point>"
        )),
        "POINT Z (1 2 3)",
    );
}

#[test]
fn an_empty_geometry_element_is_an_empty_geometry_not_null() {
    // GDAL returns no geometry here; we keep an empty one (`docs/geometry.md`,
    // "Empty, invalid and degenerate geometry").
    assert_geometry("<gml:Point/>", "POINT EMPTY");
    assert_geometry("<gml:LineString/>", "LINESTRING EMPTY");
    assert_geometry(
        "<gml:LineString><gml:posList count=\"0\"></gml:posList></gml:LineString>",
        "LINESTRING EMPTY",
    );
}

#[test]
fn more_than_one_position_in_a_point_is_an_error() {
    assert!(parse("<gml:Point><gml:pos>1 2</gml:pos><gml:pos>3 4</gml:pos></gml:Point>").is_err());
    assert!(parse("<gml:Point><gml:pos>0</gml:pos></gml:Point>").is_err());
}

#[test]
fn line_string_from_every_carrier() {
    assert_geometry(
        "<gml:LineString><gml:posList>1 2 3 4</gml:posList></gml:LineString>",
        "LINESTRING (1 2,3 4)",
    );
    assert_geometry(
        "<gml:LineString><gml:pos>1 2</gml:pos><gml:pos>3 4</gml:pos></gml:LineString>",
        "LINESTRING (1 2,3 4)",
    );
    assert_wkt(
        &g31("<gml:LineString><gml:coordinates>1,2 3,4</gml:coordinates></gml:LineString>"),
        "LINESTRING (1 2,3 4)",
    );
    // Inline `pointProperty`/`pointRep`, mixed with `pos` (§10.1.4.3–4).
    assert_geometry(
        concat!(
            "<gml:LineString><gml:pos>1 2</gml:pos>",
            "<gml:pointProperty><gml:Point><gml:pos>3 4</gml:pos></gml:Point></gml:pointProperty>",
            "</gml:LineString>"
        ),
        "LINESTRING (1 2,3 4)",
    );
}

#[test]
fn a_line_string_needs_two_positions() {
    let error = parse("<gml:LineString><gml:posList>1 2</gml:posList></gml:LineString>")
        .expect_err("one position");
    assert!(
        matches!(error, Error::PositionCount { .. }),
        "got {error:?}"
    );
}

#[test]
fn a_point_given_by_reference_is_not_resolved() {
    // xlink:href is never followed (`docs/README.md`, non-goals).
    let error = parse(
        r##"<gml:LineString><gml:pointProperty xlink:href="#p1"/><gml:pos>3 4</gml:pos></gml:LineString>"##,
    )
    .expect_err("a referenced point");
    assert!(
        matches!(error, Error::ByReference { .. } | Error::Unsupported { .. }),
        "got {error:?}"
    );
}

#[test]
fn a_ring_with_fewer_than_four_positions_is_an_error() {
    let snippet = concat!(
        "<gml:Polygon><gml:exterior><gml:LinearRing>",
        "<gml:posList>0 0 1 1 0 0</gml:posList>",
        "</gml:LinearRing></gml:exterior></gml:Polygon>"
    );
    assert!(parse(snippet).is_err(), "a ring needs at least 4 positions");
}

#[test]
fn an_unclosed_ring_is_closed_with_a_warning() {
    // GDAL refuses this. An unclosed ring isn't valid in WKB or GeoParquet, and
    // repeating the first position loses nothing, so it is always closed; there
    // is no option (`docs/geometry.md`, "Empty, invalid and degenerate geometry").
    let snippet = concat!(
        "<gml:Polygon><gml:exterior><gml:LinearRing>",
        "<gml:posList>0 0 1 0 1 1 0 1</gml:posList>",
        "</gml:LinearRing></gml:exterior></gml:Polygon>"
    );
    assert_geometry(snippet, "POLYGON ((0 0,1 0,1 1,0 1,0 0))");
    assert!(!warnings(snippet).is_empty(), "an unclosed ring warns");
}

#[test]
fn the_axis_decision_is_applied_to_every_position() {
    let snippet = r#"<gml:LineString srsName="EPSG:4326"><gml:posList>1 2 3 4</gml:posList></gml:LineString>"#;
    let parsed = parse_with(snippet, &GeometryOptions::default(), &FixedAxis::swap())
        .expect("a line string");
    assert_wkt(
        &crate::support::to_g(&parsed.geometry.unwrap()),
        "LINESTRING (2 1,4 3)",
    );

    // Z stays in place.
    let snippet3d = r#"<gml:Point srsName="EPSG:4979" srsDimension="3"><gml:pos>1 2 3</gml:pos></gml:Point>"#;
    let parsed = parse_with(snippet3d, &GeometryOptions::default(), &FixedAxis::swap())
        .expect("a point");
    assert_wkt(
        &crate::support::to_g(&parsed.geometry.unwrap()),
        "POINT Z (2 1 3)",
    );
}

#[test]
fn the_axis_decision_is_asked_once_per_srs_name_and_dialect() {
    let axis = RecordingAxis::default();
    let snippet = r#"<gml:Point srsName="EPSG:2180"><gml:pos>1 2</gml:pos></gml:Point>"#;
    parse_with(snippet, &GeometryOptions::default(), &axis).expect("a point");
    let calls = axis.calls();
    assert!(!calls.is_empty(), "the parser asks for a decision");
    assert!(
        calls
            .iter()
            .all(|(srs, dialect)| srs.as_deref() == Some("EPSG:2180") && *dialect == Dialect::Gml3),
        "got {calls:?}"
    );
}

#[test]
fn the_dialect_comes_from_the_elements_used() {
    // `coordinates` in a GML 2 structure is GML 2; in a GML 3 structure it is
    // GML 3 (`docs/geometry.md`, "Modes").
    let axis = RecordingAxis::default();
    crate::support::parse_in(
        &xeibe_testkit::gml::geometry_document_gml31(
            r#"<gml:Point srsName="EPSG:4326"><gml:coordinates>1,2</gml:coordinates></gml:Point>"#,
        ),
        &GeometryOptions::default(),
        &axis,
        &ParseContext::default(),
    )
    .expect("a point");
    assert_eq!(axis.calls()[0].1, Dialect::Gml2);

    let axis = RecordingAxis::default();
    crate::support::parse_in(
        &xeibe_testkit::gml::geometry_document_gml31(concat!(
            r#"<gml:Polygon srsName="EPSG:4326"><gml:exterior><gml:LinearRing>"#,
            "<gml:coordinates>0,0 1,0 1,1 0,0</gml:coordinates>",
            "</gml:LinearRing></gml:exterior></gml:Polygon>"
        )),
        &GeometryOptions::default(),
        &axis,
        &ParseContext::default(),
    )
    .expect("a polygon");
    assert_eq!(
        axis.calls()[0].1,
        Dialect::Gml3,
        "exterior/LinearRing is GML 3 structure"
    );
}

#[test]
fn srs_name_is_inherited_from_the_enclosing_elements() {
    // collection boundedBy → feature boundedBy → geometry → members.
    let context = ParseContext {
        srs_name: Some("EPSG:2180".into()),
        ..ParseContext::default()
    };
    let parsed = parse_in_context("<gml:Point><gml:pos>1 2</gml:pos></gml:Point>", &context)
        .expect("a point");
    assert_eq!(parsed.srs_name.as_deref(), Some("EPSG:2180"));

    // The geometry's own srsName wins.
    let parsed = parse_in_context(
        r#"<gml:Point srsName="EPSG:4326"><gml:pos>1 2</gml:pos></gml:Point>"#,
        &context,
    )
    .expect("a point");
    assert_eq!(parsed.srs_name.as_deref(), Some("EPSG:4326"));
}

#[test]
fn srs_dimension_is_inherited_by_the_positions() {
    let snippet = concat!(
        r#"<gml:LineString srsDimension="3">"#,
        "<gml:posList>1 2 3 4 5 6</gml:posList></gml:LineString>"
    );
    assert_geometry(snippet, "LINESTRING Z (1 2 3,4 5 6)");

    // The same from the context (the enclosing geometry or feature).
    let context = ParseContext {
        srs_dimension: Some(3),
        ..ParseContext::default()
    };
    let parsed = parse_in_context(
        "<gml:LineString><gml:posList>1 2 3 4 5 6</gml:posList></gml:LineString>",
        &context,
    )
    .expect("a line string");
    assert_wkt(
        &crate::support::to_g(&parsed.geometry.unwrap()),
        "LINESTRING Z (1 2 3,4 5 6)",
    );
}

#[test]
fn a_pos_list_that_could_be_3d_warns_when_read_as_2d() {
    // Values divisible by 3 but not by 2 can't be 2D (`docs/geometry.md`,
    // "Dimension").
    let snippet = "<gml:LineString><gml:posList>1 2 3 4 5 6 7 8 9</gml:posList></gml:LineString>";
    let parsed = parse(snippet);
    match parsed {
        Ok(parsed) => assert!(!parsed.warnings.is_empty(), "an ambiguous dimension warns"),
        Err(_) => { /* an error is also acceptable: it cannot be read as 2D */ }
    }
}

#[test]
fn the_source_element_kind_is_reported() {
    assert_eq!(
        parse("<gml:Point><gml:pos>1 2</gml:pos></gml:Point>")
            .unwrap()
            .source_kind,
        GeomKind::Point
    );
    assert_eq!(
        parse_gml31("<gml:LineString><gml:coordinates>1,2 3,4</gml:coordinates></gml:LineString>")
            .unwrap()
            .source_kind,
        GeomKind::LineString
    );
}

#[test]
fn legacy_elements_are_accepted_in_every_version() {
    // GML 2 spellings inside a GML 3.2 document: real data mixes versions.
    assert_wkt(
        &g("<gml:Point><gml:coordinates>1,2</gml:coordinates></gml:Point>"),
        "POINT (1 2)",
    );
}

#[test]
fn the_gml_30_dimension_attribute_is_read_as_srs_dimension() {
    // GML 3.0 wrote `posList dimension="3"`; 3.1.1 renamed it `srsDimension`.
    assert_geometry(
        r#"<gml:LineString><gml:posList dimension="3">1 2 3 4 5 6</gml:posList></gml:LineString>"#,
        "LINESTRING Z (1 2 3,4 5 6)",
    );
}

#[test]
fn without_srs_dimension_the_crs_gives_the_dimension() {
    // The standard derives `srsDimension` from the CRS (`docs/geometry.md`,
    // "Dimension"): EPSG:4979 is 3D, so three values are one position.
    assert_geometry(
        r#"<gml:LineString srsName="EPSG:4979"><gml:posList>1 2 3 4 5 6</gml:posList></gml:LineString>"#,
        "LINESTRING Z (1 2 3,4 5 6)",
    );
    assert_geometry(
        r#"<gml:LineString srsName="EPSG:2180"><gml:posList>1 2 3 4 5 6</gml:posList></gml:LineString>"#,
        "LINESTRING (1 2,3 4,5 6)",
    );
    // A compound CRS adds up its parts: 2D horizontal + 1D height. A part
    // after the first is 1D even when it names no known CRS.
    for srs in [
        "urn:adv:crs:ETRS89_UTM32*DE_DHHN2016_NH",
        "EPSG:25832+7837",
        "urn:ogc:def:crs,crs:EPSG::25832,crs:DE_XYZ",
    ] {
        assert_geometry(
            &format!(r#"<gml:LineString srsName="{srs}"><gml:posList>1 2 3 4 5 6</gml:posList></gml:LineString>"#),
            "LINESTRING Z (1 2 3,4 5 6)",
        );
    }
}
