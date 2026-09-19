use std::collections::BTreeSet;

use xeibe_core::{GmlVersion, QName};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{ElementNode, Merge};

/// Everything a scan observed. Kept in memory only; the settings file stores
/// schemas, not trees.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DatasetObservation {
    pub gml_versions: BTreeSet<GmlVersion>,
    pub layers: IndexMap<QName, LayerObservation>,
    /// The scan stopped before the end of the input (sampled scan).
    pub sampled: bool,
    /// Per-source context for axis decisions (FME, producer, WFS request).
    pub source_context: Vec<SourceContext>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceContext {
    pub fme_produced: bool,
    pub producer: Option<String>,
    pub wfs_version: Option<String>,
    pub requested_srs: Option<String>,
    pub requested_bbox: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerObservation {
    pub feature_count: u64,
    /// The feature element itself.
    pub root: ElementNode,
    /// Union of geometry bboxes (as written), for layer listings.
    pub extent: Option<[f64; 4]>,
}

impl DatasetObservation {
    pub fn layer(&self, name: &str) -> crate::Result<(&QName, &LayerObservation)> {
        todo!()
    }
}

impl Merge for DatasetObservation {
    fn merge(&mut self, other: Self) {
        todo!()
    }
}

impl Merge for LayerObservation {
    fn merge(&mut self, other: Self) {
        todo!()
    }
}
