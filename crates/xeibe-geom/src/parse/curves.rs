//! `Curve` + segments, `OrientableCurve`, `CompositeCurve`, `Ring`.

use xeibe_core::reader::GmlReader;

use crate::model::{CircularString, Curve, LineString};
use crate::options::GeometryOptions;

pub(super) fn parse_curve(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Curve> {
    todo!()
}

pub(super) fn parse_orientable_curve(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Curve> {
    todo!()
}

pub(super) fn parse_composite_curve(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Curve> {
    todo!()
}

/// `Ring` made of `curveMember`s (contiguous, closed cycle).
pub(super) fn parse_ring(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<Curve> {
    todo!()
}

/// Segments inside `gml:segments`.
pub(super) fn parse_line_string_segment(
    reader: &mut GmlReader<'_>,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<LineString> {
    todo!()
}

/// `Arc`, `ArcString` (any odd count ≥ 3 accepted), `Circle`.
pub(super) fn parse_arc_string(
    reader: &mut GmlReader<'_>,
    dim: Option<u8>,
    swap: bool,
    circle: bool,
) -> crate::Result<CircularString> {
    todo!()
}

/// `ArcByCenterPoint`, `CircleByCenterPoint`.
pub(super) fn parse_arc_by_center_point(
    reader: &mut GmlReader<'_>,
    options: &GeometryOptions,
    swap: bool,
    circle: bool,
) -> crate::Result<Curve> {
    todo!()
}

/// `ArcByBulge`, `ArcStringByBulge`.
pub(super) fn parse_arc_string_by_bulge(
    reader: &mut GmlReader<'_>,
    dim: Option<u8>,
    swap: bool,
) -> crate::Result<CircularString> {
    todo!()
}
