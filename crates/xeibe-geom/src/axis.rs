//! Axis-order modes and decisions (see `docs/geometry.md`, "CRS and axis order").
//!
//! Decisions are made per [`AxisKey`] (source, srsName as written, dialect),
//! never per feature.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use xeibe_core::{Dialect, SourceId};

use crate::crs::{CrsRef, SrsName, SrsNameForm};
use crate::epsg::{CrsTable, FirstAxis};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
    /// Evidence-based, decided per key from scan or read-sample evidence. Uses
    /// all of the evidence; [`AxisOrderMode::CrsHeuristic`] is the fallback.
    #[default]
    Auto,
}

/// In the settings file, a bare mode (`"axis": "YX"`) stands for that mode with
/// no overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "AxisOrderOptionsRepr", into = "AxisOrderOptionsRepr")]
pub struct AxisOrderOptions {
    pub mode: AxisOrderMode,
    /// srsName as written (trimmed) → mode. Only needed when one input mixes
    /// srsNames that must be read differently.
    pub overrides: IndexMap<String, AxisOrderMode>,
    /// Extra CRS facts (codes missing from the built-in table, other
    /// authorities). Set from code; not part of the settings file.
    pub crs_table: Option<CrsTable>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum AxisOrderOptionsRepr {
    Mode(AxisOrderMode),
    Full(AxisOrderOptionsFull),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AxisOrderOptionsFull {
    mode: AxisOrderMode,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    overrides: IndexMap<String, AxisOrderMode>,
}

impl From<AxisOrderOptionsRepr> for AxisOrderOptions {
    fn from(repr: AxisOrderOptionsRepr) -> Self {
        match repr {
            AxisOrderOptionsRepr::Mode(mode) => AxisOrderOptions {
                mode,
                ..Default::default()
            },
            AxisOrderOptionsRepr::Full(AxisOrderOptionsFull { mode, overrides }) => {
                AxisOrderOptions {
                    mode,
                    overrides,
                    crs_table: None,
                }
            }
        }
    }
}

impl From<AxisOrderOptions> for AxisOrderOptionsRepr {
    fn from(options: AxisOrderOptions) -> Self {
        if options.overrides.is_empty() {
            AxisOrderOptionsRepr::Mode(options.mode)
        } else {
            AxisOrderOptionsRepr::Full(AxisOrderOptionsFull {
                mode: options.mode,
                overrides: options.overrides,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AxisKey {
    pub source: SourceId,
    /// srsName as written, trimmed (possibly inherited); `None` if missing.
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
    /// File path or URL of the source, for messages.
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisDecision {
    pub swap: bool,
    /// Which rule or evidence decided, in words, for `--explain`, field
    /// metadata and the web page.
    pub reason: String,
    /// Evidence that contradicted the decision (always reported), in words.
    pub conflicts: Vec<String>,
}

/// Decide the axis order for one key. An override for the key's srsName (as
/// written) wins over the mode.
pub fn decide(
    key: &AxisKey,
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
    let decider = Decider {
        key,
        srs: srs.as_ref(),
        evidence,
        context,
        table,
    };

    let overridden = key
        .srs_name
        .as_deref()
        .and_then(|srs| Some((srs, override_for(&options.overrides, srs)?)));
    match overridden {
        Some((srs, mode)) => {
            let mut decision = decider.apply(mode);
            decision.reason = format!("override for {srs:?}, {}", decision.reason);
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
    let first = labels
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|s| !s.is_empty())?;
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

fn union_bbox(a: Option<[f64; 4]>, b: Option<[f64; 4]>) -> Option<[f64; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
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

const IS_YX: &str = "so the file is y/x and the coordinates are swapped";
const IS_XY: &str = "so the file is x/y";
/// When nothing says which order the file has.
const ASSUMED_XY: &str = "so the file is taken to be x/y";

fn file_is(swap: bool) -> &'static str {
    if swap { IS_YX } else { IS_XY }
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
    table: &'a CrsTable,
}

impl Decider<'_> {
    fn apply(&self, mode: &AxisOrderMode) -> AxisDecision {
        let decided = |swap: bool, reason: String| AxisDecision {
            swap,
            reason,
            conflicts: Vec::new(),
        };
        match mode {
            AxisOrderMode::XY => decided(
                false,
                "axis mode XY: the file is always taken to be x/y".into(),
            ),
            AxisOrderMode::YX => decided(
                true,
                "axis mode YX: the file is always taken to be y/x, and the coordinates are swapped"
                    .into(),
            ),
            AxisOrderMode::Crs => match self.authority_order() {
                Ok(vote) => decided(
                    vote.swap,
                    format!("axis mode Crs follows the CRS: {}", vote.reason),
                ),
                Err(why) => decided(
                    false,
                    format!("axis mode Crs follows the CRS, but {why}, {ASSUMED_XY}"),
                ),
            },
            AxisOrderMode::CrsHeuristic => match self.heuristic() {
                Ok(vote) => decided(
                    vote.swap,
                    format!("axis mode CrsHeuristic (GDAL's rule): {}", vote.reason),
                ),
                Err(why) => decided(
                    false,
                    format!("axis mode CrsHeuristic (GDAL's rule), but {why}, {ASSUMED_XY}"),
                ),
            },
            AxisOrderMode::GmlVersion { gml2, gml3 } => {
                let (name, inner) = match self.key.dialect {
                    Dialect::Gml2 => ("GML 2", gml2),
                    Dialect::Gml3 => ("GML 3", gml3),
                };
                let mut decision = self.apply(inner);
                decision.reason = format!(
                    "axis mode GmlVersion, for {name} geometry: {}",
                    decision.reason
                );
                decision
            }
            AxisOrderMode::Auto => self.auto(),
        }
    }

    /// `x/y` or `y/x`, with the first axis named when the CRS is known:
    /// `y/x (northing first)`.
    fn order(&self, swap: bool) -> String {
        let order = if swap { "y/x" } else { "x/y" };
        let geographic = self.srs.and_then(|srs| srs.crs.as_ref()).and_then(|crs| {
            if crs.is_lon_lat_by_definition() {
                return Some(true);
            }
            let CrsRef::Code { authority, code } = crs.horizontal() else {
                return None;
            };
            Some(self.table.get(authority, code)?.geographic)
        });
        let first = match (geographic, swap) {
            (None, _) => return order.into(),
            (Some(true), false) => "longitude",
            (Some(true), true) => "latitude",
            (Some(false), false) => "easting",
            (Some(false), true) => "northing",
        };
        format!("{order} ({first} first)")
    }

    /// The CRS's own (authority) axis order, or why it is unknown (a clause
    /// to follow "but" or "and").
    fn authority_order(&self) -> Result<Vote, String> {
        let srs = self.srs.ok_or("there is no srsName")?;
        let crs = srs
            .crs
            .as_ref()
            .ok_or_else(|| format!("the srsName {:?} isn't one xeibe knows", srs.raw))?;
        if crs.is_lon_lat_by_definition() {
            return Ok(Vote {
                swap: false,
                reason: format!(
                    "{} puts longitude first by definition, {IS_XY}",
                    crs.authority_code()
                ),
            });
        }
        let CrsRef::Code { authority, code } = crs.horizontal() else {
            return Err(format!(
                "the srsName {:?} names no known horizontal CRS",
                srs.raw
            ));
        };
        let info = self
            .table
            .get(authority, code)
            .ok_or_else(|| format!("{authority}:{code} isn't in xeibe's CRS table"))?;
        let (north, east) = if info.geographic {
            ("latitude", "longitude")
        } else {
            ("northing", "easting")
        };
        Ok(match info.first_axis {
            FirstAxis::NorthOrLat => Vote {
                swap: true,
                reason: format!("{authority}:{code} puts {north} first, {IS_YX}"),
            },
            FirstAxis::EastOrLon => Vote {
                swap: false,
                reason: format!("{authority}:{code} puts {east} first, {IS_XY}"),
            },
            FirstAxis::Other => Vote {
                swap: false,
                reason: format!(
                    "{authority}:{code}'s axes aren't an easting and a northing (or longitude and latitude) in either order, which a swap can't fix, so the coordinates are left in the file's order"
                ),
            },
        })
    }

    /// GDAL's rule: short and legacy forms as written, URN/URI forms in
    /// authority order. The reason doesn't name the rule; the caller does.
    fn heuristic(&self) -> Result<Vote, String> {
        let srs = self.srs.ok_or("there is no srsName")?;
        let form = match srs.form {
            SrsNameForm::Short => "short",
            SrsNameForm::LegacyUrl => "legacy URL",
            SrsNameForm::Unknown => {
                return Err(format!("the srsName {:?} isn't one xeibe knows", srs.raw));
            }
            SrsNameForm::HttpUri | SrsNameForm::HttpUriKvp | SrsNameForm::CompoundUri => {
                return self.follows_crs("URI");
            }
            _ => return self.follows_crs("URN"),
        };
        Ok(Vote {
            swap: false,
            reason: format!(
                "a {form} srsName like {:?} means the file is {}, whatever axis order its CRS has",
                srs.raw,
                self.order(false)
            ),
        })
    }

    fn follows_crs(&self, form: &str) -> Result<Vote, String> {
        let vote = self.authority_order()?;
        Ok(Vote {
            swap: vote.swap,
            reason: format!("a {form} srsName follows its CRS, and {}", vote.reason),
        })
    }

    fn auto(&self) -> AxisDecision {
        let mut conflicts = Vec::new();
        let mut votes: Vec<Vote> = Vec::new();

        votes.extend(self.labels_vote(&mut conflicts));
        votes.extend(self.range_vote());
        votes.extend(self.quirk_vote());
        if self.key.dialect == Dialect::Gml2 {
            votes.push(Vote {
                swap: false,
                reason: "GML 2 geometry is always x/y (it predates CRS axis order)".to_string(),
            });
        }
        votes.extend(self.wfs_vote());
        // The fallback always answers, so there is always a decision.
        let alone = votes.is_empty();
        votes.push(match (self.heuristic(), alone) {
            (Ok(vote), true) => Vote {
                swap: vote.swap,
                reason: format!(
                    "nothing in the file settles the order, so GDAL's rule applies: {}",
                    vote.reason
                ),
            },
            (Ok(vote), false) => Vote {
                swap: vote.swap,
                reason: format!("GDAL's rule: {}", vote.reason),
            },
            (Err(why), true) => Vote {
                swap: false,
                reason: format!("nothing in the file settles the order, and {why}, {ASSUMED_XY}"),
            },
            (Err(why), false) => Vote {
                swap: false,
                reason: format!("{why}, {ASSUMED_XY}"),
            },
        });

        let mut votes = votes.into_iter();
        let chosen = votes.next().expect("the fallback always votes");
        conflicts.extend(
            votes
                .filter(|vote| vote.swap != chosen.swap)
                .map(|vote| vote.reason),
        );
        self.envelope_warnings(chosen.swap, &mut conflicts);
        AxisDecision {
            swap: chosen.swap,
            reason: chosen.reason,
            conflicts,
        }
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
            conflicts.push(format!(
                "the geometries' axisLabels disagree: {}",
                self.evidence
                    .axis_labels
                    .iter()
                    .map(|labels| format!("{labels:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            return None;
        }
        let swap = first == FirstAxis::NorthOrLat;
        let which = if swap {
            "latitude or northing"
        } else {
            "longitude or easting"
        };
        Some(Vote {
            swap,
            reason: format!(
                "the geometry's axisLabels {labels:?} put {which} first, {}",
                file_is(swap)
            ),
        })
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
        let swap = match (as_written, swapped) {
            (true, false) => north_first,
            (false, true) => !north_first,
            _ => return None,
        };
        Some(Vote {
            swap,
            reason: format!(
                "the coordinates fall inside the area where {authority}:{code} is used only if the file is {}{}",
                self.order(swap),
                if swap { ", so they are swapped" } else { "" }
            ),
        })
    }

    fn quirk_vote(&self) -> Option<Vote> {
        let srs = self.srs?;
        if srs.form != SrsNameForm::Short {
            return None;
        }
        let producer = self.context.producer.as_deref().unwrap_or("");
        let wfs11 = self
            .context
            .wfs_version
            .as_deref()
            .is_some_and(|v| v.starts_with("1.1"));
        let producer = if self.context.fme_produced {
            "FME"
        } else if wfs11 && producer.contains("mapserver.gis.umn.edu") {
            "MapServer's WFS 1.1"
        } else if wfs11 && producer.to_ascii_lowercase().contains("/wfsserver") {
            "ArcGIS Server's WFS 1.1"
        } else {
            return None;
        };
        let vote = self.authority_order().ok()?;
        Some(Vote {
            swap: vote.swap,
            reason: format!(
                "the file comes from {producer}, which writes a short srsName like {:?} in its CRS's axis order, and {}",
                srs.raw, vote.reason
            ),
        })
    }

    fn wfs_vote(&self) -> Option<Vote> {
        let version = self.context.wfs_version.as_deref()?;
        if version.starts_with("1.0") {
            return Some(Vote {
                swap: false,
                reason: "WFS 1.0 responses are always x/y".into(),
            });
        }
        let authority = |what: &str| {
            let vote = self.authority_order().ok()?;
            Some(Vote {
                swap: vote.swap,
                reason: format!("{what} follow the CRS's axis order, and {}", vote.reason),
            })
        };
        let response = self.srs.map(|srs| srs.form);
        // GeoServer (GeoTools' GML encoder) writes the legacy URL exactly when
        // its coordinates are x/y, whatever the WFS version: the URN forms
        // promise the CRS's axis order, so a request for CRS84 or a short
        // `EPSG:4326` comes back as `…/epsg.xml#4326`, longitude first.
        if response == Some(SrsNameForm::LegacyUrl) {
            return Some(Vote {
                swap: false,
                reason: "WFS 1.1 and 2.0 responses with a legacy URL srsName are x/y (GeoServer writes that form exactly when its coordinates are)".into(),
            });
        }
        if version.starts_with("1.1") {
            // The response's own URN wins over the form we asked for: a
            // server may answer a request for one form with another (GeoServer
            // answers a legacy URL with a URN when it won't write x/y).
            let form = match response {
                None | Some(SrsNameForm::Short | SrsNameForm::Unknown) => {
                    self.context.requested_srs.as_ref().or(self.srs)?.form
                }
                Some(form) => form,
            };
            return match form {
                SrsNameForm::Short | SrsNameForm::LegacyUrl => Some(Vote {
                    swap: false,
                    reason: "WFS 1.1 responses with a short srsName are x/y".into(),
                }),
                SrsNameForm::Unknown => None,
                _ => authority("WFS 1.1 responses with a URN srsName"),
            };
        }
        if version.starts_with('2') {
            return authority("WFS 2.0 responses");
        }
        None
    }

    /// Supporting evidence only: report envelopes that disagree with the decision.
    fn envelope_warnings(&self, swap: bool, conflicts: &mut Vec<String>) {
        let Some(sampled) = self.evidence.sampled_bbox else {
            return;
        };
        if let Some(envelope) = self.evidence.envelope_bbox
            && !intersects(sampled, envelope)
            && intersects(swapped_bbox(sampled), envelope)
        {
            conflicts.push(
                "the boundedBy envelope is written in the other axis order than the geometries"
                    .into(),
            );
        }
        if let Some(requested) = self.context.requested_bbox {
            let output = if swap { swapped_bbox(sampled) } else { sampled };
            let other = if swap { sampled } else { swapped_bbox(sampled) };
            if !intersects(output, requested) && intersects(other, requested) {
                conflicts.push(
                    "read this way, the geometries fall outside the requested BBOX, but inside it the other way"
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

/// The override for an srsName as written. Surrounding whitespace is ignored
/// on both sides, nothing else (`docs/geometry.md`, "Decision key and scope").
fn override_for<'a>(
    overrides: &'a IndexMap<String, AxisOrderMode>,
    srs: &str,
) -> Option<&'a AxisOrderMode> {
    let srs = srs.trim();
    overrides.get(srs).or_else(|| {
        overrides
            .iter()
            .find(|(key, _)| key.trim() == srs)
            .map(|(_, mode)| mode)
    })
}
