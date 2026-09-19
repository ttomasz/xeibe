//! `MultiPoint`, `MultiLineString`, `MultiCurve`, `MultiPolygon`,
//! `MultiSurface`, `MultiGeometry` (single and plural member properties).

use xeibe_core::reader::GmlReader;

use crate::model::Geometry;
use crate::options::GeometryOptions;

pub(super) fn parse_aggregate(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Geometry> {
    todo!()
}
