use serde::{Deserialize, Serialize};

use crate::axis::AxisOrderOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GeomEncoding {
    /// Native GeoArrow for one simple kind (or a kind and its Multi form),
    /// otherwise WKB.
    #[default]
    Auto,
    Wkb,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum CurveMode {
    /// ISO WKB curve types.
    #[default]
    Preserve,
    /// Lossy. Required for Parquet output of columns with curves (GeoParquet 1.1
    /// has no curve types).
    Linearize(LinearizeOptions),
}

/// Arc linearization, with GDAL's parameters and defaults
/// (`OGR_ARC_STEPSIZE`, `OGR_ARC_MAX_GAP`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LinearizeOptions {
    /// Largest angle one line segment may span on the arc, in degrees (default 4).
    pub max_angle_step_deg: f64,
    /// Largest distance between adjacent vertices, in CRS units; `None` = no limit.
    pub max_gap: Option<f64>,
}

impl Default for LinearizeOptions {
    fn default() -> Self {
        LinearizeOptions { max_angle_step_deg: 4.0, max_gap: None }
    }
}

/// The only geometry options, all of them read parameters
/// (`docs/geometry.md`, "Options"). Everything else is fixed behaviour.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeometryOptions {
    pub axis: AxisOrderOptions,
    /// Replaces the CRS from the data.
    pub crs_override: Option<String>,
    pub curves: CurveMode,
    /// Column name of the primary geometry column (GeoParquet `primary_column`);
    /// `None` = the first geometry column in schema order.
    pub primary: Option<String>,
}
