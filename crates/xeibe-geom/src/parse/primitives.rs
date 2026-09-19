//! `Point`, `LineString`, `LinearRing`, `pointProperty`/`pointRep` positions.

use xeibe_core::reader::GmlReader;

use crate::model::{LineString, Point};

pub(super) fn parse_point(
    reader: &mut GmlReader<'_>,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Point> {
    todo!()
}

pub(super) fn parse_line_string(
    reader: &mut GmlReader<'_>,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<LineString> {
    todo!()
}

pub(super) fn parse_linear_ring(
    reader: &mut GmlReader<'_>,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<LineString> {
    todo!()
}
