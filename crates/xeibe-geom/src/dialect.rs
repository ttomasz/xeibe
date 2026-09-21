//! Per-element GML dialect classification (drives `AxisOrderMode::GmlVersion`).

use xeibe_core::{Dialect, QName, ns};

/// GML 2 coordinate carriers. Inside GML 3 structure they don't make the
/// geometry GML 2 (a deprecated encoding, still allowed in 3.x).
const GML2_CARRIERS: &[&str] = &["coordinates", "coord"];

/// GML 2 structure.
const GML2_STRUCTURE: &[&str] = &["outerBoundaryIs", "innerBoundaryIs", "Box"];

/// GML 3 carriers and structure (in the namespace GML 2 and 3.1 share).
const GML3_ELEMENTS: &[&str] = &[
    "pos",
    "posList",
    "pointProperty",
    "pointRep",
    "exterior",
    "interior",
    "Curve",
    "OrientableCurve",
    "CompositeCurve",
    "Ring",
    "Surface",
    "OrientableSurface",
    "CompositeSurface",
    "PolygonPatch",
    "Triangle",
    "Rectangle",
    "segments",
    "patches",
    "Envelope",
    "lowerCorner",
    "upperCorner",
    "MultiCurve",
    "MultiSurface",
    "curveMember",
    "curveMembers",
    "surfaceMember",
    "surfaceMembers",
    "pointMembers",
    "geometryMembers",
];

/// Classify a geometry-related element. `None` if the element is neutral
/// (e.g. `Point`, `LinearRing` without a coordinate child yet).
///
/// Anything in the GML 3.2 namespace is GML 3; elements outside the GML
/// namespaces are not classified.
pub fn classify_element(name: &QName) -> Option<Dialect> {
    match name.ns.as_deref() {
        Some(ns::GML_32) => Some(Dialect::Gml3),
        Some(ns::GML) => {
            let local = &*name.local;
            if GML2_CARRIERS.contains(&local) || GML2_STRUCTURE.contains(&local) {
                Some(Dialect::Gml2)
            } else if GML3_ELEMENTS.contains(&local) {
                Some(Dialect::Gml3)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Combine evidence while parsing one geometry: the outermost geometry element's
/// structure decides; `gml:coordinates` inside GML 3 structure stays GML 3.
#[derive(Debug, Clone, Copy, Default)]
pub struct DialectTracker {
    structure: Option<Dialect>,
    saw_gml2_carrier: bool,
}

impl DialectTracker {
    /// Feed the elements of one geometry in document order.
    pub fn observe(&mut self, name: &QName) {
        let Some(dialect) = classify_element(name) else {
            return;
        };
        let carrier = dialect == Dialect::Gml2 && GML2_CARRIERS.contains(&&*name.local);
        if carrier {
            self.saw_gml2_carrier = true;
        } else if self.structure.is_none() {
            self.structure = Some(dialect);
        }
    }

    /// The structure if any was seen, else a GML 2 carrier makes it GML 2;
    /// a geometry with no evidence at all counts as GML 3.
    pub fn result(&self) -> Dialect {
        match (self.structure, self.saw_gml2_carrier) {
            (Some(dialect), _) => dialect,
            (None, true) => Dialect::Gml2,
            (None, false) => Dialect::Gml3,
        }
    }
}
