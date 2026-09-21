//! Internal geometry model. Close enough to ISO 19107 to keep curves; the
//! linear subset implements `geo-traits` (see [`crate::traits`]).
//!
//! Coordinates are always stored in output order (x/y, easting/longitude
//! first) after the axis-order decision has been applied.

use serde::{Deserialize, Serialize};

use crate::arcs::{extend_bbox, extend_bbox_with_arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dim {
    Xy,
    Xyz,
}

impl Dim {
    pub fn size(self) -> usize {
        match self {
            Dim::Xy => 2,
            Dim::Xyz => 3,
        }
    }

    /// The dimension of a position with `n` ordinates (`None` unless 2 or 3).
    pub fn from_size(n: usize) -> Option<Dim> {
        match n {
            2 => Some(Dim::Xy),
            3 => Some(Dim::Xyz),
            _ => None,
        }
    }
}

/// A flat, interleaved coordinate sequence.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Coords {
    pub dim: Option<Dim>,
    pub values: Vec<f64>,
}

impl Coords {
    pub fn new(dim: Dim) -> Self {
        Coords { dim: Some(dim), values: Vec::new() }
    }

    /// Ordinates per position (2 while the dimension is still unknown).
    pub fn size(&self) -> usize {
        self.dim.map_or(2, Dim::size)
    }

    /// Number of positions.
    pub fn len(&self) -> usize {
        self.values.len() / self.size()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Append one position. The first push fixes the dimension if it is still
    /// unknown; a shorter position is padded with NaN, a longer one truncated.
    pub fn push(&mut self, xyz: &[f64]) {
        if self.dim.is_none() {
            self.dim = Some(if xyz.len() >= 3 { Dim::Xyz } else { Dim::Xy });
        }
        let size = self.size();
        self.values.extend(xyz.iter().copied().take(size));
        for _ in xyz.len()..size {
            self.values.push(f64::NAN);
        }
    }

    /// Position `i`. Panics if out of range.
    pub fn get(&self, i: usize) -> &[f64] {
        let size = self.size();
        &self.values[i * size..(i + 1) * size]
    }

    pub fn first(&self) -> Option<&[f64]> {
        (!self.is_empty()).then(|| self.get(0))
    }

    pub fn last(&self) -> Option<&[f64]> {
        (!self.is_empty()).then(|| self.get(self.len() - 1))
    }

    /// Iterate over positions.
    pub fn positions(&self) -> std::slice::ChunksExact<'_, f64> {
        self.values.chunks_exact(self.size())
    }

    /// Append `other`, dropping its first position if it equals our last one
    /// within `tolerance` (segment joining; stored coordinates win).
    ///
    /// Our last position is the one kept. A caller that knows it was computed
    /// while `other`'s first was stored overwrites it afterwards (see
    /// `parse::assemble`). With a gap both positions are kept.
    pub fn append_joined(&mut self, other: &Coords, tolerance: f64) -> JoinResult {
        if self.dim.is_none() {
            self.dim = other.dim;
        }
        let (Some(last), Some(first)) = (self.last(), other.first()) else {
            self.extend_positions(other, 0);
            return JoinResult::Exact;
        };
        let distance = distance_xy(last, first);
        let result = if distance == 0.0 {
            JoinResult::Exact
        } else if distance <= tolerance {
            JoinResult::WithinTolerance { distance }
        } else {
            JoinResult::Gap { distance }
        };
        let skip = usize::from(!matches!(result, JoinResult::Gap { .. }));
        self.extend_positions(other, skip);
        result
    }

    /// Append `other`'s positions from `skip` on, converted to our dimension.
    fn extend_positions(&mut self, other: &Coords, skip: usize) {
        if other.size() == self.size() {
            let start = (skip * other.size()).min(other.values.len());
            self.values.extend_from_slice(&other.values[start..]);
        } else {
            for position in other.positions().skip(skip) {
                self.push(position);
            }
        }
    }

    pub fn reverse(&mut self) {
        let size = self.size();
        let n = self.len();
        for i in 0..n / 2 {
            for k in 0..size {
                self.values.swap(i * size + k, (n - 1 - i) * size + k);
            }
        }
    }

    /// First and last position are equal (at least two positions).
    pub fn is_closed(&self) -> bool {
        self.len() >= 2 && self.first() == self.last()
    }

    /// Extend `bbox` (`[min_x, min_y, max_x, max_y]`) by every position.
    pub(crate) fn extend_bbox(&self, bbox: &mut [f64; 4]) {
        for position in self.positions() {
            extend_bbox(bbox, [position[0], position[1]]);
        }
    }

    /// The dimension, if there is at least one position.
    fn non_empty_dim(&self) -> Option<Dim> {
        self.dim.filter(|_| !self.is_empty())
    }
}

/// Planar distance between two positions (Z is ignored).
pub(crate) fn distance_xy(a: &[f64], b: &[f64]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
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

impl Point {
    pub fn dim(&self) -> Option<Dim> {
        self.coord
            .as_ref()
            .map(|c| if c.len() >= 3 { Dim::Xyz } else { Dim::Xy })
    }
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

impl CircularString {
    /// Reverse the direction of travel; computed positions stay marked.
    pub fn reverse(&mut self) {
        self.coords.reverse();
        let last = self.coords.len().saturating_sub(1);
        for index in &mut self.computed {
            *index = last.saturating_sub(*index);
        }
        self.computed.sort_unstable();
    }

    /// Extend `bbox` by the arcs themselves, not just their control points.
    pub(crate) fn extend_bbox(&self, bbox: &mut [f64; 4]) {
        let coords = &self.coords;
        let n = coords.len();
        let xy = |i: usize| {
            let p = coords.get(i);
            [p[0], p[1]]
        };
        let mut i = 0;
        while i + 2 < n {
            extend_bbox_with_arc(bbox, xy(i), xy(i + 1), xy(i + 2));
            i += 2;
        }
        // Positions that complete no arc (invalid input) still count.
        let from = if n >= 3 { i + 1 } else { 0 };
        for j in from..n {
            extend_bbox(bbox, xy(j));
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CurvePart {
    Linear(LineString),
    Circular(CircularString),
}

impl CurvePart {
    pub fn coords(&self) -> &Coords {
        match self {
            CurvePart::Linear(line) => &line.coords,
            CurvePart::Circular(arc) => &arc.coords,
        }
    }

    pub fn coords_mut(&mut self) -> &mut Coords {
        match self {
            CurvePart::Linear(line) => &mut line.coords,
            CurvePart::Circular(arc) => &mut arc.coords,
        }
    }

    fn reverse(&mut self) {
        match self {
            CurvePart::Linear(line) => line.coords.reverse(),
            CurvePart::Circular(arc) => arc.reverse(),
        }
    }

    fn extend_bbox(&self, bbox: &mut [f64; 4]) {
        match self {
            CurvePart::Linear(line) => line.coords.extend_bbox(bbox),
            CurvePart::Circular(arc) => arc.extend_bbox(bbox),
        }
    }
}

/// Each part starts where the previous one ends (the shared position is
/// stored in both, as in ISO WKB).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompoundCurve {
    pub parts: Vec<CurvePart>,
}

impl CompoundCurve {
    pub fn is_linear(&self) -> bool {
        self.parts.iter().all(|part| matches!(part, CurvePart::Linear(_)))
    }

    /// See [`Curve::into_linear`].
    pub fn into_linear(self) -> Option<LineString> {
        if !self.is_linear() {
            return None;
        }
        let mut coords = Coords::default();
        for part in &self.parts {
            coords.append_joined(part.coords(), 0.0);
        }
        Some(LineString { coords })
    }

    pub fn dim(&self) -> Option<Dim> {
        max_dim(self.parts.iter().map(|part| part.coords().non_empty_dim()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Curve {
    Linear(LineString),
    Circular(CircularString),
    Compound(CompoundCurve),
}

impl Curve {
    pub fn is_linear(&self) -> bool {
        match self {
            Curve::Linear(_) => true,
            Curve::Circular(_) => false,
            Curve::Compound(compound) => compound.is_linear(),
        }
    }

    /// The curve as one line string, if it has no arcs. The parts of a linear
    /// compound curve are joined, their shared positions appearing once.
    pub fn into_linear(self) -> Option<LineString> {
        match self {
            Curve::Linear(line) => Some(line),
            Curve::Circular(_) => None,
            Curve::Compound(compound) => compound.into_linear(),
        }
    }

    pub fn dim(&self) -> Option<Dim> {
        match self {
            Curve::Linear(line) => line.coords.non_empty_dim(),
            Curve::Circular(arc) => arc.coords.non_empty_dim(),
            Curve::Compound(compound) => compound.dim(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Curve::Linear(line) => line.coords.is_empty(),
            Curve::Circular(arc) => arc.coords.is_empty(),
            Curve::Compound(compound) => compound.parts.iter().all(|p| p.coords().is_empty()),
        }
    }

    /// First position, if any.
    pub fn start(&self) -> Option<&[f64]> {
        match self {
            Curve::Linear(line) => line.coords.first(),
            Curve::Circular(arc) => arc.coords.first(),
            Curve::Compound(compound) => compound.parts.iter().find_map(|p| p.coords().first()),
        }
    }

    /// Last position, if any.
    pub fn end(&self) -> Option<&[f64]> {
        match self {
            Curve::Linear(line) => line.coords.last(),
            Curve::Circular(arc) => arc.coords.last(),
            Curve::Compound(compound) => compound.parts.iter().rev().find_map(|p| p.coords().last()),
        }
    }

    /// Start and end are equal.
    pub fn is_closed(&self) -> bool {
        match (self.start(), self.end()) {
            (Some(start), Some(end)) => start == end,
            _ => false,
        }
    }

    /// Reverse the direction of travel (`OrientableCurve` with `orientation="-"`).
    pub fn reverse(&mut self) {
        match self {
            Curve::Linear(line) => line.coords.reverse(),
            Curve::Circular(arc) => arc.reverse(),
            Curve::Compound(compound) => {
                compound.parts.reverse();
                compound.parts.iter_mut().for_each(CurvePart::reverse);
            }
        }
    }

    pub(crate) fn extend_bbox(&self, bbox: &mut [f64; 4]) {
        match self {
            Curve::Linear(line) => line.coords.extend_bbox(bbox),
            Curve::Circular(arc) => arc.extend_bbox(bbox),
            Curve::Compound(compound) => compound.parts.iter().for_each(|p| p.extend_bbox(bbox)),
        }
    }
}

/// Linear polygon. `exterior` may be `None` only for an empty polygon;
/// interior-only polygons are rejected by the parser.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Polygon {
    pub exterior: Option<LineString>,
    pub interiors: Vec<LineString>,
}

impl Polygon {
    /// Exterior first, then the interiors.
    pub fn rings(&self) -> impl Iterator<Item = &LineString> + '_ {
        self.exterior.iter().chain(&self.interiors)
    }

    pub fn dim(&self) -> Option<Dim> {
        max_dim(self.rings().map(|ring| ring.coords.non_empty_dim()))
    }

    fn extend_bbox(&self, bbox: &mut [f64; 4]) {
        self.rings().for_each(|ring| ring.coords.extend_bbox(bbox));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CurvePolygon {
    pub exterior: Option<Curve>,
    pub interiors: Vec<Curve>,
}

impl CurvePolygon {
    /// Exterior first, then the interiors.
    pub fn rings(&self) -> impl Iterator<Item = &Curve> + '_ {
        self.exterior.iter().chain(&self.interiors)
    }

    pub fn is_linear(&self) -> bool {
        self.rings().all(Curve::is_linear)
    }

    /// The polygon with linear rings, if no ring has an arc.
    pub fn into_linear(self) -> Result<Polygon, CurvePolygon> {
        if !self.is_linear() {
            return Err(self);
        }
        let ring = |curve: Curve| curve.into_linear().unwrap_or_default();
        Ok(Polygon {
            exterior: self.exterior.map(ring),
            interiors: self.interiors.into_iter().map(ring).collect(),
        })
    }

    pub fn dim(&self) -> Option<Dim> {
        max_dim(self.rings().map(Curve::dim))
    }

    fn extend_bbox(&self, bbox: &mut [f64; 4]) {
        self.rings().for_each(|ring| ring.extend_bbox(bbox));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Surface {
    Polygon(Polygon),
    CurvePolygon(CurvePolygon),
}

impl Surface {
    pub fn is_linear(&self) -> bool {
        match self {
            Surface::Polygon(_) => true,
            Surface::CurvePolygon(polygon) => polygon.is_linear(),
        }
    }

    /// The linear polygon, if the surface has no arcs.
    pub fn into_linear(self) -> Result<Polygon, Surface> {
        match self {
            Surface::Polygon(polygon) => Ok(polygon),
            Surface::CurvePolygon(polygon) => polygon.into_linear().map_err(Surface::CurvePolygon),
        }
    }

    pub fn dim(&self) -> Option<Dim> {
        match self {
            Surface::Polygon(polygon) => polygon.dim(),
            Surface::CurvePolygon(polygon) => polygon.dim(),
        }
    }

    /// Reverse every ring (`OrientableSurface` with `orientation="-"`).
    pub fn reverse(&mut self) {
        match self {
            Surface::Polygon(polygon) => {
                for ring in polygon.exterior.iter_mut().chain(&mut polygon.interiors) {
                    ring.coords.reverse();
                }
            }
            Surface::CurvePolygon(polygon) => {
                for ring in polygon.exterior.iter_mut().chain(&mut polygon.interiors) {
                    ring.reverse();
                }
            }
        }
    }

    fn extend_bbox(&self, bbox: &mut [f64; 4]) {
        match self {
            Surface::Polygon(polygon) => polygon.extend_bbox(bbox),
            Surface::CurvePolygon(polygon) => polygon.extend_bbox(bbox),
        }
    }
}

/// The larger of the known dimensions (mixed 2D/3D counts as 3D).
pub(crate) fn max_dim(dims: impl Iterator<Item = Option<Dim>>) -> Option<Dim> {
    dims.flatten().max_by_key(|dim| dim.size())
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
        match self {
            Geometry::Point(_) => OutputKind::Point,
            Geometry::LineString(_) => OutputKind::LineString,
            Geometry::Polygon(_) => OutputKind::Polygon,
            Geometry::MultiPoint(_) => OutputKind::MultiPoint,
            Geometry::MultiLineString(_) => OutputKind::MultiLineString,
            Geometry::MultiPolygon(_) => OutputKind::MultiPolygon,
            Geometry::GeometryCollection(_) => OutputKind::GeometryCollection,
            Geometry::CircularString(_) => OutputKind::CircularString,
            Geometry::CompoundCurve(_) => OutputKind::CompoundCurve,
            Geometry::CurvePolygon(_) => OutputKind::CurvePolygon,
            Geometry::MultiCurve(_) => OutputKind::MultiCurve,
            Geometry::MultiSurface(_) => OutputKind::MultiSurface,
        }
    }

    /// Dimension of the coordinates; `None` for an empty geometry. Parts of
    /// different dimensions give [`Dim::Xyz`].
    pub fn dim(&self) -> Option<Dim> {
        match self {
            Geometry::Point(point) => point.dim(),
            Geometry::LineString(line) => line.coords.non_empty_dim(),
            Geometry::Polygon(polygon) => polygon.dim(),
            Geometry::MultiPoint(points) => max_dim(points.0.iter().map(Point::dim)),
            Geometry::MultiLineString(lines) => {
                max_dim(lines.0.iter().map(|line| line.coords.non_empty_dim()))
            }
            Geometry::MultiPolygon(polygons) => max_dim(polygons.0.iter().map(Polygon::dim)),
            Geometry::GeometryCollection(members) => max_dim(members.0.iter().map(Geometry::dim)),
            Geometry::CircularString(arc) => arc.coords.non_empty_dim(),
            Geometry::CompoundCurve(curve) => curve.dim(),
            Geometry::CurvePolygon(polygon) => polygon.dim(),
            Geometry::MultiCurve(curves) => max_dim(curves.0.iter().map(Curve::dim)),
            Geometry::MultiSurface(surfaces) => max_dim(surfaces.0.iter().map(Surface::dim)),
        }
    }

    /// `true` if there are no curve parts anywhere (implements `geo-traits` fully).
    ///
    /// A curve-free curve type (e.g. a `MultiSurface` of linear polygons)
    /// counts as simple; [`Geometry::simplify_types`] turns it into the simple
    /// type that the `geo-traits` implementation expects.
    pub fn is_simple(&self) -> bool {
        match self {
            Geometry::Point(_)
            | Geometry::LineString(_)
            | Geometry::Polygon(_)
            | Geometry::MultiPoint(_)
            | Geometry::MultiLineString(_)
            | Geometry::MultiPolygon(_) => true,
            Geometry::GeometryCollection(members) => members.0.iter().all(Geometry::is_simple),
            Geometry::CircularString(_) => false,
            Geometry::CompoundCurve(curve) => curve.is_linear(),
            Geometry::CurvePolygon(polygon) => polygon.is_linear(),
            Geometry::MultiCurve(curves) => curves.0.iter().all(Curve::is_linear),
            Geometry::MultiSurface(surfaces) => surfaces.0.iter().all(Surface::is_linear),
        }
    }

    /// Convert curve-free `MultiCurve`/`MultiSurface`/`CompoundCurve` etc. into
    /// their simple-feature equivalents.
    pub fn simplify_types(self) -> Self {
        match self {
            Geometry::CompoundCurve(curve) if curve.is_linear() => {
                Geometry::LineString(curve.into_linear().unwrap_or_default())
            }
            Geometry::CurvePolygon(polygon) => match polygon.into_linear() {
                Ok(polygon) => Geometry::Polygon(polygon),
                Err(polygon) => Geometry::CurvePolygon(polygon),
            },
            Geometry::MultiCurve(curves) if curves.0.iter().all(Curve::is_linear) => {
                Geometry::MultiLineString(MultiLineString(
                    curves.0.into_iter().map(|c| c.into_linear().unwrap_or_default()).collect(),
                ))
            }
            Geometry::MultiSurface(surfaces) if surfaces.0.iter().all(Surface::is_linear) => {
                Geometry::MultiPolygon(MultiPolygon(
                    surfaces.0.into_iter().map(|s| s.into_linear().unwrap_or_default()).collect(),
                ))
            }
            Geometry::GeometryCollection(members) => Geometry::GeometryCollection(
                GeometryCollection(members.0.into_iter().map(Geometry::simplify_types).collect()),
            ),
            other => other,
        }
    }

    /// `[min_x, min_y, max_x, max_y]` over every part; `None` if empty. Arcs
    /// count with their full extent, not just their control points.
    pub fn bbox(&self) -> Option<[f64; 4]> {
        let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        self.extend_bbox(&mut bbox);
        (bbox[0] <= bbox[2] && bbox[1] <= bbox[3]).then_some(bbox)
    }

    fn extend_bbox(&self, bbox: &mut [f64; 4]) {
        let point = |bbox: &mut [f64; 4], point: &Point| {
            if let Some(c) = point.coord.as_deref().filter(|c| c.len() >= 2) {
                extend_bbox(bbox, [c[0], c[1]]);
            }
        };
        match self {
            Geometry::Point(p) => point(bbox, p),
            Geometry::LineString(line) => line.coords.extend_bbox(bbox),
            Geometry::Polygon(polygon) => polygon.extend_bbox(bbox),
            Geometry::MultiPoint(points) => points.0.iter().for_each(|p| point(bbox, p)),
            Geometry::MultiLineString(lines) => {
                lines.0.iter().for_each(|line| line.coords.extend_bbox(bbox));
            }
            Geometry::MultiPolygon(polygons) => polygons.0.iter().for_each(|p| p.extend_bbox(bbox)),
            Geometry::GeometryCollection(members) => {
                members.0.iter().for_each(|g| g.extend_bbox(bbox));
            }
            Geometry::CircularString(arc) => arc.extend_bbox(bbox),
            Geometry::CompoundCurve(curve) => curve.parts.iter().for_each(|p| p.extend_bbox(bbox)),
            Geometry::CurvePolygon(polygon) => polygon.extend_bbox(bbox),
            Geometry::MultiCurve(curves) => curves.0.iter().for_each(|c| c.extend_bbox(bbox)),
            Geometry::MultiSurface(surfaces) => surfaces.0.iter().for_each(|s| s.extend_bbox(bbox)),
        }
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
    /// The envelope as a closed 2D ring of 5 positions, starting at the lower
    /// corner as GDAL does: `(lx ly, ux ly, ux uy, lx uy, lx ly)`. Corners with
    /// fewer than two ordinates give an empty polygon.
    pub fn to_polygon(&self) -> Polygon {
        if self.lower.len() < 2 || self.upper.len() < 2 {
            return Polygon::default();
        }
        let (lx, ly, ux, uy) = (self.lower[0], self.lower[1], self.upper[0], self.upper[1]);
        Polygon {
            exterior: Some(LineString {
                coords: Coords {
                    dim: Some(Dim::Xy),
                    values: vec![lx, ly, ux, ly, ux, uy, lx, uy, lx, ly],
                },
            }),
            interiors: Vec::new(),
        }
    }
}
