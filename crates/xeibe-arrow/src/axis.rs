//! Applies per-key axis decisions: from overrides (settings file), or from
//! evidence gathered by a scan or by the features buffered before a read's first batch.

use std::collections::BTreeMap;

use xeibe_core::{Dialect, SourceId};
use xeibe_geom::{AxisDecision, AxisKey, AxisResolver};
use xeibe_schema::DatasetObservation;

/// All decisions for one layer/column, looked up while parsing.
pub struct AxisDecisions {
    decisions: BTreeMap<AxisKey, AxisDecision>,
    source: SourceId,
}

impl AxisDecisions {
    /// `observation`: a scan, or the path tree of a read's buffered features.
    pub fn from_observation(
        observation: &DatasetObservation,
        layer: &str,
        column: &str,
        options: &xeibe_geom::AxisOrderOptions,
    ) -> Self {
        todo!()
    }

    pub fn for_source(&self, source: SourceId) -> Self {
        todo!()
    }
}

impl AxisResolver for AxisDecisions {
    fn resolve(&self, srs_name: Option<&str>, dialect: Dialect) -> AxisDecision {
        todo!()
    }
}
