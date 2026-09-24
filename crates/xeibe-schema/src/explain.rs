//! Human-readable schema explanation (`xeibe scan --explain`).
//!
//! One block per column: the name, the type and each reason on its own line
//! (`docs/schema-inference.md` §7).

use arrow_schema::{DataType, Field};

use crate::rules::LayerSchema;

pub fn explain(schema: &LayerSchema) -> String {
    let rows: Vec<(String, String, Vec<String>)> = schema
        .schema
        .fields()
        .iter()
        .map(|field| {
            let reasons = schema
                .decisions
                .iter()
                .find(|decision| &decision.field == field.name())
                .map(|decision| decision.reasons.clone())
                .unwrap_or_default();
            (field.name().clone(), short_type(field), reasons)
        })
        .collect();

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

fn extension(field: &Field) -> Option<&str> {
    field.metadata().get("ARROW:extension:name").map(String::as_str)
}

/// The type as `arrow-schema` prints it, but a GeoArrow column by its
/// extension name.
fn short_type(field: &Field) -> String {
    if let Some(name) = extension(field) {
        return name.to_string();
    }
    match field.data_type() {
        DataType::List(item) => match extension(item) {
            Some(name) => format!("List({name})"),
            None => format!("List({})", item.data_type()),
        },
        DataType::Map(..) => "Map(Utf8View → Utf8View)".to_string(),
        data_type => data_type.to_string(),
    }
}
