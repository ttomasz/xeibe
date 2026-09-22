use std::io::Write;
use std::path::PathBuf;

use xeibe_arrow::ColumnSpec;
use xeibe_schema::ScanExtent;

use crate::args::{InputArgs, ReadArgs};

/// Prints axis-order conflicts first, then the layer summary (and `--explain`);
/// with `-o`, writes the settings file.
pub fn run(
    input: InputArgs,
    read: ReadArgs,
    sample: Option<u64>,
    explain: bool,
    output: Option<PathBuf>,
) -> super::Result {
    let settings = super::settings(&read)?;
    let sources = super::sources(&input)?;
    let names: Vec<String> = sources.iter().map(|source| source.name()).collect();
    let extent = match sample {
        Some(max_features) => ScanExtent::Sample { max_features },
        None => ScanExtent::Full,
    };
    let scan = xeibe_arrow::scan(sources, extent, &settings.options)?;

    for (layer, key, decision) in scan.axis_decisions() {
        if decision.conflicts.is_empty() {
            continue;
        }
        let source = names.get(key.source.0 as usize).cloned().unwrap_or_else(|| format!("source #{}", key.source.0));
        let srs = key.srs_name.as_deref().unwrap_or("(no srsName)");
        let order = if decision.swap { "y/x (swapped)" } else { "x/y" };
        eprintln!("axis-order conflict: {source}, layer {}, srsName {srs} ({:?}):", layer.local, key.dialect);
        eprintln!("  decided {order}: {}", decision.reason);
        for conflict in &decision.conflicts {
            eprintln!("  against: {conflict}");
        }
    }

    let saved = scan.to_settings()?;
    let mut out = std::io::stdout().lock();
    let layers = scan.layers();
    if layers.is_empty() {
        writeln!(out, "no layers found")?;
    }
    if !scan.is_complete() {
        writeln!(out, "sampled scan: layers starting after the sample are missing; counts are lower bounds")?;
    }
    for (info, (name, columns)) in layers.iter().zip(&saved.layers) {
        let mut line = format!("{name}: {} features", info.feature_count);
        if !info.geometry_columns.is_empty() {
            line.push_str(&format!(", geometry {}", info.geometry_columns.join(", ")));
        }
        if !info.crs.is_empty() {
            line.push_str(&format!(", crs {}", info.crs.join(", ")));
        }
        if let Some([x0, y0, x1, y1]) = info.extent {
            line.push_str(&format!(", extent ({x0}, {y0}) - ({x1}, {y1}) as written"));
        }
        writeln!(out, "{line}")?;
        if explain {
            for row in scan.explain(&info.name.to_clark())?.lines() {
                writeln!(out, "  {row}")?;
            }
        } else {
            let width = columns.keys().map(|column| column.chars().count()).max().unwrap_or(0);
            for (column, spec) in columns {
                let data_type = match spec {
                    ColumnSpec::Type(data_type) | ColumnSpec::Detailed { data_type, .. } => data_type,
                };
                writeln!(out, "  {column:<width$}  {data_type}")?;
            }
        }
    }

    if let Some(path) = output {
        saved.save(&path)?;
        eprintln!("settings written to {}", path.display());
    }
    Ok(())
}
