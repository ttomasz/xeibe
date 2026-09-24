//! `Polygon`, `Surface` + patches (`PolygonPatch`, `Triangle`, `Rectangle`),
//! `OrientableSurface`, `CompositeSurface`.

use xeibe_core::reader::GmlReader;

use super::assemble::check_curve_ring;
use super::{Elem, Parser, Scope};
use crate::model::{Curve, CurvePolygon, Surface};

impl Parser<'_> {
    /// `Polygon`, or a patch with the same content (`PolygonPatch`,
    /// `Triangle`, `Rectangle`): `exterior`/`interior`, or GML 2's
    /// `outerBoundaryIs`/`innerBoundaryIs`.
    pub(super) fn polygon(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<Surface> {
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
                        self.warnings.push("a second exterior ring is read as an interior ring".into());
                    }
                    interiors.push(ring);
                }
            }
        }
        if exterior.is_none() && !interiors.is_empty() {
            // Allowed for "general manifold" surfaces (§10.5.5), not in WKB.
            return Err(self.unsupported(reader, format!("{} with interior rings only", elem.local())));
        }
        let polygon = CurvePolygon { exterior, interiors };
        Ok(match polygon.into_linear() {
            Ok(polygon) => Surface::Polygon(polygon),
            Err(polygon) => Surface::CurvePolygon(polygon),
        })
    }

    /// The content of `exterior`/`interior`: `LinearRing` or `Ring`, or
    /// leniently any curve (**[GDAL]**), which must then be closed like a `Ring`.
    fn ring_element(&mut self, reader: &mut GmlReader<'_>, elem: Elem, scope: Scope) -> crate::Result<Curve> {
        let checked = elem.is("LinearRing") || elem.is("Ring");
        let mut curve = self.curve(reader, elem, scope)?;
        if !checked {
            let warnings = check_curve_ring(&mut curve);
            self.warnings.extend(warnings);
        }
        Ok(curve)
    }

    /// `Surface`: one surface per patch, kept apart (§10.5.10).
    pub(super) fn surface(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<Vec<Surface>> {
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
