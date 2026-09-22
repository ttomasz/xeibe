//! Built-in EPSG facts, from `xeibe-crs` (generated from the EPSG dataset by
//! `scripts/gen_crs_tables.py`, so there is no PROJ dependency). User tables
//! can extend or override it.

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
        // The built-in half is a `static` in `xeibe-crs`; nothing to load.
        CrsTable { user: Vec::new() }
    }

    pub fn with_user_entries(mut self, entries: Vec<(String, String, CrsInfo)>) -> Self {
        self.user.extend(entries);
        self
    }

    pub fn get(&self, authority: &str, code: &str) -> Option<CrsInfo> {
        // A user entry wins over the built-in one, and later entries win over
        // earlier ones, so search from the back.
        if let Some((_, _, info)) = self.user.iter().rev().find(|(a, c, _)| {
            a.eq_ignore_ascii_case(authority) && c == code
        }) {
            return Some(info.clone());
        }
        if !authority.eq_ignore_ascii_case("EPSG") {
            return None;
        }
        CrsInfo::from_record(xeibe_crs::get(code.parse().ok()?)?).into()
    }
}

impl CrsInfo {
    fn from_record(record: &xeibe_crs::CrsRecord) -> Self {
        CrsInfo {
            first_axis: match record.first_axis {
                xeibe_crs::FirstAxis::EastOrLon => FirstAxis::EastOrLon,
                xeibe_crs::FirstAxis::NorthOrLat => FirstAxis::NorthOrLat,
                xeibe_crs::FirstAxis::Other => FirstAxis::Other,
            },
            dimension: record.dimension,
            geographic: record.kind.is_geographic(),
            // Only geographic CRSs get one: the range check compares against
            // coordinates as written, and EPSG publishes the area of use in
            // degrees. Reprojecting it into a projected CRS's units would need
            // a projection engine, so there the check abstains instead.
            area_of_use: record
                .area_in_axis_order()
                .map(|area| area.map(f64::from)),
            linear_unit_m: record.linear_unit_m.map(decimal_f64),
        }
    }
}

/// An `f32` table value as the decimal it was written as (`0.3048_f32` →
/// `0.3048`, not `0.30480000376…`), so a unit conversion does not pick up the
/// `f32` rounding error.
fn decimal_f64(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(f64::from(value))
}
