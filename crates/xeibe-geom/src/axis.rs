//! Axis-order modes and decisions (see `docs/geometry.md`, "CRS and axis order").
//!
//! Decisions are made per [`AxisKey`] (source, srsName as written, dialect),
//! never per feature.

use xeibe_core::{Dialect, SourceId};
use serde::{Deserialize, Serialize};

use crate::crs::{CrsRef, SrsName, SrsNameForm};
use crate::epsg::{CrsTable, FirstAxis};

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
        AxisOrderMode::Auto
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoAxisOptions {
    pub use_axis_labels: bool,
    pub use_range_check: bool,
    pub use_producer_quirks: bool,
    /// A GML 2-dialect geometry is x/y whatever the srsName says.
    pub use_gml2_dialect: bool,
    pub use_wfs_context: bool,
    pub use_envelope_consistency: bool,
    /// Rule applied when no decisive evidence exists.
    pub fallback: Box<AxisOrderMode>,
}

impl Default for AutoAxisOptions {
    fn default() -> Self {
        AutoAxisOptions {
            use_axis_labels: true,
            use_range_check: true,
            use_producer_quirks: true,
            use_gml2_dialect: true,
            use_wfs_context: true,
            use_envelope_consistency: true,
            fallback: Box::new(AxisOrderMode::CrsHeuristic),
        }
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
    /// Combine the evidence of two samples: boxes are united, counts added,
    /// labels kept as a set (in first-seen order).
    pub fn merge(&mut self, other: AxisEvidence) {
        self.sampled_bbox = union_bbox(self.sampled_bbox, other.sampled_bbox);
        self.samples += other.samples;
        for label in other.axis_labels {
            if !self.axis_labels.contains(&label) {
                self.axis_labels.push(label);
            }
        }
        self.envelope_bbox = union_bbox(self.envelope_bbox, other.envelope_bbox);
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
    /// The `BBOX` we sent (for consistency checks), in output (x/y) order.
    pub requested_bbox: Option<[f64; 4]>,
    /// File path or URL of the source, matched by [`AxisSelector::source`].
    pub source: Option<String>,
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
    let builtin;
    let table = match &options.crs_table {
        Some(table) => table,
        None => {
            builtin = CrsTable::builtin();
            &builtin
        }
    };
    let srs = key.srs_name.as_deref().map(SrsName::parse);
    let decider = Decider { key, srs: srs.as_ref(), evidence, context, options, table };

    // The most specific matching override wins; among equals, the later one.
    let chosen = options
        .overrides
        .iter()
        .filter(|(selector, _)| selector.matches(key, layer, column, context))
        .max_by_key(|(selector, _)| selector.specificity());
    match chosen {
        Some((selector, mode)) => {
            let mut decision = decider.apply(mode);
            decision.reason = format!("override {}: {}", selector.describe(), decision.reason);
            decision
        }
        None => decider.apply(&options.mode),
    }
}

/// Map `axisLabels` (e.g. `"Lat Long"`, `"x y"`) to a first-axis direction.
///
/// The first label decides: `Lat`/`φ`/`N`/`Northing`/`y` → north first,
/// `Long`/`Lon`/`λ`/`E`/`Easting`/`x` → east first, `W`/`S`/`Westing`/
/// `Southing` → [`FirstAxis::Other`]. `None` for anything else.
pub fn first_axis_from_labels(labels: &str) -> Option<crate::epsg::FirstAxis> {
    let first = labels.split(|c: char| c.is_whitespace() || c == ',').find(|s| !s.is_empty())?;
    let first = first.to_lowercase();
    match first.as_str() {
        "lat" | "latitude" | "φ" | "phi" | "n" | "north" | "northing" | "y" => {
            Some(FirstAxis::NorthOrLat)
        }
        "long" | "lon" | "lng" | "longitude" | "λ" | "lambda" | "e" | "east" | "easting" | "x" => {
            Some(FirstAxis::EastOrLon)
        }
        "w" | "west" | "westing" | "s" | "south" | "southing" => Some(FirstAxis::Other),
        _ => None,
    }
}

impl AxisSelector {
    fn matches(
        &self,
        key: &AxisKey,
        layer: Option<&str>,
        column: Option<&str>,
        context: &AxisContext,
    ) -> bool {
        let pattern = |pattern: &Option<String>, value: Option<&str>| match pattern {
            None => true,
            Some(pattern) => value.is_some_and(|value| glob_match(pattern, value)),
        };
        pattern(&self.source, context.source.as_deref())
            && pattern(&self.layer, layer)
            && pattern(&self.column, column)
            && self
                .srs_name
                .as_ref()
                .is_none_or(|srs| key.srs_name.as_deref() == Some(srs.as_str()))
            && self.dialect.is_none_or(|dialect| dialect == key.dialect)
    }

    /// Number of fields given: more fields, more specific.
    fn specificity(&self) -> usize {
        usize::from(self.source.is_some())
            + usize::from(self.layer.is_some())
            + usize::from(self.column.is_some())
            + usize::from(self.srs_name.is_some())
            + usize::from(self.dialect.is_some())
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(source) = &self.source {
            parts.push(format!("source={source}"));
        }
        if let Some(layer) = &self.layer {
            parts.push(format!("layer={layer}"));
        }
        if let Some(column) = &self.column {
            parts.push(format!("column={column}"));
        }
        if let Some(srs) = &self.srs_name {
            parts.push(format!("srs={srs}"));
        }
        if let Some(dialect) = &self.dialect {
            parts.push(format!("dialect={dialect:?}"));
        }
        parts.join(",")
    }
}

/// Glob match (`AD_*`); an invalid pattern only matches itself.
fn glob_match(pattern: &str, value: &str) -> bool {
    match globset::Glob::new(pattern) {
        Ok(glob) => glob.compile_matcher().is_match(value),
        Err(_) => pattern == value,
    }
}

fn union_bbox(a: Option<[f64; 4]>, b: Option<[f64; 4]>) -> Option<[f64; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]),
        (a, None) => a,
        (None, b) => b,
    }
}

fn intersects(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn swapped_bbox(b: [f64; 4]) -> [f64; 4] {
    [b[1], b[0], b[3], b[2]]
}

/// One piece of evidence's answer.
struct Vote {
    swap: bool,
    reason: String,
}

/// Everything one decision needs.
struct Decider<'a> {
    key: &'a AxisKey,
    srs: Option<&'a SrsName>,
    evidence: &'a AxisEvidence,
    context: &'a AxisContext,
    options: &'a AxisOrderOptions,
    table: &'a CrsTable,
}

impl Decider<'_> {
    fn apply(&self, mode: &AxisOrderMode) -> AxisDecision {
        let decided = |swap: bool, reason: String| AxisDecision { swap, reason, conflicts: Vec::new() };
        match mode {
            AxisOrderMode::XY => decided(false, "mode XY: read as written (x/y)".into()),
            AxisOrderMode::YX => decided(true, "mode YX: written y/x, swapped".into()),
            AxisOrderMode::Crs => match self.authority_order() {
                Ok(vote) => decided(vote.swap, format!("mode Crs: {}", vote.reason)),
                Err(why) => decided(false, format!("mode Crs: {why}; read as written (x/y assumed)")),
            },
            AxisOrderMode::CrsHeuristic => match self.heuristic() {
                Ok(vote) => decided(vote.swap, format!("mode CrsHeuristic: {}", vote.reason)),
                Err(why) => {
                    decided(false, format!("mode CrsHeuristic: {why}; read as written (x/y assumed)"))
                }
            },
            AxisOrderMode::GmlVersion { gml2, gml3 } => {
                let (name, inner) = match self.key.dialect {
                    Dialect::Gml2 => ("GML 2", gml2),
                    Dialect::Gml3 => ("GML 3", gml3),
                };
                let mut decision = self.apply(inner);
                decision.reason = format!("mode GmlVersion, {name} geometry: {}", decision.reason);
                decision
            }
            AxisOrderMode::Auto => self.auto(),
        }
    }

    /// The CRS's own (authority) axis order, or why it is unknown.
    fn authority_order(&self) -> Result<Vote, String> {
        let srs = self.srs.ok_or("no srsName")?;
        let crs = srs.crs.as_ref().ok_or_else(|| format!("unknown srsName {:?}", srs.raw))?;
        if crs.is_lon_lat_by_definition() {
            return Ok(Vote { swap: false, reason: format!("{} is lon/lat by definition", crs.authority_code()) });
        }
        let CrsRef::Code { authority, code } = crs.horizontal() else {
            return Err(format!("no horizontal CRS in {:?}", srs.raw));
        };
        let info = self
            .table
            .get(authority, code)
            .ok_or_else(|| format!("unknown CRS {authority}:{code} (not in the CRS table)"))?;
        Ok(match info.first_axis {
            FirstAxis::NorthOrLat => Vote {
                swap: true,
                reason: format!("{authority}:{code} is northing/latitude first, swapped"),
            },
            FirstAxis::EastOrLon => Vote {
                swap: false,
                reason: format!("{authority}:{code} is easting/longitude first"),
            },
            FirstAxis::Other => Vote {
                swap: false,
                reason: format!(
                    "{authority}:{code} axes are neither east nor north first; passed through as written"
                ),
            },
        })
    }

    /// GDAL's rule: short and legacy forms as written, URN/URI forms in
    /// authority order.
    fn heuristic(&self) -> Result<Vote, String> {
        let srs = self.srs.ok_or("no srsName")?;
        match srs.form {
            SrsNameForm::Short | SrsNameForm::LegacyUrl => Ok(Vote {
                swap: false,
                reason: format!("{:?} is a short/legacy form, read as written", srs.raw),
            }),
            SrsNameForm::Unknown => Err(format!("unknown srsName {:?}", srs.raw)),
            _ => self.authority_order(),
        }
    }

    fn auto(&self) -> AxisDecision {
        let auto = &self.options.auto;
        let mut conflicts = Vec::new();
        let mut votes: Vec<Vote> = Vec::new();

        if auto.use_axis_labels {
            votes.extend(self.labels_vote(&mut conflicts));
        }
        if auto.use_range_check {
            votes.extend(self.range_vote());
        }
        if auto.use_producer_quirks {
            votes.extend(self.quirk_vote());
        }
        if auto.use_gml2_dialect && self.key.dialect == Dialect::Gml2 {
            votes.push(Vote {
                swap: false,
                reason: "GML 2 geometry, which predates authority axis order: read as written".into(),
            });
        }
        if auto.use_wfs_context {
            votes.extend(self.wfs_vote());
        }
        // The fallback always answers, so there is always a decision.
        let fallback = match auto.fallback.as_ref() {
            AxisOrderMode::Auto => &AxisOrderMode::CrsHeuristic,
            mode => mode,
        };
        let fallback = self.apply(fallback);
        votes.push(Vote { swap: fallback.swap, reason: format!("fallback, {}", fallback.reason) });

        let mut votes = votes.into_iter();
        let chosen = votes.next().expect("the fallback always votes");
        for vote in votes.filter(|vote| vote.swap != chosen.swap) {
            let verb = if vote.swap { "swap" } else { "no swap" };
            conflicts.push(format!("{} (would mean {verb})", vote.reason));
        }
        if auto.use_envelope_consistency {
            self.envelope_warnings(chosen.swap, &mut conflicts);
        }
        AxisDecision { swap: chosen.swap, reason: format!("Auto: {}", chosen.reason), conflicts }
    }

    fn labels_vote(&self, conflicts: &mut Vec<String>) -> Option<Vote> {
        let axes: Vec<(&String, FirstAxis)> = self
            .evidence
            .axis_labels
            .iter()
            .filter_map(|labels| Some((labels, first_axis_from_labels(labels)?)))
            .filter(|(_, axis)| *axis != FirstAxis::Other)
            .collect();
        let (labels, first) = *axes.first()?;
        if axes.iter().any(|(_, axis)| *axis != first) {
            conflicts.push(format!("axisLabels disagree: {:?}", self.evidence.axis_labels));
            return None;
        }
        let swap = first == FirstAxis::NorthOrLat;
        Some(Vote { swap, reason: format!("axisLabels {labels:?} declare the order") })
    }

    /// Decisive only if exactly one reading fits the CRS area of use.
    fn range_vote(&self) -> Option<Vote> {
        if self.evidence.samples == 0 {
            return None;
        }
        let bbox = self.evidence.sampled_bbox?;
        let crs = self.srs?.crs.as_ref()?;
        if crs.is_lon_lat_by_definition() {
            return None;
        }
        let CrsRef::Code { authority, code } = crs.horizontal() else {
            return None;
        };
        let info = self.table.get(authority, code)?;
        let area = info.area_of_use?;
        // The area is in authority axis order: [min first, min second, max first, max second].
        let fits = |first: (f64, f64), second: (f64, f64)| {
            within(first, area[0], area[2]) && within(second, area[1], area[3])
        };
        let as_written = fits((bbox[0], bbox[2]), (bbox[1], bbox[3]));
        let swapped = fits((bbox[1], bbox[3]), (bbox[0], bbox[2]));
        let north_first = info.first_axis == FirstAxis::NorthOrLat;
        let (swap, order) = match (as_written, swapped) {
            (true, false) => (north_first, "authority order"),
            (false, true) => (!north_first, "the reverse of authority order"),
            _ => return None,
        };
        Some(Vote {
            swap,
            reason: format!(
                "only {order} puts the sampled coordinates inside the area of use of {authority}:{code}"
            ),
        })
    }

    fn quirk_vote(&self) -> Option<Vote> {
        let srs = self.srs?;
        if srs.form != SrsNameForm::Short {
            return None;
        }
        let producer = self.context.producer.as_deref().unwrap_or("");
        let wfs11 = self.context.wfs_version.as_deref().is_some_and(|v| v.starts_with("1.1"));
        let quirk = if self.context.fme_produced {
            "FME writes the short form in authority order [GDAL]"
        } else if wfs11 && producer.contains("mapserver.gis.umn.edu") {
            "MapServer WFS 1.1 writes the short form in authority order"
        } else if wfs11 && producer.to_ascii_lowercase().contains("/wfsserver") {
            "ArcGIS Server WFS 1.1 writes the short form in authority order"
        } else {
            return None;
        };
        let vote = self.authority_order().ok()?;
        Some(Vote { swap: vote.swap, reason: format!("{quirk}: {}", vote.reason) })
    }

    fn wfs_vote(&self) -> Option<Vote> {
        let version = self.context.wfs_version.as_deref()?;
        if version.starts_with("1.0") {
            return Some(Vote { swap: false, reason: "WFS 1.0 response: x/y".into() });
        }
        let authority = |what: &str| {
            let vote = self.authority_order().ok()?;
            Some(Vote { swap: vote.swap, reason: format!("{what}: {}", vote.reason) })
        };
        if version.starts_with("1.1") {
            let form = self.context.requested_srs.as_ref().or(self.srs)?.form;
            return match form {
                SrsNameForm::Short | SrsNameForm::LegacyUrl => {
                    Some(Vote { swap: false, reason: "WFS 1.1 with a short srsName: x/y".into() })
                }
                SrsNameForm::Unknown => None,
                _ => authority("WFS 1.1 with a URN srsName, authority order"),
            };
        }
        if version.starts_with('2') {
            return authority("WFS 2.0 response, authority order");
        }
        None
    }

    /// Supporting evidence only: report envelopes that disagree with the decision.
    fn envelope_warnings(&self, swap: bool, conflicts: &mut Vec<String>) {
        let Some(sampled) = self.evidence.sampled_bbox else {
            return;
        };
        if let Some(envelope) = self.evidence.envelope_bbox {
            if !intersects(sampled, envelope) && intersects(swapped_bbox(sampled), envelope) {
                conflicts.push(
                    "the boundedBy envelope is written in the other axis order than the geometry".into(),
                );
            }
        }
        if let Some(requested) = self.context.requested_bbox {
            let output = if swap { swapped_bbox(sampled) } else { sampled };
            let other = if swap { sampled } else { swapped_bbox(sampled) };
            if !intersects(output, requested) && intersects(other, requested) {
                conflicts.push(
                    "the geometry falls outside the requested BBOX under this decision, but inside it the other way"
                        .into(),
                );
            }
        }
    }
}

/// `[a, b]` lies in `[lo, hi]`, with a little slack for rounding in EPSG's
/// published areas. `lo > hi` is an area crossing the antimeridian.
fn within((a, b): (f64, f64), lo: f64, hi: f64) -> bool {
    const SLACK: f64 = 0.1;
    let inside = |v: f64| {
        if lo <= hi {
            v >= lo - SLACK && v <= hi + SLACK
        } else {
            v >= lo - SLACK || v <= hi + SLACK
        }
    };
    inside(a) && inside(b)
}
