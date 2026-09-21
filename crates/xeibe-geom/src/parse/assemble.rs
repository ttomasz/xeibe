//! Assembling parsed pieces into model geometry: joining curve segments and
//! members, ring checks, the dimension rule, `Circle` completion and surface
//! aggregation. Pure logic on coordinates; the element parsers call it once
//! they have read an element's positions. See `docs/geometry.md`, "Joining
//! segments and members" and "Empty, invalid and degenerate geometry".

use crate::arcs::circle_closing_midpoint;
use crate::crs::{CrsRef, SrsName};
use crate::epsg::CrsTable;
use crate::model::{
    CircularString, CompoundCurve, Coords, Curve, CurvePart, Geometry, LineString,
    MultiSurface, Polygon, Surface, distance_xy,
};

/// Joins segments (of a `Curve`) or members (of a `CompositeCurve` or
/// `Ring`) into one curve: consecutive linear pieces become one LineString
/// part, consecutive arcs one CircularString part.
///
/// Pieces are collected first, so the tolerance can be relative to the
/// extent of the whole curve (`join_tolerance`).
#[derive(Debug, Default)]
pub(crate) struct CurveBuilder {
    pieces: Vec<CurvePart>,
}

/// The joined curve plus the warnings joining produced.
#[derive(Debug)]
pub(crate) struct JoinedCurve {
    /// `None` if nothing (or only empty pieces) was pushed.
    pub curve: Option<Curve>,
    pub warnings: Vec<String>,
}

impl CurveBuilder {
    pub fn push_linear(&mut self, line: LineString) {
        self.pieces.push(CurvePart::Linear(line));
    }

    pub fn push_circular(&mut self, arc: CircularString) {
        self.pieces.push(CurvePart::Circular(arc));
    }

    pub fn push_curve(&mut self, curve: Curve) {
        match curve {
            Curve::Linear(line) => self.push_linear(line),
            Curve::Circular(arc) => self.push_circular(arc),
            Curve::Compound(compound) => self.pieces.extend(compound.parts),
        }
    }

    /// Join everything pushed. `relative_tolerance` is `join_tolerance`
    /// (relative to the curve's extent).
    pub fn finish(self, relative_tolerance: f64) -> JoinedCurve {
        let tolerance = relative_tolerance * extent(self.pieces.iter().map(CurvePart::coords));
        let mut parts: Vec<CurvePart> = Vec::new();
        let mut warnings = Vec::new();
        for piece in self.pieces {
            if piece.coords().is_empty() {
                continue;
            }
            join_piece(&mut parts, piece, tolerance, &mut warnings);
        }
        let curve = match parts.len() {
            0 => None,
            1 => Some(match parts.pop().expect("one part") {
                CurvePart::Linear(line) => Curve::Linear(line),
                CurvePart::Circular(arc) => Curve::Circular(arc),
            }),
            _ => Some(Curve::Compound(CompoundCurve { parts })),
        };
        JoinedCurve { curve, warnings }
    }
}

fn join_piece(parts: &mut Vec<CurvePart>, mut piece: CurvePart, tolerance: f64, warnings: &mut Vec<String>) {
    let Some(last) = parts.last_mut() else {
        parts.push(piece);
        return;
    };
    let end = last.coords().last().expect("parts are never empty").to_vec();
    let start = piece.coords().first().expect("empty pieces are skipped").to_vec();
    let distance = distance_xy(&end, &start);

    if distance > tolerance {
        warnings.push(format!(
            "gap of {distance} between the end of one segment and the start of the next; both positions kept"
        ));
        // Bridge the gap with a straight line, so every part still starts where
        // the previous one ends (as ISO WKB requires).
        match (last, &mut piece) {
            (CurvePart::Linear(line), _) => line.coords.push(&start),
            (CurvePart::Circular(_), CurvePart::Linear(line)) => {
                let mut coords = Coords { dim: line.coords.dim, values: Vec::new() };
                coords.push(&end);
                coords.append_joined(&line.coords, 0.0);
                line.coords = coords;
            }
            (CurvePart::Circular(_), CurvePart::Circular(_)) => {
                let mut bridge = Coords { dim: piece.coords().dim, values: Vec::new() };
                bridge.push(&end);
                bridge.push(&start);
                parts.push(CurvePart::Linear(LineString { coords: bridge }));
            }
        }
        merge_or_push(parts, piece);
        return;
    }
    if distance > 0.0 {
        warnings.push(format!(
            "segment start differs from the previous end by {distance} (within join_tolerance); joined"
        ));
    }

    // Stored coordinates win over computed ones.
    let end_computed = is_last_computed(last);
    let start_computed = is_first_computed(&piece);
    let kept = if end_computed && !start_computed { start } else { end };
    set_last(last, &kept);
    set_first(&mut piece, &kept);
    merge_or_push(parts, piece);
}

/// Append `piece` to the last part if both are the same kind (dropping the
/// shared position), else push it as a new part.
fn merge_or_push(parts: &mut Vec<CurvePart>, piece: CurvePart) {
    let shares_start = parts
        .last()
        .and_then(|last| last.coords().last())
        .zip(piece.coords().first())
        .is_some_and(|(end, start)| end == start);
    match (parts.last_mut(), piece) {
        (Some(CurvePart::Linear(last)), CurvePart::Linear(line)) if shares_start => {
            last.coords.append_joined(&line.coords, 0.0);
        }
        (Some(CurvePart::Circular(last)), CurvePart::Circular(arc)) if shares_start => {
            let offset = last.coords.len() - 1;
            last.coords.append_joined(&arc.coords, 0.0);
            last.computed.extend(arc.computed.iter().filter(|&&i| i > 0).map(|i| i + offset));
        }
        (_, piece) => parts.push(piece),
    }
}

fn is_last_computed(part: &CurvePart) -> bool {
    match part {
        CurvePart::Linear(_) => false,
        CurvePart::Circular(arc) => arc.computed.contains(&(arc.coords.len() - 1)),
    }
}

fn is_first_computed(part: &CurvePart) -> bool {
    match part {
        CurvePart::Linear(_) => false,
        CurvePart::Circular(arc) => arc.computed.contains(&0),
    }
}

fn set_last(part: &mut CurvePart, position: &[f64]) {
    let coords = part.coords_mut();
    let size = coords.size();
    let start = coords.values.len() - size;
    overwrite(&mut coords.values[start..], position);
    if let CurvePart::Circular(arc) = part {
        let last = arc.coords.len() - 1;
        arc.computed.retain(|&i| i != last);
    }
}

fn set_first(part: &mut CurvePart, position: &[f64]) {
    let coords = part.coords_mut();
    let size = coords.size();
    overwrite(&mut coords.values[..size], position);
    if let CurvePart::Circular(arc) = part {
        arc.computed.retain(|&i| i != 0);
    }
}

fn overwrite(target: &mut [f64], source: &[f64]) {
    for (k, value) in target.iter_mut().enumerate() {
        *value = source.get(k).copied().unwrap_or(f64::NAN);
    }
}

/// The larger of the width and height of the positions' bounding box.
pub(crate) fn extent<'a>(coords: impl Iterator<Item = &'a Coords>) -> f64 {
    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for c in coords {
        c.extend_bbox(&mut bbox);
    }
    if bbox[0] > bbox[2] {
        return 0.0;
    }
    (bbox[2] - bbox[0]).max(bbox[3] - bbox[1])
}

/// A `LineString` needs at least 2 positions (07-036 §10.4.4); an empty one
/// is an empty geometry.
pub(crate) fn check_line_string(coords: &Coords) -> Result<(), String> {
    match coords.len() {
        1 => Err("a LineString needs at least 2 positions, found 1".into()),
        _ => Ok(()),
    }
}

/// Check a linear ring (§10.5.8): closed, at least 4 positions. An unclosed
/// ring is a warning, and is closed only with `close_rings`. Too few
/// positions is an error unless `lenient_degenerate`. Returns the warnings.
pub(crate) fn check_linear_ring(
    coords: &mut Coords,
    close_rings: bool,
    lenient_degenerate: bool,
) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    if coords.is_empty() {
        return Ok(warnings);
    }
    if !coords.is_closed() {
        if close_rings {
            let first = coords.get(0).to_vec();
            coords.push(&first);
            warnings.push("unclosed ring closed (close_rings)".into());
        } else {
            warnings.push("ring is not closed; kept as written".into());
        }
    }
    if coords.len() < 4 {
        let message = format!("a LinearRing needs at least 4 positions, found {}", coords.len());
        if !lenient_degenerate {
            return Err(message);
        }
        warnings.push(format!("{message}; kept as a degenerate ring (lenient_degenerate)"));
    }
    Ok(warnings)
}

/// Check a `Ring` made of curve members: it must be closed. Closed with a
/// straight segment only with `close_rings`. Returns the warnings.
pub(crate) fn check_curve_ring(curve: &mut Curve, close_rings: bool) -> Vec<String> {
    if curve.is_empty() || curve.is_closed() {
        return Vec::new();
    }
    if !close_rings {
        return vec!["ring is not closed; kept as written".into()];
    }
    let start = curve.start().expect("not empty").to_vec();
    match curve {
        Curve::Linear(line) => line.coords.push(&start),
        Curve::Circular(arc) => {
            let end = arc.coords.last().expect("not empty").to_vec();
            let mut closing = Coords { dim: arc.coords.dim, values: Vec::new() };
            closing.push(&end);
            closing.push(&start);
            *curve = Curve::Compound(CompoundCurve {
                parts: vec![
                    CurvePart::Circular(std::mem::take(arc)),
                    CurvePart::Linear(LineString { coords: closing }),
                ],
            });
        }
        Curve::Compound(compound) => match compound.parts.last_mut() {
            Some(CurvePart::Linear(line)) => line.coords.push(&start),
            Some(CurvePart::Circular(arc)) => {
                let end = arc.coords.last().expect("not empty").to_vec();
                let mut closing = Coords { dim: arc.coords.dim, values: Vec::new() };
                closing.push(&end);
                closing.push(&start);
                compound.parts.push(CurvePart::Linear(LineString { coords: closing }));
            }
            None => {}
        },
    }
    vec!["unclosed ring closed (close_rings)".into()]
}

/// `ArcString`/`Arc`: `2 × numArc + 1` positions (§10.4.7.5). **[GDAL]** any
/// odd count ≥ 3 is accepted for `Arc` too. A `numArc` that doesn't match is
/// a warning; the positions are used.
pub(crate) fn check_arc_positions(found: usize, num_arc: Option<usize>) -> Result<Option<String>, String> {
    if found < 3 || found % 2 == 0 {
        return Err(format!("{found} positions (an odd number of at least 3 expected)"));
    }
    Ok(num_arc.filter(|n| 2 * n + 1 != found).map(|n| {
        format!("numArc=\"{n}\" does not match {found} positions; the positions are used")
    }))
}

/// `Circle` (§10.4.7.7): 3 points → closed CircularString `p1 p2 p3 m p1`,
/// **[GDAL]**, where only `m` (the midpoint of the arc from `p3` back to `p1`)
/// is computed. A 3D `m` takes the mean Z of `p3` and `p1`.
pub(crate) fn circle_from_points(coords: &Coords) -> Result<CircularString, String> {
    if coords.len() != 3 {
        return Err(format!("a Circle needs 3 positions, found {}", coords.len()));
    }
    let (p1, p2, p3) = (coords.get(0), coords.get(1), coords.get(2));
    let m = circle_closing_midpoint([p1[0], p1[1]], [p2[0], p2[1]], [p3[0], p3[1]])
        .ok_or("the 3 positions of a Circle are collinear or not distinct")?;
    let mut out = Coords { dim: coords.dim, values: Vec::with_capacity(coords.values.len() + 2 * coords.size()) };
    out.push(p1);
    out.push(p2);
    out.push(p3);
    match (p1.get(2), p3.get(2)) {
        (Some(z1), Some(z3)) => out.push(&[m[0], m[1], (z1 + z3) / 2.0]),
        _ => out.push(&m),
    }
    out.push(p1);
    Ok(CircularString { coords: out, computed: vec![3] })
}

/// Surfaces of one geometry → the output type: one surface is a Polygon or
/// CurvePolygon; several are a MultiPolygon, or a MultiSurface if any has
/// arcs. None is an empty Polygon.
pub(crate) fn surfaces_to_geometry(mut surfaces: Vec<Surface>) -> Geometry {
    match surfaces.len() {
        0 => Geometry::Polygon(Polygon::default()),
        1 => match surfaces.pop().expect("one surface") {
            Surface::Polygon(polygon) => Geometry::Polygon(polygon),
            Surface::CurvePolygon(polygon) => match polygon.into_linear() {
                Ok(polygon) => Geometry::Polygon(polygon),
                Err(polygon) => Geometry::CurvePolygon(polygon),
            },
        },
        _ => Geometry::MultiSurface(MultiSurface(surfaces)).simplify_types(),
    }
}

/// A curve as a geometry of its own.
pub(crate) fn curve_to_geometry(curve: Curve) -> Geometry {
    match curve {
        Curve::Linear(line) => Geometry::LineString(line),
        Curve::Circular(arc) => Geometry::CircularString(arc),
        Curve::Compound(compound) => Geometry::CompoundCurve(compound).simplify_types(),
    }
}

/// Inputs to the dimension rule (`docs/geometry.md`, "Dimension").
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DimensionInputs {
    /// `srsDimension` (or the 3.0 `dimension`) on the `pos`/`posList`.
    pub own: Option<u8>,
    /// `srsDimension` on the nearest ancestor geometry.
    pub inherited: Option<u8>,
    /// The CRS's dimension from the CRS table.
    pub crs: Option<u8>,
    /// `count` on a `posList`.
    pub count: Option<usize>,
    /// `true` for `pos` (one position), `false` for `posList`.
    pub single_position: bool,
}

/// The effective dimension for `values` numbers, plus a warning when the
/// default of 2 was used although the values look 3D.
pub(crate) fn effective_dimension(inputs: DimensionInputs, values: usize) -> (usize, Option<String>) {
    if let Some(dim) = inputs.own.or(inputs.inherited).or(inputs.crs) {
        return (usize::from(dim), None);
    }
    if inputs.single_position && (values == 2 || values == 3) {
        return (values, None);
    }
    if let Some(count) = inputs.count.filter(|&c| c > 0 && values % c == 0) {
        return (values / count, None);
    }
    let warning = (values % 2 != 0 && values % 3 == 0).then(|| {
        format!("{values} values without srsDimension are not divisible by 2 but are by 3; read as 2D")
    });
    (2, warning)
}

/// Dimension of the CRS an srsName names, from the CRS table. A compound
/// CRS adds up its components.
pub(crate) fn crs_dimension(srs_name: &str, table: &CrsTable) -> Option<u8> {
    fn of(crs: &CrsRef, table: &CrsTable) -> Option<u8> {
        match crs {
            CrsRef::Code { authority, code } if authority == "OGC" => {
                Some(if code.eq_ignore_ascii_case("CRS84h") { 3 } else { 2 })
            }
            CrsRef::Code { authority, code } => Some(table.get(authority, code)?.dimension),
            CrsRef::Compound(parts) => parts.iter().map(|p| of(p, table)).sum(),
        }
    }
    of(SrsName::parse(srs_name).crs.as_ref()?, table)
}
