use xeibe_wfs::pages::Progress;
use xeibe_wfs::{WfsClient, WfsOptions};

use crate::args::WfsCommand;

pub fn run(command: WfsCommand) -> super::Result {
    match command {
        WfsCommand::Layers { url } => layers(&url),
        WfsCommand::Count { url, type_name } => {
            let client = WfsClient::new(&url, WfsOptions::default())?;
            match client.count(&type_name)? {
                Some(count) => println!("{count}"),
                None => println!("unknown"),
            }
            Ok(())
        }
        WfsCommand::Convert {
            url,
            type_name,
            page_size,
            srs_name,
            sort_by,
            params,
            output,
            read,
        } => {
            let vendor_params = params
                .iter()
                .map(|param| {
                    param
                        .split_once('=')
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .ok_or_else(|| format!("--param {param:?}: expected key=value"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let options = WfsOptions { page_size, srs_name, sort_by, vendor_params, ..WfsOptions::default() };
            let client = WfsClient::new(&url, options)?;
            let settings = super::settings(&read)?;
            // `--layer` names the layer in the settings file when it differs
            // from the type name (another prefix, say).
            let layer = output.layer.clone().unwrap_or_else(|| type_name.clone());
            let schema = super::convert::layer_schema(&settings, &layer)?;
            let sources = client.pages(&type_name, Box::new(progress))?;
            let mut reader = xeibe_arrow::read(sources, &type_name, schema, &settings.options)?;
            let primary = settings.options.inference.geometry.primary.clone();
            let rows = super::convert::write(&mut reader, &output, &type_name, primary.as_deref())?;
            super::print_report(&reader.report());
            eprintln!("{rows} rows written to {}", output.output.display());
            Ok(())
        }
    }
}

/// One line per feature type: name, title, default CRS, WGS 84 bbox.
fn layers(url: &str) -> super::Result {
    let client = WfsClient::new(url, WfsOptions::default())?;
    let capabilities = client.capabilities()?;
    println!("WFS {}{}", capabilities.version.as_str(), capabilities.service_title.as_deref().map(|t| format!(": {t}")).unwrap_or_default());
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
        println!("{line}");
    }
    Ok(())
}

/// Page progress and paging warnings, on stderr.
fn progress(progress: &Progress) {
    for warning in &progress.warnings {
        eprintln!("warning: {warning}");
    }
    match progress.number_matched {
        Some(total) => eprintln!("page {}: {} of {total} features", progress.pages, progress.features),
        None => eprintln!("page {}: {} features", progress.pages, progress.features),
    }
}
