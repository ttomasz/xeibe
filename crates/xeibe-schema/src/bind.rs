//! Binding a given schema (settings file or Arrow schema) to XML paths.
//!
//! Columns are matched by name, the way spark-xml applies a user schema (see
//! `docs/architecture.md#settings-file`): XML names are mapped to column names
//! with `NamingOptions`; type-wrapper elements without a column of their own are
//! looked through; a `gml:path` field-metadata entry wins over the name. Anything
//! unmatched is routed to `_overflow` (or dropped / an error) at read time.
//!
//! Nothing of the data is known here, so the routes name elements by local
//! name only unless the column says otherwise (Clark notation, or a prefix
//! declared in `gml:ns`); see [`LayerSchema::match_by_name`].

use arrow_schema::{DataType, Field, Schema};
use xeibe_core::QName;

use crate::rules::{FieldRoute, RouteValue, meta};
use crate::{InferenceOptions, LayerSchema};

/// The column that collects data outside the schema (`OnSchemaMismatch::Overflow`).
pub const OVERFLOW_COLUMN: &str = "_overflow";

pub fn bind_schema(layer: &QName, schema: &Schema, options: &InferenceOptions) -> crate::Result<LayerSchema> {
    let options = options.for_layer(&layer.to_clark());
    let binder = Binder { layer, options: &options };
    let mut routes = Vec::new();
    for (index, field) in schema.fields().iter().enumerate() {
        if field.name() == OVERFLOW_COLUMN {
            if !matches!(field.data_type(), DataType::Map(..)) {
                return Err(binder.error(field, "`_overflow` must be a Map(Utf8View → Utf8View)"));
            }
            routes.push(FieldRoute {
                source_path: Vec::new(),
                attribute: None,
                value: RouteValue::Overflow,
                field_path: vec![index],
            });
            continue;
        }
        binder.bind(field, &[], vec![index], &mut routes)?;
    }
    Ok(LayerSchema {
        layer: layer.clone(),
        schema: schema.clone(),
        routes,
        decisions: Vec::new(),
        match_by_name: true,
    })
}

struct Binder<'a> {
    layer: &'a QName,
    options: &'a InferenceOptions,
}

/// Where a column's values are in the XML.
struct Source {
    elements: Vec<QName>,
    attribute: Option<QName>,
    offset: bool,
}

impl Binder<'_> {
    fn error(&self, field: &Field, message: &str) -> crate::Error {
        crate::Error::Bind {
            layer: self.layer.to_string(),
            message: format!("column {:?}: {message}", field.name()),
        }
    }

    /// Bind `field`, whose parent element is at `parent` (feature = empty).
    fn bind(
        &self,
        field: &Field,
        parent: &[QName],
        field_path: Vec<usize>,
        routes: &mut Vec<FieldRoute>,
    ) -> crate::Result<()> {
        let source = match field.metadata().get(meta::PATH) {
            Some(path) => self.parse_path(path, field.metadata().get(meta::NS).map(String::as_str)),
            None => self.parse_name(field.name(), parent),
        };
        let mut data_type = field.data_type();
        let mut item = field;
        if let DataType::List(inner) | DataType::LargeList(inner) | DataType::ListView(inner)
        | DataType::LargeListView(inner) | DataType::FixedSizeList(inner, _) = data_type
        {
            item = inner.as_ref();
            data_type = inner.data_type();
        }

        let value = if is_geoarrow(field) || is_geoarrow(item) {
            let name = extension_name(field).or_else(|| extension_name(item)).unwrap_or_default();
            if name == "geoarrow.box" { RouteValue::BoundingBox } else { RouteValue::Geometry }
        } else if source.offset {
            if !is_integer(data_type) {
                return Err(self.error(field, "an offset column must be an integer"));
            }
            RouteValue::OffsetMinutes
        } else {
            match data_type {
                DataType::Struct(_) => RouteValue::Struct,
                DataType::Map(..) => RouteValue::Map,
                data_type if is_scalar(data_type) => RouteValue::Text,
                DataType::Decimal32(..) | DataType::Decimal64(..) | DataType::Decimal128(..)
                | DataType::Decimal256(..) => {
                    return Err(self.error(
                        field,
                        "decimal types are not supported (docs/type-mapping.md); use Float64 or Utf8View",
                    ));
                }
                other => {
                    return Err(self.error(field, &format!("{other} can't be filled from XML")));
                }
            }
        };
        // The container's route comes before its children's.
        routes.push(FieldRoute {
            source_path: source.elements.clone(),
            attribute: source.attribute,
            value,
            field_path: field_path.clone(),
        });
        if let (RouteValue::Struct, DataType::Struct(children)) = (value, data_type) {
            for (index, child) in children.iter().enumerate() {
                let mut child_path = field_path.clone();
                child_path.push(index);
                self.bind(child, &source.elements, child_path, routes)?;
            }
        }
        Ok(())
    }

    /// A column name relative to its parent element: `area`, `@id`,
    /// `idIIP.lokalnyId` (flattened), `area.@uom`, `#text`, `t.@offset_min`.
    fn parse_name(&self, name: &str, parent: &[QName]) -> Source {
        let naming = &self.options.naming;
        let mut source = Source { elements: parent.to_vec(), attribute: None, offset: false };
        let separator = naming.flatten_separator.as_str();
        let steps: Vec<&str> = if separator.is_empty() || !name.contains(separator) || name.starts_with('{') {
            vec![name]
        } else {
            name.split(separator).collect()
        };
        for step in steps {
            if step == naming.text_field {
                continue;
            }
            if let Some(attribute) = step.strip_prefix(naming.attribute_prefix.as_str()).filter(|_| !naming.attribute_prefix.is_empty()) {
                if attribute == "offset_min" {
                    source.offset = true;
                } else {
                    source.attribute = Some(name_of(attribute, None));
                }
                continue;
            }
            source.elements.push(name_of(step, None));
        }
        source
    }

    /// A `gml:path` (`prgad:idIIP/prgad:lokalnyId`, `…/@uom`), from the feature.
    fn parse_path(&self, path: &str, namespaces: Option<&str>) -> Source {
        let namespaces: serde_json::Map<String, serde_json::Value> = namespaces
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        let mut source = Source { elements: Vec::new(), attribute: None, offset: false };
        for step in path.split('/').filter(|step| !step.is_empty()) {
            match step.strip_prefix('@') {
                Some(attribute) => source.attribute = Some(name_of(attribute, Some(&namespaces))),
                None => source.elements.push(name_of(step, Some(&namespaces))),
            }
        }
        source
    }
}

/// `{uri}local` → exact; `prefix:local` → the prefix's URI from `gml:ns` if
/// known, else any namespace; `local` → any namespace.
fn name_of(step: &str, namespaces: Option<&serde_json::Map<String, serde_json::Value>>) -> QName {
    if let Some((uri, local)) = step.strip_prefix('{').and_then(|rest| rest.split_once('}')) {
        return QName::new(Some(uri), local);
    }
    match step.split_once(':') {
        Some((prefix, local)) => {
            let uri = namespaces
                .and_then(|namespaces| namespaces.get(prefix))
                .and_then(|uri| uri.as_str());
            QName::new(uri, local)
        }
        None => QName::new(None, step),
    }
}

fn extension_name(field: &Field) -> Option<&str> {
    field.metadata().get("ARROW:extension:name").map(String::as_str)
}

fn is_geoarrow(field: &Field) -> bool {
    extension_name(field).is_some_and(|name| name.starts_with("geoarrow."))
}

fn is_integer(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64
            | DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64
    )
}

/// Types a text value can be parsed into.
fn is_scalar(data_type: &DataType) -> bool {
    is_integer(data_type)
        || matches!(
            data_type,
            DataType::Null
                | DataType::Boolean
                | DataType::Float16
                | DataType::Float32
                | DataType::Float64
                | DataType::Utf8
                | DataType::LargeUtf8
                | DataType::Utf8View
                | DataType::Date32
                | DataType::Date64
                | DataType::Timestamp(..)
                | DataType::Time32(..)
                | DataType::Time64(..)
        )
}
