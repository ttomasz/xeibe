//! Axis-order modes and decisions (see `docs/geometry.md`, "CRS and axis order").
//!
//! Decisions are made per [`AxisKey`] (source, srsName as written, dialect),
//! never per feature.

use xeibe_core::{Dialect, SourceId};
use serde::{Deserialize, Serialize};

use crate::crs::SrsName;
use crate::epsg::CrsTable;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AxisOrderMode {
    /// Coordinates are x/y (easting/longitude first) as written. Never swap.
    XY,
    /// Coordinates are y/x: swap the first two ordinates.
    YX,
    /// The CRS's axis order decides, for every srsName form (incl. `EPSG:XXXX`).
    Crs,
    /// By srsName form: short/legacy forms → x/y; URN/URI forms → CRS order (GDAL default).
    CrsHeuristic,
    /// By the GML version each geometry element is encoded in (GML 2 or GML 3 style).
    GmlVersion {
        gml2: Box<AxisOrderMode>,
        gml3: Box<AxisOrderMode>,
    },
    /// Evidence-based, decided per key from scan or read-sample evidence
    /// (configured by [`AxisOrderOptions::auto`]).
    Auto,
}

impl Default for AxisOrderMode {
    fn default() -> Self {
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoAxisOptions {
    pub use_axis_labels: bool,
    pub use_range_check: bool,
    pub use_producer_quirks: bool,
    pub use_wfs_context: bool,
    pub use_envelope_consistency: bool,
    /// Rule applied when no decisive evidence exists.
    pub fallback: Box<AxisOrderMode>,
}

impl Default for AutoAxisOptions {
    fn default() -> Self {
        todo!()
    }
}

/// Selects where an override applies. Every given field must match.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AxisSelector {
    pub source: Option<String>,
    pub layer: Option<String>,
    pub column: Option<String>,
    pub srs_name: Option<String>,
    pub dialect: Option<Dialect>,
}

/// In the settings file, a bare mode (`"axis": "YX"`) stands for that mode with
/// default `auto` settings and no overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "AxisOrderOptionsRepr", into = "AxisOrderOptionsRepr")]
pub struct AxisOrderOptions {
    pub mode: AxisOrderMode,
    /// Most specific match wins. Only needed when one input mixes srsNames that
    /// must be read differently.
    pub overrides: Vec<(AxisSelector, AxisOrderMode)>,
    /// Evidence used by [`AxisOrderMode::Auto`].
    pub auto: AutoAxisOptions,
    /// Extra CRS facts (codes missing from the built-in table, other authorities).
    pub crs_table: Option<CrsTable>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum AxisOrderOptionsRepr {
    Mode(AxisOrderMode),
    Full {
        mode: AxisOrderMode,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        overrides: Vec<(AxisSelector, AxisOrderMode)>,
        #[serde(default)]
        auto: AutoAxisOptions,
    },
}

impl From<AxisOrderOptionsRepr> for AxisOrderOptions {
    fn from(repr: AxisOrderOptionsRepr) -> Self {
        match repr {
            AxisOrderOptionsRepr::Mode(mode) => AxisOrderOptions { mode, ..Default::default() },
            AxisOrderOptionsRepr::Full { mode, overrides, auto } => {
                AxisOrderOptions { mode, overrides, auto, crs_table: None }
            }
        }
    }
}

impl From<AxisOrderOptions> for AxisOrderOptionsRepr {
    fn from(options: AxisOrderOptions) -> Self {
        if options.overrides.is_empty() && options.auto == AutoAxisOptions::default() {
            AxisOrderOptionsRepr::Mode(options.mode)
        } else {
            AxisOrderOptionsRepr::Full { mode: options.mode, overrides: options.overrides, auto: options.auto }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AxisKey {
    pub source: SourceId,
    /// srsName exactly as written (possibly inherited); `None` if missing.
    pub srs_name: Option<String>,
    pub dialect: Dialect,
}

/// Evidence gathered by a scan or read sample for one key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AxisEvidence {
    /// Bounding box of sampled first positions, in as-written order.
    pub sampled_bbox: Option<[f64; 4]>,
    pub samples: u64,
    /// Distinct `axisLabels` values seen.
    pub axis_labels: Vec<String>,
    /// Envelopes (collection/feature `boundedBy`) in as-written order.
    pub envelope_bbox: Option<[f64; 4]>,
}

impl AxisEvidence {
    pub fn merge(&mut self, other: AxisEvidence) {
        todo!()
    }
}

/// Context outside the data itself.
#[derive(Debug, Clone, Default)]
pub struct AxisContext {
    /// Root declares the FME namespace.
    pub fme_produced: bool,
    /// Producer fingerprint (root attributes, comments, WFS ServiceIdentification).
    pub producer: Option<String>,
    /// WFS version and the srsName form we requested, if the source is a WFS download.
    pub wfs_version: Option<String>,
    pub requested_srs: Option<SrsName>,
    /// The `BBOX` we sent (for consistency checks).
    pub requested_bbox: Option<[f64; 4]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisDecision {
    pub swap: bool,
    /// Which rule/evidence decided, for `--explain` and field metadata.
    pub reason: String,
    /// Evidence that contradicted the decision (always reported).
    pub conflicts: Vec<String>,
}

/// Decide the axis order for one key.
pub fn decide(
    key: &AxisKey,
    layer: Option<&str>,
    column: Option<&str>,
    evidence: &AxisEvidence,
    context: &AxisContext,
    options: &AxisOrderOptions,
) -> AxisDecision {
    todo!()
}

/// Map `axisLabels` (e.g. `"Lat Long"`, `"x y"`) to a first-axis direction.
pub fn first_axis_from_labels(labels: &str) -> Option<crate::epsg::FirstAxis> {
    todo!()
}
