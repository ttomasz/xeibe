//! `InferenceOptions`: the policy that turns a path tree into an Arrow schema.

use arrow_schema::{DataType, TimeUnit};
use xeibe_geom::GeometryOptions;
use serde::{Deserialize, Serialize};

use crate::{PathPattern, TypeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceOptions {
    pub naming: NamingOptions,
    pub structure: StructureOptions,
    pub types: TypeOptions,
    pub gml: GmlOptions,
    pub geometry: GeometryOptions,
    pub overrides: Vec<(PathPattern, FieldOverride)>,
    /// Per-layer adjustments; later entries win.
    pub layers: Vec<(String, InferenceOptionsPatch)>,
    pub limits: Limits,
}

impl Default for InferenceOptions {
    fn default() -> Self {
        todo!()
    }
}

impl InferenceOptions {
    /// Options effective for one layer (global + matching patches).
    pub fn for_layer(&self, layer: &str) -> InferenceOptions {
        todo!()
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
pub struct NamingOptions {
    pub namespaces: NsMode,
    /// `@` by decision; configurable for compatibility presets.
    pub attribute_prefix: String,
    /// Text of an element that also has attributes: `#text`.
    pub text_field: String,
    pub flatten_separator: String,
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
pub struct TimestampOptions {
    pub unit: TimeUnit,
    /// Add `<name>.@offset_min` (Int16) when offsets differ.
    pub keep_offset: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllNull {
    #[default]
    Utf8View,
    Null,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        todo!()
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
        todo!()
    }
}
