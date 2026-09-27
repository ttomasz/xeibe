//! `Polygon`, `Surface` + patches (`PolygonPatch`, `Triangle`, `Rectangle`),
//! `OrientableSurface`, `CompositeSurface`, and GML 3.3's compact polygons
//! (`SimplePolygon`, `SimpleRectangle`, `SimpleTriangle`,
//! `SimpleTrianglePatch`).

use xeibe_core::ns;
use xeibe_core::reader::GmlReader;

use super::assemble::check_curve_ring;
use super::{COMPACT_SURFACES, Elem, Parser, Scope};
use crate::model::{Curve, CurvePolygon, LineString, Polygon, Surface};

impl Parser<'_> {
    /// `Polygon`, or a patch with the same content (`PolygonPatch`,
    /// `Triangle`, `Rectangle`): `exterior`/`interior`, or GML 2's
    /// `outerBoundaryIs`/`innerBoundaryIs`.
    pub(super) fn polygon(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        scope: Scope,
    ) -> crate::Result<Surface> {
        let scope = self.enter(elem, scope);
        let mut exterior: Option<Curve> = None;
        let mut interiors = Vec::new();
        while let Some(child) = self.next_child(reader)? {
            let is_exterior = child.is("exterior") || child.is("outerBoundaryIs");
            if !is_exterior && !child.is("interior") && !child.is("innerBoundaryIs") {
                self.skip(reader)?;
                continue;
            }
            let mut rings = Vec::new();
            let count = self.members(reader, &child, scope, |parser, reader, member, scope| {
                rings.push(parser.ring_element(reader, member, scope)?);
                Ok(())
            })?;
            // [GDAL] An empty `exterior` is an empty polygon, an empty `interior` an error.
            if count == 0 && !is_exterior {
                return Err(self.invalid(reader, format!("an empty {}", child.local())));
            }
            for ring in rings.into_iter().filter(|ring| !ring.is_empty()) {
                if is_exterior && exterior.is_none() {
                    exterior = Some(ring);
                } else {
                    if is_exterior {
                        self.warnings
                            .push("a second exterior ring is read as an interior ring".into());
                    }
                    interiors.push(ring);
                }
            }
        }
        if exterior.is_none() && !interiors.is_empty() {
            // Allowed for "general manifold" surfaces (§10.5.5), not in WKB.
            return Err(
                self.unsupported(reader, format!("{} with interior rings only", elem.local()))
            );
        }
        let polygon = CurvePolygon {
            exterior,
            interiors,
        };
        Ok(match polygon.into_linear() {
            Ok(polygon) => Surface::Polygon(polygon),
            Err(polygon) => Surface::CurvePolygon(polygon),
        })
    }

    /// A GML 3.3 compact polygon (`SimplePolygon`, `SimpleRectangle`,
    /// `SimpleTriangle`, or a `gmltin:SimpleTrianglePatch` of a `Surface`):
    /// the corners of its one ring, closed by repeating the first corner
    /// unless it is repeated already (OGC 10-129r1 §7.3). As for the
    /// `Rectangle` and `Triangle` patches, the number of corners is not
    /// checked beyond a ring's minimum of 3. No corners: an empty polygon.
    pub(super) fn simple_polygon(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        scope: Scope,
    ) -> crate::Result<Surface> {
        let scope = self.enter(elem, scope);
        let mut coords = self.positions(reader, scope, true)?;
        if coords.is_empty() {
            return Ok(Surface::Polygon(Polygon::default()));
        }
        let corners = coords.len();
        if !coords.is_closed() {
            let first = coords.get(0).to_vec();
            coords.push(&first);
        }
        if coords.len() < 4 {
            let element = COMPACT_SURFACES
                .iter()
                .find(|local| **local == elem.local())
                .copied()
                .unwrap_or("SimpleTrianglePatch");
            return Err(self.position_count(reader, element, corners, "at least 3 corners"));
        }
        Ok(Surface::Polygon(Polygon {
            exterior: Some(LineString { coords }),
            interiors: Vec::new(),
        }))
    }

    /// The content of `exterior`/`interior`: `LinearRing` or `Ring`, or
    /// leniently any curve (**[GDAL]**), which must then be closed like a `Ring`.
    fn ring_element(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: Elem,
        scope: Scope,
    ) -> crate::Result<Curve> {
        let checked = elem.is("LinearRing") || elem.is("Ring");
        let mut curve = self.curve(reader, elem, scope)?;
        if !checked {
            let warnings = check_curve_ring(&mut curve);
            self.warnings.extend(warnings);
        }
        Ok(curve)
    }

    /// `Surface`: one surface per patch, kept apart (§10.5.10).
    pub(super) fn surface(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        scope: Scope,
    ) -> crate::Result<Vec<Surface>> {
        let scope = self.enter(elem, scope);
        let mut surfaces = Vec::new();
        while let Some(child) = self.next_child(reader)? {
            if !child.is("patches") {
                self.skip(reader)?;
                continue;
            }
            let scope = self.enter(&child, scope);
            while let Some(patch) = self.next_child(reader)? {
                if patch.is("PolygonPatch") || patch.is("Triangle") || patch.is("Rectangle") {
                    surfaces.push(self.polygon(reader, &patch, scope)?);
                } else if is_simple_triangle_patch(&patch) {
                    surfaces.push(self.simple_polygon(reader, &patch, scope)?);
                } else {
                    return Err(self.wrong_kind(reader, &patch, "a surface patch"));
                }
            }
        }
        Ok(surfaces)
    }

    /// `OrientableSurface`: its `baseSurface`, every ring reversed if
    /// `orientation="-"`. May nest.
    pub(super) fn orientable_surface(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        scope: Scope,
    ) -> crate::Result<Vec<Surface>> {
        let scope = self.enter(elem, scope);
        let mut base = None;
        while let Some(child) = self.next_child(reader)? {
            if child.is("baseSurface") {
                self.members(reader, &child, scope, |parser, reader, member, scope| {
                    base = Some(parser.surfaces(reader, member, scope)?);
                    Ok(())
                })?;
            } else {
                self.skip(reader)?;
            }
        }
        let mut surfaces =
            base.ok_or_else(|| self.invalid(reader, "an OrientableSurface needs a baseSurface"))?;
        if elem.attrs.reversed {
            surfaces.iter_mut().for_each(Surface::reverse);
        }
        Ok(surfaces)
    }

    /// `CompositeSurface`: its members, kept apart.
    pub(super) fn composite_surface(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        scope: Scope,
    ) -> crate::Result<Vec<Surface>> {
        let scope = self.enter(elem, scope);
        let mut surfaces = Vec::new();
        while let Some(child) = self.next_child(reader)? {
            if child.is("surfaceMember") || child.is("surfaceMembers") {
                self.members(reader, &child, scope, |parser, reader, member, scope| {
                    surfaces.extend(parser.surfaces(reader, member, scope)?);
                    Ok(())
                })?;
            } else {
                self.skip(reader)?;
            }
        }
        Ok(surfaces)
    }
}

/// GML 3.3's `gmltin:SimpleTrianglePatch` (OGC 10-129r1 §8.4): a `Triangle`
/// patch given by its three corners.
fn is_simple_triangle_patch(elem: &Elem) -> bool {
    elem.name.ns.as_deref() == Some(ns::GML_33_TIN) && elem.local() == "SimpleTrianglePatch"
}
