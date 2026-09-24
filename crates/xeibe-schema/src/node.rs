use xeibe_core::{Location, QName, ns};
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
    /// Instances whose content was exactly one child element (type-wrapper
    /// detection).
    pub single_child: u64,
    pub empty: u64,
    pub nil: NilStats,

    // structure
    /// Every attribute except `xsi:nil`, including `gml:id` and `xlink:*`.
    /// A `nilReason` on a nil element is kept in [`NilStats`] instead.
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
    /// By the first character: GML encodes object/type elements in
    /// UpperCamelCase and property elements in lowerCamelCase.
    pub fn of(local_name: &str) -> Self {
        match local_name.chars().next() {
            Some(c) if c.is_uppercase() => NameShape::UpperCamel,
            Some(c) if c.is_lowercase() => NameShape::LowerCamel,
            _ => NameShape::Other,
        }
    }
}

/// Bound on [`NilStats::reasons`].
const MAX_NIL_REASONS: usize = 16;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NilStats {
    pub count: u64,
    /// Distinct `nilReason` values (bounded).
    pub reasons: Vec<String>,
}

impl NilStats {
    pub fn add_reason(&mut self, reason: &str) {
        if self.reasons.len() < MAX_NIL_REASONS && !self.reasons.iter().any(|r| r == reason) {
            self.reasons.push(reason.to_string());
        }
    }
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

/// Attributes that describe the encoding rather than the data: `xsi:*`,
/// `xlink:*` and `nilReason`. They don't make an element "text + attributes"
/// and don't stop a property from being a type wrapper.
pub fn is_link_or_nil_attribute(name: &QName) -> bool {
    matches!(name.ns.as_deref(), Some(ns::XLINK) | Some(ns::XSI)) || &*name.local == "nilReason"
}

/// `gml:id` in either GML namespace.
pub fn is_gml_id(name: &QName) -> bool {
    name.is_gml_named("id")
}

impl ElementNode {
    /// A node for an element seen for the first time at `first_seen`.
    pub fn new(local_name: &str, first_seen: Option<(u32, u64)>) -> Self {
        ElementNode {
            name_shape: NameShape::of(local_name),
            first_seen,
            ..ElementNode::default()
        }
    }

    pub fn text_count(&self) -> u64 {
        self.text.as_ref().map_or(0, |text| text.count)
    }

    /// Attributes that carry data (not `xsi`, `xlink` or `nilReason`).
    pub fn data_attributes(&self) -> impl Iterator<Item = (&QName, &ValueStats)> + '_ {
        self.attributes
            .iter()
            .filter(|(name, _)| !is_link_or_nil_attribute(name))
    }

    pub fn shape(&self) -> Shape {
        if self.geometry.is_some() {
            Shape::Geometry
        } else if self.mixed {
            Shape::Mixed
        } else if !self.children.is_empty() || self.truncated {
            Shape::ElementsOnly
        } else if self.text_count() > 0 {
            if self.data_attributes().next().is_some() {
                Shape::TextAndAttributes
            } else {
                Shape::TextOnly
            }
        } else if self.by_reference > 0 {
            Shape::ByReferenceOnly
        } else {
            Shape::Empty
        }
    }

    /// INSPIRE-style type wrapper (see `collapse_type_wrappers`): every
    /// instance of this property that has content holds exactly one child
    /// element; every child name seen there is UpperCamel, never repeats and
    /// has no attributes but `gml:id`; the property has no text and no
    /// attributes but `xlink`/`nil` ones. Several child names (XPlanung's
    /// `XP_ExterneReferenz` or `XP_SpezExterneReferenz`) are several wrapper
    /// types, merged by the rule engine.
    pub fn is_type_wrapper(&self) -> bool {
        if self.children.is_empty()
            || self.geometry.is_some()
            || self.truncated
            || self.mixed
            || self.text_count() > 0
            || self.data_attributes().next().is_some()
        {
            return false;
        }
        let with_content = self.instances.saturating_sub(self.empty + self.by_reference);
        self.single_child == with_content
            && self.children.values().all(|child| {
                child.name_shape == NameShape::UpperCamel
                    && child.max_occurs == 1
                    && child.attributes.keys().all(is_gml_id)
            })
    }

    pub fn child_mut(&mut self, name: &QName, first_seen: (u32, u64)) -> &mut ElementNode {
        self.children
            .entry(name.clone())
            .or_insert_with(|| ElementNode::new(&name.local, Some(first_seen)))
    }
}

impl Merge for ElementNode {
    fn merge(&mut self, other: Self) {
        self.instances += other.instances;
        self.parents_with += other.parents_with;
        self.max_occurs = self.max_occurs.max(other.max_occurs);
        self.first_multi = earliest(self.first_multi.take(), other.first_multi);

        self.text = merge_option(self.text.take(), other.text);
        self.mixed |= other.mixed;
        self.single_child += other.single_child;
        self.empty += other.empty;
        self.nil.merge(other.nil);

        for (name, stats) in other.attributes {
            match self.attributes.get_mut(&name) {
                Some(existing) => existing.merge(stats),
                None => {
                    self.attributes.insert(name, stats);
                }
            }
        }
        let mut reorder = false;
        for (name, child) in other.children {
            match self.children.get_mut(&name) {
                Some(existing) => existing.merge(child),
                None => {
                    self.children.insert(name, child);
                    reorder = true;
                }
            }
        }
        if reorder || other.first_seen < self.first_seen {
            // Children in order of first appearance, whatever the merge order.
            // `None` (never seen) sorts last.
            self.children
                .sort_by_cached_key(|_, child| child.first_seen.map_or((1, 0, 0), |(s, o)| (0, s, o)));
        }
        self.first_seen = match (self.first_seen, other.first_seen) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };

        self.geometry = merge_option(self.geometry.take(), other.geometry);
        self.by_reference += other.by_reference;
        self.href_and_content += other.href_and_content;
        self.has_gml_id += other.has_gml_id;
        self.truncated |= other.truncated;
    }
}

impl Merge for NilStats {
    fn merge(&mut self, other: Self) {
        self.count += other.count;
        for reason in &other.reasons {
            self.add_reason(reason);
        }
    }
}

pub(crate) fn merge_option<T: Merge>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(mut a), Some(b)) => {
            a.merge(b);
            Some(a)
        }
        (a, b) => a.or(b),
    }
}

/// The location earlier in the input: by source, then byte offset.
fn earliest(a: Option<Location>, b: Option<Location>) -> Option<Location> {
    match (a, b) {
        (Some(a), Some(b)) => {
            if (b.source, b.byte_offset) < (a.source, a.byte_offset) {
                Some(b)
            } else {
                Some(a)
            }
        }
        (a, b) => a.or(b),
    }
}
