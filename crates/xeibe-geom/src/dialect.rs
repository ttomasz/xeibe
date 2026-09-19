//! Per-element GML dialect classification (drives `AxisOrderMode::GmlVersion`).

use xeibe_core::{Dialect, QName};

/// Classify a geometry-related element. `None` if the element is neutral
/// (e.g. `Point`, `LinearRing` without a coordinate child yet).
pub fn classify_element(name: &QName) -> Option<Dialect> {
    todo!()
}

/// Combine evidence while parsing one geometry: the outermost geometry element's
/// structure decides; `gml:coordinates` inside GML 3 structure stays GML 3.
#[derive(Debug, Clone, Copy, Default)]
pub struct DialectTracker {
    structure: Option<Dialect>,
    saw_gml2_carrier: bool,
}

impl DialectTracker {
    pub fn observe(&mut self, name: &QName) {
        todo!()
    }

    pub fn result(&self) -> Dialect {
        todo!()
    }
}
