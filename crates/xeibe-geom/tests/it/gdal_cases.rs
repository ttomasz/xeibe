//! The 292 GML snippets of GDAL's test suite, with GDAL 3.13.3's results
//! (`tests/data/gdal/gml_geometry_cases.jsonl`, MIT, see `LICENSE-GDAL.txt`).
//!
//! GDAL is the reference where the specs are silent (`docs/geometry.md`), so
//! most snippets must give GDAL's geometry. The places where the design
//! deliberately differs are listed in [`OVERRIDES`] with their reason; anything
//! else that differs is a failure.
//!
//! Comparison uses [`G::canonical`], because how parts are grouped (a compound
//! curve with one part, a curve-free `MULTISURFACE`) is a representation
//! choice, not a difference in the geometry. Type choices are pinned exactly in
//! the other test modules.

use std::panic::AssertUnwindSafe;

use xeibe_testkit::gdal::{GeometryCase, geometry_cases};
use xeibe_testkit::wkt::{G, Tol, diff};

use crate::support::{parse_in, to_g, FixedAxis};
use xeibe_geom::GeometryOptions;
use xeibe_geom::parse::ParseContext;

/// What a snippet is expected to produce.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// GDAL's geometry, compared in canonical form.
    Gdal,
    /// Our own result, where the design differs from GDAL.
    Wkt(&'static str),
    /// An error for this feature, whatever the message.
    Error,
    /// No geometry: an error, a null geometry or an empty geometry.
    NoGeometry,
    /// Undecided in the design docs.
    Skip,
}

/// Cases where we differ from GDAL on purpose, or that the design has not
/// settled yet.
const OVERRIDES: &[(&str, Expect, &str)] = &[
    // An empty geometry element is an empty geometry, not a null one
    // (`docs/geometry.md`, "Empty, invalid and degenerate geometry").
    ("ogr_gml_geom:1365", Expect::Wkt("POINT EMPTY"), "empty element"),
    ("ogr_gml_geom:1367", Expect::Wkt("LINESTRING EMPTY"), "empty element"),
    // A gap between members is a warning, not an error ("Joining segments and
    // members"); GDAL says "Non contiguous curves". Both positions are kept,
    // joined by a straight line so that every part of the compound curve
    // starts where the previous one ends, as ISO WKB requires.
    (
        "ogr_gml_geom:1671",
        Expect::Wkt(
            "COMPOUNDCURVE ((0 0,1 0,0 0,-10 0),CIRCULARSTRING (-10 0,1 0,0 0),(0 0,-1 0,0 0))",
        ),
        "a gap keeps both positions and warns",
    ),
    // GDAL reverses a segment whose end (rather than start) meets the previous
    // one; we keep the order as written and bridge the gap ("Joining segments
    // and members": otherwise both points are kept and a warning is logged).
    (
        "ogr_gml_geom:1664",
        Expect::Wkt("COMPOUNDCURVE (CIRCULARSTRING (0 0,1 0,0 0),(0 0,-10 0,-1 0,0 0))"),
        "a gap keeps both positions and warns; segments are not reversed",
    ),
    // An empty member is an empty geometry, not a null one, so it is not an
    // error ("Empty, invalid and degenerate geometry"); GDAL's null member is.
    ("ogr_gml_geom:1482", Expect::Wkt("MULTIPOINT EMPTY"), "empty element"),
    ("ogr_gml_geom:1523", Expect::Wkt("MULTILINESTRING EMPTY"), "empty element"),
    // A patch with only interior rings follows `unsupported_geometry` ("Empty,
    // invalid and degenerate geometry"); GDAL makes it a hole of the previous
    // member.
    ("ogr_gml_geom:2050", Expect::Error, "polygon with no exterior is unsupported"),
    // Solids are out of scope (support matrix §5.1, "Unsupported geometry");
    // GDAL reads this one's exterior as a polygon.
    ("ogr_gml_geom:1591", Expect::Error, "solids are unsupported"),
    // The root is `gml:Point`: `root_element` stops at the XML declaration.
    ("ogr_gml_geom:2191", Expect::Gdal, "the XML declaration and a comment precede the root"),
    // Arc angles are measured in output (x/y) order ("Arcs given by
    // parameters"). EPSG:2326 is northing first and GDAL measures in that
    // order, while this harness declares the coordinates x/y (no swap). With a
    // swap decision we give GDAL's arc: see
    // `parse_curves::arc_angles_are_measured_in_output_order`.
    ("ogr_gml_geom:3050", Expect::Skip, "angles in output order; the harness never swaps"),
    // Triangle and Rectangle patches are polygons, so a surface of both is a
    // MultiPolygon, not GDAL's GEOMETRYCOLLECTION of TRIANGLE + POLYGON.
    (
        "ogr_gml_geom:1242",
        Expect::Wkt(
            "MULTIPOLYGON Z (((0 0 0,0 0 1,0 1 0,0 0 0)),((0 0 10,0 1 10,1 1 10,1 0 10,0 0 10)))",
        ),
        "patches are polygons",
    ),
    // `trianglePatches` inside a `Surface` is not valid GML; GDAL reads it as a TIN.
    ("ogr_gml_geom:1099", Expect::Skip, "invalid GML, GDAL guesses a TIN"),
    // Arcs by parameters in a geographic CRS need geodesic linearization,
    // which is 🤔 Considering / P2 (support matrix §5.2).
    ("ogr_gml_geom:2424", Expect::Skip, "geodesic arc, P2"),
    ("ogr_gml_geom:2428", Expect::Skip, "geodesic arc, P2"),
    ("ogr_gml_geom:2440", Expect::Skip, "geodesic arc, P2"),
    ("ogr_gml_geom:2448", Expect::Skip, "geodesic arc, P2"),
];

/// Curve segments and surface patches are not geometries on their own. GDAL
/// accepts them as document roots; we wrap them in the element they belong to,
/// which is what real data contains.
const SEGMENTS: &[&str] = &[
    "Arc",
    "ArcString",
    "Circle",
    "ArcByBulge",
    "ArcStringByBulge",
    "ArcByCenterPoint",
    "CircleByCenterPoint",
    "LineStringSegment",
    "GeodesicString",
    "CubicSpline",
    "Bezier",
    "BSpline",
    "Clothoid",
    "OffsetCurve",
];
const PATCHES: &[&str] = &["PolygonPatch", "Triangle", "Rectangle"];

fn root_element(gml: &str) -> &str {
    let start = gml.find('<').map(|i| i + 1).unwrap_or(0);
    let rest = &gml[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(rest.len());
    &rest[..end]
}

fn wrap(gml: &str) -> String {
    let root = root_element(gml);
    let local = root.split(':').next_back().unwrap_or(root);
    if SEGMENTS.contains(&local) {
        format!("<gml:Curve><gml:segments>{gml}</gml:segments></gml:Curve>")
    } else if local == "segments" {
        format!("<gml:Curve>{gml}</gml:Curve>")
    } else if PATCHES.contains(&local) {
        format!("<gml:Surface><gml:patches>{gml}</gml:patches></gml:Surface>")
    } else if local == "Ring" || local == "LinearRing" {
        format!("<gml:Polygon><gml:exterior>{gml}</gml:exterior></gml:Polygon>")
    } else {
        gml.to_string()
    }
}

fn expectation(case: &GeometryCase) -> (Expect, &'static str) {
    if let Some((_, expect, reason)) = OVERRIDES.iter().find(|(id, _, _)| *id == case.id) {
        return (*expect, reason);
    }
    // GML 3.3 compact encodings are not planned.
    if case.gml.contains("gmlce:") {
        return (Expect::Error, "GML 3.3 compact encodings are not planned");
    }
    // Elements are matched by namespace: AIXM and unprefixed roots are not GML.
    let root = root_element(&case.gml);
    if !root.starts_with("gml:") {
        return (Expect::Error, "the root element is not in a GML namespace");
    }
    let wkt = case.gdal.wkt.as_deref().unwrap_or_default();
    if wkt.contains("POLYHEDRALSURFACE") || wkt.contains("TIN") {
        return (
            Expect::Error,
            "solids, polyhedral and triangulated surfaces are out of scope",
        );
    }
    match case.gdal.error.as_deref() {
        Some("null geometry") => (Expect::NoGeometry, "GDAL produced no geometry"),
        Some(_) => (Expect::Error, "GDAL rejected the snippet"),
        None => (Expect::Gdal, "GDAL's geometry"),
    }
}

/// `Ok(None)` is a null geometry, `Err` the message of a failed parse.
fn run(case: &GeometryCase) -> Result<Option<G>, String> {
    let document = xeibe_testkit::gml::geometry_document_gml31(&wrap(&case.gml));
    let options = GeometryOptions::default();
    let parsed = parse_in(
        &document,
        &options,
        &FixedAxis::no_swap(),
        &ParseContext::default(),
    )
    .map_err(|error| error.to_string())?;
    Ok(parsed.geometry.as_ref().map(to_g))
}

fn is_empty(geometry: &G) -> bool {
    geometry.vertices().is_empty()
}

#[test]
fn gdal_geometry_cases() {
    let cases = geometry_cases();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    let mut skipped = 0usize;

    // The parser panics while its body is `todo!()`; report that per case
    // instead of ending the whole run.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let results: Vec<_> = cases
        .iter()
        .map(|case| std::panic::catch_unwind(AssertUnwindSafe(|| run(case))))
        .collect();
    std::panic::set_hook(hook);

    for (case, result) in cases.iter().zip(results) {
        let (expect, reason) = expectation(case);
        if matches!(expect, Expect::Skip) {
            skipped += 1;
            continue;
        }
        checked += 1;
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                failures.push(format!("{}: panicked ({reason})", case.id));
                continue;
            }
        };
        let outcome = match (&expect, &result) {
            (Expect::Error, Err(_)) => Ok(()),
            (Expect::Error, Ok(geometry)) => Err(format!(
                "expected an error ({reason}), got {}",
                geometry
                    .as_ref()
                    .map_or("no geometry".to_string(), G::to_string)
            )),
            (Expect::NoGeometry, Err(_) | Ok(None)) => Ok(()),
            (Expect::NoGeometry, Ok(Some(geometry))) => {
                if is_empty(geometry) {
                    Ok(())
                } else {
                    Err(format!("expected no geometry ({reason}), got {geometry}"))
                }
            }
            (Expect::Gdal | Expect::Wkt(_), Err(error)) => {
                Err(format!("expected a geometry ({reason}), got error: {error}"))
            }
            (Expect::Gdal | Expect::Wkt(_), Ok(None)) => {
                Err(format!("expected a geometry ({reason}), got none"))
            }
            (expect, Ok(Some(geometry))) => {
                let text = match expect {
                    Expect::Wkt(wkt) => wkt,
                    _ => case.gdal.wkt.as_deref().unwrap_or_default(),
                };
                let expected = G::parse(text).expect("expected WKT parses");
                match diff(
                    &geometry.canonical(),
                    &expected.canonical(),
                    Tol { abs: 1e-9, rel: 1e-12 },
                ) {
                    None => Ok(()),
                    Some(message) => Err(format!("{message}\n      got {geometry}\n      want {expected}")),
                }
            }
            _ => Ok(()),
        };
        if let Err(message) = outcome {
            failures.push(format!("{} ({}): {message}", case.id, case.source));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {checked} GDAL geometry cases differ ({skipped} skipped):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn every_override_names_a_case_that_exists() {
    let ids: Vec<String> = geometry_cases().into_iter().map(|case| case.id).collect();
    for (id, _, reason) in OVERRIDES {
        assert!(ids.iter().any(|known| known == id), "{id}: {reason}");
    }
}

#[test]
fn wrapping_only_touches_segments_and_patches() {
    assert_eq!(wrap("<gml:Point><gml:pos>1 2</gml:pos></gml:Point>"), "<gml:Point><gml:pos>1 2</gml:pos></gml:Point>");
    assert!(wrap("<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>").starts_with("<gml:Curve>"));
    assert!(wrap("<gml:Triangle/>").starts_with("<gml:Surface>"));
    assert_eq!(root_element("<gml:Point srsName=\"x\">"), "gml:Point");
    assert_eq!(root_element("<gml:Point/>"), "gml:Point");
}
