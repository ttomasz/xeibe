//! Lightweight geometry scan used by scans and read samples: element kinds, curve
//! presence, srsName/srsDimension/axisLabels, dialect, and the first position
//! (for the axis-order range check). Does not build geometries.

use xeibe_core::{Dialect, reader::GmlReader};

use crate::model::GeomKind;

#[derive(Debug, Clone, Default)]
pub struct GeometrySniff {
    pub kinds: Vec<GeomKind>,
    pub has_curves: bool,
    pub has_unsupported: bool,
    pub srs_name: Option<String>,
    pub srs_dimension: Option<u8>,
    pub axis_labels: Option<String>,
    pub dialect: Option<Dialect>,
    /// First position, as written (no axis decision applied).
    pub first_position: Option<Vec<f64>>,
    pub by_reference: bool,
    pub empty: bool,
}

/// Called with the reader positioned on a geometry start element; consumes it.
pub fn sniff_geometry(
    reader: &mut GmlReader<'_>,
    inherited_srs: Option<&str>,
) -> crate::Result<GeometrySniff> {
    todo!()
}
