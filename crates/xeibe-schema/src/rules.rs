//! Rule engine: one walk over a layer's tree → a flat Arrow schema
//! (`docs/schema-inference.md` §3–§4).
//!
//! The walk emits one [`Col`] per leaf (an element's text, an attribute, a
//! geometry property) with its path in the settings-file syntax: local names,
//! `*` for a type wrapper, `[]` on the anchor of a list, a prefix only where
//! siblings differ only by namespace. Names are chosen afterwards, shortest
//! unique first (§3.1). The routes a read follows come from binding the
//! finished schema, exactly as for a schema given by the user.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use arrow_schema::{DataType, Field, Fields, Schema, TimeUnit};
use geoarrow_schema::{
    BoxType, CoordType, Crs, Dimension, LineStringType, Metadata, MultiLineStringType,
    MultiPointType, MultiPolygonType, PointType, PolygonType, WkbType,
};
use indexmap::IndexMap;
use xeibe_core::{QName, SourceId, ns};
use xeibe_geom::options::{CurveMode, GeomEncoding};
use xeibe_geom::{AxisKey, GeomKind, SrsName};

use crate::geometry_stats::GeometryStats;
use crate::node::{Shape, is_gml_id};
use crate::options::{
    AllNull, AttrSelect, BoundedBy, ConstantAttrs, FieldOverride, IdMode, IntWidth, ListRule,
    Lossless, MixedContent, XlinkMode, string_type,
};
use crate::value::{TzShape, ValueStats};
use crate::{DatasetObservation, ElementNode, InferenceOptions, Merge, SampleOptions, TypeSet};

/// Metadata keys written on fields and schemas (see `docs/type-mapping.md`).
pub mod meta {
    /// The column's path, in the settings-file syntax.
    pub const PATH: &str = "gml:path";
    /// Schema-level: JSON object prefix → URI for prefixed path steps.
    pub const NS: &str = "gml:ns";
    pub const MAX_SCALE: &str = "gml:max_scale";
    pub const ATTR_PREFIX: &str = "gml:attr:";
    pub const TZ_OFFSET: &str = "gml:tz_offset";
    pub const SRS_NAME: &str = "gml:srs_name";
    pub const AXIS_SWAPPED: &str = "gml:axis_swapped";
    pub const AXIS_DECISION: &str = "gml:axis_decision";
    /// `text`: a `text` column that takes an element's text with the markup
    /// removed (`MixedContent::TextOnly`) instead of its raw XML.
    pub const CONTENT: &str = "gml:content";
    /// Schema-level: the settings a read used (optional).
    pub const SETTINGS: &str = "gml:settings";
    /// Schema-level: the GML versions seen, e.g. `3.2` or `2,3.1`.
    pub const VERSIONS: &str = "gml:versions";
}

/// A layer's schema bound to XML paths: inferred (with the decisions behind it)
/// or given by the user and bound with [`crate::bind_schema`].
#[derive(Debug, Clone)]
pub struct LayerSchema {
    pub layer: QName,
    pub schema: Schema,
    /// One per column, in schema order.
    pub routes: Vec<FieldRoute>,
    /// One entry per column, for `--explain`. Empty for a given schema.
    pub decisions: Vec<FieldDecision>,
}

/// Where one column gets its values: its path, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRoute {
    /// Element path from the feature element (which is not included). Empty
    /// for the feature's own attributes. A step without a namespace matches
    /// that local name in any namespace; `*` matches any element.
    pub source_path: Vec<QName>,
    /// The value is this attribute of the last element of `source_path`.
    pub attribute: Option<QName>,
    pub value: RouteValue,
    /// `[column]`: the column's index in the schema (schemas are flat).
    pub field_path: Vec<usize>,
    /// List columns: the index in `source_path` of the anchor, the element
    /// whose occurrences the list follows.
    pub anchor: Option<usize>,
}

/// What a route takes from its element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteValue {
    /// The element's text (or the attribute's value), parsed as the column's
    /// type. A `text` column at an element with child elements gets its raw
    /// XML.
    Text,
    /// The element's text with the markup removed (`gml:content = text`).
    InnerText,
    /// A geometry property: the geometry element inside it.
    Geometry,
    /// The subtree as `Map(path → text)`.
    Map,
    /// The `gml:Envelope` inside the element, as a box.
    BoundingBox,
}

#[derive(Debug, Clone)]
pub struct FieldDecision {
    pub field: String,
    pub reasons: Vec<String>,
}

/// `sampled`: the observation covers only part of the data; apply the
/// conservative rules of `SampleOptions` (docs/schema-inference.md §6.2).
pub fn infer_schema(
    observation: &DatasetObservation,
    layer: &QName,
    options: &InferenceOptions,
    sampled: Option<&SampleOptions>,
) -> crate::Result<LayerSchema> {
    let (layer, layer_observation) = observation
        .layers
        .get_key_value(layer)
        .ok_or_else(|| crate::Error::UnknownLayer(layer.to_string()))?;
    let options = options.for_layer(&layer.to_clark());
    let engine = Engine {
        observation,
        options: &options,
        sampled: sampled.filter(|sample| sample.conservative),
        layer,
        layer_clark: layer.to_clark(),
        prefixes: Prefixes::for_layer(observation, &layer_observation.root),
    };
    let root = &layer_observation.root;
    let mut cols = Vec::new();
    let attributes = engine.attributes(root, &[], None);
    cols.extend(attributes.cols);
    cols.extend(attributes.constants.into_iter().map(|(_, _, col)| col));
    engine.children(root, &[], None, &mut cols);

    let names = column_names(&cols, &engine.prefixes);
    let mut namespaces = serde_json::Map::new();
    let mut fields = Vec::new();
    let mut decisions = Vec::new();
    for (col, name) in cols.into_iter().zip(names) {
        for (prefix, uri) in engine.prefixes_used(&col) {
            namespaces.insert(prefix, uri.into());
        }
        let path = engine.path_string(&col);
        let (field, reasons) = col.into_field(&name, path);
        fields.push(field);
        decisions.push(FieldDecision { field: name, reasons });
    }

    let mut metadata = HashMap::new();
    if !observation.gml_versions.is_empty() {
        let versions: Vec<&str> = observation.gml_versions.iter().map(version_name).collect();
        metadata.insert(meta::VERSIONS.to_string(), versions.join(","));
    }
    if !namespaces.is_empty() {
        metadata.insert(meta::NS.to_string(), serde_json::Value::Object(namespaces).to_string());
    }
    let schema = Schema::new_with_metadata(fields, metadata);
    let mut bound = crate::bind_schema(layer, &schema, &options)?;
    bound.decisions = decisions;
    Ok(bound)
}

fn version_name(version: &xeibe_core::GmlVersion) -> &'static str {
    match version {
        xeibe_core::GmlVersion::V2 => "2",
        xeibe_core::GmlVersion::V3_0 => "3.0",
        xeibe_core::GmlVersion::V3_1 => "3.1",
        xeibe_core::GmlVersion::V3_2 => "3.2",
    }
}

/// One element step of a column's path.
#[derive(Debug, Clone)]
struct Step {
    name: QName,
    /// A type wrapper: `*` in the path, left out of the name.
    wrapper: bool,
    /// A sibling has the same local name in another namespace: the step keeps
    /// its prefix, in the path and in the name.
    prefixed: bool,
}

/// A column before naming.
struct Col {
    steps: Vec<Step>,
    /// A last `@name` step, and whether it keeps its prefix.
    attribute: Option<(QName, bool)>,
    /// The index in `steps` of the list anchor; `Some` makes the column a list.
    anchor: Option<usize>,
    /// The property holds only an `xlink:href`: the name leaves out `@href`.
    by_reference: bool,
    /// The feature's GML 2 `fid`: named `@id`, like a `gml:id`.
    feature_id: bool,
    kind: ColKind,
    metadata: HashMap<String, String>,
    reasons: Vec<String>,
}

enum ColKind {
    Scalar(DataType),
    /// A ready-made field (GeoArrow extension types); its name is replaced.
    Field(Field),
}

impl Col {
    fn new(steps: &[Step], anchor: Option<usize>, data_type: DataType) -> Self {
        Col {
            steps: steps.to_vec(),
            attribute: None,
            anchor,
            by_reference: false,
            feature_id: false,
            kind: ColKind::Scalar(data_type),
            metadata: HashMap::new(),
            reasons: Vec::new(),
        }
    }

    /// The Arrow field: a list of the value type under an anchor.
    fn into_field(self, name: &str, path: String) -> (Field, Vec<String>) {
        let item = match self.kind {
            ColKind::Scalar(data_type) => Field::new(name, data_type, true),
            ColKind::Field(field) => field.with_name(name),
        };
        let mut metadata = self.metadata;
        metadata.insert(meta::PATH.to_string(), path);
        let field = if self.anchor.is_some() {
            Field::new(name, DataType::List(Arc::new(item.with_name("item"))), true)
        } else {
            metadata.extend(item.metadata().clone());
            item
        };
        (field.with_metadata(metadata), self.reasons)
    }

    /// The steps a name is made of: wrappers and namespace prefixes left out
    /// (unless a sibling differs only by namespace), and `@href` of a
    /// property given only by reference.
    fn name_steps(&self, prefixes: &Prefixes) -> Vec<String> {
        let display = |name: &QName, prefixed: bool| {
            if prefixed { prefixes.prefixed(name) } else { name.local.to_string() }
        };
        let mut steps: Vec<String> = self
            .steps
            .iter()
            .filter(|step| !step.wrapper)
            .map(|step| display(&step.name, step.prefixed))
            .collect();
        if let Some((attribute, prefixed)) = &self.attribute {
            if self.feature_id {
                steps.push("@id".to_string());
            } else if !self.by_reference || steps.is_empty() {
                steps.push(format!("@{}", display(attribute, *prefixed)));
            }
        }
        if steps.is_empty() {
            // A path of wrappers only; name it by its path.
            steps.push("*".to_string());
        }
        steps
    }
}

/// Shortest unique names (`docs/schema-inference.md` §3.1): start with the
/// last step; while two columns share a name, each of them that has steps
/// left takes one more from the front. Columns that are still alike (their
/// steps are the same) are told apart by a number.
fn column_names(cols: &[Col], prefixes: &Prefixes) -> Vec<String> {
    column_names_of(&cols.iter().map(|col| col.name_steps(prefixes)).collect::<Vec<_>>())
}

fn column_names_of(steps: &[Vec<String>]) -> Vec<String> {
    let mut taken: Vec<usize> = vec![1; steps.len()];
    let name = |i: usize, taken: &[usize]| steps[i][steps[i].len() - taken[i]..].join(".");
    loop {
        let names: Vec<String> = (0..steps.len()).map(|i| name(i, &taken)).collect();
        let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, name) in names.iter().enumerate() {
            groups.entry(name).or_default().push(i);
        }
        let mut grew = false;
        for members in groups.values().filter(|members| members.len() > 1) {
            for &i in members {
                if taken[i] < steps[i].len() {
                    taken[i] += 1;
                    grew = true;
                }
            }
        }
        if !grew {
            let mut seen: HashMap<String, usize> = HashMap::new();
            return names
                .into_iter()
                .map(|name| {
                    let count = seen.entry(name.clone()).or_default();
                    *count += 1;
                    if *count == 1 { name } else { format!("{name}#{count}") }
                })
                .collect();
        }
    }
}

/// An override plan for one path: all matching overrides, most specific last.
#[derive(Default)]
struct Plan {
    drop: bool,
    data_type: Option<DataType>,
    raw_xml: bool,
    map: bool,
    list: Option<bool>,
    reasons: Vec<String>,
}

/// A scalar type decision.
struct Scalar {
    data_type: DataType,
    metadata: Vec<(String, String)>,
    reasons: Vec<String>,
}

/// The attribute columns of one element.
struct AttributeCols {
    cols: Vec<Col>,
    /// Attributes with one value throughout: `(metadata key, value, column)`.
    /// They go into the metadata of the element's own column if it has one,
    /// else they stay columns.
    constants: Vec<(String, String, Col)>,
}

struct Engine<'a> {
    observation: &'a DatasetObservation,
    options: &'a InferenceOptions,
    /// Conservative sample rules, if they apply.
    sampled: Option<&'a SampleOptions>,
    layer: &'a QName,
    layer_clark: String,
    prefixes: Prefixes,
}

impl Engine<'_> {
    fn string(&self) -> DataType {
        string_type(&self.options.types)
    }

    fn all_null(&self) -> DataType {
        match self.options.types.all_null {
            AllNull::Utf8View => self.string(),
            AllNull::Null => DataType::Null,
        }
    }

    // ---- paths --------------------------------------------------------------

    /// The path in the settings-file syntax: `idIIP/*/lokalnyId`,
    /// `adres[]/numer`, `miejscowosc/@href`, `app:code`.
    fn path_string(&self, col: &Col) -> String {
        let mut parts: Vec<String> = col
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let mut part = if step.wrapper {
                    "*".to_string()
                } else if step.prefixed {
                    self.prefixes.prefixed(&step.name)
                } else {
                    step.name.local.to_string()
                };
                if col.anchor == Some(index) {
                    part.push_str("[]");
                }
                part
            })
            .collect();
        if let Some((attribute, prefixed)) = &col.attribute {
            let name = if *prefixed { self.prefixes.prefixed(attribute) } else { attribute.local.to_string() };
            parts.push(format!("@{name}"));
        }
        parts.join("/")
    }

    /// `(prefix, uri)` of the prefixed steps of a column's path.
    fn prefixes_used(&self, col: &Col) -> Vec<(String, String)> {
        let steps = col.steps.iter().filter(|step| step.prefixed && !step.wrapper).map(|step| &step.name);
        let attribute = col.attribute.iter().filter(|(_, prefixed)| *prefixed).map(|(name, _)| name);
        steps
            .chain(attribute)
            .filter_map(|name| {
                let uri = name.ns.as_ref()?;
                Some((self.prefixes.prefix(uri).to_string(), uri.to_string()))
            })
            .collect()
    }

    fn with_step(steps: &[Step], step: Step) -> Vec<Step> {
        let mut steps = steps.to_vec();
        steps.push(step);
        steps
    }

    // ---- overrides ----------------------------------------------------------

    fn plan(&self, steps: &[Step], attribute: Option<&QName>) -> Plan {
        let mut names: Vec<String> = steps.iter().map(|step| step.name.to_clark()).collect();
        if let Some(attribute) = attribute {
            names.push(format!("@{}", attribute.to_clark()));
        }
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let matches = |pattern: &crate::PathPattern| pattern.matches(&self.layer_clark, &names);

        let mut plan = Plan::default();
        let structure = &self.options.structure;
        if structure.force_list.iter().any(matches) {
            plan.list = Some(true);
            plan.reasons.push("list: force_list".to_string());
        }
        if structure.force_scalar.iter().any(matches) {
            plan.list = Some(false);
            plan.reasons.push("scalar: force_scalar".to_string());
        }
        let mut overrides: Vec<_> = self
            .options
            .overrides
            .iter()
            .filter(|(pattern, _)| matches(pattern))
            .collect();
        overrides.sort_by_key(|(pattern, _)| pattern.specificity());
        for (pattern, action) in overrides {
            plan.reasons.push(format!("override {}: {action:?}", pattern.raw));
            match action {
                FieldOverride::Type(data_type) => plan.data_type = Some(data_type.clone()),
                FieldOverride::Drop => plan.drop = true,
                FieldOverride::AsRawXml => plan.raw_xml = true,
                FieldOverride::AsMap => plan.map = true,
                FieldOverride::List => plan.list = Some(true),
                FieldOverride::Scalar => plan.list = Some(false),
            }
        }
        plan
    }

    // ---- attributes ---------------------------------------------------------

    fn keep_attribute(&self, name: &QName) -> bool {
        let gml = &self.options.gml;
        if name.ns.as_deref() == Some(ns::XSI) || (name.ns.is_none() && &*name.local == "nilReason") {
            return false;
        }
        if is_gml_id(name) {
            return gml.gml_id == IdMode::Column;
        }
        if name.ns.as_deref() == Some(ns::XLINK) {
            return match &*name.local {
                "href" => gml.xlink != XlinkMode::Drop,
                "title" | "role" | "arcrole" => gml.xlink == XlinkMode::Full,
                _ => gml.xlink != XlinkMode::Drop && !gml.drop_control_attributes,
            };
        }
        let control = matches!(&*name.local, "owns" | "remoteSchema" | "aggregationType")
            && (name.ns.is_none() || name.is_gml());
        if control && gml.drop_control_attributes {
            return false;
        }
        let listed = |list: &[String]| {
            list.iter().any(|entry| {
                let entry = entry.strip_prefix('@').unwrap_or(entry);
                entry == &*name.local || entry == name.to_clark() || entry == self.prefixes.prefixed(name)
            })
        };
        match &self.options.structure.xml_attributes {
            AttrSelect::All => true,
            AttrSelect::None => false,
            AttrSelect::Only(list) => listed(list),
            AttrSelect::Except(list) => !listed(list),
        }
    }

    /// Columns for the attributes of `node` (at `steps`), and the constant
    /// attributes that can move into the element's own column's metadata.
    fn attributes(&self, node: &ElementNode, steps: &[Step], anchor: Option<usize>) -> AttributeCols {
        let kept: Vec<(&QName, &ValueStats)> =
            node.attributes.iter().filter(|(name, _)| self.keep_attribute(name)).collect();
        let shared = shared_locals(kept.iter().map(|(name, _)| *name));
        let mut out = AttributeCols { cols: Vec::new(), constants: Vec::new() };
        for (name, stats) in kept {
            let plan = self.plan(steps, Some(name));
            if plan.drop {
                continue;
            }
            let prefixed = shared.contains(&*name.local);
            let scalar = self.scalar(Some(stats));
            let mut col = Col::new(steps, anchor, plan.data_type.clone().unwrap_or(scalar.data_type));
            col.attribute = Some((name.clone(), prefixed));
            col.by_reference = is_href(name) && node.shape() == Shape::ByReferenceOnly;
            // GML 2 writes the feature's id as `fid`: it is `@id` too, unless the
            // feature also has a `gml:id`.
            col.feature_id = steps.is_empty()
                && name.ns.is_none()
                && &*name.local == "fid"
                && !node.attributes.keys().any(is_gml_id);
            col.metadata.extend(scalar.metadata);
            col.reasons = plan.reasons;
            col.reasons.extend(scalar.reasons);
            if col.feature_id {
                col.reasons.insert(0, "the feature's GML 2 id (fid)".to_string());
            }
            if col.by_reference {
                let strip = if self.options.gml.strip_local_href_hash { ", '#' stripped" } else { "" };
                col.reasons.insert(0, format!("by-reference only (xlink:href){strip}"));
            }
            let constant = self.options.structure.constant_attrs == ConstantAttrs::ToFieldMetadata
                && !is_gml_id(name)
                && !col.feature_id
                && !is_href(name)
                && stats.count == node.instances
                && plan.data_type.is_none();
            match stats.distinct.single().filter(|_| constant) {
                Some(value) => {
                    let key = if prefixed { self.prefixes.prefixed(name) } else { name.local.to_string() };
                    out.constants.push((format!("{}{key}", meta::ATTR_PREFIX), value.to_string(), col));
                }
                None => out.cols.push(col),
            }
        }
        out
    }

    /// `<path>/@nilReason`, when nil values had a reason (or the attribute
    /// was seen) and `nil_reason` is on.
    fn nil_reason(&self, node: &ElementNode, steps: &[Step], anchor: Option<usize>, out: &mut Vec<Col>) {
        let attribute = QName::new(None, "nilReason");
        let seen = node.attributes.contains_key(&attribute);
        if !self.options.gml.nil_reason || (node.nil.reasons.is_empty() && !seen) {
            return;
        }
        let mut col = Col::new(steps, anchor, self.string());
        col.attribute = Some((attribute, false));
        col.reasons.push(format!("nilReason of {} nil values", node.nil.count));
        out.push(col);
    }

    // ---- elements -----------------------------------------------------------

    /// Columns for the element children of `node` (at `steps`).
    fn children(&self, node: &ElementNode, steps: &[Step], anchor: Option<usize>, out: &mut Vec<Col>) {
        let shared = shared_locals(node.children.keys());
        for (name, child) in &node.children {
            let step = Step { name: name.clone(), wrapper: false, prefixed: shared.contains(&*name.local) };
            let child_steps = Self::with_step(steps, step);
            if steps.is_empty() && name.is_gml_named("boundedBy") {
                self.bounded_by(child, &child_steps, out);
                continue;
            }
            self.element(child, &child_steps, anchor, node.instances, out);
        }
    }

    /// The columns of one element (the last of `steps`) and everything below it.
    fn element(
        &self,
        node: &ElementNode,
        steps: &[Step],
        anchor: Option<usize>,
        parent_instances: u64,
        out: &mut Vec<Col>,
    ) {
        let plan = self.plan(steps, None);
        if plan.drop {
            return;
        }
        let mut reasons = plan.reasons.clone();
        if node.parents_with < parent_instances {
            reasons.push(format!("present in {} of {} parents", node.parents_with, parent_instances));
        }
        let repeated = node.max_occurs > 1;
        let list = plan.list.unwrap_or(repeated && self.options.structure.lists == ListRule::Infer);
        let anchor = if list {
            let at = node
                .first_multi
                .as_ref()
                .map(|location| format!(" (first at {location})"))
                .unwrap_or_default();
            let name = &steps.last().expect("an element step").name.local;
            reasons.push(format!("list anchored on {name}: max_occurs={}{at}", node.max_occurs));
            Some(steps.len() - 1)
        } else {
            if repeated {
                reasons.push(format!("repeated (max_occurs={}), but not a list: a repetition is a feature error", node.max_occurs));
            }
            anchor
        };
        let first = out.len();
        self.content(node, steps, anchor, &plan, out);
        if let Some(col) = out.get_mut(first) {
            col.reasons.splice(0..0, reasons);
        }
    }

    /// The columns of an element's content.
    fn content(&self, node: &ElementNode, steps: &[Step], anchor: Option<usize>, plan: &Plan, out: &mut Vec<Col>) {
        let string = self.string();
        let leaf = |data_type: DataType, reason: &str| {
            let mut col = Col::new(steps, anchor, data_type);
            col.reasons.push(reason.to_string());
            col
        };
        if plan.raw_xml {
            out.push(leaf(string, "raw XML (override)"));
            return;
        }
        if plan.map {
            out.push(leaf(map_type(&string), "map (override)"));
            return;
        }
        if let Some(stats) = &node.geometry {
            match &plan.data_type {
                Some(data_type) => out.push(leaf(data_type.clone(), "geometry, type given by override")),
                None => out.push(self.geometry_col(stats, steps, anchor)),
            }
            return;
        }
        if node.truncated {
            out.push(leaf(map_type(&string), "too deep or too many distinct child names: map of path → text"));
            return;
        }
        let name = &steps.last().expect("an element step").name;
        if name.is_gml_named("metaDataProperty") {
            out.push(leaf(string, "gml:metaDataProperty: raw XML"));
            return;
        }
        if self.options.structure.collapse_type_wrappers && node.is_type_wrapper() {
            self.wrapper(node, steps, anchor, out);
            return;
        }
        self.own_content(node, steps, anchor, plan, out);
    }

    /// A type wrapper: the property's attributes (`xlink:href`, `nilReason`),
    /// then the content of its child, `*` in the path. Several wrapper types
    /// are merged first.
    fn wrapper(&self, node: &ElementNode, steps: &[Step], anchor: Option<usize>, out: &mut Vec<Col>) {
        let attributes = self.attributes(node, steps, anchor);
        out.extend(attributes.cols);
        out.extend(attributes.constants.into_iter().map(|(_, _, col)| col));
        self.nil_reason(node, steps, anchor, out);

        let mut wrappers = node.children.iter();
        let (first_name, first) = wrappers.next().expect("a wrapper has a child");
        let mut merged = first.clone();
        let mut names = vec![first_name.local.to_string()];
        for (name, other) in wrappers {
            merged.merge(other.clone());
            names.push(name.local.to_string());
        }
        let step = Step { name: first_name.clone(), wrapper: true, prefixed: false };
        let wrapper_steps = Self::with_step(steps, step);
        let first = out.len();
        self.own_content(&merged, &wrapper_steps, anchor, &Plan::default(), out);
        if let Some(col) = out.get_mut(first) {
            let reason = match names.as_slice() {
                [one] => format!("type wrapper {one} collapsed"),
                many => format!("type wrappers {} merged and collapsed", many.join(", ")),
            };
            col.reasons.insert(0, reason);
        }
    }

    /// An element's own text and attributes, then its children.
    fn own_content(&self, node: &ElementNode, steps: &[Step], anchor: Option<usize>, plan: &Plan, out: &mut Vec<Col>) {
        let attributes = self.attributes(node, steps, anchor);
        // The element's own column, if it has one.
        let own = match node.shape() {
            Shape::TextOnly | Shape::TextAndAttributes => {
                let scalar = self.scalar(node.text.as_ref());
                let mut col = Col::new(steps, anchor, plan.data_type.clone().unwrap_or(scalar.data_type));
                col.metadata.extend(scalar.metadata);
                col.reasons = scalar.reasons;
                Some(col)
            }
            Shape::Mixed => match self.options.structure.mixed_content {
                MixedContent::RawXml => {
                    let mut col = Col::new(steps, anchor, self.string());
                    col.reasons.push("mixed content: raw XML".to_string());
                    Some(col)
                }
                MixedContent::TextOnly => {
                    let mut col = Col::new(steps, anchor, self.string());
                    col.metadata.insert(meta::CONTENT.to_string(), "text".to_string());
                    col.reasons.push("mixed content: text only".to_string());
                    Some(col)
                }
                MixedContent::Drop => None,
            },
            // An element that never had a value is a column only if nothing
            // else is: no attribute columns and no children.
            Shape::Empty if attributes.cols.is_empty() => {
                let mut col = Col::new(steps, anchor, plan.data_type.clone().unwrap_or_else(|| self.all_null()));
                col.reasons.push("never had a value".to_string());
                Some(col)
            }
            Shape::Empty | Shape::ElementsOnly | Shape::ByReferenceOnly | Shape::Geometry => None,
        };
        match own {
            Some(mut col) => {
                for (key, value, _) in attributes.constants {
                    col.reasons.push(format!("constant attribute moved to field metadata: {key} = {value:?}"));
                    col.metadata.insert(key, value);
                }
                out.push(col);
                out.extend(attributes.cols);
            }
            None => {
                out.extend(attributes.cols);
                out.extend(attributes.constants.into_iter().map(|(_, _, col)| col));
            }
        }
        self.nil_reason(node, steps, anchor, out);
        self.children(node, steps, anchor, out);
    }

    fn bounded_by(&self, node: &ElementNode, steps: &[Step], out: &mut Vec<Col>) {
        let Some(stats) = &node.geometry else {
            return;
        };
        match self.options.gml.bounded_by {
            BoundedBy::Drop => {}
            BoundedBy::Geometry => out.push(self.geometry_col(stats, steps, None)),
            BoundedBy::BoxStruct => {
                let dimension = dimension(stats);
                let field = BoxType::new(dimension, Arc::new(self.crs_metadata(stats))).to_field("boundedBy", true);
                let mut col = Col::new(steps, None, DataType::Null);
                col.kind = ColKind::Field(field);
                col.reasons.push("gml:boundedBy as a box (BoxStruct)".to_string());
                out.push(col);
            }
        }
    }

    // ---- scalars ------------------------------------------------------------

    fn scalar(&self, stats: Option<&ValueStats>) -> Scalar {
        let types = &self.options.types;
        let mut scalar = Scalar {
            data_type: self.string(),
            metadata: Vec::new(),
            reasons: Vec::new(),
        };
        let Some(stats) = stats.filter(|stats| stats.count > 0) else {
            scalar.data_type = self.all_null();
            scalar.reasons.push("never had a value".to_string());
            return scalar;
        };
        let lossy = types.lossless == Lossless::Lossy;
        let mut set = (stats.types(types.lossless) & types.enabled) | TypeSet::STRING;
        if let Some(sample) = self.sampled
            && stats.count < sample.min_typed_values && set != TypeSet::STRING {
                scalar.reasons.push(format!(
                    "only {} values in the sample (fewer than {}): kept as text",
                    stats.count, sample.min_typed_values
                ));
                set = TypeSet::STRING;
            }
        let temporal = stats.temporal.unwrap_or_default();
        let unit = types.timestamps.unit;
        let fraction_fits = temporal.max_fraction_digits <= unit_digits(unit);

        for candidate in [
            TypeSet::BOOL,
            TypeSet::INT,
            TypeSet::FLOAT,
            TypeSet::DATE,
            TypeSet::DATETIME,
            TypeSet::TIME,
        ] {
            if !set.contains(candidate) {
                continue;
            }
            let chosen = match candidate {
                TypeSet::BOOL => Some(DataType::Boolean),
                TypeSet::INT => Some(self.int_type(stats)),
                TypeSet::FLOAT => {
                    if let Some(shape) = stats.float_shape {
                        scalar.metadata.push((meta::MAX_SCALE.to_string(), shape.max_scale.to_string()));
                    }
                    Some(DataType::Float64)
                }
                TypeSet::DATE | TypeSet::TIME => {
                    let data_type = if candidate == TypeSet::DATE {
                        DataType::Date32
                    } else if matches!(unit, TimeUnit::Second | TimeUnit::Millisecond) {
                        DataType::Time32(unit)
                    } else {
                        DataType::Time64(unit)
                    };
                    if candidate == TypeSet::TIME && !fraction_fits && !lossy {
                        scalar.reasons.push("more fractional digits than the time unit holds".to_string());
                        None
                    } else {
                        match temporal.tz {
                            TzShape::Absent => Some(data_type),
                            TzShape::Fixed(offset) => {
                                scalar.metadata.push((meta::TZ_OFFSET.to_string(), format_offset(offset)));
                                Some(data_type)
                            }
                            _ if lossy => Some(data_type),
                            TzShape::Mixed => {
                                scalar.reasons.push("different time zones can't share a column: text".to_string());
                                None
                            }
                            TzShape::Inconsistent => {
                                scalar.reasons.push("some values have a time zone, some don't: text".to_string());
                                None
                            }
                        }
                    }
                }
                TypeSet::DATETIME => {
                    if !fraction_fits && !lossy {
                        scalar.reasons.push("more fractional digits than the timestamp unit holds".to_string());
                        None
                    } else {
                        match temporal.tz {
                            TzShape::Absent => Some(DataType::Timestamp(unit, None)),
                            TzShape::Fixed(offset) => {
                                scalar.metadata.push((meta::TZ_OFFSET.to_string(), format_offset(offset)));
                                Some(DataType::Timestamp(unit, Some("UTC".into())))
                            }
                            TzShape::Mixed => {
                                scalar.reasons.push(
                                    "time-zone offsets differ: normalized to UTC, the offsets are not kept".to_string(),
                                );
                                Some(DataType::Timestamp(unit, Some("UTC".into())))
                            }
                            TzShape::Inconsistent => {
                                scalar.reasons.push("some values have a time zone, some don't: text".to_string());
                                None
                            }
                        }
                    }
                }
                _ => None,
            };
            if let Some(data_type) = chosen {
                scalar.reasons.insert(0, format!("{} values, candidates {}", stats.count, type_names(set)));
                scalar.data_type = data_type;
                return scalar;
            }
        }
        scalar.reasons.insert(0, format!("{} values, candidates {}", stats.count, type_names(set)));
        if let Some(example) = stats.distinct.values.first()
            && set == TypeSet::STRING && stats.types(Lossless::Lossy) != TypeSet::STRING {
                scalar.reasons.push(format!("typed values rejected (e.g. {example:?})"));
            }
        scalar
    }

    fn int_type(&self, stats: &ValueStats) -> DataType {
        if self.options.types.integers == IntWidth::Int64 || self.sampled.is_some() {
            return DataType::Int64;
        }
        let (lo, hi) = stats.int_range.unwrap_or((i64::MIN, i64::MAX));
        if lo >= i8::MIN as i64 && hi <= i8::MAX as i64 {
            DataType::Int8
        } else if lo >= i16::MIN as i64 && hi <= i16::MAX as i64 {
            DataType::Int16
        } else if lo >= i32::MIN as i64 && hi <= i32::MAX as i64 {
            DataType::Int32
        } else {
            DataType::Int64
        }
    }

    // ---- geometry -----------------------------------------------------------

    /// GeoArrow CRS metadata: PROJJSON for EPSG codes and compounds of them,
    /// else `authority:code`, else the srsName as an opaque string.
    fn crs_metadata(&self, stats: &GeometryStats) -> Metadata {
        let srs = self.options.geometry.crs_override.as_deref().or_else(|| stats.main_srs());
        let Some(srs) = srs else {
            return Metadata::default();
        };
        let crs = match SrsName::parse(srs).crs {
            Some(crs) => match crs.projjson() {
                Some(value) => Crs::from_projjson(value),
                None => Crs::from_authority_code(crs.authority_code()),
            },
            None => Crs::from_unknown_crs_type(srs.to_string()),
        };
        Metadata::new(crs, None)
    }

    /// A geometry property's column: a native GeoArrow type or WKB, or a list
    /// of WKB below a repeated element (`docs/geometry.md`, "Column encoding").
    fn geometry_col(&self, stats: &GeometryStats, steps: &[Step], anchor: Option<usize>) -> Col {
        let geometry = &self.options.geometry;
        let mut reasons = vec![format!(
            "geometry: {} values, kinds={{{}}}, dims={:?}",
            stats.count,
            stats.kinds.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>().join(", "),
            stats.dims,
        )];
        let linearize = matches!(geometry.curves, CurveMode::Linearize(_));
        let native = if anchor.is_some() {
            reasons.push("below a repeated element: a list of WKB (geometry[])".to_string());
            None
        } else {
            match self.options.geometry_encoding {
                GeomEncoding::Wkb => {
                    reasons.push("encoding Wkb".to_string());
                    None
                }
                GeomEncoding::Auto if self.sampled.is_some() => {
                    reasons.push("encoding Auto, sampled: WKB (later features may have other kinds)".to_string());
                    None
                }
                GeomEncoding::Auto if stats.has_curves && !linearize => {
                    reasons.push("encoding Auto: curves → WKB".to_string());
                    None
                }
                GeomEncoding::Auto if stats.has_unsupported => {
                    reasons.push("unsupported geometry kinds → WKB".to_string());
                    None
                }
                GeomEncoding::Auto if stats.kinds.is_empty() => {
                    reasons.push("no geometry kind seen → WKB".to_string());
                    None
                }
                GeomEncoding::Auto => match native_kind(&stats.kinds) {
                    Some(kind) => {
                        reasons.push(format!("encoding Auto → native {}", kind.name()));
                        Some(kind)
                    }
                    None => {
                        reasons.push("encoding Auto: no native type holds every kind → WKB".to_string());
                        None
                    }
                },
            }
        };

        let metadata = Arc::new(self.crs_metadata(stats));
        let dimension = dimension(stats);
        let coord_type = CoordType::Separated;
        let name = "geometry";
        let field = match native {
            None => Field::new(name, DataType::Binary, true).with_extension_type(WkbType::new(metadata)),
            Some(NativeKind::Point) => PointType::new(dimension, metadata).with_coord_type(coord_type).to_field(name, true),
            Some(NativeKind::LineString) => LineStringType::new(dimension, metadata).with_coord_type(coord_type).to_field(name, true),
            Some(NativeKind::Polygon) => PolygonType::new(dimension, metadata).with_coord_type(coord_type).to_field(name, true),
            Some(NativeKind::MultiPoint) => MultiPointType::new(dimension, metadata).with_coord_type(coord_type).to_field(name, true),
            Some(NativeKind::MultiLineString) => {
                MultiLineStringType::new(dimension, metadata).with_coord_type(coord_type).to_field(name, true)
            }
            Some(NativeKind::MultiPolygon) => {
                MultiPolygonType::new(dimension, metadata).with_coord_type(coord_type).to_field(name, true)
            }
        };

        let mut col = Col::new(steps, anchor, DataType::Null);
        col.kind = ColKind::Field(field);
        if let Some(srs) = geometry.crs_override.as_deref().or_else(|| stats.main_srs()) {
            col.metadata.insert(meta::SRS_NAME.to_string(), srs.to_string());
            let spellings = stats.srs.len();
            if spellings > 1 {
                reasons.push(format!("{spellings} srsNames; column CRS from {srs:?}"));
            }
        }
        let column = steps.last().map_or("", |step| &*step.name.local);
        let (swapped, decision) = self.axis(column, stats);
        col.metadata.insert(meta::AXIS_SWAPPED.to_string(), swapped);
        col.metadata.insert(meta::AXIS_DECISION.to_string(), decision.clone());
        reasons.push(format!("axis: {decision}"));
        col.reasons = reasons;
        col
    }

    /// Apply the axis-order options to each decision key of the column.
    fn axis(&self, column: &str, stats: &GeometryStats) -> (String, String) {
        let mut swaps = BTreeSet::new();
        let mut reasons = Vec::new();
        for (key, evidence) in &stats.axis_evidence {
            let axis_key = AxisKey {
                source: SourceId(key.source),
                srs_name: key.srs_name.clone(),
                dialect: key.dialect,
            };
            let context = self.axis_context(key.source);
            let decision = xeibe_geom::axis::decide(
                &axis_key,
                Some(&self.layer.local),
                Some(column),
                evidence,
                &context,
                &self.options.geometry.axis,
            );
            swaps.insert(decision.swap);
            let srs = key.srs_name.as_deref().unwrap_or("no srsName");
            let mut reason = format!("{srs}: {}", decision.reason);
            if !decision.conflicts.is_empty() {
                reason.push_str(&format!(" (conflicts: {})", decision.conflicts.join(", ")));
            }
            reasons.push(reason);
        }
        let swapped = match (swaps.contains(&true), swaps.contains(&false)) {
            (true, true) => "mixed",
            (true, false) => "true",
            _ => "false",
        };
        let decision = if reasons.is_empty() { "no coordinates seen".to_string() } else { reasons.join("; ") };
        (swapped.to_string(), decision)
    }

    fn axis_context(&self, source: u32) -> xeibe_geom::axis::AxisContext {
        let Some(context) = self.observation.source_context.get(source as usize) else {
            return xeibe_geom::axis::AxisContext::default();
        };
        xeibe_geom::axis::AxisContext {
            fme_produced: context.fme_produced,
            producer: context.producer.clone(),
            wfs_version: context.wfs_version.clone(),
            requested_srs: context.requested_srs.as_deref().map(SrsName::parse),
            requested_bbox: context.requested_bbox,
            source: None,
        }
    }
}

/// Local names that more than one of `names` has: those keep their prefix.
fn shared_locals<'n>(names: impl Iterator<Item = &'n QName>) -> BTreeSet<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for name in names {
        *counts.entry(&name.local).or_default() += 1;
    }
    counts.into_iter().filter(|(_, count)| *count > 1).map(|(local, _)| local.to_string()).collect()
}

fn is_href(name: &QName) -> bool {
    name.ns.as_deref() == Some(ns::XLINK) && &*name.local == "href"
}

/// `xyz` if any geometry had three dimensions (2D values then get a NaN Z).
fn dimension(stats: &GeometryStats) -> Dimension {
    if stats.dims.iter().any(|&d| d >= 3) { Dimension::XYZ } else { Dimension::XY }
}

/// `Map(Utf8View → Utf8View)`, with Arrow's required non-null entries and keys.
pub fn map_type(string: &DataType) -> DataType {
    let entries = Field::new(
        "entries",
        DataType::Struct(Fields::from(vec![
            Field::new("key", string.clone(), false),
            Field::new("value", string.clone(), true),
        ])),
        false,
    );
    DataType::Map(Arc::new(entries), false)
}

fn unit_digits(unit: TimeUnit) -> u8 {
    match unit {
        TimeUnit::Second => 0,
        TimeUnit::Millisecond => 3,
        TimeUnit::Microsecond => 6,
        TimeUnit::Nanosecond => 9,
    }
}

/// `+02:00`, `-05:30`, `+00:00`.
fn format_offset(minutes: i16) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let minutes = minutes.unsigned_abs();
    format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

fn type_names(set: TypeSet) -> String {
    let names: Vec<&str> = set.iter_names().map(|(name, _)| name).collect();
    format!("{{{}}}", names.join(", "))
}

/// Native GeoArrow geometry types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeKind {
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
}

impl NativeKind {
    fn name(self) -> &'static str {
        match self {
            NativeKind::Point => "point",
            NativeKind::LineString => "linestring",
            NativeKind::Polygon => "polygon",
            NativeKind::MultiPoint => "multipoint",
            NativeKind::MultiLineString => "multilinestring",
            NativeKind::MultiPolygon => "multipolygon",
        }
    }
}

/// The native type that holds every kind seen (curves count as linear), or
/// `None` if they need `geoarrow.geometry`. A `Surface` or `CompositeSurface`
/// may have several patches, so it needs a multipolygon.
fn native_kind(kinds: &BTreeSet<GeomKind>) -> Option<NativeKind> {
    #[derive(PartialEq)]
    enum Family {
        Point,
        Line,
        Area,
    }
    let mut family = None;
    let mut multi = false;
    for kind in kinds {
        let (f, m) = match kind {
            GeomKind::Point => (Family::Point, false),
            GeomKind::MultiPoint => (Family::Point, true),
            GeomKind::LineString
            | GeomKind::LinearRing
            | GeomKind::Curve
            | GeomKind::OrientableCurve
            | GeomKind::CompositeCurve
            | GeomKind::Ring => (Family::Line, false),
            GeomKind::MultiLineString | GeomKind::MultiCurve => (Family::Line, true),
            GeomKind::Polygon | GeomKind::Envelope | GeomKind::Box => (Family::Area, false),
            GeomKind::Surface
            | GeomKind::OrientableSurface
            | GeomKind::CompositeSurface
            | GeomKind::MultiPolygon
            | GeomKind::MultiSurface => (Family::Area, true),
            GeomKind::MultiGeometry | GeomKind::Unsupported => return None,
        };
        match &family {
            Some(existing) if *existing != f => return None,
            _ => family = Some(f),
        }
        multi |= m;
    }
    Some(match (family?, multi) {
        (Family::Point, false) => NativeKind::Point,
        (Family::Point, true) => NativeKind::MultiPoint,
        (Family::Line, false) => NativeKind::LineString,
        (Family::Line, true) => NativeKind::MultiLineString,
        (Family::Area, false) => NativeKind::Polygon,
        (Family::Area, true) => NativeKind::MultiPolygon,
    })
}

/// Namespace prefixes for `gml:path` and prefixed column names: the prefix
/// the scan saw first, well-known ones, or generated `ns1`, `ns2`, ….
struct Prefixes {
    map: IndexMap<Arc<str>, String>,
}

impl Prefixes {
    fn for_layer(observation: &DatasetObservation, root: &ElementNode) -> Self {
        let mut uris = IndexMap::new();
        collect_namespaces(root, &mut uris);
        let mut map: IndexMap<Arc<str>, String> = IndexMap::new();
        let mut taken: BTreeSet<String> = BTreeSet::new();
        let mut generated = 0;
        for uri in uris.into_keys() {
            let known = match &*uri {
                ns::GML | ns::GML_32 => Some("gml"),
                ns::XLINK => Some("xlink"),
                ns::XSI => Some("xsi"),
                _ => None,
            };
            let mut prefix = observation
                .prefixes
                .get(&uri)
                .cloned()
                .or_else(|| known.map(str::to_string));
            if prefix.as_ref().is_none_or(|p| taken.contains(p)) {
                prefix = loop {
                    generated += 1;
                    let candidate = format!("ns{generated}");
                    if !taken.contains(&candidate) {
                        break Some(candidate);
                    }
                };
            }
            let prefix = prefix.expect("a prefix was chosen");
            taken.insert(prefix.clone());
            map.insert(uri, prefix);
        }
        Prefixes { map }
    }

    fn prefix(&self, uri: &str) -> &str {
        self.map.get(uri).map_or("ns", String::as_str)
    }

    fn prefixed(&self, name: &QName) -> String {
        match &name.ns {
            Some(uri) => format!("{}:{}", self.prefix(uri), name.local),
            None => name.local.to_string(),
        }
    }
}

fn collect_namespaces(node: &ElementNode, uris: &mut IndexMap<Arc<str>, ()>) {
    for name in node.attributes.keys().chain(node.children.keys()) {
        if let Some(uri) = &name.ns {
            uris.entry(uri.clone()).or_insert(());
        }
    }
    for child in node.children.values() {
        collect_namespaces(child, uris);
    }
}

