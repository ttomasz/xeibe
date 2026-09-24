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

/// What to do when one column's srsNames resolve to more than one CRS.
/// Different spellings of the same CRS are not mixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MixedCrs {
    #[default]
    Error,
    /// One column per CRS.
    SplitColumns,
    /// Adds `<column>.crs` (e.g. `EPSG:2180` per row); the column CRS stays unset.
    PerRowCrs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DimMode {
    #[default]
    Auto,
    Force2D,
    ForceZ,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UnsupportedGeometry {
    #[default]
    Error,
    Null,
    /// Null geometry + raw GML in `<column>.gml`.
    RawXml,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeometryOptions {
    pub curves: CurveMode,
    /// Path pattern of the primary geometry column (GeoParquet `primary_column`).
    pub primary: Option<String>,
    pub axis: AxisOrderOptions,
    pub crs_override: Option<String>,
    pub mixed_crs: MixedCrs,
    pub dimension: DimMode,
    pub unsupported_geometry: UnsupportedGeometry,
    /// Keep source XML for parameter-defined arcs (`ArcByCenterPoint`, bulge).
    pub raw_xml_for_computed_arcs: bool,
    /// Relative to the geometry's extent; see "Joining segments and members".
    pub join_tolerance: f64,
    pub close_rings: bool,
    pub lenient_degenerate: bool,
    /// Step for geodesic arc linearization (degrees).
    pub arc_step_degrees: f64,
    /// Separated (struct) vs interleaved coordinates in native columns.
    pub interleaved: bool,
}

impl Default for GeometryOptions {
    fn default() -> Self {
        GeometryOptions {
            curves: CurveMode::Preserve,
            primary: None,
            axis: AxisOrderOptions::default(),
            crs_override: None,
            mixed_crs: MixedCrs::Error,
            dimension: DimMode::Auto,
            unsupported_geometry: UnsupportedGeometry::Error,
            raw_xml_for_computed_arcs: false,
            join_tolerance: 1e-9,
            close_rings: false,
            lenient_degenerate: false,
            // GDAL's default for geodesic arcs.
            arc_step_degrees: 4.0,
            interleaved: false,
        }
    }
}
