use std::collections::{BTreeMap, BTreeSet};

use xeibe_core::Dialect;
use xeibe_geom::sniff::GeometrySniff;
use xeibe_geom::{AxisEvidence, GeomKind};
use serde::{Deserialize, Serialize};

use crate::Merge;

/// Key of axis evidence within one column (source is added by the dataset).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ColumnAxisKey {
    pub source: u32,
    pub srs_name: Option<String>,
    pub dialect: Dialect,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeometryStats {
    pub count: u64,
    pub kinds: BTreeSet<GeomKind>,
    pub has_curves: bool,
    pub has_unsupported: bool,
    /// Effective srsDimension values.
    pub dims: BTreeSet<u8>,
    /// srsName as written → count.
    pub srs: BTreeMap<String, u64>,
    /// Per (source, srsName, dialect): sampled positions, axisLabels, envelopes.
    pub axis_evidence: BTreeMap<ColumnAxisKey, AxisEvidence>,
    /// Geometry given only as `xlink:href`.
    pub by_reference: u64,
    pub empty: u64,
}

impl GeometryStats {
    /// Record one sniffed geometry of source `source`.
    pub fn observe(&mut self, source: u32, sniff: &GeometrySniff) {
        self.count += 1;
        // The outermost element is the geometry's kind; nested kinds (a
        // Surface's patches, a Curve's segments) are not the column's.
        if let Some(kind) = sniff.kinds.first() {
            self.kinds.insert(*kind);
        }
        self.has_curves |= sniff.has_curves;
        self.has_unsupported |= sniff.has_unsupported
            || sniff.kinds.contains(&GeomKind::Unsupported);
        if sniff.by_reference {
            self.by_reference += 1;
        }
        if sniff.empty {
            self.empty += 1;
        }
        let position = sniff.first_position.as_deref().filter(|p| p.len() >= 2);
        let dim = sniff
            .srs_dimension
            .or_else(|| position.map(|p| p.len().min(3) as u8));
        if let Some(dim) = dim {
            self.dims.insert(dim);
        } else if !sniff.empty && !sniff.by_reference {
            self.dims.insert(2);
        }
        if let Some(srs) = &sniff.srs_name {
            *self.srs.entry(srs.clone()).or_default() += 1;
        }

        let key = ColumnAxisKey {
            source,
            srs_name: sniff.srs_name.clone(),
            dialect: sniff.dialect.unwrap_or(Dialect::Gml3),
        };
        let evidence = self.axis_evidence.entry(key).or_default();
        if let Some(labels) = &sniff.axis_labels
            && !evidence.axis_labels.contains(labels) {
                evidence.axis_labels.push(labels.clone());
            }
        if let Some(p) = position {
            let point = Some([p[0], p[1], p[0], p[1]]);
            if is_envelope(sniff) {
                evidence.envelope_bbox = union_bbox(evidence.envelope_bbox, point);
            } else {
                evidence.sampled_bbox = union_bbox(evidence.sampled_bbox, point);
                evidence.samples += 1;
            }
        }
    }

    /// The srsName seen most often (ties: the smallest string).
    pub fn main_srs(&self) -> Option<&str> {
        self.srs
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(name, _)| name.as_str())
    }
}

/// `true` for an envelope (`boundedBy`): its corners are envelope evidence,
/// not geometry positions.
pub(crate) fn is_envelope(sniff: &GeometrySniff) -> bool {
    matches!(sniff.kinds.first(), Some(GeomKind::Envelope | GeomKind::Box))
}

pub(crate) fn union_bbox(a: Option<[f64; 4]>, b: Option<[f64; 4]>) -> Option<[f64; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]),
        (a, b) => a.or(b),
    }
}

impl Merge for GeometryStats {
    fn merge(&mut self, other: Self) {
        self.count += other.count;
        self.kinds.extend(other.kinds);
        self.has_curves |= other.has_curves;
        self.has_unsupported |= other.has_unsupported;
        self.dims.extend(other.dims);
        for (srs, count) in other.srs {
            *self.srs.entry(srs).or_default() += count;
        }
        for (key, evidence) in other.axis_evidence {
            match self.axis_evidence.get_mut(&key) {
                Some(existing) => existing.merge(evidence),
                None => {
                    self.axis_evidence.insert(key, evidence);
                }
            }
        }
        self.by_reference += other.by_reference;
        self.empty += other.empty;
    }
}
