//! `Polygon`, `Surface` + patches (`PolygonPatch`, `Triangle`, `Rectangle`),
//! `OrientableSurface`, `CompositeSurface`.

use xeibe_core::reader::GmlReader;

use crate::model::Surface;
use crate::options::GeometryOptions;

pub(super) fn parse_polygon(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Surface> {
    todo!()
}

/// Returns one surface per patch.
pub(super) fn parse_surface(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Vec<Surface>> {
    todo!()
}

pub(super) fn parse_orientable_surface(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Vec<Surface>> {
    todo!()
}

pub(super) fn parse_composite_surface(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Vec<Surface>> {
    todo!()
}
