use std::collections::{BTreeMap, BTreeSet};

use xeibe_core::Dialect;
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

impl Merge for GeometryStats {
    fn merge(&mut self, other: Self) {
        todo!()
    }
}
