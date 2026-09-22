//! `Curve` + segments, `OrientableCurve`, `CompositeCurve`, `Ring`, and the
//! arcs (`docs/geometry.md`, "Arcs given by points", "Arcs given by
//! parameters", "Joining segments and members").

use xeibe_core::reader::GmlReader;

use super::assemble::{CurveBuilder, check_arc_positions, check_curve_ring, circle_from_points};
use super::coords::parse_number;
use super::primitives::append;
use super::{Elem, Parser, Scope};
use crate::arcs::{
    angle_to_degrees, arc_by_bulge_midpoint, arc_by_center_point, circle_by_center_point,
    uom_to_metres,
};
use crate::error::Error;
use crate::model::{CircularString, Coords, Curve, Dim, LineString};

impl Parser<'_> {
    /// Any curve element as a [`Curve`]: `LineString`, `Curve`,
    /// `OrientableCurve`, `CompositeCurve`, `Ring` (and, leniently, `LinearRing`).
    pub(super) fn curve(&mut self, reader: &mut GmlReader<'_>, elem: Elem, scope: Scope) -> crate::Result<Curve> {
        if !elem.name.is_gml() {
            return Err(self.wrong_kind(reader, &elem, "a curve"));
        }
        let curve = match elem.local() {
            "LineString" => Curve::Linear(self.line_string(reader, &elem, scope)?),
            "LinearRing" => Curve::Linear(self.linear_ring(reader, &elem, scope)?),
            "Curve" => self.segmented_curve(reader, &elem, scope)?,
            "OrientableCurve" => self.orientable_curve(reader, &elem, scope)?,
            "CompositeCurve" => self.composite_curve(reader, &elem, scope)?,
            "Ring" => self.ring(reader, &elem, scope)?,
            // [GDAL] A bare segment where a curve belongs (e.g. an `Arc` as
            // a `curveMember`) is read as a curve of that one segment.
            local if super::SEGMENTS.contains(&local) => {
                let mut builder = CurveBuilder::default();
                self.segment(reader, &elem, scope, &mut builder)?;
                let joined = builder.finish(self.options.join_tolerance);
                self.warnings.extend(joined.warnings);
                joined.curve.unwrap_or_else(|| Curve::Linear(LineString::default()))
            }
            _ => return Err(self.wrong_kind(reader, &elem, "a curve")),
        };
        if elem.attrs.href && curve.is_empty() {
            return Err(Error::ByReference { location: reader.location() });
        }
        Ok(curve)
    }

    /// `Curve` with its `segments`, joined. `segments` is required; an empty
    /// one is an empty curve.
    fn segmented_curve(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<Curve> {
        let scope = self.enter(elem, scope);
        let mut builder = CurveBuilder::default();
        let mut has_segments = false;
        while let Some(child) = self.next_child(reader)? {
            if !child.is("segments") {
                self.skip(reader)?;
                continue;
            }
            has_segments = true;
            let scope = self.enter(&child, scope);
            while let Some(segment) = self.next_child(reader)? {
                self.segment(reader, &segment, scope, &mut builder)?;
            }
        }
        if !has_segments {
            return Err(self.invalid(reader, "a Curve needs a segments element"));
        }
        let joined = builder.finish(self.options.join_tolerance);
        self.warnings.extend(joined.warnings);
        Ok(joined.curve.unwrap_or_else(|| Curve::Linear(LineString::default())))
    }

    /// One element of `segments`, pushed onto `builder`.
    fn segment(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        scope: Scope,
        builder: &mut CurveBuilder,
    ) -> crate::Result<()> {
        if !elem.name.is_gml() {
            return Err(self.unsupported(reader, elem.name.to_clark()));
        }
        let scope = self.enter(elem, scope);
        let (element, interpolation): (&'static str, &str) = match elem.local() {
            // [GDAL] A `LineString` among the segments is read as a `LineStringSegment`.
            "LineStringSegment" | "LineString" => ("LineStringSegment", "linear"),
            "Arc" => ("Arc", "circularArc3Points"),
            "ArcString" => ("ArcString", "circularArc3Points"),
            "Circle" => ("Circle", "circularArc3Points"),
            "ArcByCenterPoint" => ("ArcByCenterPoint", "circularArcCenterPointWithRadius"),
            "CircleByCenterPoint" => ("CircleByCenterPoint", "circularArcCenterPointWithRadius"),
            "ArcByBulge" => ("ArcByBulge", "circularArc2PointWithBulge"),
            "ArcStringByBulge" => ("ArcStringByBulge", "circularArc2PointWithBulge"),
            other => return Err(self.unsupported(reader, other)),
        };
        // The element name decides how the segment is read (support matrix §5.4).
        if let Some(written) = elem.attrs.interpolation.as_deref().filter(|i| *i != interpolation) {
            self.warnings.push(format!(
                "{element} has interpolation=\"{written}\" (expected \"{interpolation}\"); read as {element}"
            ));
        }
        match element {
            "LineStringSegment" => {
                // Not a geometry of its own: an empty segment is an error.
                let coords = self.positions(reader, scope, true)?;
                if coords.len() < 2 {
                    return Err(self.position_count(reader, element, coords.len(), "at least 2"));
                }
                builder.push_linear(LineString { coords });
            }
            "Arc" | "ArcString" => {
                let coords = self.positions(reader, scope, true)?;
                match check_arc_positions(coords.len(), elem.attrs.num_arc) {
                    Ok(warning) => self.warnings.extend(warning),
                    Err(_) => {
                        return Err(self.position_count(reader, element, coords.len(), "an odd number of at least 3"));
                    }
                }
                builder.push_circular(CircularString { coords, computed: Vec::new() });
            }
            "Circle" => {
                let coords = self.positions(reader, scope, true)?;
                if coords.len() != 3 {
                    return Err(self.position_count(reader, element, coords.len(), "3"));
                }
                let circle = circle_from_points(&coords).map_err(|message| self.invalid(reader, message))?;
                builder.push_circular(circle);
            }
            "ArcByCenterPoint" | "CircleByCenterPoint" => {
                let arc = self.arc_by_center_point(reader, element, scope)?;
                builder.push_circular(arc);
            }
            _ => {
                let arc = self.arc_string_by_bulge(reader, elem, element, scope)?;
                builder.push_circular(arc);
            }
        }
        Ok(())
    }

    /// `ArcByCenterPoint`/`CircleByCenterPoint`, GDAL convention (projected
    /// or unknown CRS): angles counter-clockwise from +x in output order.
    ///
    /// The model is built as written and swapped at the end, so the points
    /// are computed in output order and turned back into written order.
    fn arc_by_center_point(
        &mut self,
        reader: &mut GmlReader<'_>,
        element: &'static str,
        scope: Scope,
    ) -> crate::Result<CircularString> {
        let circle = element == "CircleByCenterPoint";
        let mut center = Coords::default();
        let mut radius = None;
        let mut angles = [None, None];
        while let Some(child) = self.next_child(reader)? {
            if let Some(carried) = self.carrier(reader, &child, scope)? {
                append(&mut center, carried);
                continue;
            }
            match child.local() {
                "pointProperty" | "pointRep" => {
                    let mut found = None;
                    self.members(reader, &child, scope, |parser, reader, member, scope| {
                        if !member.is("Point") {
                            return Err(parser.wrong_kind(reader, &member, "a point"));
                        }
                        found = parser.point(reader, &member, scope)?.coord;
                        Ok(())
                    })?;
                    if let Some(coord) = found {
                        center.push(&coord);
                    }
                }
                "radius" => {
                    let value = self.number(reader)?;
                    radius = Some((value, child.attrs.uom));
                }
                "startAngle" | "endAngle" => {
                    let value = self.number(reader)?;
                    let degrees = angle_to_degrees(value, child.attrs.uom.as_deref()).ok_or_else(|| {
                        self.invalid(reader, format!("unknown angle unit {:?}", child.attrs.uom))
                    })?;
                    angles[usize::from(child.local() == "endAngle")] = Some(degrees);
                }
                _ => self.skip(reader)?,
            }
        }
        if center.len() != 1 {
            return Err(self.position_count(reader, element, center.len(), "1 (the center)"));
        }
        let (radius, uom) = radius.ok_or_else(|| self.invalid(reader, format!("{element} without a radius")))?;
        let (start, end) = match angles {
            [Some(start), Some(end)] => (start, end),
            _ if circle => (0.0, 0.0),
            _ => return Err(self.invalid(reader, format!("{element} needs startAngle and endAngle"))),
        };

        let info = self.crs_info();
        let unit_m = uom_to_metres(uom.as_deref());
        if unit_m.is_some() && info.as_ref().is_some_and(|info| info.geographic) {
            // Bearings and a geodesic radius: needs linearization on the
            // ellipsoid (🤔 Considering, support matrix §5.2).
            return Err(self.unsupported(reader, format!("{element} in a geographic CRS")));
        }
        let radius = match unit_m {
            Some(metres) => radius * metres / info.and_then(|info| info.linear_unit_m).unwrap_or(1.0),
            None => radius,
        };

        let swap = self.swap();
        let written = center.get(0);
        let mut c = [written[0], written[1]];
        if swap {
            c.swap(0, 1);
        }
        let z = written.get(2).copied();
        let points: Vec<[f64; 2]> = if circle {
            circle_by_center_point(c, radius).to_vec()
        } else {
            arc_by_center_point(c, radius, start, end).to_vec()
        };
        let mut coords = Coords::new(if z.is_some() { Dim::Xyz } else { Dim::Xy });
        for mut point in points {
            if swap {
                point.swap(0, 1);
            }
            coords.values.extend(point);
            coords.values.extend(z);
        }
        self.computed_arcs = true;
        let computed = (0..coords.len()).collect();
        Ok(CircularString { coords, computed })
    }

    /// `ArcByBulge`/`ArcStringByBulge`: `numArc + 1` positions, one `bulge`
    /// and one `normal` per arc; the mid-arc points are computed (GDAL's
    /// formula, in output order like the center-point arcs).
    fn arc_string_by_bulge(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        element: &'static str,
        scope: Scope,
    ) -> crate::Result<CircularString> {
        let mut coords = Coords::default();
        let mut bulges = Vec::new();
        let mut normals = Vec::new();
        while let Some(child) = self.next_child(reader)? {
            if let Some(carried) = self.carrier(reader, &child, scope)? {
                append(&mut coords, carried);
                continue;
            }
            match child.local() {
                "bulge" => bulges.push(self.number(reader)?),
                "normal" => {
                    let mut vector = Vec::new();
                    self.read_numbers(reader, &mut vector)?;
                    let first = vector.first().copied().ok_or_else(|| self.invalid(reader, "an empty normal"))?;
                    normals.push(first);
                }
                _ => self.skip(reader)?,
            }
        }
        let n = coords.len();
        if n < 2 {
            return Err(self.position_count(reader, element, n, "at least 2"));
        }
        let arcs = n - 1;
        if bulges.len() < arcs || normals.len() < arcs {
            return Err(self.invalid(reader, format!("{element} needs a bulge and a normal for each of its {arcs} arcs")));
        }
        if let Some(num_arc) = elem.attrs.num_arc.filter(|&num_arc| num_arc != arcs) {
            self.warnings.push(format!("numArc=\"{num_arc}\" does not match {n} positions; the positions are used"));
        }

        let swap = self.swap();
        let xy = |position: &[f64]| {
            if swap { [position[1], position[0]] } else { [position[0], position[1]] }
        };
        let mut out = Coords { dim: coords.dim, values: Vec::with_capacity(coords.values.len() * 2) };
        let mut computed = Vec::with_capacity(arcs);
        out.push(coords.get(0));
        for i in 0..arcs {
            let (p0, p1) = (coords.get(i), coords.get(i + 1));
            let mut mid = arc_by_bulge_midpoint(xy(p0), xy(p1), bulges[i], normals[i]);
            if swap {
                mid.swap(0, 1);
            }
            computed.push(out.len());
            match (p0.get(2), p1.get(2)) {
                (Some(z0), Some(z1)) => out.push(&[mid[0], mid[1], (z0 + z1) / 2.0]),
                _ => out.push(&mid),
            }
            out.push(p1);
        }
        self.computed_arcs = true;
        Ok(CircularString { coords: out, computed })
    }

    /// One number: the text of the current element.
    fn number(&mut self, reader: &mut GmlReader<'_>) -> crate::Result<f64> {
        let text = self.read_text(reader)?;
        parse_number(text.trim()).map_err(|error| error.at(reader.location()))
    }

    /// `OrientableCurve`: its `baseCurve`, reversed if `orientation="-"`. May nest.
    fn orientable_curve(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<Curve> {
        let scope = self.enter(elem, scope);
        let mut base = None;
        while let Some(child) = self.next_child(reader)? {
            if child.is("baseCurve") {
                self.members(reader, &child, scope, |parser, reader, member, scope| {
                    base = Some(parser.curve(reader, member, scope)?);
                    Ok(())
                })?;
            } else {
                self.skip(reader)?;
            }
        }
        let mut curve = base.ok_or_else(|| self.invalid(reader, "an OrientableCurve needs a baseCurve"))?;
        if elem.attrs.reversed {
            curve.reverse();
        }
        Ok(curve)
    }

    /// `CompositeCurve`: its members joined into one curve.
    fn composite_curve(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<Curve> {
        let builder = self.curve_members(reader, elem, scope)?;
        let joined = builder.finish(self.options.join_tolerance);
        self.warnings.extend(joined.warnings);
        joined.curve.ok_or_else(|| self.invalid(reader, "a CompositeCurve needs at least one member"))
    }

    /// `Ring` made of `curveMember`s: contiguous, closed cycle. An empty
    /// ring is an empty curve.
    fn ring(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<Curve> {
        let builder = self.curve_members(reader, elem, scope)?;
        let joined = builder.finish(self.options.join_tolerance);
        self.warnings.extend(joined.warnings);
        let Some(mut curve) = joined.curve else {
            return Ok(Curve::Linear(LineString::default()));
        };
        let warnings = check_curve_ring(&mut curve, self.options.close_rings);
        self.warnings.extend(warnings);
        Ok(curve)
    }

    /// The curves of `curveMember`/`curveMembers`, collected for joining.
    fn curve_members(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<CurveBuilder> {
        let scope = self.enter(elem, scope);
        let mut builder = CurveBuilder::default();
        while let Some(child) = self.next_child(reader)? {
            if child.is("curveMember") || child.is("curveMembers") {
                self.members(reader, &child, scope, |parser, reader, member, scope| {
                    builder.push_curve(parser.curve(reader, member, scope)?);
                    Ok(())
                })?;
            } else {
                self.skip(reader)?;
            }
        }
        Ok(builder)
    }
}
