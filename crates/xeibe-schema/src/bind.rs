//! Binding a schema (settings file, Arrow schema, or an inferred one) to XML
//! paths (`docs/architecture.md`, "Settings file", "Paths").
//!
//! A read only matches paths. A column's path is its `gml:path` field
//! metadata; without one, the name is the path. No naming rule is applied and
//! nothing is looked through: a type wrapper is `*` in the path.
//!
//! A step without a prefix matches an element (or attribute) of that local
//! name in any namespace. A prefixed step matches its namespace only; its
//! prefix is declared in the schema metadata `gml:ns` (the settings file's
//! top-level `namespaces`), never taken from the document.

use std::collections::HashMap;

use arrow_schema::{DataType, Field, Schema, TimeUnit};
use xeibe_core::QName;

use crate::rules::{FieldRoute, RouteValue, meta};
use crate::{InferenceOptions, LayerSchema};

/// A parsed path: `idIIP/*/lokalnyId`, `adres[]/numer`, `miejscowosc/@href`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnPath {
    /// Element steps from the feature. `*` (any element) is a name with the
    /// local name `*`; a step without a namespace matches any namespace.
    pub steps: Vec<QName>,
    /// A last `@name` step.
    pub attribute: Option<QName>,
    /// The step marked `[]`: the anchor of a list column.
    pub anchor: Option<usize>,
}

impl ColumnPath {
    /// Parse a path. `namespaces` maps the prefixes of prefixed steps to URIs.
    pub fn parse(path: &str, namespaces: &HashMap<String, String>) -> Result<Self, String> {
        if path.is_empty() {
            return Err("the path is empty".into());
        }
        let parts: Vec<&str> = path.split('/').collect();
        let mut parsed = ColumnPath { steps: Vec::new(), attribute: None, anchor: None };
        for (index, part) in parts.iter().enumerate() {
            if part.is_empty() {
                return Err(format!("empty step in {path:?}"));
            }
            if let Some(attribute) = part.strip_prefix('@') {
                if index + 1 != parts.len() {
                    return Err(format!("an attribute must be the last step of {path:?}"));
                }
                parsed.attribute = Some(step_name(attribute, namespaces)?);
                continue;
            }
            let (step, anchor) = match part.strip_suffix("[]") {
                Some(step) => (step, true),
                None => (*part, false),
            };
            if anchor {
                if parsed.anchor.is_some() {
                    return Err(format!("{path:?} has more than one `[]`: there are no lists of lists"));
                }
                parsed.anchor = Some(parsed.steps.len());
            }
            parsed.steps.push(if step == "*" { QName::new(None, "*") } else { step_name(step, namespaces)? });
        }
        Ok(parsed)
    }
}

/// `{uri}local`, `prefix:local` (declared prefix) or `local` (any namespace).
fn step_name(step: &str, namespaces: &HashMap<String, String>) -> Result<QName, String> {
    if step.is_empty() || step.contains(['[', ']', '@', '*']) {
        return Err(format!("{step:?} is not a valid step"));
    }
    if let Some((uri, local)) = step.strip_prefix('{').and_then(|rest| rest.split_once('}')) {
        return Ok(QName::new(Some(uri), local));
    }
    match step.split_once(':') {
        Some((prefix, local)) => match namespaces.get(prefix) {
            Some(uri) => Ok(QName::new(Some(uri), local)),
            None => Err(format!(
                "the prefix {prefix:?} is not declared (settings file `namespaces`, or schema metadata `{}`)",
                meta::NS
            )),
        },
        None => Ok(QName::new(None, step)),
    }
}

/// The prefixes a schema declares in its `gml:ns` metadata.
pub fn schema_namespaces(schema: &Schema) -> Result<HashMap<String, String>, String> {
    match schema.metadata().get(meta::NS) {
        None => Ok(HashMap::new()),
        Some(json) => serde_json::from_str(json).map_err(|e| format!("invalid `{}` metadata: {e}", meta::NS)),
    }
}

/// Bind `schema` to the layer's XML: one route per column. The schema is used
/// as it is; its columns are the only data the read takes.
///
/// `options` is taken for symmetry with [`crate::infer_schema`]: binding
/// applies no inference rule.
pub fn bind_schema(layer: &QName, schema: &Schema, _options: &InferenceOptions) -> crate::Result<LayerSchema> {
    let error = |field: Option<&Field>, message: String| crate::Error::Bind {
        layer: layer.to_string(),
        message: match field {
            Some(field) => format!("column {:?}: {message}", field.name()),
            None => message,
        },
    };
    let namespaces = schema_namespaces(schema).map_err(|message| error(None, message))?;
    let mut routes = Vec::new();
    for (index, field) in schema.fields().iter().enumerate() {
        let text = field.metadata().get(meta::PATH).map_or(field.name().as_str(), String::as_str);
        let path = ColumnPath::parse(text, &namespaces).map_err(|message| error(Some(field), message))?;
        let (list, item) = match field.data_type() {
            DataType::List(item) | DataType::LargeList(item) if !is_geoarrow(field) => (true, item.as_ref()),
            _ => (false, field.as_ref()),
        };
        let value = route_value(field, item, list).map_err(|message| error(Some(field), message))?;
        let anchor = match (list, path.anchor) {
            (false, Some(_)) => {
                return Err(error(Some(field), format!("`[]` in {text:?} marks the anchor of a list, but the column is not a list")));
            }
            (true, _) if path.steps.is_empty() => {
                return Err(error(Some(field), "a list column follows an element; its path has none".into()));
            }
            // Without a marker, a list is anchored on its first step.
            (true, anchor) => Some(anchor.unwrap_or(0)),
            (false, None) => None,
        };
        routes.push(FieldRoute {
            source_path: path.steps,
            attribute: path.attribute,
            value,
            field_path: vec![index],
            anchor,
        });
    }
    Ok(LayerSchema { layer: layer.clone(), schema: schema.clone(), routes, decisions: Vec::new() })
}

/// What a column (or a list column's `item`) takes from its element.
fn route_value(field: &Field, item: &Field, list: bool) -> Result<RouteValue, String> {
    if let Some(name) = extension_name(item).filter(|name| name.starts_with("geoarrow.")) {
        return match (name, list) {
            ("geoarrow.box", false) => Ok(RouteValue::BoundingBox),
            ("geoarrow.wkb", _) | (_, false) => Ok(RouteValue::Geometry),
            _ => Err("a geometry list holds WKB (`geometry[]`, List(geoarrow.wkb))".into()),
        };
    }
    match item.data_type() {
        // `bytea`: the geometry at the path as plain WKB (`bytea[]`, a list).
        DataType::Binary => Ok(RouteValue::Geometry),
        DataType::Map(..) => Ok(RouteValue::Map),
        data_type if is_scalar(data_type) => {
            let text_only = field.metadata().get(meta::CONTENT).is_some_and(|content| content == "text");
            Ok(if text_only { RouteValue::InnerText } else { RouteValue::Text })
        }
        DataType::Struct(_) => Err("schemas are flat: a Struct column can't be filled from XML (use one column per leaf path)".into()),
        DataType::List(_) | DataType::LargeList(_) => Err("a list of lists can't be filled from XML: a path has one anchor".into()),
        DataType::Decimal32(..) | DataType::Decimal64(..) | DataType::Decimal128(..) | DataType::Decimal256(..) => {
            Err("decimal types are not supported (docs/type-mapping.md); use double or text".into())
        }
        other => Err(format!("{other} can't be filled from XML")),
    }
}

fn extension_name(field: &Field) -> Option<&str> {
    field.metadata().get("ARROW:extension:name").map(String::as_str)
}

fn is_geoarrow(field: &Field) -> bool {
    extension_name(field).is_some_and(|name| name.starts_with("geoarrow."))
}

/// Types a text value can be parsed into.
pub fn is_scalar(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Null
            | DataType::Boolean
            | DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float32
            | DataType::Float64
            | DataType::Utf8
            | DataType::LargeUtf8
            | DataType::Utf8View
            | DataType::Date32
            | DataType::Date64
            | DataType::Timestamp(..)
            | DataType::Time32(TimeUnit::Second | TimeUnit::Millisecond)
            | DataType::Time64(TimeUnit::Microsecond | TimeUnit::Nanosecond)
    )
}
