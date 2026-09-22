//! `MultiPoint`, `MultiLineString`, `MultiCurve`, `MultiPolygon`,
//! `MultiSurface`, `MultiGeometry` (single and plural member properties).

use xeibe_core::reader::GmlReader;

use super::{Elem, Parser, Scope};
use crate::model::{GeometryCollection, Geometry, MultiCurve, MultiPoint, MultiSurface};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Members {
    Points,
    Curves,
    Surfaces,
    Any,
}

impl Parser<'_> {
    /// An aggregate. Any `…Member`/`…Members` property is read (lenient:
    /// real data mixes spellings); the member's kind must fit the aggregate.
    pub(super) fn aggregate(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<Geometry> {
        let scope = self.enter(elem, scope);
        let kind = match elem.local() {
            "MultiPoint" => Members::Points,
            "MultiLineString" | "MultiCurve" => Members::Curves,
            "MultiPolygon" | "MultiSurface" => Members::Surfaces,
            _ => Members::Any,
        };
        let mut points = Vec::new();
        let mut curves = Vec::new();
        let mut surfaces = Vec::new();
        let mut members = Vec::new();
        while let Some(child) = self.next_child(reader)? {
            let local = child.local();
            if !child.name.is_gml() || !(local.ends_with("Member") || local.ends_with("Members")) {
                self.skip(reader)?;
                continue;
            }
            // [GDAL] Foreign content of these two is skipped, not an error.
            let skip_foreign = child.is("pointMembers") || child.is("surfaceMembers");
            let count = self.members(reader, &child, scope, |parser, reader, member, scope| {
                if skip_foreign && !member.name.is_gml() {
                    return parser.skip(reader);
                }
                match kind {
                    Members::Points if member.is("Point") => points.push(parser.point(reader, &member, scope)?),
                    Members::Points => return Err(parser.wrong_kind(reader, &member, "a point")),
                    Members::Curves => curves.push(parser.curve(reader, member, scope)?),
                    Members::Surfaces => surfaces.extend(parser.surfaces(reader, member, scope)?),
                    Members::Any => members.push(parser.geometry(reader, member, scope)?),
                }
                Ok(())
            })?;
            // [GDAL] Other empty member properties are ignored.
            if count == 0 && child.is("lineStringMember") {
                return Err(self.invalid(reader, "an empty lineStringMember"));
            }
        }
        // [GDAL] Empty members of the homogeneous aggregates are dropped
        // (they are empty geometries, not nulls; see "Empty, invalid and
        // degenerate geometry").
        points.retain(|point| point.coord.is_some());
        curves.retain(|curve| !curve.is_empty());
        surfaces.retain(|surface| surface.dim().is_some());
        Ok(match kind {
            Members::Points => Geometry::MultiPoint(MultiPoint(points)),
            Members::Curves => Geometry::MultiCurve(MultiCurve(curves)).simplify_types(),
            Members::Surfaces => Geometry::MultiSurface(MultiSurface(surfaces)).simplify_types(),
            Members::Any => Geometry::GeometryCollection(GeometryCollection(members)),
        })
    }
}
