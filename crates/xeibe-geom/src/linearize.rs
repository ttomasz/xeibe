//! Opt-in, lossy conversion of curves to line strings.

use crate::model::{CircularString, Geometry, LineString};
use crate::options::LinearizeOptions;

/// Stored control points are always kept as vertices.
pub fn linearize_circular(arc: &CircularString, options: &LinearizeOptions) -> LineString {
    todo!()
}

pub fn linearize(geometry: Geometry, options: &LinearizeOptions) -> Geometry {
    todo!()
}
