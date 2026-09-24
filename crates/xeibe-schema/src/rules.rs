//! Rule engine: one recursive walk over a layer's tree → Arrow schema.
//!
//! The walk first builds a tree of [`Col`]s (name, source, type, list flag,
//! metadata, reasons), then emits it as Arrow fields and routes, flattening
//! single structs when the nesting mode asks for it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use arrow_schema::{DataType, Field, Fields, Schema, TimeUnit};
use geoarrow_schema::{
    BoxType, CoordType, Crs, Dimension, GeometryType, LineStringType, Metadata,
    MultiLineStringType, MultiPointType, MultiPolygonType, PointType, PolygonType, WkbType,
};
use indexmap::IndexMap;
use xeibe_core::{QName, SourceId, ns};
use xeibe_geom::options::{CurveMode, DimMode, GeomEncoding};
use xeibe_geom::{AxisKey, CrsRef, GeomKind, SrsName};

use crate::geometry_stats::GeometryStats;
use crate::node::{Shape, is_gml_id};
use crate::options::{
    AllNull, AttrSelect, BoundedBy, ConstantAttrs, FieldOverride, IdMode, IntWidth, ListRule,
    Lossless, MixedContent, Nesting, NsMode, OnRepeat, SimpleContent, XlinkMode, string_type,
};
use crate::value::{TzShape, ValueStats};
use crate::{DatasetObservation, ElementNode, InferenceOptions, SampleOptions, TypeSet};

/// Metadata keys written on fields (see `docs/type-mapping.md`).
pub mod meta {
    pub const PATH: &str = "gml:path";
    pub const NS: &str = "gml:ns";
    pub const MAX_SCALE: &str = "gml:max_scale";
    pub const ATTR_PREFIX: &str = "gml:attr:";
    pub const TZ_OFFSET: &str = "gml:tz_offset";
    pub const SRS_NAME: &str = "gml:srs_name";
    pub const AXIS_SWAPPED: &str = "gml:axis_swapped";
    pub const AXIS_DECISION: &str = "gml:axis_decision";
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
    /// Source path → field index path (for routing values to builders).
    pub routes: Vec<FieldRoute>,
    /// One entry per field, for `--explain`. Empty for a given schema.
    pub decisions: Vec<FieldDecision>,
    /// The routes of a given schema were matched by name, not observed: a
    /// name without a namespace (`ns: None`) matches an element or attribute
    /// of that local name in any namespace, and a type wrapper (one
    /// UpperCamel child) that has no field of its own is looked through.
    /// `false` for inferred schemas, whose routes are exact.
    pub match_by_name: bool,
}

/// Where one Arrow field (at any nesting level) gets its values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRoute {
    /// Element path from the feature element (which is not included). Empty
    /// for the feature's own attributes, and for `_overflow`.
    pub source_path: Vec<QName>,
    /// The value is this attribute of the last element of `source_path`.
    pub attribute: Option<QName>,
    pub value: RouteValue,
    /// Index path into nested struct fields. A `List` level adds no index:
    /// the children of a `List(Struct(…))` field `i` are `[i, j]`.
    pub field_path: Vec<usize>,
}

/// What a route takes from its element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteValue {
    /// The element's text (or the attribute's value), parsed as the field's type.
    /// With a given schema (`match_by_name`), an element that has no text but
    /// an `xlink:href` gives its href.
    Text,
    /// The element's `xlink:href`; a leading `#` is stripped when
    /// `GmlOptions::strip_local_href_hash` is set.
    Href,
    /// A `Struct` (or `List(Struct)`): its children have routes of their own.
    Struct,
    /// A geometry property: the geometry element inside it.
    Geometry,
    /// The element's subtree as raw XML (mixed content, `AsRawXml`, lists
    /// and subtrees that a `Flatten` nesting can't hold).
    RawXml,
    /// The element's text with the markup removed (`MixedContent::TextOnly`).
    InnerText,
    /// The subtree as `Map(path → text)` (width/depth limits, `AsMap`).
    Map,
    /// The time-zone offset in minutes of the element's timestamp
    /// (`<name>.@offset_min`, mixed offsets).
    OffsetMinutes,
    /// The `gml:Envelope` inside the element, as a box.
    BoundingBox,
    /// `_overflow`: everything no other route takes.
    Overflow,
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
    let mut cols = engine.attribute_cols(root, &[], true).0;
    cols.extend(engine.children_cols(root, &[], 0));

    let mut emitter = Emitter {
        nesting: options.structure.nesting,
        separator: &options.naming.flatten_separator,
        routes: Vec::new(),
        decisions: Vec::new(),
    };
    let mut fields = Vec::new();
    emitter.emit(cols, "", &[], &mut fields);

    let mut metadata = HashMap::new();
    if !observation.gml_versions.is_empty() {
        let versions: Vec<&str> = observation.gml_versions.iter().map(version_name).collect();
        metadata.insert(meta::VERSIONS.to_string(), versions.join(","));
    }
    Ok(LayerSchema {
        layer: layer.clone(),
        schema: Schema::new_with_metadata(fields, metadata),
        routes: emitter.routes,
        decisions: emitter.decisions,
        match_by_name: false,
    })
}

fn version_name(version: &xeibe_core::GmlVersion) -> &'static str {
    match version {
        xeibe_core::GmlVersion::V2 => "2",
        xeibe_core::GmlVersion::V3_0 => "3.0",
        xeibe_core::GmlVersion::V3_1 => "3.1",
        xeibe_core::GmlVersion::V3_2 => "3.2",
    }
}

/// A column before emission.
struct Col {
    name: String,
    path: Vec<QName>,
    attribute: Option<QName>,
    value: RouteValue,
    kind: ColKind,
    list: bool,
    metadata: HashMap<String, String>,
    reasons: Vec<String>,
}

enum ColKind {
    Leaf(DataType),
    /// A ready-made field (GeoArrow extension types); its name is replaced.
    Field(Field),
    Struct(Vec<Col>),
}

impl Col {
    fn leaf(name: String, path: &[QName], value: RouteValue, data_type: DataType) -> Self {
        Col {
            name,
            path: path.to_vec(),
            attribute: None,
            value,
            kind: ColKind::Leaf(data_type),
            list: false,
            metadata: HashMap::new(),
            reasons: Vec::new(),
        }
    }
}

/// An override plan for one path: all matching overrides, most specific last.
#[derive(Default)]
struct Plan {
    drop: bool,
    rename: Option<String>,
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
    /// Mixed time-zone offsets: add `<name>.@offset_min`.
    offset_column: bool,
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

    // ---- overrides ----------------------------------------------------------

    fn plan(&self, path: &[QName], attribute: Option<&QName>) -> Plan {
        let mut names: Vec<String> = path.iter().map(QName::to_clark).collect();
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
                FieldOverride::Rename(name) => plan.rename = Some(name.clone()),
                FieldOverride::Drop => plan.drop = true,
                FieldOverride::AsRawXml => plan.raw_xml = true,
                FieldOverride::AsMap => plan.map = true,
                FieldOverride::List => plan.list = Some(true),
                FieldOverride::Scalar => plan.list = Some(false),
            }
        }
        plan
    }

    // ---- naming -------------------------------------------------------------

    /// Column names of the element children, with `StripUnlessCollision`
    /// keeping the prefix of siblings that share a local name.
    fn child_names(&self, node: &ElementNode) -> Vec<String> {
        let names: Vec<&QName> = node.children.keys().collect();
        self.sibling_names(&names)
    }

    fn sibling_names(&self, names: &[&QName]) -> Vec<String> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for name in names {
            *counts.entry(&name.local).or_default() += 1;
        }
        names
            .iter()
            .map(|name| match self.options.naming.namespaces {
                NsMode::Strip => name.local.to_string(),
                NsMode::StripUnlessCollision if counts[&*name.local] <= 1 => name.local.to_string(),
                NsMode::StripUnlessCollision | NsMode::Prefix => self.prefixes.prefixed(name),
                NsMode::Clark => name.to_clark(),
            })
            .collect()
    }

    /// `gml:path` and `gml:ns` for a column.
    fn path_metadata(&self, path: &[QName], attribute: Option<&QName>) -> HashMap<String, String> {
        let mut steps: Vec<String> = path.iter().map(|name| self.prefixes.prefixed(name)).collect();
        if let Some(attribute) = attribute {
            steps.push(format!("@{}", self.prefixes.prefixed(attribute)));
        }
        let mut used = serde_json::Map::new();
        for name in path.iter().chain(attribute) {
            if let Some(uri) = &name.ns {
                used.insert(self.prefixes.prefix(uri).to_string(), uri.to_string().into());
            }
        }
        HashMap::from([
            (meta::PATH.to_string(), steps.join("/")),
            (meta::NS.to_string(), serde_json::Value::Object(used).to_string()),
        ])
    }

    fn separator(&self) -> &str {
        &self.options.naming.flatten_separator
    }

    // ---- attributes ---------------------------------------------------------

    fn keep_attribute(&self, name: &QName) -> bool {
        let gml = &self.options.gml;
        if name.ns.as_deref() == Some(ns::XSI) {
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

    /// Columns for the attributes of `node` (at `path`), and the metadata
    /// entries of the constant attributes moved out of the data.
    fn attribute_cols(
        &self,
        node: &ElementNode,
        path: &[QName],
        include_href: bool,
    ) -> (Vec<Col>, Vec<(String, String)>) {
        let kept: Vec<(&QName, &ValueStats)> = node
            .attributes
            .iter()
            .filter(|(name, _)| self.keep_attribute(name))
            .filter(|(name, _)| include_href || !is_href(name))
            .collect();
        let names: Vec<&QName> = kept.iter().map(|(name, _)| *name).collect();
        let column_names = self.sibling_names(&names);

        let mut cols = Vec::new();
        let mut constants = Vec::new();
        for ((name, stats), column_name) in kept.into_iter().zip(column_names) {
            let plan = self.plan(path, Some(name));
            if plan.drop {
                continue;
            }
            let constant = self.options.structure.constant_attrs == ConstantAttrs::ToFieldMetadata
                && !is_gml_id(name)
                && !is_href(name)
                && stats.count == node.instances
                && plan.data_type.is_none();
            if let Some(value) = stats.distinct.single().filter(|_| constant) {
                constants.push((format!("{}{column_name}", meta::ATTR_PREFIX), value.to_string()));
                continue;
            }
            let column = plan
                .rename
                .clone()
                .unwrap_or_else(|| format!("{}{column_name}", self.options.naming.attribute_prefix));
            let scalar = self.scalar(Some(stats));
            let data_type = plan.data_type.clone().unwrap_or(scalar.data_type);
            let mut col = Col::leaf(column, path, RouteValue::Text, data_type);
            col.attribute = Some(name.clone());
            col.metadata = self.path_metadata(path, Some(name));
            col.metadata.extend(scalar.metadata);
            col.reasons = plan.reasons;
            col.reasons.extend(scalar.reasons);
            cols.push(col);
        }
        (cols, constants)
    }

    // ---- elements -----------------------------------------------------------

    /// Columns for the element children of `node` (at `path`, `depth`).
    fn children_cols(&self, node: &ElementNode, path: &[QName], depth: usize) -> Vec<Col> {
        let mut cols = Vec::new();
        for ((name, child), column_name) in node.children.iter().zip(self.child_names(node)) {
            let mut child_path = path.to_vec();
            child_path.push(name.clone());
            if depth == 0 && name.is_gml_named("boundedBy") {
                cols.extend(self.bounded_by(column_name, child, &child_path));
                continue;
            }
            cols.extend(self.element_cols(name, column_name, child, child_path, node.instances));
        }
        cols
    }

    fn element_cols(
        &self,
        name: &QName,
        column_name: String,
        node: &ElementNode,
        path: Vec<QName>,
        parent_instances: u64,
    ) -> Vec<Col> {
        let plan = self.plan(&path, None);
        if plan.drop {
            return Vec::new();
        }
        let column_name = plan.rename.clone().unwrap_or(column_name);
        let mut reasons = plan.reasons.clone();
        if node.parents_with < parent_instances {
            reasons.push(format!(
                "present in {} of {} parents",
                node.parents_with, parent_instances
            ));
        }

        let repeated = node.max_occurs > 1;
        let lists = self.options.structure.lists;
        let list = plan.list.unwrap_or(repeated && lists == ListRule::Infer);
        if repeated && list {
            let at = node
                .first_multi
                .as_ref()
                .map(|location| format!(" (first at {location})"))
                .unwrap_or_default();
            reasons.push(format!("list: max_occurs={}{at}", node.max_occurs));
        } else if repeated {
            reasons.push(match lists {
                ListRule::Never(OnRepeat::Error) => {
                    format!("repeated (max_occurs={}); a repetition is an error", node.max_occurs)
                }
                _ => format!("repeated (max_occurs={}); the first value is kept", node.max_occurs),
            });
        }

        let mut cols = self.content(name, &column_name, node, &path, list, &plan);
        let Some(main) = cols.first_mut() else {
            return cols;
        };
        // `Flatten` keeps lists of scalars only; a list of structs is raw XML.
        if list
            && matches!(self.options.structure.nesting, Nesting::Flatten { .. })
            && matches!(main.kind, ColKind::Struct(_))
        {
            *main = self.raw_xml_col(&column_name, &path, "list of structs under Flatten nesting: raw XML");
            cols.truncate(1);
        }
        for col in &mut cols {
            col.list = list;
        }
        cols[0].reasons.splice(0..0, reasons);
        cols
    }

    fn raw_xml_col(&self, column_name: &str, path: &[QName], reason: &str) -> Col {
        let mut col = Col::leaf(column_name.to_string(), path, RouteValue::RawXml, self.string());
        col.metadata = self.path_metadata(path, None);
        col.reasons.push(reason.to_string());
        col
    }

    /// The column(s) of one element: the main column first, then siblings
    /// (`.@offset_min`, `.@nilReason`, split attributes).
    fn content(
        &self,
        name: &QName,
        column_name: &str,
        node: &ElementNode,
        path: &[QName],
        list: bool,
        plan: &Plan,
    ) -> Vec<Col> {
        let string = self.string();
        let leaf = |value: RouteValue, data_type: DataType, reason: &str| {
            let mut col = Col::leaf(column_name.to_string(), path, value, data_type);
            col.metadata = self.path_metadata(path, None);
            if !reason.is_empty() {
                col.reasons.push(reason.to_string());
            }
            col
        };

        if plan.raw_xml {
            return vec![leaf(RouteValue::RawXml, string, "raw XML (override)")];
        }
        if plan.map {
            return vec![leaf(RouteValue::Map, map_type(&string), "map (override)")];
        }
        if let Some(stats) = &node.geometry {
            if let Some(data_type) = &plan.data_type {
                return vec![leaf(RouteValue::Geometry, data_type.clone(), "geometry, type given by override")];
            }
            return vec![self.geometry_col(column_name, stats, path)];
        }
        if node.truncated {
            return vec![leaf(
                RouteValue::Map,
                map_type(&string),
                "too deep or too many distinct child names: map of path → text",
            )];
        }
        if let Nesting::Flatten { max_depth } = self.options.structure.nesting {
            if path.len() > max_depth as usize && !node.children.is_empty() {
                return vec![leaf(RouteValue::RawXml, string, "beyond the Flatten depth: raw XML")];
            }
        }
        if name.is_gml_named("metaDataProperty") {
            return vec![leaf(RouteValue::RawXml, string, "gml:metaDataProperty: raw XML")];
        }
        if self.options.structure.collapse_type_wrappers && node.is_type_wrapper() {
            let (wrapper_name, wrapper) = node.children.get_index(0).expect("a wrapper has one child");
            let mut wrapper_path = path.to_vec();
            wrapper_path.push(wrapper_name.clone());
            let mut col = self.struct_col(column_name, wrapper, &wrapper_path, path);
            col.reasons.insert(0, format!("type wrapper {} collapsed", wrapper_name.local));
            return vec![col];
        }

        let mut cols = match node.shape() {
            Shape::TextOnly => self.text_cols(column_name, node, path, plan, Vec::new()),
            Shape::TextAndAttributes => self.simple_content_cols(column_name, node, path, list, plan),
            Shape::ElementsOnly => vec![self.struct_col(column_name, node, path, path)],
            Shape::ByReferenceOnly => {
                if self.options.gml.xlink == XlinkMode::Drop {
                    return Vec::new();
                }
                let (attributes, _) = self.attribute_cols(node, path, true);
                if attributes.len() > 1 {
                    vec![self.struct_col(column_name, node, path, path)]
                } else {
                    let strip = if self.options.gml.strip_local_href_hash { ", '#' stripped" } else { "" };
                    let reason = format!("by-reference only (xlink:href){strip}");
                    vec![leaf(RouteValue::Href, plan.data_type.clone().unwrap_or(string), &reason)]
                }
            }
            Shape::Mixed => match self.options.structure.mixed_content {
                MixedContent::RawXml => vec![leaf(RouteValue::RawXml, string, "mixed content: raw XML")],
                MixedContent::TextOnly => {
                    vec![leaf(RouteValue::InnerText, string, "mixed content: text only")]
                }
                MixedContent::Drop => return Vec::new(),
            },
            Shape::Empty => {
                let (attributes, constants) = self.attribute_cols(node, path, true);
                if attributes.is_empty() {
                    let mut col = leaf(
                        RouteValue::Text,
                        plan.data_type.clone().unwrap_or_else(|| self.all_null()),
                        "never had a value",
                    );
                    col.metadata.extend(constants);
                    vec![col]
                } else {
                    vec![self.struct_col(column_name, node, path, path)]
                }
            }
            Shape::Geometry => unreachable!("handled above"),
        };

        if node.nil.count > 0 && !node.nil.reasons.is_empty() && self.options.gml.nil_reason {
            let nil_reason = QName::new(None, "nilReason");
            let mut col = Col::leaf(
                format!("{column_name}{}{}nilReason", self.separator(), self.options.naming.attribute_prefix),
                path,
                RouteValue::Text,
                self.string(),
            );
            col.metadata = self.path_metadata(path, Some(&nil_reason));
            col.attribute = Some(nil_reason);
            col.reasons.push(format!("nilReason of {} nil values", node.nil.count));
            cols.push(col);
        }
        cols
    }

    /// A scalar column from the node's text, plus an offset column if needed.
    fn text_cols(
        &self,
        column_name: &str,
        node: &ElementNode,
        path: &[QName],
        plan: &Plan,
        constants: Vec<(String, String)>,
    ) -> Vec<Col> {
        let scalar = self.scalar(node.text.as_ref());
        let data_type = plan.data_type.clone().unwrap_or(scalar.data_type);
        let mut col = Col::leaf(column_name.to_string(), path, RouteValue::Text, data_type);
        col.metadata = self.path_metadata(path, None);
        col.metadata.extend(scalar.metadata);
        col.metadata.extend(constants);
        col.reasons = scalar.reasons;
        let mut cols = vec![col];
        if scalar.offset_column && plan.data_type.is_none() {
            cols.push(self.offset_col(column_name, path));
        }
        cols
    }

    fn offset_col(&self, column_name: &str, path: &[QName]) -> Col {
        let mut col = Col::leaf(
            format!("{column_name}{}{}offset_min", self.separator(), self.options.naming.attribute_prefix),
            path,
            RouteValue::OffsetMinutes,
            DataType::Int16,
        );
        col.metadata = self.path_metadata(path, None);
        col.reasons.push("time-zone offsets differ: original offset in minutes".to_string());
        col
    }

    /// `<area uom="m2">1523.40</area>` by `simple_with_attrs`.
    fn simple_content_cols(
        &self,
        column_name: &str,
        node: &ElementNode,
        path: &[QName],
        list: bool,
        plan: &Plan,
    ) -> Vec<Col> {
        let (attributes, constants) = self.attribute_cols(node, path, true);
        if attributes.is_empty() {
            let mut cols = self.text_cols(column_name, node, path, plan, constants);
            cols[0].reasons.push("attributes are constant: moved to field metadata".to_string());
            return cols;
        }
        match self.options.structure.simple_with_attrs {
            SimpleContent::ValueOnly => {
                let mut cols = self.text_cols(column_name, node, path, plan, constants);
                cols[0].reasons.push("attributes dropped (ValueOnly)".to_string());
                cols
            }
            SimpleContent::Split if !list => {
                let mut cols = self.text_cols(column_name, node, path, plan, constants);
                for mut attribute in attributes {
                    attribute.name = format!("{column_name}{}{}", self.separator(), attribute.name);
                    cols.push(attribute);
                }
                cols
            }
            SimpleContent::Struct | SimpleContent::Split => {
                let text = self.text_cols(&self.options.naming.text_field, node, path, plan, Vec::new());
                let mut children = text;
                children.extend(attributes);
                let mut col = Col {
                    name: column_name.to_string(),
                    path: path.to_vec(),
                    attribute: None,
                    value: RouteValue::Struct,
                    kind: ColKind::Struct(children),
                    list: false,
                    metadata: self.path_metadata(path, None),
                    reasons: vec!["text and attributes".to_string()],
                };
                col.metadata.extend(constants);
                vec![col]
            }
        }
    }

    /// A struct of `node`'s attributes, text and children. `node_path` is the
    /// path of `node` (inside a collapsed wrapper), `column_path` the path the
    /// column is routed from (the property).
    fn struct_col(
        &self,
        column_name: &str,
        node: &ElementNode,
        node_path: &[QName],
        column_path: &[QName],
    ) -> Col {
        let (mut children, constants) = self.attribute_cols(node, node_path, true);
        if node.text_count() > 0 {
            children.extend(self.text_cols(&self.options.naming.text_field, node, node_path, &Plan::default(), Vec::new()));
        }
        children.extend(self.children_cols(node, node_path, node_path.len()));
        let mut metadata = self.path_metadata(column_path, None);
        metadata.extend(constants);
        if children.is_empty() {
            let mut col = Col::leaf(column_name.to_string(), column_path, RouteValue::Text, self.all_null());
            col.metadata = metadata;
            col.reasons.push("every child was dropped".to_string());
            return col;
        }
        let mut reasons = Vec::new();
        if node.href_and_content > 0 {
            reasons.push("xlink:href and inline content: the content is used, the href kept".to_string());
        }
        if node.has_gml_id > 0 && node_path.len() == column_path.len() {
            reasons.push("nested object with its own gml:id".to_string());
        }
        Col {
            name: column_name.to_string(),
            path: column_path.to_vec(),
            attribute: None,
            value: RouteValue::Struct,
            kind: ColKind::Struct(children),
            list: false,
            metadata,
            reasons,
        }
    }

    fn bounded_by(&self, column_name: String, node: &ElementNode, path: &[QName]) -> Vec<Col> {
        let Some(stats) = &node.geometry else {
            return Vec::new();
        };
        match self.options.gml.bounded_by {
            BoundedBy::Drop => Vec::new(),
            BoundedBy::Geometry => vec![self.geometry_col(&column_name, stats, path)],
            BoundedBy::BoxStruct => {
                let dimension = self.dimension(stats);
                let field = BoxType::new(dimension, Arc::new(self.crs_metadata(stats))).to_field(&column_name, true);
                let mut col = Col::leaf(column_name, path, RouteValue::BoundingBox, DataType::Null);
                col.kind = ColKind::Field(field);
                col.metadata = self.path_metadata(path, None);
                col.reasons.push("gml:boundedBy as a box (BoxStruct)".to_string());
                vec![col]
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
            offset_column: false,
        };
        let Some(stats) = stats.filter(|stats| stats.count > 0) else {
            scalar.data_type = self.all_null();
            scalar.reasons.push("never had a value".to_string());
            return scalar;
        };
        let lossy = types.lossless == Lossless::Lossy;
        let mut set = (stats.types(types.lossless) & types.enabled) | TypeSet::STRING;
        if let Some(sample) = self.sampled {
            if stats.count < sample.min_typed_values && set != TypeSet::STRING {
                scalar.reasons.push(format!(
                    "only {} values in the sample (fewer than {}): kept as text",
                    stats.count, sample.min_typed_values
                ));
                set = TypeSet::STRING;
            }
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
                                scalar.offset_column = types.timestamps.keep_offset;
                                scalar.reasons.push("time-zone offsets differ: normalized to UTC".to_string());
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
        if let Some(example) = stats.distinct.values.first() {
            if set == TypeSet::STRING && stats.types(Lossless::Lossy) != TypeSet::STRING {
                scalar.reasons.push(format!("typed values rejected (e.g. {example:?})"));
            }
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

    fn dimension(&self, stats: &GeometryStats) -> Dimension {
        match self.options.geometry.dimension {
            DimMode::Force2D => Dimension::XY,
            DimMode::ForceZ => Dimension::XYZ,
            DimMode::Auto if stats.dims.iter().any(|&d| d >= 3) => Dimension::XYZ,
            DimMode::Auto => Dimension::XY,
        }
    }

    /// GeoArrow CRS metadata: PROJJSON for EPSG codes, else `authority:code`,
    /// else the srsName as an opaque string.
    fn crs_metadata(&self, stats: &GeometryStats) -> Metadata {
        let srs = self.options.geometry.crs_override.as_deref().or_else(|| stats.main_srs());
        let Some(srs) = srs else {
            return Metadata::default();
        };
        let crs = match SrsName::parse(srs).crs {
            Some(crs) => {
                let projjson = match &crs {
                    CrsRef::Code { authority, code } if authority == "EPSG" => code
                        .parse::<u32>()
                        .ok()
                        .and_then(xeibe_crs::projjson)
                        .and_then(|json| serde_json::from_str(json).ok()),
                    _ => None,
                };
                match projjson {
                    Some(value) => Crs::from_projjson(value),
                    None => Crs::from_authority_code(crs.authority_code()),
                }
            }
            None => Crs::from_unknown_crs_type(srs.to_string()),
        };
        Metadata::new(crs, None)
    }

    fn geometry_col(&self, column_name: &str, stats: &GeometryStats, path: &[QName]) -> Col {
        let geometry = &self.options.geometry;
        let mut reasons = vec![format!(
            "geometry: {} values, kinds={{{}}}, dims={:?}",
            stats.count,
            stats.kinds.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>().join(", "),
            stats.dims,
        )];
        let linearize = matches!(geometry.curves, CurveMode::Linearize(_));
        let native = native_kind(&stats.kinds);
        let encoding = match self.options.geometry_encoding {
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
            _ if stats.has_unsupported => {
                reasons.push("unsupported geometry kinds → WKB".to_string());
                None
            }
            _ if stats.kinds.is_empty() => {
                reasons.push("no geometry kind seen → WKB".to_string());
                None
            }
            _ => {
                let label = match native {
                    Some(kind) => format!("native {}", kind.name()),
                    None => "geoarrow.geometry (several kinds)".to_string(),
                };
                reasons.push(format!("encoding {:?} → {label}", self.options.geometry_encoding));
                Some(native)
            }
        };

        let metadata = Arc::new(self.crs_metadata(stats));
        let dimension = self.dimension(stats);
        let coord_type = if geometry.interleaved { CoordType::Interleaved } else { CoordType::Separated };
        let field = match encoding {
            None => Field::new(column_name, DataType::Binary, true).with_extension_type(WkbType::new(metadata)),
            Some(None) => GeometryType::new(metadata).with_coord_type(coord_type).to_field(column_name, true),
            Some(Some(kind)) => match kind {
                NativeKind::Point => PointType::new(dimension, metadata).with_coord_type(coord_type).to_field(column_name, true),
                NativeKind::LineString => LineStringType::new(dimension, metadata).with_coord_type(coord_type).to_field(column_name, true),
                NativeKind::Polygon => PolygonType::new(dimension, metadata).with_coord_type(coord_type).to_field(column_name, true),
                NativeKind::MultiPoint => MultiPointType::new(dimension, metadata).with_coord_type(coord_type).to_field(column_name, true),
                NativeKind::MultiLineString => MultiLineStringType::new(dimension, metadata).with_coord_type(coord_type).to_field(column_name, true),
                NativeKind::MultiPolygon => MultiPolygonType::new(dimension, metadata).with_coord_type(coord_type).to_field(column_name, true),
            },
        };

        let mut col = Col::leaf(column_name.to_string(), path, RouteValue::Geometry, DataType::Null);
        col.kind = ColKind::Field(field);
        col.metadata = self.path_metadata(path, None);
        if let Some(srs) = geometry.crs_override.as_deref().or_else(|| stats.main_srs()) {
            col.metadata.insert(meta::SRS_NAME.to_string(), srs.to_string());
            let spellings = stats.srs.len();
            if spellings > 1 {
                reasons.push(format!("{spellings} srsNames; column CRS from {srs:?}"));
            }
        }
        let (swapped, decision) = self.axis(column_name, stats);
        col.metadata.insert(meta::AXIS_SWAPPED.to_string(), swapped);
        col.metadata.insert(meta::AXIS_DECISION.to_string(), decision.clone());
        reasons.push(format!("axis: {decision}"));
        col.reasons = reasons;
        col
    }

    /// Apply the axis-order options to each decision key of the column.
    fn axis(&self, column_name: &str, stats: &GeometryStats) -> (String, String) {
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
                Some(column_name),
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

fn is_href(name: &QName) -> bool {
    name.ns.as_deref() == Some(ns::XLINK) && &*name.local == "href"
}

/// `Map(Utf8View → Utf8View)`, with Arrow's required non-null entries and keys.
pub(crate) fn map_type(string: &DataType) -> DataType {
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

/// Turns [`Col`]s into Arrow fields, routes and decisions.
struct Emitter<'a> {
    nesting: Nesting,
    separator: &'a str,
    routes: Vec<FieldRoute>,
    decisions: Vec<FieldDecision>,
}

impl Emitter<'_> {
    /// Emit `cols` into `fields`, the fields of the struct at `field_path`.
    /// `prefix` is prepended to names when single structs are flattened.
    fn emit(&mut self, cols: Vec<Col>, prefix: &str, field_path: &[usize], fields: &mut Vec<Field>) {
        for col in cols {
            let flatten = self.nesting != Nesting::Struct && !col.list;
            match col.kind {
                ColKind::Struct(children) if flatten => {
                    let prefix = format!("{prefix}{}{}", col.name, self.separator);
                    self.emit(children, &prefix, field_path, fields);
                }
                kind => {
                    let name = format!("{prefix}{}", col.name);
                    let mut path = field_path.to_vec();
                    path.push(fields.len());
                    self.routes.push(FieldRoute {
                        source_path: col.path,
                        attribute: col.attribute,
                        value: col.value,
                        field_path: path.clone(),
                    });
                    self.decisions.push(FieldDecision { field: name.clone(), reasons: col.reasons });
                    let item = match kind {
                        ColKind::Leaf(data_type) => Field::new(&name, data_type, true),
                        ColKind::Field(field) => field.with_name(&name),
                        ColKind::Struct(children) => {
                            let mut child_fields = Vec::new();
                            // Nested names are relative to the struct; decisions
                            // show them with the parent's name in front.
                            let before = self.decisions.len();
                            self.emit(children, "", &path, &mut child_fields);
                            for decision in &mut self.decisions[before..] {
                                decision.field = format!("{name}{}{}", self.separator, decision.field);
                            }
                            Field::new(&name, DataType::Struct(Fields::from(child_fields)), true)
                        }
                    };
                    let mut metadata = item.metadata().clone();
                    let field = if col.list {
                        let item = item.with_name("item");
                        metadata = col.metadata;
                        Field::new(&name, DataType::List(Arc::new(item)), true)
                    } else {
                        metadata.extend(col.metadata);
                        item
                    };
                    fields.push(field.with_metadata(metadata));
                }
            }
        }
    }
}
