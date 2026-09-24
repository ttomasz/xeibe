//! The settings file: read options and/or per-layer schemas, as JSON
//! (see `docs/architecture.md#settings-file`).
//!
//! A column is `"name": "type"` or `"name": { "type": …, "path": … }`;
//! without a path, the name is the path. Types are Arrow `DataType` strings
//! or PostgreSQL/DuckDB-style aliases (`text`, `bigint`, `timestamptz`,
//! `text[]`, `geometry(Point)`), which is what the scan writes.

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use geoarrow_schema::{
    BoxType, Dimension, GeoArrowType, LineStringType, Metadata, MultiLineStringType, MultiPointType,
    MultiPolygonType, PointType, PolygonType, WkbType,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use xeibe_schema::bind::schema_namespaces;
use xeibe_schema::rules::{map_type, meta};

use crate::ReadOptions;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "SettingsFile", into = "SettingsFile")]
pub struct Settings {
    pub format_version: u32,
    /// Every key optional; callers' parameters override it. The file's
    /// top-level `namespaces` are kept in `options.namespaces`.
    pub options: ReadOptions,
    /// Layer name (prefixed or Clark notation) → columns in order.
    pub layers: IndexMap<String, IndexMap<String, ColumnSpec>>,
}

/// The file as written: `namespaces` at the top level.
#[derive(Serialize, Deserialize)]
struct SettingsFile {
    format_version: u32,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    namespaces: IndexMap<String, String>,
    #[serde(default)]
    options: ReadOptions,
    #[serde(default)]
    layers: IndexMap<String, IndexMap<String, ColumnSpec>>,
}

impl From<SettingsFile> for Settings {
    fn from(file: SettingsFile) -> Self {
        let mut options = file.options;
        options.namespaces = file.namespaces;
        Settings { format_version: file.format_version, options, layers: file.layers }
    }
}

impl From<Settings> for SettingsFile {
    fn from(settings: Settings) -> Self {
        let mut options = settings.options;
        let namespaces = std::mem::take(&mut options.namespaces);
        SettingsFile { format_version: settings.format_version, namespaces, options, layers: settings.layers }
    }
}

/// One column: a type, or a type and the path it comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColumnSpec {
    /// The column's path is its name.
    Type(String),
    Detailed {
        #[serde(rename = "type")]
        data_type: String,
        /// XML path relative to the feature (`idIIP/*/lokalnyId`,
        /// `adres[]/numer`, `miejscowosc/@href`).
        #[serde(default)]
        path: Option<String>,
    },
}

impl Settings {
    pub const FORMAT_VERSION: u32 = 1;

    /// Settings with the given options and no layers.
    pub fn new(options: ReadOptions) -> Self {
        Settings { format_version: Self::FORMAT_VERSION, options, layers: IndexMap::new() }
    }

    pub fn load(path: &Path) -> crate::Result<Self> {
        let error = |message: String| crate::Error::Settings { path: path.display().to_string(), message };
        let text = std::fs::read_to_string(path).map_err(|e| error(e.to_string()))?;
        let settings: Settings = serde_json::from_str(&text).map_err(|e| error(e.to_string()))?;
        if settings.format_version > Self::FORMAT_VERSION {
            return Err(error(format!(
                "format_version {} is newer than this release reads ({})",
                settings.format_version,
                Self::FORMAT_VERSION
            )));
        }
        Ok(settings)
    }

    pub fn save(&self, path: &Path) -> crate::Result<()> {
        let error = |message: String| crate::Error::Settings { path: path.display().to_string(), message };
        let mut text = serde_json::to_string_pretty(self).map_err(|e| error(e.to_string()))?;
        text.push('\n');
        std::fs::write(path, text).map_err(|e| error(e.to_string()))
    }

    /// Arrow schema of one layer, with GeoArrow extension types on geometry
    /// columns, `gml:path` where a column names its path, and the file's
    /// namespaces in the schema metadata `gml:ns`.
    ///
    /// `layer` is looked up as written, then by local name (`AD_PunktAdresowy`
    /// finds `prgad:AD_PunktAdresowy` and `{uri}AD_PunktAdresowy`).
    pub fn schema(&self, layer: &str) -> crate::Result<SchemaRef> {
        let (_, columns) = self.layer(layer)?;
        let fields = columns
            .iter()
            .map(|(name, spec)| spec.to_field(name))
            .collect::<crate::Result<Vec<_>>>()?;
        let mut metadata = std::collections::HashMap::new();
        if !self.options.namespaces.is_empty() {
            let namespaces = serde_json::to_string(&self.options.namespaces).expect("strings serialize");
            metadata.insert(meta::NS.to_string(), namespaces);
        }
        Ok(Arc::new(Schema::new_with_metadata(fields, metadata)))
    }

    /// Store an Arrow schema as `column → type` (the reverse of
    /// [`Self::schema`]). Its `gml:ns` prefixes join the file's namespaces.
    pub fn set_schema(&mut self, layer: &str, schema: &Schema) -> crate::Result<()> {
        let error = |message: String| crate::Error::Settings { path: String::new(), message };
        for (prefix, uri) in schema_namespaces(schema).map_err(error)? {
            match self.options.namespaces.get(&prefix) {
                Some(known) if *known != uri => {
                    return Err(error(format!(
                        "layer {layer}: prefix {prefix:?} is {uri:?}, but the settings have it as {known:?}"
                    )));
                }
                Some(_) => {}
                None => {
                    self.options.namespaces.insert(prefix, uri);
                }
            }
        }
        let mut columns = IndexMap::new();
        for field in schema.fields() {
            columns.insert(field.name().clone(), ColumnSpec::from_field(field)?);
        }
        self.layers.insert(layer.to_string(), columns);
        Ok(())
    }

    /// The layer's entry: by exact key, else by local name (unique).
    fn layer(&self, layer: &str) -> crate::Result<(&String, &IndexMap<String, ColumnSpec>)> {
        if let Some(entry) = self.layers.get_key_value(layer) {
            return Ok(entry);
        }
        let wanted = local_name(layer);
        let mut matches = self.layers.iter().filter(|(key, _)| local_name(key) == wanted);
        match (matches.next(), matches.next()) {
            (Some(entry), None) => Ok(entry),
            (Some((a, _)), Some((b, _))) => Err(crate::Error::Settings {
                path: String::new(),
                message: format!("layer {layer:?} is ambiguous: {a:?} and {b:?}"),
            }),
            (None, _) => Err(xeibe_schema::Error::UnknownLayer(layer.to_string()).into()),
        }
    }
}

/// `prefix:local`, `{uri}local` or `local` → `local`.
pub(crate) fn local_name(name: &str) -> &str {
    match name.rsplit_once('}') {
        Some((_, local)) => local,
        None => name.rsplit_once(':').map_or(name, |(_, local)| local),
    }
}

impl ColumnSpec {
    /// The column as a nullable Arrow field.
    pub fn to_field(&self, name: &str) -> crate::Result<Field> {
        let (type_string, path) = match self {
            ColumnSpec::Type(data_type) => (data_type.as_str(), None),
            ColumnSpec::Detailed { data_type, path } => (data_type.as_str(), path.as_deref()),
        };
        let error = |message: String| crate::Error::ColumnType {
            column: name.to_string(),
            type_string: type_string.to_string(),
            message,
        };
        let field = parse_type(type_string, name).map_err(error)?;
        Ok(match path {
            Some(path) => {
                let mut metadata = field.metadata().clone();
                metadata.insert(meta::PATH.to_string(), path.to_string());
                field.with_metadata(metadata)
            }
            None => field,
        })
    }

    /// The settings form of a field: its alias (or Arrow type string), and its
    /// path only where it isn't the name.
    pub fn from_field(field: &Field) -> crate::Result<Self> {
        let data_type = type_string(field).map_err(|message| crate::Error::ColumnType {
            column: field.name().clone(),
            type_string: field.data_type().to_string(),
            message,
        })?;
        let path = field.metadata().get(meta::PATH).filter(|path| *path != field.name()).cloned();
        Ok(match path {
            Some(path) => ColumnSpec::Detailed { data_type, path: Some(path) },
            None => ColumnSpec::Type(data_type),
        })
    }
}

/// A type string (alias or Arrow `DataType`) as a nullable field.
fn parse_type(text: &str, name: &str) -> Result<Field, String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    if normalized.starts_with("numeric") || normalized.starts_with("decimal") {
        return Err(
            "there is no numeric or decimal type: use double (lossless by value) or text (docs/type-mapping.md)".into(),
        );
    }
    if let Some(item) = text.trim().strip_suffix("[]") {
        let item = parse_type(item, "item")?;
        if matches!(item.data_type(), DataType::List(_) | DataType::LargeList(_) | DataType::Struct(_))
            && !is_geoarrow(&item)
        {
            return Err("a list holds scalars, maps or WKB geometry".into());
        }
        if is_geoarrow(&item) && extension_name(&item) != Some("geoarrow.wkb") {
            return Err("a geometry list is `geometry[]` (WKB)".into());
        }
        return Ok(Field::new(name, DataType::List(Arc::new(item)), true));
    }
    if let Some(geometry) = parse_geometry(&normalized)? {
        return Ok(geometry.to_field(name, true));
    }
    let data_type = match alias(&normalized) {
        Some(data_type) => data_type,
        None => DataType::from_str(text.trim()).map_err(|e| e.to_string())?,
    };
    Ok(Field::new(name, data_type, true))
}

/// The Arrow type of an alias (lower case, single spaces).
fn alias(text: &str) -> Option<DataType> {
    let micros = TimeUnit::Microsecond;
    Some(match text {
        "text" | "varchar" | "string" => DataType::Utf8View,
        "boolean" | "bool" => DataType::Boolean,
        "smallint" | "int2" => DataType::Int16,
        "integer" | "int" | "int4" => DataType::Int32,
        "bigint" | "int8" => DataType::Int64,
        "real" | "float4" => DataType::Float32,
        "double" | "double precision" | "float8" => DataType::Float64,
        "date" => DataType::Date32,
        "timestamp" => DataType::Timestamp(micros, None),
        "timestamptz" => DataType::Timestamp(micros, Some("UTC".into())),
        "time" => DataType::Time64(micros),
        "bytea" | "blob" => DataType::Binary,
        "map" => map_type(&DataType::Utf8View),
        _ => return None,
    })
}

/// `geometry` (WKB) or `geometry(<kind>[, <dims>])`; `Ok(None)` for other
/// types. `text` is lower case.
fn parse_geometry(text: &str) -> Result<Option<GeoArrowType>, String> {
    let Some(rest) = text.strip_prefix("geometry") else {
        return Ok(None);
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(Some(GeoArrowType::Wkb(WkbType::new(Default::default()))));
    }
    let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) else {
        return Err("expected geometry or geometry(<kind>[, <dims>])".into());
    };
    let mut parts = inner.split(',').map(str::trim);
    let kind = parts.next().unwrap_or_default();
    let dimension = match parts.next() {
        None | Some("xy") => Dimension::XY,
        Some("xyz") => Dimension::XYZ,
        Some(other) => return Err(format!("unknown dimensions {other:?} (XY or XYZ)")),
    };
    if parts.next().is_some() {
        return Err("too many arguments to geometry(…)".into());
    }
    let metadata: Arc<Metadata> = Default::default();
    Ok(Some(match kind {
        "wkb" => GeoArrowType::Wkb(WkbType::new(metadata)),
        "point" => GeoArrowType::Point(PointType::new(dimension, metadata)),
        "linestring" => GeoArrowType::LineString(LineStringType::new(dimension, metadata)),
        "polygon" => GeoArrowType::Polygon(PolygonType::new(dimension, metadata)),
        "multipoint" => GeoArrowType::MultiPoint(MultiPointType::new(dimension, metadata)),
        "multilinestring" => GeoArrowType::MultiLineString(MultiLineStringType::new(dimension, metadata)),
        "multipolygon" => GeoArrowType::MultiPolygon(MultiPolygonType::new(dimension, metadata)),
        "box" => GeoArrowType::Rect(BoxType::new(dimension, metadata)),
        other => return Err(format!("unknown geometry kind {other:?}")),
    }))
}

/// The settings form of a field's type: an alias where there is one, else
/// the Arrow type string.
fn type_string(field: &Field) -> Result<String, String> {
    if let Some(geometry) = GeoArrowType::from_extension_field(field).map_err(|e| e.to_string())? {
        return geometry_string(&geometry).ok_or_else(|| "no settings form for this GeoArrow type".to_string());
    }
    Ok(match field.data_type() {
        DataType::List(item) | DataType::LargeList(item) => format!("{}[]", type_string(item)?),
        data_type => alias_of(data_type).map_or_else(|| data_type.to_string(), str::to_string),
    })
}

/// The alias the scan writes for a type.
fn alias_of(data_type: &DataType) -> Option<&'static str> {
    let micros = TimeUnit::Microsecond;
    Some(match data_type {
        DataType::Utf8View => "text",
        DataType::Boolean => "boolean",
        DataType::Int16 => "smallint",
        DataType::Int32 => "integer",
        DataType::Int64 => "bigint",
        DataType::Float32 => "real",
        DataType::Float64 => "double",
        DataType::Date32 => "date",
        DataType::Timestamp(unit, None) if *unit == micros => "timestamp",
        DataType::Timestamp(unit, Some(tz)) if *unit == micros && &**tz == "UTC" => "timestamptz",
        DataType::Time64(unit) if *unit == micros => "time",
        DataType::Binary => "bytea",
        data_type if *data_type == map_type(&DataType::Utf8View) => "map",
        _ => return None,
    })
}

/// The settings form of a GeoArrow type (the reverse of [`parse_geometry`]).
fn geometry_string(geometry: &GeoArrowType) -> Option<String> {
    let with_dims = |kind: &str, dimension: Dimension| match dimension {
        Dimension::XY => format!("geometry({kind})"),
        Dimension::XYZ => format!("geometry({kind}, XYZ)"),
        _ => format!("geometry({kind}, {dimension:?})"),
    };
    Some(match geometry {
        GeoArrowType::Wkb(_) | GeoArrowType::LargeWkb(_) | GeoArrowType::WkbView(_) => "geometry".to_string(),
        GeoArrowType::Point(t) => with_dims("Point", t.dimension()),
        GeoArrowType::LineString(t) => with_dims("LineString", t.dimension()),
        GeoArrowType::Polygon(t) => with_dims("Polygon", t.dimension()),
        GeoArrowType::MultiPoint(t) => with_dims("MultiPoint", t.dimension()),
        GeoArrowType::MultiLineString(t) => with_dims("MultiLineString", t.dimension()),
        GeoArrowType::MultiPolygon(t) => with_dims("MultiPolygon", t.dimension()),
        GeoArrowType::Rect(t) => with_dims("Box", t.dimension()),
        _ => return None,
    })
}

fn extension_name(field: &Field) -> Option<&str> {
    field.metadata().get("ARROW:extension:name").map(String::as_str)
}

fn is_geoarrow(field: &Field) -> bool {
    extension_name(field).is_some_and(|name| name.starts_with("geoarrow."))
}
