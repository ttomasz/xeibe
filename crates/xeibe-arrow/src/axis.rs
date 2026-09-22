//! Applies per-key axis decisions: from overrides (settings file), or from
//! evidence gathered by a scan or by the features buffered before a read's first batch.
//!
//! A decision is made once per key (source, srsName as written, dialect) and
//! shared by every worker; keys the evidence never saw (a srsName that first
//! turns up after the sample) are decided when they first turn up, with the
//! evidence there is (none), which leaves `Auto` to its fallback.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use xeibe_core::{Dialect, SourceId};
use xeibe_geom::axis::{AxisContext, decide};
use xeibe_geom::epsg::CrsTable;
use xeibe_geom::{AxisDecision, AxisEvidence, AxisKey, AxisResolver, CrsRef, SrsName};
use xeibe_schema::DatasetObservation;
use xeibe_schema::observation::SourceContext;

use crate::report::{Warning, WarningKind};

/// Per-source axis context, shared with the read's source stream, which adds
/// an entry for every source it opens.
pub type SharedContexts = Arc<Mutex<Vec<SourceContext>>>;

/// All decisions for one layer/column, looked up while parsing.
pub struct AxisDecisions {
    evidence: Arc<BTreeMap<AxisKey, AxisEvidence>>,
    options: Arc<xeibe_geom::AxisOrderOptions>,
    layer: Arc<str>,
    column: Arc<str>,
    contexts: SharedContexts,
    /// Every decision made so far, by any worker.
    decisions: Arc<Mutex<BTreeMap<AxisKey, AxisDecision>>>,
    /// This copy's own cache (one source), so that a lookup doesn't contend.
    local: Mutex<Vec<(Option<String>, Dialect, AxisDecision)>>,
    source: SourceId,
}

impl AxisDecisions {
    /// `observation`: a scan, or the path tree of a read's buffered features.
    /// The evidence of every geometry property of the layer counts: decisions
    /// are per key, not per column (the column matters for overrides only).
    pub fn from_observation(
        observation: &DatasetObservation,
        layer: &str,
        column: &str,
        options: &xeibe_geom::AxisOrderOptions,
    ) -> Self {
        let mut evidence: BTreeMap<AxisKey, AxisEvidence> = BTreeMap::new();
        if let Ok((_, layer_observation)) = observation.layer(layer) {
            collect_evidence(&layer_observation.root, &mut evidence);
        }
        AxisDecisions {
            evidence: Arc::new(evidence),
            options: Arc::new(options.clone()),
            layer: Arc::from(crate::settings::local_name(layer)),
            column: Arc::from(column),
            contexts: Arc::new(Mutex::new(observation.source_context.clone())),
            decisions: Arc::new(Mutex::new(BTreeMap::new())),
            local: Mutex::new(Vec::new()),
            source: SourceId(0),
        }
    }

    /// Use the read's live per-source contexts instead of the observation's.
    pub fn with_contexts(mut self, contexts: SharedContexts) -> Self {
        self.contexts = contexts;
        self
    }

    pub fn for_source(&self, source: SourceId) -> Self {
        AxisDecisions {
            evidence: self.evidence.clone(),
            options: self.options.clone(),
            layer: self.layer.clone(),
            column: self.column.clone(),
            contexts: self.contexts.clone(),
            decisions: self.decisions.clone(),
            local: Mutex::new(Vec::new()),
            source,
        }
    }

    /// Every decision made so far.
    pub fn decisions(&self) -> Vec<(AxisKey, AxisDecision)> {
        let decisions = self.decisions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        decisions.iter().map(|(key, decision)| (key.clone(), decision.clone())).collect()
    }

    /// `UnknownSrs` / `UnknownCrs` / `AxisConflict` warnings, one per key decided.
    pub fn warnings(&self) -> Vec<Warning> {
        let table = self.options.crs_table.clone().unwrap_or_else(CrsTable::builtin);
        let mut warnings = Vec::new();
        for (key, decision) in self.decisions() {
            let srs = key.srs_name.as_deref().map(SrsName::parse);
            match srs.as_ref().and_then(|srs| srs.crs.as_ref()) {
                None => warnings.push(Warning {
                    kind: WarningKind::UnknownSrs,
                    location: None,
                    message: match &key.srs_name {
                        Some(srs) => format!("{}: srsName {srs:?} not recognised; read as written", key.source),
                        None => format!("{}: geometry without srsName; read as written", key.source),
                    },
                }),
                Some(crs) => {
                    if let CrsRef::Code { authority, code } = crs.horizontal() {
                        if !crs.is_lon_lat_by_definition() && table.get(authority, code).is_none() {
                            warnings.push(Warning {
                                kind: WarningKind::UnknownCrs,
                                location: None,
                                message: format!(
                                    "{}: {authority}:{code} is not in the CRS table; x/y assumed where its axis order was needed",
                                    key.source
                                ),
                            });
                        }
                    }
                }
            }
            if !decision.conflicts.is_empty() {
                warnings.push(Warning {
                    kind: WarningKind::AxisConflict,
                    location: None,
                    message: format!(
                        "{} {:?}: {} (conflicts: {})",
                        key.source,
                        key.srs_name.as_deref().unwrap_or("no srsName"),
                        decision.reason,
                        decision.conflicts.join(", ")
                    ),
                });
            }
        }
        warnings
    }

    fn context(&self, source: SourceId) -> AxisContext {
        let contexts = self.contexts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(context) = contexts.get(source.0 as usize) else {
            return AxisContext::default();
        };
        AxisContext {
            fme_produced: context.fme_produced,
            producer: context.producer.clone(),
            wfs_version: context.wfs_version.clone(),
            requested_srs: context.requested_srs.as_deref().map(SrsName::parse),
            requested_bbox: context.requested_bbox,
            source: None,
        }
    }
}

impl AxisResolver for AxisDecisions {
    fn resolve(&self, srs_name: Option<&str>, dialect: Dialect) -> AxisDecision {
        let mut local = self.local.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((_, _, decision)) = local
            .iter()
            .find(|(srs, d, _)| srs.as_deref() == srs_name && *d == dialect)
        {
            return decision.clone();
        }
        let key = AxisKey { source: self.source, srs_name: srs_name.map(str::to_string), dialect };
        let decision = {
            let mut decisions = self.decisions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match decisions.get(&key) {
                Some(decision) => decision.clone(),
                None => {
                    let empty = AxisEvidence::default();
                    let evidence = self.evidence.get(&key).unwrap_or(&empty);
                    let context = self.context(self.source);
                    let decision = decide(&key, Some(&self.layer), Some(&self.column), evidence, &context, &self.options);
                    decisions.insert(key, decision.clone());
                    decision
                }
            }
        };
        local.push((srs_name.map(str::to_string), dialect, decision.clone()));
        decision
    }
}

/// Axis evidence of every geometry property below `node`, keyed per source.
fn collect_evidence(node: &xeibe_schema::ElementNode, out: &mut BTreeMap<AxisKey, AxisEvidence>) {
    if let Some(stats) = &node.geometry {
        for (key, evidence) in &stats.axis_evidence {
            let key = AxisKey {
                source: SourceId(key.source),
                srs_name: key.srs_name.clone(),
                dialect: key.dialect,
            };
            match out.get_mut(&key) {
                Some(existing) => existing.merge(evidence.clone()),
                None => {
                    out.insert(key, evidence.clone());
                }
            }
        }
    }
    for child in node.children.values() {
        collect_evidence(child, out);
    }
}
