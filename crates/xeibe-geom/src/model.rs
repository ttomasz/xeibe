//! Internal geometry model. Close enough to ISO 19107 to keep curves; the
//! linear subset implements `geo-traits` (see [`crate::traits`]).
//!
//! Coordinates are always stored in output order (x/y, easting/longitude
//! first) after the axis-order decision has been applied.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dim {
    Xy,
    Xyz,
}

impl Dim {
    pub fn size(self) -> usize {
        todo!()
    }
}

/// A flat, interleaved coordinate sequence.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Coords {
    pub dim: Option<Dim>,
    pub values: Vec<f64>,
}

impl Coords {
    pub fn len(&self) -> usize {
        todo!()
    }

    pub fn is_empty(&self) -> bool {
        todo!()
    }

    pub fn push(&mut self, xyz: &[f64]) {
        todo!()
    }

    /// Append `other`, dropping its first position if it equals our last one
    /// within `tolerance` (segment joining; stored coordinates win).
    pub fn append_joined(&mut self, other: &Coords, tolerance: f64) -> JoinResult {
        todo!()
    }

    pub fn reverse(&mut self) {
        todo!()
    }

    pub fn is_closed(&self) -> bool {
        todo!()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JoinResult {
    Exact,
    WithinTolerance { distance: f64 },
    Gap { distance: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    /// `None` for an empty point.
    pub coord: Option<Vec<f64>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LineString {
    pub coords: Coords,
}

/// Three-point circular arcs: 2n+1 control points.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CircularString {
    pub coords: Coords,
    /// Positions computed rather than read (e.g. `Circle`'s 5th control point,
    /// all points of `ArcByCenterPoint`). Used for joining (stored wins).
    pub computed: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CurvePart {
    Linear(LineString),
    Circular(CircularString),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompoundCurve {
    pub parts: Vec<CurvePart>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Curve {
    Linear(LineString),
    Circular(CircularString),
    Compound(CompoundCurve),
}

impl Curve {
    pub fn is_linear(&self) -> bool {
        todo!()
    }

    pub fn into_linear(self) -> Option<LineString> {
        todo!()
    }
}

/// Linear polygon. `exterior` may be `None` only for an empty polygon;
/// interior-only polygons are rejected by the parser.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Polygon {
    pub exterior: Option<LineString>,
    pub interiors: Vec<LineString>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CurvePolygon {
    pub exterior: Option<Curve>,
    pub interiors: Vec<Curve>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Surface {
    Polygon(Polygon),
    CurvePolygon(CurvePolygon),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MultiPoint(pub Vec<Point>);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MultiLineString(pub Vec<LineString>);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MultiPolygon(pub Vec<Polygon>);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MultiCurve(pub Vec<Curve>);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MultiSurface(pub Vec<Surface>);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GeometryCollection(pub Vec<Geometry>);

/// A parsed geometry in output axis order.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Point(Point),
    LineString(LineString),
    Polygon(Polygon),
    MultiPoint(MultiPoint),
    MultiLineString(MultiLineString),
    MultiPolygon(MultiPolygon),
    GeometryCollection(GeometryCollection),
    // Curve types: only representable as ISO WKB.
    CircularString(CircularString),
    CompoundCurve(CompoundCurve),
    CurvePolygon(CurvePolygon),
    MultiCurve(MultiCurve),
    MultiSurface(MultiSurface),
}

impl Geometry {
    pub fn kind(&self) -> OutputKind {
        todo!()
    }

    pub fn dim(&self) -> Option<Dim> {
        todo!()
    }

    /// `true` if there are no curve parts anywhere (implements `geo-traits` fully).
    pub fn is_simple(&self) -> bool {
        todo!()
    }

    /// Convert curve-free `MultiCurve`/`MultiSurface`/`CompoundCurve` etc. into
    /// their simple-feature equivalents.
    pub fn simplify_types(self) -> Self {
        todo!()
    }

    pub fn bbox(&self) -> Option<[f64; 4]> {
        todo!()
    }
}

/// Output geometry type (what gets written).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OutputKind {
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    GeometryCollection,
    CircularString,
    CompoundCurve,
    CurvePolygon,
    MultiCurve,
    MultiSurface,
}

/// Source GML geometry element kind (what was read). Recorded in the scan's
/// `GeometryStats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GeomKind {
    Point,
    LineString,
    LinearRing,
    Polygon,
    Curve,
    OrientableCurve,
    CompositeCurve,
    Ring,
    Surface,
    OrientableSurface,
    CompositeSurface,
    MultiPoint,
    MultiLineString,
    MultiCurve,
    MultiPolygon,
    MultiSurface,
    MultiGeometry,
    Envelope,
    Box,
    /// Solid, Tin, splines, grids, … (see `unsupported_geometry` policy).
    Unsupported,
}

/// An envelope in output axis order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub lower: Vec<f64>,
    pub upper: Vec<f64>,
    pub srs_name: Option<String>,
}

impl Envelope {
    pub fn to_polygon(&self) -> Polygon {
        todo!()
    }
}
