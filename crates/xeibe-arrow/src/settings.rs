//! The settings file: read options and/or per-layer schemas, as JSON
//! (see `docs/architecture.md#settings-file`).

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use geoarrow_schema::{
    BoxType, Dimension, GeoArrowType, GeometryType, LineStringType, Metadata, MultiLineStringType,
    MultiPointType, MultiPolygonType, PointType, PolygonType, WkbType,
};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use xeibe_schema::rules::meta;

use crate::ReadOptions;
use crate::overflow::OVERFLOW_COLUMN;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub format_version: u32,
    /// Every key optional; callers' parameters override it.
    #[serde(default)]
    pub options: ReadOptions,
    /// Layer name (prefixed or Clark notation) → columns in order.
    #[serde(default)]
    pub layers: IndexMap<String, IndexMap<String, ColumnSpec>>,
}

/// One column: a type string, or an object for the less common cases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColumnSpec {
    /// Arrow `DataType` string (`Utf8View`, `List(Utf8View)`, …), or `Geometry` /
    /// `Geometry(<kind>[, <dims>])` for geometry columns. Nullable.
    Type(String),
    Detailed {
        #[serde(rename = "type")]
        data_type: String,
        /// XML path relative to the feature, for renamed columns (`gml:path`).
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

    /// Arrow schema of one layer, with GeoArrow extension types on geometry columns.
    ///
    /// `layer` is looked up as written, then by local name (`AD_PunktAdresowy`
    /// finds `prgad:AD_PunktAdresowy` and `{uri}AD_PunktAdresowy`).
    pub fn schema(&self, layer: &str) -> crate::Result<SchemaRef> {
        let (_, columns) = self.layer(layer)?;
        let fields = columns
            .iter()
            .map(|(name, spec)| spec.to_field(name))
            .collect::<crate::Result<Vec<_>>>()?;
        Ok(Arc::new(Schema::new(fields)))
    }

    /// Store an Arrow schema as `column → type` (the reverse of [`Self::schema`]).
    ///
    /// `_overflow` is left out: every read adds it again when it is wanted.
    pub fn set_schema(&mut self, layer: &str, schema: &Schema) -> crate::Result<()> {
        let mut columns = IndexMap::new();
        for field in schema.fields() {
            if field.name() == OVERFLOW_COLUMN {
                continue;
            }
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
        let field = match parse_geometry(type_string).map_err(error)? {
            Some(geometry) => geometry.to_field(name, true),
            None => {
                let data_type = DataType::from_str(type_string).map_err(|e| error(e.to_string()))?;
                Field::new(name, data_type, true)
            }
        };
        Ok(match path {
            Some(path) => {
                let mut metadata = field.metadata().clone();
                metadata.insert(meta::PATH.to_string(), path.to_string());
                field.with_metadata(metadata)
            }
            None => field,
        })
    }

    pub fn from_field(field: &Field) -> crate::Result<Self> {
        let error = |message: String| crate::Error::ColumnType {
            column: field.name().clone(),
            type_string: field.data_type().to_string(),
            message,
        };
        let data_type = match GeoArrowType::from_extension_field(field).map_err(|e| error(e.to_string()))? {
            Some(geometry) => geometry_string(&geometry).ok_or_else(|| error("no settings form for this GeoArrow type".into()))?,
            None => without_metadata(field.data_type()).to_string(),
        };
        // A path is written only when the name alone would not find the data.
        let path = field
            .metadata()
            .get(meta::PATH)
            .filter(|path| !path_matches_name(path, field.name()))
            .cloned();
        Ok(match path {
            Some(path) => ColumnSpec::Detailed { data_type, path: Some(path) },
            None => ColumnSpec::Type(data_type),
        })
    }
}

/// The type without the metadata of nested fields (`gml:path`, …), which
/// `arrow-schema` would print and can't parse back, and with every nested
/// field nullable, as settings-file columns are. A nested geometry field has
/// no settings form: it is left with its storage type only.
fn without_metadata(data_type: &DataType) -> DataType {
    let field = |field: &Field| Arc::new(Field::new(field.name(), without_metadata(field.data_type()), true));
    match data_type {
        DataType::Struct(fields) => DataType::Struct(fields.iter().map(|f| field(f)).collect()),
        DataType::List(item) => DataType::List(field(item)),
        DataType::LargeList(item) => DataType::LargeList(field(item)),
        DataType::Map(entries, sorted) => DataType::Map(field(entries), *sorted),
        other => other.clone(),
    }
}

/// `true` if binding by name finds `path` from `name`: the local names of the
/// steps, type wrappers (UpperCamel steps before the last) left out, joined
/// with `.` as a flattened name is (`@` for an attribute).
fn path_matches_name(path: &str, name: &str) -> bool {
    let steps: Vec<&str> = path.split('/').filter(|step| !step.is_empty()).collect();
    // A name with a `.` in it (`SHAPE.AREA`) would be read as a flattened one.
    if steps.iter().any(|step| local_name(step).contains('.')) {
        return false;
    }
    let last = steps.len().saturating_sub(1);
    let expected: Vec<String> = steps
        .iter()
        .enumerate()
        .filter(|(i, step)| *i == last || !local_name(step).starts_with(char::is_uppercase))
        .map(|(_, step)| match step.strip_prefix('@') {
            Some(attribute) => format!("@{}", local_name(attribute)),
            None => local_name(step).to_string(),
        })
        .collect();
    expected.join(".") == name
}

/// `Geometry` (WKB) or `Geometry(<kind>[, <dims>])`; `Ok(None)` for other types.
fn parse_geometry(text: &str) -> Result<Option<GeoArrowType>, String> {
    let text = text.trim();
    let Some(rest) = text.strip_prefix("Geometry") else {
        return Ok(None);
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(Some(GeoArrowType::Wkb(WkbType::new(Default::default()))));
    }
    let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) else {
        return Err("expected Geometry or Geometry(<kind>[, <dims>])".into());
    };
    let mut parts = inner.split(',').map(str::trim);
    let kind = parts.next().unwrap_or_default();
    let dimension = match parts.next() {
        None | Some("XY") => Dimension::XY,
        Some("XYZ") => Dimension::XYZ,
        Some(other) => return Err(format!("unknown dimensions {other:?} (XY or XYZ)")),
    };
    if parts.next().is_some() {
        return Err("too many arguments to Geometry(…)".into());
    }
    let metadata: Arc<Metadata> = Default::default();
    Ok(Some(match kind {
        "Wkb" | "WKB" => GeoArrowType::Wkb(WkbType::new(metadata)),
        "Point" => GeoArrowType::Point(PointType::new(dimension, metadata)),
        "LineString" => GeoArrowType::LineString(LineStringType::new(dimension, metadata)),
        "Polygon" => GeoArrowType::Polygon(PolygonType::new(dimension, metadata)),
        "MultiPoint" => GeoArrowType::MultiPoint(MultiPointType::new(dimension, metadata)),
        "MultiLineString" => GeoArrowType::MultiLineString(MultiLineStringType::new(dimension, metadata)),
        "MultiPolygon" => GeoArrowType::MultiPolygon(MultiPolygonType::new(dimension, metadata)),
        "Geometry" => GeoArrowType::Geometry(GeometryType::new(metadata)),
        "Box" => GeoArrowType::Rect(BoxType::new(dimension, metadata)),
        other => return Err(format!("unknown geometry kind {other:?}")),
    }))
}

/// The settings form of a GeoArrow type (the reverse of [`parse_geometry`]).
fn geometry_string(geometry: &GeoArrowType) -> Option<String> {
    let with_dims = |kind: &str, dimension: Dimension| match dimension {
        Dimension::XY => format!("Geometry({kind})"),
        Dimension::XYZ => format!("Geometry({kind}, XYZ)"),
        _ => format!("Geometry({kind}, {dimension:?})"),
    };
    Some(match geometry {
        GeoArrowType::Wkb(_) | GeoArrowType::LargeWkb(_) | GeoArrowType::WkbView(_) => "Geometry".to_string(),
        GeoArrowType::Point(t) => with_dims("Point", t.dimension()),
        GeoArrowType::LineString(t) => with_dims("LineString", t.dimension()),
        GeoArrowType::Polygon(t) => with_dims("Polygon", t.dimension()),
        GeoArrowType::MultiPoint(t) => with_dims("MultiPoint", t.dimension()),
        GeoArrowType::MultiLineString(t) => with_dims("MultiLineString", t.dimension()),
        GeoArrowType::MultiPolygon(t) => with_dims("MultiPolygon", t.dimension()),
        GeoArrowType::Geometry(_) => "Geometry(Geometry)".to_string(),
        GeoArrowType::Rect(t) => with_dims("Box", t.dimension()),
        _ => return None,
    })
}
