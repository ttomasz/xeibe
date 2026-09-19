use xeibe_core::{Location, QName};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::geometry_stats::GeometryStats;
use crate::{Merge, ValueStats};

/// One element path in a layer's tree. Records facts only; no decisions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ElementNode {
    // occurrence → List detection; presence is shown by `--explain`
    /// Total occurrences of this element.
    pub instances: u64,
    /// Parent instances that contained it at least once.
    pub parents_with: u64,
    /// Max repetitions within one parent instance.
    pub max_occurs: u32,
    /// Evidence: where `max_occurs` first exceeded 1.
    pub first_multi: Option<Location>,

    // content shape
    pub text: Option<ValueStats>,
    /// Text and child elements in the same instance.
    pub mixed: bool,
    pub empty: u64,
    pub nil: NilStats,

    // structure
    pub attributes: IndexMap<QName, ValueStats>,
    /// First-seen order → stable column order.
    pub children: IndexMap<QName, ElementNode>,
    /// Earliest `(source, byte_offset)` seen; used to order children on merge.
    pub first_seen: Option<(u32, u64)>,

    // GML-specific
    pub geometry: Option<GeometryStats>,
    /// Instances that were only `xlink:href`, no content.
    pub by_reference: u64,
    /// Instances with both `xlink:href` and content.
    pub href_and_content: u64,
    /// Instances with their own `gml:id` (nested object/feature).
    pub has_gml_id: u64,
    pub name_shape: NameShape,

    /// Depth/width limit hit; subtree not tracked.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NameShape {
    /// Object/type element, e.g. `AD_IdentyfikatorIIP`.
    UpperCamel,
    /// Property element, e.g. `lokalnyId`.
    #[default]
    LowerCamel,
    Other,
}

impl NameShape {
    pub fn of(local_name: &str) -> Self {
        todo!()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NilStats {
    pub count: u64,
    /// Distinct `nilReason` values (bounded).
    pub reasons: Vec<String>,
}

/// Content shape of a node, derived from its stats (used by the rule engine).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    TextOnly,
    TextAndAttributes,
    ElementsOnly,
    ByReferenceOnly,
    Mixed,
    Geometry,
    /// Never had content.
    Empty,
}

impl ElementNode {
    pub fn shape(&self) -> Shape {
        todo!()
    }

    /// INSPIRE-style type wrapper (see `collapse_type_wrappers`).
    pub fn is_type_wrapper(&self) -> bool {
        todo!()
    }

    pub fn child_mut(&mut self, name: &QName, first_seen: (u32, u64)) -> &mut ElementNode {
        todo!()
    }
}

impl Merge for ElementNode {
    fn merge(&mut self, other: Self) {
        todo!()
    }
}

impl Merge for NilStats {
    fn merge(&mut self, other: Self) {
        todo!()
    }
}
