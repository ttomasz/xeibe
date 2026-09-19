use std::collections::BTreeMap;

use xeibe_core::Location;

/// Produced by every read (see "Error handling" in `docs/architecture.md`).
#[derive(Debug, Clone, Default)]
pub struct ReadReport {
    pub features_per_layer: BTreeMap<String, u64>,
    pub skipped: Vec<(Location, String)>,
    /// Overflow entries per source path.
    pub overflow_per_path: BTreeMap<String, u64>,
    pub warnings: Vec<Warning>,
    /// Applied axis decisions, one per (source, srsName, dialect).
    pub axis_decisions: Vec<(xeibe_geom::AxisKey, xeibe_geom::AxisDecision)>,
    /// The schema a read without a schema inferred, ready to save.
    pub inferred: Option<crate::Settings>,
}

#[derive(Debug, Clone)]
pub struct Warning {
    pub kind: WarningKind,
    pub location: Option<Location>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WarningKind {
    /// srsName missing or in no recognised form; read as written (x/y).
    UnknownSrs,
    /// srsName recognised, but its code is not in the CRS table: where the CRS's
    /// axis order was needed, x/y was assumed.
    UnknownCrs,
    AxisConflict,
    SegmentGap,
    UnclosedRing,
    NumArcMismatch,
    ReferencedMember,
    Other,
}

impl ReadReport {
    pub fn merge(&mut self, other: ReadReport) {
        todo!()
    }
}
