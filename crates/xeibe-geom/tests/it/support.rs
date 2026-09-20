//! Helpers shared by the `xeibe-geom` tests.
//!
//! Geometry snippets are wrapped in a tiny document that declares the usual
//! prefixes, parsed with [`GeometryParser`], and turned into the testkit's
//! [`G`] value so they can be compared with WKT.

#![allow(dead_code)]

use std::cell::RefCell;

use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_core::{Dialect, NamespaceContext};
use xeibe_geom::model::{
    CircularString, CompoundCurve, Coords, Curve, CurvePart, CurvePolygon, Dim, Geometry,
    LineString, Point, Polygon, Surface,
};
use xeibe_geom::parse::{AxisResolver, GeometryParser, ParseContext, ParsedGeometry};
use xeibe_geom::{AxisDecision, GeometryOptions};
use xeibe_testkit::gml;
use xeibe_testkit::wkt::{G, Tol, assert_wkt, assert_wkt_tol};

/// An axis resolver that answers the same way for every key.
pub struct FixedAxis {
    pub swap: bool,
}

impl FixedAxis {
    pub fn no_swap() -> Self {
        FixedAxis { swap: false }
    }

    pub fn swap() -> Self {
        FixedAxis { swap: true }
    }
}

impl AxisResolver for FixedAxis {
    fn resolve(&self, _srs_name: Option<&str>, _dialect: Dialect) -> AxisDecision {
        AxisDecision {
            swap: self.swap,
            reason: "test".into(),
            conflicts: Vec::new(),
        }
    }
}

/// Records what the parser asked about, to check srsName inheritance and the
/// per-element dialect.
#[derive(Default)]
pub struct RecordingAxis {
    pub calls: RefCell<Vec<(Option<String>, Dialect)>>,
}

impl RecordingAxis {
    pub fn calls(&self) -> Vec<(Option<String>, Dialect)> {
        self.calls.borrow().clone()
    }
}

impl AxisResolver for RecordingAxis {
    fn resolve(&self, srs_name: Option<&str>, dialect: Dialect) -> AxisDecision {
        self.calls
            .borrow_mut()
            .push((srs_name.map(str::to_string), dialect));
        AxisDecision {
            swap: false,
            reason: "test".into(),
            conflicts: Vec::new(),
        }
    }
}

/// Parse a geometry snippet in the GML 3.2 namespace, with default options.
pub fn parse(snippet: &str) -> xeibe_geom::Result<ParsedGeometry> {
    parse_with(snippet, &GeometryOptions::default(), &FixedAxis::no_swap())
}

/// The same in the shared GML 2 / 3.1 namespace.
pub fn parse_gml31(snippet: &str) -> xeibe_geom::Result<ParsedGeometry> {
    parse_in(
        &gml::geometry_document_gml31(snippet),
        &GeometryOptions::default(),
        &FixedAxis::no_swap(),
        &ParseContext::default(),
    )
}

pub fn parse_with(
    snippet: &str,
    options: &GeometryOptions,
    axis: &dyn AxisResolver,
) -> xeibe_geom::Result<ParsedGeometry> {
    parse_in(
        &gml::geometry_document(snippet),
        options,
        axis,
        &ParseContext::default(),
    )
}

pub fn parse_in_context(
    snippet: &str,
    context: &ParseContext,
) -> xeibe_geom::Result<ParsedGeometry> {
    parse_in(
        &gml::geometry_document(snippet),
        &GeometryOptions::default(),
        &FixedAxis::no_swap(),
        context,
    )
}

/// Parse a whole document: the parser is positioned on the first start element
/// below the root, which is the convention `GeometryParser::parse` expects
/// (the geometry element's start event has just been returned).
pub fn parse_in(
    document: &str,
    options: &GeometryOptions,
    axis: &dyn AxisResolver,
    context: &ParseContext,
) -> xeibe_geom::Result<ParsedGeometry> {
    let namespaces = NamespaceContext::new();
    let mut reader = GmlReader::new(document.as_bytes(), &namespaces, 0);
    // The wrapper element, then the geometry element.
    let mut seen_wrapper = false;
    loop {
        match reader.next_event()? {
            XmlEvent::Start { .. } if !seen_wrapper => seen_wrapper = true,
            XmlEvent::Start { .. } => break,
            XmlEvent::Eof => panic!("no geometry element in {document}"),
            _ => {}
        }
    }
    GeometryParser::new(options).parse(&mut reader, context, axis)
}

/// Parse a snippet and return the geometry it produced.
pub fn geometry(snippet: &str) -> Geometry {
    parse(snippet)
        .unwrap_or_else(|e| panic!("parsing {snippet}: {e}"))
        .geometry
        .unwrap_or_else(|| panic!("no geometry from {snippet}"))
}

/// Parse a snippet and return it as a comparable value.
pub fn g(snippet: &str) -> G {
    to_g(&geometry(snippet))
}

/// Parse a GML 2 / 3.1 snippet and return it as a comparable value.
pub fn g31(snippet: &str) -> G {
    let parsed = parse_gml31(snippet).unwrap_or_else(|e| panic!("parsing {snippet}: {e}"));
    to_g(&parsed.geometry.expect("a geometry"))
}

/// Assert that a snippet parses to the given WKT.
#[track_caller]
pub fn assert_geometry(snippet: &str, expected_wkt: &str) {
    assert_wkt(&g(snippet), expected_wkt);
}

/// The same with an explicit tolerance (computed arcs).
#[track_caller]
pub fn assert_geometry_tol(snippet: &str, expected_wkt: &str, tol: Tol) {
    assert_wkt_tol(&g(snippet), expected_wkt, tol);
}

/// Warnings of a successful parse.
pub fn warnings(snippet: &str) -> Vec<String> {
    parse(snippet)
        .unwrap_or_else(|e| panic!("parsing {snippet}: {e}"))
        .warnings
}

// --------------------------------------------- the model as a testkit value

pub fn coords_to_vec(coords: &Coords) -> Vec<Vec<f64>> {
    let size = match coords.dim {
        Some(Dim::Xyz) => 3,
        Some(Dim::Xy) | None => 2,
    };
    coords.values.chunks(size).map(<[f64]>::to_vec).collect()
}

pub fn line_to_g(line: &LineString) -> G {
    G::LineString(coords_to_vec(&line.coords))
}

pub fn circular_to_g(arc: &CircularString) -> G {
    G::CircularString(coords_to_vec(&arc.coords))
}

pub fn compound_to_g(curve: &CompoundCurve) -> G {
    G::CompoundCurve(
        curve
            .parts
            .iter()
            .map(|part| match part {
                CurvePart::Linear(line) => line_to_g(line),
                CurvePart::Circular(arc) => circular_to_g(arc),
            })
            .collect(),
    )
}

pub fn curve_to_g(curve: &Curve) -> G {
    match curve {
        Curve::Linear(line) => line_to_g(line),
        Curve::Circular(arc) => circular_to_g(arc),
        Curve::Compound(compound) => compound_to_g(compound),
    }
}

pub fn point_to_g(point: &Point) -> G {
    G::Point(point.coord.clone())
}

pub fn polygon_to_g(polygon: &Polygon) -> G {
    let mut rings = Vec::new();
    if let Some(exterior) = &polygon.exterior {
        rings.push(coords_to_vec(&exterior.coords));
    }
    rings.extend(polygon.interiors.iter().map(|r| coords_to_vec(&r.coords)));
    G::Polygon(rings)
}

pub fn curve_polygon_to_g(polygon: &CurvePolygon) -> G {
    let mut rings = Vec::new();
    if let Some(exterior) = &polygon.exterior {
        rings.push(curve_to_g(exterior));
    }
    rings.extend(polygon.interiors.iter().map(curve_to_g));
    G::CurvePolygon(rings)
}

pub fn surface_to_g(surface: &Surface) -> G {
    match surface {
        Surface::Polygon(polygon) => polygon_to_g(polygon),
        Surface::CurvePolygon(polygon) => curve_polygon_to_g(polygon),
    }
}

/// The library's geometry as a testkit value.
pub fn to_g(geometry: &Geometry) -> G {
    match geometry {
        Geometry::Point(point) => point_to_g(point),
        Geometry::LineString(line) => line_to_g(line),
        Geometry::CircularString(arc) => circular_to_g(arc),
        Geometry::CompoundCurve(curve) => compound_to_g(curve),
        Geometry::Polygon(polygon) => polygon_to_g(polygon),
        Geometry::CurvePolygon(polygon) => curve_polygon_to_g(polygon),
        Geometry::MultiPoint(points) => G::MultiPoint(points.0.iter().map(point_to_g).collect()),
        Geometry::MultiLineString(lines) => {
            G::MultiLineString(lines.0.iter().map(line_to_g).collect())
        }
        Geometry::MultiPolygon(polygons) => {
            G::MultiPolygon(polygons.0.iter().map(polygon_to_g).collect())
        }
        Geometry::MultiCurve(curves) => G::MultiCurve(curves.0.iter().map(curve_to_g).collect()),
        Geometry::MultiSurface(surfaces) => {
            G::MultiSurface(surfaces.0.iter().map(surface_to_g).collect())
        }
        Geometry::GeometryCollection(members) => {
            G::GeometryCollection(members.0.iter().map(to_g).collect())
        }
    }
}
