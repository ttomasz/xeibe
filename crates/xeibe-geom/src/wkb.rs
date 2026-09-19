//! ISO WKB writer, including curve types (CircularString, CompoundCurve,
//! CurvePolygon, MultiCurve, MultiSurface) that `geo-traits` cannot express.

use crate::model::Geometry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Endianness {
    #[default]
    Little,
    Big,
}

pub fn write_wkb(geometry: &Geometry, endianness: Endianness, out: &mut Vec<u8>) {
    todo!()
}

pub fn wkb_size(geometry: &Geometry) -> usize {
    todo!()
}
