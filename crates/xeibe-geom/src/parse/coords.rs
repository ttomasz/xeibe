//! Coordinate carriers: `pos`, `posList`, `coordinates`, `coord`.

use crate::model::Coords;

/// `gml:coordinates` separators (GML 2.1.2 §4.3.1 defaults: `.`, `,`, space).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatesFormat {
    pub decimal: String,
    pub cs: String,
    pub ts: String,
}

impl Default for CoordinatesFormat {
    fn default() -> Self {
        todo!()
    }
}

/// Parse `posList`/`pos` text. `dimension` from `srsDimension` (or the 3.0
/// `dimension` attribute), the CRS, `count`, or 2 — in that order.
pub fn parse_pos_list(text: &str, dimension: usize, count: Option<usize>) -> crate::Result<Coords> {
    todo!()
}

pub fn parse_coordinates(text: &str, format: &CoordinatesFormat) -> crate::Result<Coords> {
    todo!()
}

/// Swap the first two ordinates of every position in place.
pub fn swap_xy(coords: &mut Coords) {
    todo!()
}
