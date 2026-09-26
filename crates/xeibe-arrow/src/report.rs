use std::collections::BTreeMap;

use xeibe_core::Location;

/// Produced by every read (see "Error handling" in `docs/architecture.md`).
#[derive(Debug, Clone, Default)]
pub struct ReadReport {
    pub features_per_layer: BTreeMap<String, u64>,
    pub skipped: Vec<(Location, String)>,
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
    /// axis order was needed, x/y was assumed. Also a compound CRS's part that
    /// names no known CRS: the CRS is its other parts.
    UnknownCrs,
    AxisConflict,
    SegmentGap,
    UnclosedRing,
    NumArcMismatch,
    ReferencedMember,
    /// A source without a feature collection or feature member (ISO metadata
    /// next to the GML in a zip, say).
    SkippedSource,
    Other,
}

/// Warnings kept in a report. Later ones are dropped so that a warning per
/// feature can't make the report grow with the input; the first ones show
/// what is wrong.
pub const MAX_WARNINGS: usize = 1000;

impl ReadReport {
    pub fn merge(&mut self, other: ReadReport) {
        for (layer, count) in other.features_per_layer {
            *self.features_per_layer.entry(layer).or_default() += count;
        }
        self.skipped.extend(other.skipped);
        for warning in other.warnings {
            self.warn(warning);
        }
        for (key, decision) in other.axis_decisions {
            if !self.axis_decisions.iter().any(|(k, _)| *k == key) {
                self.axis_decisions.push((key, decision));
            }
        }
        if other.inferred.is_some() {
            self.inferred = other.inferred;
        }
    }

    /// Add a warning, up to [`MAX_WARNINGS`].
    pub fn warn(&mut self, warning: Warning) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(warning);
        }
    }
}

impl Warning {
    /// A warning from the geometry parser, classified by its text.
    pub fn from_geometry(message: String, location: Option<Location>) -> Self {
        let lower = message.to_lowercase();
        let kind = if lower.contains("not closed") || lower.contains("unclosed") {
            WarningKind::UnclosedRing
        } else if lower.contains("gap") {
            WarningKind::SegmentGap
        } else if lower.contains("numarc") {
            WarningKind::NumArcMismatch
        } else {
            WarningKind::Other
        };
        Warning { kind, location, message }
    }
}
