mod convert;
mod geoparquet;
mod scan;
mod wfs;

use std::path::Path;
use std::str::FromStr;

use arrow_schema::DataType;
use clap::ValueEnum;
use xeibe_arrow::{ReadOptions, ReadReport, Settings};
use xeibe_core::{Dialect, Source};
use xeibe_geom::axis::AxisSelector;
use xeibe_geom::AxisOrderMode;
use xeibe_geom::options::{CurveMode, LinearizeOptions};
use xeibe_schema::options::FieldOverride;
use xeibe_schema::{InferenceOptions, OnSchemaMismatch, PathPattern};

use crate::args::{AxisMode, Cli, Command, InputArgs, Mismatch, Preset, ReadArgs};

pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn run(cli: Cli) -> Result {
    match cli.command {
        Command::Scan {
            input,
            read,
            sample,
            explain,
            output,
        } => scan::run(input, read, sample, explain, output),
        Command::Convert {
            input,
            output,
            read,
        } => convert::run(input, output, read),
        Command::Wfs(command) => wfs::run(command),
    }
}

/// Load the settings file (if any) and apply CLI overrides to its options.
///
/// Without a settings file the CLI starts from the `flat` preset, not the
/// library's `Struct` default (`docs/README.md`, open questions).
pub(crate) fn settings(read: &ReadArgs) -> Result<Settings> {
    let mut settings = match &read.settings {
        Some(path) => Settings::load(path)?,
        None => Settings::new(ReadOptions {
            inference: InferenceOptions::flat(),
            ..ReadOptions::default()
        }),
    };
    apply(read, &mut settings.options)?;
    Ok(settings)
}

/// CLI read parameters on top of `options`.
fn apply(read: &ReadArgs, options: &mut ReadOptions) -> Result {
    if let Some(preset) = read.preset {
        // A preset is a starting point for inference; geometry settings (axis
        // order, CRS, curves) from the file are kept.
        let geometry = options.inference.geometry.clone();
        options.inference = match preset {
            Preset::Default => InferenceOptions::default(),
            Preset::Flat => InferenceOptions::flat(),
            Preset::GdalLike => InferenceOptions::gdal_like(),
            Preset::SparkXmlLike => InferenceOptions::spark_xml_like(),
            Preset::Strings => InferenceOptions::strings(),
        };
        options.inference.geometry = geometry;
    }
    let geometry = &mut options.inference.geometry;
    if let Some(mode) = read.axis_order {
        geometry.axis.mode = axis_mode(mode);
    }
    for text in &read.axis_overrides {
        geometry.axis.overrides.push(axis_override(text)?);
    }
    if let Some(crs) = &read.crs {
        geometry.crs_override = Some(crs.clone());
    }
    if let Some(step) = read.linearize {
        if step.is_nan() || step <= 0.0 {
            return Err(format!("--linearize: the step must be a positive number of degrees, not {step}").into());
        }
        geometry.curves = CurveMode::Linearize(LinearizeOptions {
            max_angle_step_deg: step,
            max_gap: read.arc_max_gap,
        });
    }
    for text in &read.overrides {
        options.inference.overrides.push(field_override(text)?);
    }
    if let Some(mismatch) = read.on_mismatch {
        options.on_mismatch = match mismatch {
            Mismatch::Overflow => OnSchemaMismatch::Overflow,
            Mismatch::Error => OnSchemaMismatch::Error,
            Mismatch::Drop => OnSchemaMismatch::Drop,
        };
    }
    if let Some(features) = read.sample_features {
        options.sample.features_per_layer = features;
    }
    if let Some(threads) = read.threads {
        options.threads = threads.max(1);
    }
    Ok(())
}

fn axis_mode(mode: AxisMode) -> AxisOrderMode {
    match mode {
        AxisMode::XY => AxisOrderMode::XY,
        AxisMode::YX => AxisOrderMode::YX,
        AxisMode::Crs => AxisOrderMode::Crs,
        AxisMode::CrsHeuristic => AxisOrderMode::CrsHeuristic,
        AxisMode::GmlVersion => AxisOrderMode::GmlVersion {
            gml2: Box::new(AxisOrderMode::XY),
            gml3: Box::new(AxisOrderMode::Crs),
        },
        AxisMode::Auto => AxisOrderMode::Auto,
    }
}

/// `selector:mode`, the selector being `key=value` pairs joined by `,`:
/// `srs=EPSG:4326:yx`, `layer=AD_*,dialect=gml2:xy`. The mode follows the
/// last `:`, so srsNames may contain colons.
fn axis_override(text: &str) -> Result<(AxisSelector, AxisOrderMode)> {
    let error = |message: &str| format!("--axis-override {text:?}: {message}");
    let (selector, mode) = text.rsplit_once(':').ok_or_else(|| error("expected selector:mode"))?;
    let mode = AxisMode::from_str(mode, true)
        .map(axis_mode)
        .map_err(|_| error("the mode must be xy, yx, crs, crs-heuristic, gml-version or auto"))?;
    let mut parsed = AxisSelector::default();
    for pair in selector.split(',') {
        let (key, value) = pair.split_once('=').ok_or_else(|| error("expected key=value"))?;
        let value = value.to_string();
        match key.trim() {
            "srs" | "srs_name" => parsed.srs_name = Some(value),
            "layer" => parsed.layer = Some(value),
            "column" => parsed.column = Some(value),
            "source" => parsed.source = Some(value),
            "dialect" => {
                parsed.dialect = Some(match value.to_ascii_lowercase().as_str() {
                    "gml2" => Dialect::Gml2,
                    "gml3" => Dialect::Gml3,
                    _ => return Err(error("dialect must be gml2 or gml3").into()),
                })
            }
            other => return Err(error(&format!("unknown selector key {other:?} (srs, layer, column, source, dialect)")).into()),
        }
    }
    Ok((parsed, mode))
}

/// `pattern=action`: an Arrow type (`utf8`, `Int64`, `List(Utf8View)`), or
/// `drop`, `raw-xml`, `map`, `list`, `scalar`, `rename:<name>`.
/// The action follows the last `=` outside braces, so patterns may hold `{uri}`.
fn field_override(text: &str) -> Result<(PathPattern, FieldOverride)> {
    let error = |message: String| format!("--override {text:?}: {message}");
    let split = text
        .char_indices()
        .scan(0i32, |depth, (i, c)| {
            match c {
                '{' => *depth += 1,
                '}' => *depth -= 1,
                _ => {}
            }
            Some((i, c, *depth))
        })
        .filter(|(_, c, depth)| *c == '=' && *depth == 0)
        .map(|(i, _, _)| i)
        .last()
        .ok_or_else(|| error("expected pattern=type or pattern=action".into()))?;
    let (pattern, action) = (&text[..split], &text[split + 1..]);
    let pattern = PathPattern::parse(pattern).map_err(|e| error(e.to_string()))?;
    let action = match action.to_ascii_lowercase().as_str() {
        "drop" => FieldOverride::Drop,
        "raw-xml" | "rawxml" | "xml" => FieldOverride::AsRawXml,
        "map" => FieldOverride::AsMap,
        "list" => FieldOverride::List,
        "scalar" => FieldOverride::Scalar,
        lower => match lower.strip_prefix("rename:") {
            Some(_) => FieldOverride::Rename(action["rename:".len()..].to_string()),
            None => FieldOverride::Type(data_type(action).map_err(error)?),
        },
    };
    Ok((pattern, action))
}

/// An Arrow type string as `arrow-schema` parses it, plus lower-case
/// spellings of the common scalar types.
fn data_type(text: &str) -> std::result::Result<DataType, String> {
    if let Ok(data_type) = DataType::from_str(text) {
        return Ok(data_type);
    }
    Ok(match text.to_ascii_lowercase().as_str() {
        "utf8" | "string" | "str" | "text" => DataType::Utf8View,
        "int" | "int64" | "integer" | "long" => DataType::Int64,
        "int32" => DataType::Int32,
        "float" | "float64" | "double" => DataType::Float64,
        "bool" | "boolean" => DataType::Boolean,
        "date" | "date32" => DataType::Date32,
        _ => return Err(format!("{text:?} is not an Arrow type or override action")),
    })
}

/// Resolve the inputs. Local zip archives are filtered by `--member`; the
/// rest goes through `xeibe-io` (paths, globs, `-`, URLs).
pub(crate) fn sources(input: &InputArgs) -> Result<Vec<Source>> {
    let mut sources = Vec::new();
    let options = xeibe_io::IoOptions::default();
    for item in &input.inputs {
        let path = Path::new(item);
        let is_zip = item.to_ascii_lowercase().ends_with(".zip") && path.is_file();
        if is_zip && !input.member.is_empty() {
            sources.extend(xeibe_core::archive::expand(path, &input.member)?);
        } else {
            sources.extend(xeibe_io::resolve_sources(std::slice::from_ref(item), &options)?);
        }
    }
    if sources.is_empty() {
        return Err("no input files found".into());
    }
    Ok(sources)
}

/// Warnings, skipped features and overflow of a finished read, on stderr.
pub(crate) fn print_report(report: &ReadReport) {
    for warning in &report.warnings {
        match &warning.location {
            Some(location) => eprintln!("warning: {location}: {}", warning.message),
            None => eprintln!("warning: {}", warning.message),
        }
    }
    for (location, reason) in &report.skipped {
        eprintln!("skipped: {location}: {reason}");
    }
    let overflow: u64 = report.overflow_per_path.values().sum();
    if overflow > 0 {
        eprintln!("{overflow} values outside the schema went to _overflow:");
        for (path, count) in &report.overflow_per_path {
            eprintln!("  {path}: {count}");
        }
    }
}
