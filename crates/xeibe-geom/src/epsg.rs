//! Built-in EPSG facts, generated from the EPSG database at build time
//! (avoids a PROJ dependency). User tables can extend or override it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FirstAxis {
    /// Easting / longitude first (x/y).
    EastOrLon,
    /// Northing / latitude first (y/x) — authority order requires a swap.
    NorthOrLat,
    /// Westing/southing etc.: swapping alone does not normalize (warning).
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrsInfo {
    pub first_axis: FirstAxis,
    pub dimension: u8,
    pub geographic: bool,
    /// Area of use in CRS units and **authority axis order** — used by the
    /// axis-order range check.
    pub area_of_use: Option<[f64; 4]>,
    /// Linear unit in metres (projected CRSs), for `ArcByCenterPoint` radii.
    pub linear_unit_m: Option<f64>,
}

/// Lookup table: built-in data plus user additions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrsTable {
    user: Vec<(String, String, CrsInfo)>,
}

impl CrsTable {
    pub fn builtin() -> Self {
        todo!()
    }

    pub fn with_user_entries(self, entries: Vec<(String, String, CrsInfo)>) -> Self {
        todo!()
    }

    pub fn get(&self, authority: &str, code: &str) -> Option<CrsInfo> {
        todo!()
    }
}
