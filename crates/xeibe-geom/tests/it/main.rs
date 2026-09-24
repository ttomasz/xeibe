//! Tests for `xeibe-geom`: geometry parsing, the model, CRS and axis order,
//! arcs, linearization, WKB and `geo-traits`.
//!
//! The crate is still a skeleton, so these tests describe what
//! `docs/geometry.md` and `docs/support-matrix.md` specify.

mod arcs;
mod axis;
mod coords;
mod crs;
mod dialect;
mod envelope;
mod epsg;
mod gdal_cases;
mod linearize;
mod model;
mod options;
mod parse_aggregates;
mod parse_curves;
mod parse_primitives;
mod parse_surfaces;
mod sniff;
mod support;
mod traits;
mod wkb;
