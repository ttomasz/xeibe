use std::path::Path;
use std::sync::Arc;

use datafusion::arrow::array::{Array, AsArray};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, SchemaRef};
use datafusion::catalog::{TableFunctionArgs, TableFunctionImpl, TableProvider};
use datafusion::common::{ScalarValue, plan_datafusion_err, plan_err};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use xeibe_arrow::{ReadOptions, Settings};
use xeibe_geom::AxisOrderMode;
use xeibe_schema::{InferenceOptions, OnSchemaMismatch};

use crate::sources::{blocking, external};
use crate::table::{GmlTable, sample_schema, with_overflow};

/// `read_gml('path/*.gml', 'Name' [, 'settings=file.json'] [, 'preset=flat'] [, 'axis_order=auto'])`.
/// Without a schema for the layer in `settings`, the schema is sampled.
///
/// DataFusion's SQL planner does not pass named arguments (`layer => 'Name'`)
/// to table functions, so the path and the layer are positional and options
/// are `'key=value'` strings: `settings`, `preset` (`default`, `flat`,
/// `gdal_like`, `spark_xml_like`, `strings`), `axis_order` (`xy`, `yx`, `crs`,
/// `crs_heuristic`, `gml_version`, `auto`), `crs`, `on_mismatch` (`overflow`,
/// `error`, `drop`), `sample_features` and `threads`. A third argument without
/// `=` is the settings file. The path may also be an array of paths.
#[derive(Debug, Default)]
pub struct ReadGmlFunction;

impl TableFunctionImpl for ReadGmlFunction {
    fn call_with_args(&self, args: TableFunctionArgs) -> Result<Arc<dyn TableProvider>> {
        let exprs = args.exprs();
        let (Some(paths), Some(layer)) = (exprs.first(), exprs.get(1)) else {
            return plan_err!("read_gml(path, layer [, 'key=value' ...]) needs a path and a layer");
        };
        let paths = strings(paths)?;
        let layer = string(layer)?;

        let mut pairs = Vec::new();
        for (index, expr) in exprs.iter().enumerate().skip(2) {
            let option = string(expr)?;
            match option.split_once('=') {
                Some((key, value)) => pairs.push((key.trim().to_string(), value.trim().to_string())),
                None if index == 2 => pairs.push(("settings".to_string(), option)),
                None => return plan_err!("read_gml: expected 'key=value', got {option:?}"),
            }
        }
        let (schema, options) = read_options(&layer, &pairs)?;

        let table = match schema {
            Some(schema) => GmlTable::with_schema(paths, &layer, with_overflow(schema, &options), options),
            None => {
                let runtime = args.session().runtime_env().clone();
                let schema = blocking(&paths, |handle| sample_schema(&runtime, &paths, &layer, &options, handle))?;
                GmlTable::with_schema(paths, &layer, schema, options)
            }
        };
        Ok(Arc::new(table))
    }
}

/// The settings file's options (and the layer's schema, if it has one), then
/// the other options over them.
fn read_options(layer: &str, pairs: &[(String, String)]) -> Result<(Option<SchemaRef>, ReadOptions)> {
    let mut schema = None;
    let mut options = ReadOptions::default();
    if let Some((_, path)) = pairs.iter().find(|(key, _)| key == "settings") {
        let settings = Settings::load(Path::new(path)).map_err(external)?;
        schema = match settings.schema(layer) {
            Ok(schema) => Some(schema),
            Err(xeibe_arrow::Error::Schema(xeibe_schema::Error::UnknownLayer(_))) => None,
            Err(error) => return Err(external(error)),
        };
        options = settings.options;
    }
    for (key, value) in pairs {
        let bad = || plan_datafusion_err!("read_gml: invalid {key} {value:?}");
        match key.as_str() {
            "settings" => {}
            "preset" => {
                options.inference = match value.replace('-', "_").as_str() {
                    "default" => InferenceOptions::default(),
                    "flat" => InferenceOptions::flat(),
                    "gdal_like" => InferenceOptions::gdal_like(),
                    "spark_xml_like" => InferenceOptions::spark_xml_like(),
                    "strings" => InferenceOptions::strings(),
                    _ => return Err(bad()),
                };
            }
            "axis_order" => {
                options.geometry.axis.mode = match value.replace('-', "_").as_str() {
                    "xy" => AxisOrderMode::XY,
                    "yx" => AxisOrderMode::YX,
                    "crs" => AxisOrderMode::Crs,
                    "crs_heuristic" => AxisOrderMode::CrsHeuristic,
                    "gml_version" => AxisOrderMode::GmlVersion {
                        gml2: Box::new(AxisOrderMode::XY),
                        gml3: Box::new(AxisOrderMode::Crs),
                    },
                    "auto" => AxisOrderMode::Auto,
                    _ => return Err(bad()),
                }
            }
            "crs" => options.geometry.crs_override = Some(value.clone()),
            "on_mismatch" => {
                options.on_mismatch = match value.as_str() {
                    "overflow" => OnSchemaMismatch::Overflow,
                    "error" => OnSchemaMismatch::Error,
                    "drop" => OnSchemaMismatch::Drop,
                    _ => return Err(bad()),
                }
            }
            "sample_features" => options.sample.features_per_layer = value.parse().map_err(|_| bad())?,
            "threads" => options.threads = value.parse().map_err(|_| bad())?,
            _ => return plan_err!("read_gml: unknown option {key:?}"),
        }
    }
    Ok((schema, options))
}

fn string(expr: &Expr) -> Result<String> {
    match expr {
        Expr::Literal(
            ScalarValue::Utf8(Some(value)) | ScalarValue::Utf8View(Some(value)) | ScalarValue::LargeUtf8(Some(value)),
            _,
        ) => Ok(value.clone()),
        _ => plan_err!("read_gml: expected a string literal, got {expr}"),
    }
}

/// A string, or an array of strings (`['a.gml', 'b.gml']`).
fn strings(expr: &Expr) -> Result<Vec<String>> {
    match expr {
        Expr::Literal(ScalarValue::List(list), _) if list.len() == 1 => {
            let values = cast(&list.value(0), &DataType::Utf8)?;
            let values = values.as_string::<i32>();
            (0..values.len())
                .map(|i| match values.is_valid(i) {
                    true => Ok(values.value(i).to_string()),
                    false => plan_err!("read_gml: a path is null"),
                })
                .collect()
        }
        Expr::ScalarFunction(function) if function.name() == "make_array" => {
            function.args.iter().map(string).collect()
        }
        _ => Ok(vec![string(expr)?]),
    }
}
