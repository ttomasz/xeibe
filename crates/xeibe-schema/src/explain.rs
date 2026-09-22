//! Human-readable schema explanation (`xeibe scan --explain`).
//!
//! One block per field, nested fields with their dotted names: the name, the
//! type and each reason on its own line (`docs/schema-inference.md` §7).

use arrow_schema::{DataType, Field};

use crate::rules::LayerSchema;

pub fn explain(schema: &LayerSchema) -> String {
    let mut rows: Vec<(String, String, Vec<String>)> = Vec::new();
    let mut types = Vec::new();
    for field in schema.schema.fields() {
        collect_types(field, "", &mut types);
    }
    for (name, data_type) in types {
        let reasons = schema
            .decisions
            .iter()
            .find(|decision| decision.field == name)
            .map(|decision| decision.reasons.clone())
            .unwrap_or_default();
        rows.push((name, data_type, reasons));
    }

    let name_width = rows.iter().map(|(name, _, _)| name.chars().count()).max().unwrap_or(0);
    let type_width = rows.iter().map(|(_, t, _)| t.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for (name, data_type, reasons) in rows {
        let first = reasons.first().map(String::as_str).unwrap_or("");
        let line = format!("{name:<name_width$}  {data_type:<type_width$}  {first}");
        out.push_str(line.trim_end());
        out.push('\n');
        for reason in reasons.iter().skip(1) {
            out.push_str(&format!("{:<name_width$}  {:<type_width$}  {reason}\n", "", ""));
        }
    }
    out
}

/// `(dotted name, short type)` for a field and, depth first, its nested fields.
fn collect_types(field: &Field, prefix: &str, out: &mut Vec<(String, String)>) {
    let name = format!("{prefix}{}", field.name());
    out.push((name.clone(), short_type(field)));
    let children = match field.data_type() {
        DataType::Struct(children) => Some(children),
        DataType::List(item) | DataType::LargeList(item) => match item.data_type() {
            DataType::Struct(children) if extension(item).is_none() => Some(children),
            _ => None,
        },
        _ => None,
    };
    if extension(field).is_some() {
        return;
    }
    if let Some(children) = children {
        // Decisions name nested fields with the separator the schema used;
        // `.` is the default and what the emitter writes.
        for child in children {
            collect_types(child, &format!("{name}."), out);
        }
    }
}

fn extension(field: &Field) -> Option<&str> {
    field.metadata().get("ARROW:extension:name").map(String::as_str)
}

/// The type as `arrow-schema` prints it, but a GeoArrow column by its
/// extension name and a struct as `Struct(…)` (its fields follow).
fn short_type(field: &Field) -> String {
    if let Some(name) = extension(field) {
        return name.to_string();
    }
    match field.data_type() {
        DataType::Struct(_) => "Struct(…)".to_string(),
        DataType::List(item) => match (extension(item), item.data_type()) {
            (Some(name), _) => format!("List({name})"),
            (None, DataType::Struct(_)) => "List(Struct(…))".to_string(),
            (None, data_type) => format!("List({data_type})"),
        },
        DataType::Map(..) => "Map(Utf8View → Utf8View)".to_string(),
        data_type => data_type.to_string(),
    }
}
