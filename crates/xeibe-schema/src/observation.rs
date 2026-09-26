use std::collections::BTreeSet;
use std::sync::Arc;

use xeibe_core::{GmlVersion, QName};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::geometry_stats::union_bbox;
use crate::{ElementNode, Merge};

/// Everything a scan observed. Kept in memory only; the settings file stores
/// schemas, not trees.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DatasetObservation {
    pub gml_versions: BTreeSet<GmlVersion>,
    pub layers: IndexMap<QName, LayerObservation>,
    /// The scan stopped before the end of the input (sampled scan).
    pub sampled: bool,
    /// Per-source context for axis decisions (FME, producer, WFS request),
    /// indexed by `SourceId` when the observation comes from one
    /// [`crate::Scanner::run`].
    pub source_context: Vec<SourceContext>,
    /// Namespace URI → the prefix it was first declared with, for readable
    /// `gml:path` metadata. Bounded; URIs without an entry get `ns1`, `ns2`, ….
    pub prefixes: IndexMap<Arc<str>, String>,
    /// Union of the collections' `boundedBy` envelopes (as written): the
    /// extent the producer declares for the whole dataset, all layers.
    #[serde(default)]
    pub extent: Option<[f64; 4]>,
    /// Sources in which the splitter found no feature collection or feature
    /// member (ISO metadata next to the GML in a zip, say), by name. They
    /// keep their index in `source_context`.
    #[serde(default)]
    pub skipped_sources: Vec<String>,
}

/// Bound on [`DatasetObservation::prefixes`].
pub(crate) const MAX_PREFIXES: usize = 256;

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
    /// Union of geometry bboxes (as written), for layer listings. Built from
    /// the first position of each geometry, which is what the scan reads.
    pub extent: Option<[f64; 4]>,
}

impl DatasetObservation {
    /// A layer by local name (`AD_PunktAdresowy`), prefixed name
    /// (`prgad:AD_PunktAdresowy`) or Clark notation (`{uri}AD_PunktAdresowy`).
    ///
    /// A prefixed name is resolved with the prefixes the scan saw; if the
    /// prefix is unknown, the local name decides. A local name shared by
    /// layers in different namespaces is ambiguous.
    pub fn layer(&self, name: &str) -> crate::Result<(&QName, &LayerObservation)> {
        if name.starts_with('{') {
            return self
                .layers
                .iter()
                .find(|(qname, _)| qname.to_clark() == name)
                .ok_or_else(|| crate::Error::UnknownLayer(name.to_string()));
        }
        let (prefix, local) = match name.split_once(':') {
            Some((prefix, local)) => (Some(prefix), local),
            None => (None, name),
        };
        let uri = prefix.and_then(|prefix| {
            self.prefixes
                .iter()
                .find(|(_, p)| p.as_str() == prefix)
                .map(|(uri, _)| uri.clone())
        });
        let mut matches = self.layers.iter().filter(|(qname, _)| {
            &*qname.local == local && uri.as_ref().is_none_or(|uri| qname.ns.as_ref() == Some(uri))
        });
        let first = matches
            .next()
            .ok_or_else(|| crate::Error::UnknownLayer(name.to_string()))?;
        if let Some(other) = matches.next() {
            return Err(crate::Error::UnknownLayer(format!(
                "{name} is ambiguous: {} and {}",
                first.0, other.0
            )));
        }
        Ok(first)
    }

    /// Remember the prefix of a namespace URI, if it has none yet.
    pub(crate) fn note_prefix(&mut self, uri: &str, prefix: &str) {
        if self.prefixes.len() < MAX_PREFIXES && !self.prefixes.contains_key(uri) {
            self.prefixes.insert(Arc::from(uri), prefix.to_string());
        }
    }
}

impl Merge for DatasetObservation {
    fn merge(&mut self, other: Self) {
        self.gml_versions.extend(other.gml_versions);
        let mut reorder = false;
        for (name, layer) in other.layers {
            match self.layers.get_mut(&name) {
                Some(existing) => existing.merge(layer),
                None => {
                    self.layers.insert(name, layer);
                    reorder = true;
                }
            }
        }
        if reorder {
            // Layers in order of their first feature, whatever the merge order.
            self.layers
                .sort_by_cached_key(|_, layer| layer.root.first_seen.map_or((1, 0, 0), |(s, o)| (0, s, o)));
        }
        self.sampled |= other.sampled;
        self.source_context.extend(other.source_context);
        self.extent = union_bbox(self.extent, other.extent);
        self.skipped_sources.extend(other.skipped_sources);
        for (uri, prefix) in other.prefixes {
            self.note_prefix(&uri, &prefix);
        }
    }
}

impl Merge for LayerObservation {
    fn merge(&mut self, other: Self) {
        self.feature_count += other.feature_count;
        self.root.merge(other.root);
        self.extent = union_bbox(self.extent, other.extent);
    }
}
