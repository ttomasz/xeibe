use std::io::Write;
use std::sync::Arc;

use xeibe_core::QName;
use xeibe_schema::SchemaHint;
use xeibe_schema::hint::DeclaredProperty;
use xeibe_wfs::pages::Progress;
use xeibe_wfs::{FeatureTypeSchema, WfsClient, WfsOptions};

use crate::args::WfsCommand;

pub fn run(command: WfsCommand) -> super::Result {
    match command {
        WfsCommand::Layers { service } => layers(&service.url),
        WfsCommand::Count {
            service,
            feature_type,
        } => {
            let client = WfsClient::new(&service.url, WfsOptions::default())?;
            let mut out = std::io::stdout().lock();
            match client.count(&feature_type.type_name)? {
                Some(count) => writeln!(out, "{count}")?,
                None => writeln!(out, "unknown")?,
            }
            Ok(())
        }
        WfsCommand::Convert {
            service,
            feature_type,
            page_size,
            srs_name,
            sort_by,
            params,
            layer,
            output,
            schema,
            read,
        } => {
            let type_name = feature_type.type_name;
            let vendor_params = params
                .iter()
                .map(|param| {
                    param
                        .split_once('=')
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .ok_or_else(|| format!("--param {param:?}: expected key=value"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let options = WfsOptions {
                page_size,
                srs_name,
                sort_by: sort_by.map(|keys| keys.0).unwrap_or_default(),
                vendor_params,
                ..WfsOptions::default()
            };
            let sort_by = options.sort_by.clone();
            let client = WfsClient::new(&service.url, options)?;
            // `--layer` names the layer in the settings file when it differs
            // from the type name (another prefix, say).
            let layer = layer.unwrap_or_else(|| type_name.clone());
            // In the order the server sends: the download sets the pace, so
            // keeping it costs nothing, and `--sort-by` asks for it.
            let (mut settings, schema) = super::convert::settings(&schema, &read, true, &layer)?;
            // An unknown type fails here, before anything else is requested.
            let sources = client.pages(&type_name, Box::new(progress))?;
            // Without a schema, the type's declared properties fill what the
            // sample can't show. Without them, the sample alone decides.
            if schema.is_none() {
                match client.describe(&type_name) {
                    Ok(declared) => {
                        for property in undeclared_sort_keys(&sort_by, &declared) {
                            eprintln!(
                                "warning: --sort-by {property}: the DescribeFeatureType schema declares no such property; the server may reject or ignore the sort"
                            );
                        }
                        settings.options.inference.hint = Some(Arc::new(hint(declared)))
                    }
                    Err(error) => eprintln!(
                        "warning: {error}; the columns come from the sampled features alone"
                    ),
                }
            }
            let mut reader = xeibe_arrow::read(sources, &type_name, schema, &settings.options)?;
            let primary = settings.options.geometry.primary.clone();
            let rows = super::convert::write(&mut reader, &output, &type_name, primary.as_deref())?;
            super::print_report(&reader.report());
            eprintln!("{rows} rows written to {}", output.output.display());
            Ok(())
        }
    }
}

/// Sort properties the type's schema doesn't declare, by the local name of
/// their first step (`app:address/app:street` → `address`). GML's own
/// properties (`gml:name`, …) and attributes (`@gml:id`) come from base types
/// outside the schema, so they are not checked.
fn undeclared_sort_keys<'a>(
    keys: &'a [(String, xeibe_wfs::request::SortOrder)],
    declared: &FeatureTypeSchema,
) -> Vec<&'a str> {
    keys.iter()
        .map(|(property, _)| property.as_str())
        .filter(|property| {
            let step = property.split('/').next().unwrap_or_default();
            let (prefix, local) = step.split_once(':').unwrap_or(("", step));
            !(step.starts_with('@')
                || prefix == "gml"
                || declared.properties.iter().any(|p| p.name == local))
        })
        .collect()
}

/// The schema hint for inference: the type's direct properties.
fn hint(declared: FeatureTypeSchema) -> SchemaHint {
    let qname = |ns: Option<String>, local: String| QName::new(ns.as_deref(), &local);
    SchemaHint {
        layer: qname(declared.namespace, declared.name),
        source: "the DescribeFeatureType schema".to_string(),
        properties: declared
            .properties
            .into_iter()
            .map(|property| DeclaredProperty {
                name: qname(property.namespace, property.name),
                type_name: property.type_name.map(|(ns, local)| qname(ns, local)),
                repeated: property.repeated,
            })
            .collect(),
    }
}

/// One line per feature type: name, title, default CRS, WGS 84 bbox.
fn layers(url: &str) -> super::Result {
    let client = WfsClient::new(url, WfsOptions::default())?;
    let capabilities = client.capabilities()?;
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "WFS {}{}",
        capabilities.version.as_str(),
        capabilities
            .service_title
            .as_deref()
            .map(|t| format!(": {t}"))
            .unwrap_or_default()
    )?;
    for feature_type in &capabilities.feature_types {
        let mut line = feature_type.name.clone();
        if let Some(title) = &feature_type.title {
            line.push_str(&format!("  \"{title}\""));
        }
        if let Some(crs) = &feature_type.default_crs {
            line.push_str(&format!("  crs {crs}"));
            if !feature_type.other_crs.is_empty() {
                line.push_str(&format!(" (also {})", feature_type.other_crs.join(", ")));
            }
        }
        if let Some([x0, y0, x1, y1]) = feature_type.wgs84_bbox {
            line.push_str(&format!("  bbox lon/lat ({x0}, {y0}) - ({x1}, {y1})"));
        }
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// Page progress and paging warnings, on stderr.
fn progress(progress: &Progress) {
    for warning in &progress.warnings {
        eprintln!("warning: {warning}");
    }
    match progress.number_matched {
        Some(total) => eprintln!(
            "page {}: {} of {total} features",
            progress.pages, progress.features
        ),
        None => eprintln!("page {}: {} features", progress.pages, progress.features),
    }
}
