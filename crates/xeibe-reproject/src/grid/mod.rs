//! Grids for grid-based transformations (`docs/reprojection.md`, level 3).
//!
//! EPSG's grid-based operations name their grid files as the producing
//! agency published them. xeibe reads PROJ-data's GeoTIFF versions, which
//! `cdn.proj.org` serves openly, and maps the names with PROJ's catalog
//! (`catalog.rs`). A grid the catalog doesn't place on the CDN is
//! unavailable, so the selection skips its operations, as PROJ does with
//! network access.
//!
//! Grids are not shipped: [`crate::Transform::grid_requests`] lists the
//! byte ranges to fetch and [`crate::Transform::add_grid_data`] takes them.
//! Converting a coordinate whose grid isn't loaded is an error, never a
//! silent fallback to an operation without the grid.

mod api;
mod catalog;
pub(crate) mod catalog_types;
mod lzw;
pub(crate) mod set;
pub(crate) mod shift;
pub(crate) mod store;
pub(crate) mod tiff;
pub(crate) mod tin;

pub(crate) use catalog_types::{Entry, lookup};
pub use store::clear_grids;

/// Where PROJ-data's grids are served.
pub const CDN: &str = "https://cdn.proj.org/";

/// Which grids the selection may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Grids {
    /// The grids PROJ-data publishes on `cdn.proj.org` (PROJ with network
    /// access). They have to be loaded before converting coordinates that
    /// use them.
    #[default]
    ProjData,
    /// No grid-based operation: only EPSG's parametric transformations
    /// (PROJ without grids).
    None,
}

/// A byte range of a grid file to load, from [`CDN`] or a local copy of
/// PROJ-data.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GridRequest {
    /// PROJ-data's file name, e.g. `fr_ign_ntf_r93.tif`.
    pub file: String,
    pub offset: u64,
    /// The bytes wanted (fewer may come back at the end of the file), or
    /// `None` for the rest of the file: a triangulation (`.json`) is read
    /// whole.
    pub length: Option<u64>,
}

impl GridRequest {
    /// The file's URL on [`CDN`]; fetch `offset..offset + length` with an
    /// HTTP range request (`bytes=offset-` without a length).
    pub fn url(&self) -> String {
        format!("{CDN}{}", self.file)
    }
}

/// The catalog entry of a grid file EPSG names in an operation, when PROJ
/// knows the file to be on the CDN.
pub(crate) fn on_cdn(epsg_name: &str, op_name: &str) -> Option<&'static Entry> {
    lookup(epsg_name, op_name).filter(|e| e.cdn)
}
