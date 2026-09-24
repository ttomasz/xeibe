mod convert;
mod geoparquet;
mod scan;
mod wfs;

use std::path::Path;

use arrow_schema::DataType;
use clap::ValueEnum;
use xeibe_arrow::{ReadOptions, ReadReport, Settings};
use xeibe_core::Source;
use xeibe_geom::AxisOrderMode;
use xeibe_geom::options::{CurveMode, LinearizeOptions};
use xeibe_schema::options::FieldOverride;
use xeibe_schema::{InferenceOptions, PathPattern};

use crate::args::{AxisMode, Cli, Command, InputArgs, Preset, ReadArgs};

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
pub(crate) fn settings(read: &ReadArgs) -> Result<Settings> {
    let mut settings = match &read.settings {
        Some(path) => Settings::load(path)?,
        None => Settings::new(ReadOptions::default()),
    };
    apply(read, &mut settings.options)?;
    Ok(settings)
}

/// CLI read parameters on top of `options`.
fn apply(read: &ReadArgs, options: &mut ReadOptions) -> Result {
    if let Some(preset) = read.preset {
        // A preset is a starting point for inference; geometry settings (axis
        // order, CRS, curves) are read options and stay as they are.
        options.inference = match preset {
            Preset::Default => InferenceOptions::default(),
            Preset::Strings => InferenceOptions::strings(),
        };
    }
    let geometry = &mut options.geometry;
    if let Some(mode) = read.axis_order {
        geometry.axis.mode = axis_mode(mode);
    }
    for text in &read.axis_overrides {
        let (srs_name, mode) = axis_override(text)?;
        geometry.axis.overrides.insert(srs_name, mode);
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

/// `srsName=mode` (`EPSG:4326=yx`): the srsName exactly as written. The mode
/// follows the last `=`, so srsNames (URLs) may contain `=`.
fn axis_override(text: &str) -> Result<(String, AxisOrderMode)> {
    let error = |message: &str| format!("--axis-override {text:?}: {message}");
    let (srs_name, mode) = text.rsplit_once('=').ok_or_else(|| error("expected srsName=mode, e.g. EPSG:4326=yx"))?;
    if srs_name.is_empty() {
        return Err(error("the srsName is empty").into());
    }
    let mode = AxisMode::from_str(mode.trim(), true)
        .map(axis_mode)
        .map_err(|_| error("the mode must be xy, yx, crs, crs-heuristic, gml-version or auto"))?;
    Ok((srs_name.to_string(), mode))
}

/// `pattern=action`: a type (`text`, `bigint`, `Int64`, `List(Utf8View)`), or
/// `drop`, `raw-xml`, `map`, `list`, `scalar`.
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
        _ => FieldOverride::Type(data_type(action).map_err(error)?),
    };
    Ok((pattern, action))
}

/// A type as the settings file writes it (`text`, `bigint`, `Int64`, …).
fn data_type(text: &str) -> std::result::Result<DataType, String> {
    // `utf8` and `int64` have been accepted before the aliases existed.
    let text = match text.to_ascii_lowercase().as_str() {
        "utf8" | "str" => "text".to_string(),
        "int64" | "long" => "bigint".to_string(),
        "float64" | "float" => "double".to_string(),
        _ => text.to_string(),
    };
    xeibe_arrow::ColumnSpec::Type(text.clone())
        .to_field("override")
        .map(|field| field.data_type().clone())
        .map_err(|e| e.to_string())
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

/// Warnings and skipped features of a finished read, on stderr.
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
}
