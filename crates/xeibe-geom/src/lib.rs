//! GML geometry → internal model → `geo-traits` / ISO WKB.
//!
//! See `docs/geometry.md`. This crate has no Arrow dependency.

// Skeleton phase: signatures only, bodies are `todo!()`.
#![allow(dead_code, unused_variables)]

pub mod arcs;
pub mod axis;
pub mod crs;
pub mod dialect;
pub mod epsg;
pub mod error;
pub mod linearize;
pub mod model;
pub mod options;
pub mod parse;
pub mod sniff;
pub mod traits;
pub mod wkb;

pub use axis::{AxisDecision, AxisEvidence, AxisKey, AxisOrderMode, AxisOrderOptions};
pub use crs::{CrsRef, SrsName, SrsNameForm};
pub use error::{Error, Result};
pub use model::{Dim, GeomKind, Geometry, OutputKind};
pub use options::GeometryOptions;
pub use parse::{AxisResolver, GeometryParser, ParseContext, ParsedGeometry};
