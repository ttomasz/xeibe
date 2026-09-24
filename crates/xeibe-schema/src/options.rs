//! `InferenceOptions`: the policy that turns a path tree into an Arrow schema.

use arrow_schema::{DataType, TimeUnit};
use xeibe_geom::GeometryOptions;
use xeibe_geom::options::GeomEncoding;
use serde::{Deserialize, Serialize};

use crate::{PathPattern, TypeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InferenceOptions {
    pub naming: NamingOptions,
    pub structure: StructureOptions,
    pub types: TypeOptions,
    pub gml: GmlOptions,
    /// Geometry column encoding chosen by the scan and read samples.
    pub geometry_encoding: GeomEncoding,
    /// The read's geometry options (`ReadOptions.geometry`: axis order, CRS
    /// override, curves), copied in by the reader. Not part of the settings
    /// file's `inference` section.
    #[serde(skip)]
    pub geometry: GeometryOptions,
    pub overrides: Vec<(PathPattern, FieldOverride)>,
    /// Per-layer adjustments; later entries win.
    pub layers: Vec<(String, InferenceOptionsPatch)>,
    pub limits: Limits,
}

impl Default for InferenceOptions {
    /// Lossless by value, `Struct` nesting, `@` attributes, rich types,
    /// geometry encoding `Auto` (`docs/schema-inference.md` §3).
    fn default() -> Self {
        InferenceOptions {
            naming: NamingOptions::default(),
            structure: StructureOptions::default(),
            types: TypeOptions::default(),
            gml: GmlOptions::default(),
            geometry_encoding: GeomEncoding::Auto,
            geometry: GeometryOptions::default(),
            overrides: Vec::new(),
            layers: Vec::new(),
            limits: Limits::default(),
        }
    }
}

impl InferenceOptions {
    /// Options effective for one layer (global + matching patches).
    ///
    /// A patch's selector matches the layer's local name, its prefixed or
    /// Clark name, and may use `*`/`?` globs (`AD_*`). `layer` may be given
    /// in any of those forms. Later patches win.
    pub fn for_layer(&self, layer: &str) -> InferenceOptions {
        let mut options = self.clone();
        options.layers.clear();
        for (selector, patch) in &self.layers {
            if layer_selector_matches(selector, layer) {
                patch.apply_to(&mut options);
            }
        }
        options
    }
}

/// Match a per-layer selector against a layer name in any notation.
pub(crate) fn layer_selector_matches(selector: &str, layer: &str) -> bool {
    let local = |name: &str| -> String {
        match name.rsplit_once('}') {
            Some((_, local)) => local.to_string(),
            None => name.rsplit_once(':').map_or(name, |(_, l)| l).to_string(),
        }
    };
    let glob = |pattern: &str, text: &str| crate::pattern::wildcard(pattern.as_bytes(), text.as_bytes());
    glob(selector, layer)
        || (!selector.contains(['{', ':']) && glob(selector, &local(layer)))
        || (!layer.contains(['{', ':']) && glob(&local(selector), layer))
}

impl InferenceOptionsPatch {
    pub fn apply_to(&self, options: &mut InferenceOptions) {
        if let Some(naming) = &self.naming {
            options.naming = naming.clone();
        }
        if let Some(structure) = &self.structure {
            options.structure = structure.clone();
        }
        if let Some(types) = &self.types {
            options.types = types.clone();
        }
        if let Some(gml) = &self.gml {
            options.gml = gml.clone();
        }
        if let Some(geometry) = &self.geometry {
            options.geometry = geometry.clone();
        }
    }
}

/// Partial options applied on top of the global ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InferenceOptionsPatch {
    pub naming: Option<NamingOptions>,
    pub structure: Option<StructureOptions>,
    pub types: Option<TypeOptions>,
    pub gml: Option<GmlOptions>,
    pub geometry: Option<GeometryOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NamingOptions {
    pub namespaces: NsMode,
    /// `@` by decision; configurable for compatibility presets.
    pub attribute_prefix: String,
    /// Text of an element that also has attributes: `#text`.
    pub text_field: String,
    pub flatten_separator: String,
}

impl Default for NamingOptions {
    fn default() -> Self {
        NamingOptions {
            namespaces: NsMode::StripUnlessCollision,
            attribute_prefix: "@".to_string(),
            text_field: "#text".to_string(),
            flatten_separator: ".".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NsMode {
    Strip,
    #[default]
    StripUnlessCollision,
    Prefix,
    Clark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StructureOptions {
    pub nesting: Nesting,
    pub lists: ListRule,
    pub force_list: Vec<PathPattern>,
    pub force_scalar: Vec<PathPattern>,
    pub simple_with_attrs: SimpleContent,
    pub constant_attrs: ConstantAttrs,
    pub collapse_type_wrappers: bool,
    pub mixed_content: MixedContent,
    pub xml_attributes: AttrSelect,
}

impl Default for StructureOptions {
    fn default() -> Self {
        StructureOptions {
            nesting: Nesting::Struct,
            lists: ListRule::Infer,
            force_list: Vec::new(),
            force_scalar: Vec::new(),
            simple_with_attrs: SimpleContent::Struct,
            constant_attrs: ConstantAttrs::ToFieldMetadata,
            collapse_type_wrappers: true,
            mixed_content: MixedContent::RawXml,
            xml_attributes: AttrSelect::All,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Nesting {
    #[default]
    Struct,
    FlattenSingleOnly,
    Flatten {
        max_depth: u16,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListRule {
    /// `max_occurs > 1` → List.
    #[default]
    Infer,
    Never(OnRepeat),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnRepeat {
    TakeFirst,
    Error,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimpleContent {
    /// `area: Struct("#text": …, "@uom": Utf8View)`
    #[default]
    Struct,
    /// `area`, `area.@uom`
    Split,
    /// `area` (attributes dropped)
    ValueOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstantAttrs {
    #[default]
    ToFieldMetadata,
    Keep,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MixedContent {
    #[default]
    RawXml,
    TextOnly,
    Drop,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttrSelect {
    #[default]
    All,
    None,
    Only(Vec<String>),
    Except(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TypeOptions {
    pub lossless: Lossless,
    /// `STRING` only ⇒ GDAL's `ALWAYS_STRING`.
    pub enabled: TypeSet,
    pub integers: IntWidth,
    pub timestamps: TimestampOptions,
    pub empty_as_null: bool,
    pub all_null: AllNull,
    pub string_view: bool,
}

impl Default for TypeOptions {
    fn default() -> Self {
        TypeOptions {
            lossless: Lossless::Value,
            enabled: TypeSet::all(),
            integers: IntWidth::Int64,
            timestamps: TimestampOptions::default(),
            empty_as_null: true,
            all_null: AllNull::Utf8View,
            string_view: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lossless {
    Text,
    #[default]
    Value,
    Lossy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntWidth {
    #[default]
    Int64,
    Smallest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimestampOptions {
    pub unit: TimeUnit,
    /// Add `<name>.@offset_min` (Int16) when offsets differ.
    pub keep_offset: bool,
}

impl Default for TimestampOptions {
    fn default() -> Self {
        TimestampOptions { unit: TimeUnit::Microsecond, keep_offset: true }
    }
}

/// The type an Arrow `DataType` gets for a string column.
pub(crate) fn string_type(types: &TypeOptions) -> DataType {
    if types.string_view { DataType::Utf8View } else { DataType::Utf8 }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllNull {
    #[default]
    Utf8View,
    Null,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GmlOptions {
    pub gml_id: IdMode,
    pub xlink: XlinkMode,
    /// `#PL.X.1` → `PL.X.1`.
    pub strip_local_href_hash: bool,
    pub nil_reason: bool,
    pub bounded_by: BoundedBy,
    /// Drop `owns`, `remoteSchema`, `aggregationType`, `xlink:type/show/actuate`.
    pub drop_control_attributes: bool,
}

impl Default for GmlOptions {
    fn default() -> Self {
        GmlOptions {
            gml_id: IdMode::Column,
            xlink: XlinkMode::Href,
            strip_local_href_hash: true,
            nil_reason: true,
            bounded_by: BoundedBy::Drop,
            drop_control_attributes: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdMode {
    #[default]
    Column,
    Drop,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum XlinkMode {
    #[default]
    Href,
    Full,
    Drop,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoundedBy {
    #[default]
    Drop,
    BoxStruct,
    Geometry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldOverride {
    Type(DataType),
    Rename(String),
    Drop,
    AsRawXml,
    AsMap,
    List,
    Scalar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Deeper subtrees become Map / raw XML.
    pub max_depth: u16,
    /// Elements with more distinct child names become Map.
    pub max_children: u16,
    /// `BoundedSet` capacity.
    pub distinct_values: u16,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_depth: 16,
            max_children: 512,
            distinct_values: crate::value::DEFAULT_DISTINCT_VALUES,
        }
    }
}

/// Data that doesn't fit the schema used by a read (docs/schema-inference.md §6.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnSchemaMismatch {
    /// `_overflow: Map(Utf8View → Utf8View)`.
    #[default]
    Overflow,
    Error,
    Drop,
}

/// Reads without a schema (docs/schema-inference.md §6): the schema is inferred
/// from the first features **of the requested layer**, then frozen.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SampleOptions {
    /// Default 10_000.
    pub features_per_layer: u64,
    /// Buffered sample features (default 256 MiB). When exceeded, the schema is
    /// frozen early with the sample it has.
    pub max_buffer_bytes: u64,
    /// Fewer non-null values than this → `Utf8View` (default 100). Conservative only.
    pub min_typed_values: u64,
    /// Apply the conservative type rules (default true).
    pub conservative: bool,
}

impl Default for SampleOptions {
    fn default() -> Self {
        SampleOptions {
            features_per_layer: 10_000,
            max_buffer_bytes: 256 << 20,
            min_typed_values: 100,
            conservative: true,
        }
    }
}
