//! The native part of the Python package `xeibe` (`xeibe._xeibe`): `scan()`
//! and `read()`. Arrow objects are arro3's (`arro3.core.Schema`,
//! `arro3.core.RecordBatchReader`), made with `pyo3-arrow`; they implement the
//! Arrow PyCapsule interface, so they can be passed to PyArrow, GeoPandas
//! (`from_arrow`), DuckDB, Polars or SedonaDB.
//! The package's Python modules (`python/xeibe`) re-export them and add the
//! SedonaDB data source.

use std::path::PathBuf;
use std::sync::Arc;

use arrow_array::RecordBatchReader;
use arrow_schema::SchemaRef;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyUserWarning, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use pyo3_arrow::{PyRecordBatchReader, PySchema};
use xeibe_arrow::{ReadOptions, Settings};
use xeibe_core::Source;
use xeibe_io::IoOptions;
use xeibe_schema::ScanExtent;

create_exception!(
    xeibe,
    XeibeError,
    PyException,
    "A GML input could not be scanned or read."
);

create_exception!(
    xeibe,
    UnknownLayerError,
    XeibeError,
    "The layer is not in the input (or not in the settings)."
);

fn error(error: impl std::fmt::Display) -> PyErr {
    XeibeError::new_err(error.to_string())
}

create_exception!(
    xeibe,
    NoFeaturesError,
    XeibeError,
    "No input holds a feature collection or feature member: none is GML."
);

create_exception!(
    xeibe,
    SkippedSourceWarning,
    PyUserWarning,
    "A source without a feature collection or feature member was skipped: it isn't GML."
);

/// [`error`], but an unknown layer is an [`UnknownLayerError`] and an input
/// without features a [`NoFeaturesError`].
fn read_error(error: xeibe_arrow::Error) -> PyErr {
    match error {
        xeibe_arrow::Error::Schema(xeibe_schema::Error::UnknownLayer(_)) => {
            UnknownLayerError::new_err(error.to_string())
        }
        error if is_no_features(&error) => NoFeaturesError::new_err(error.to_string()),
        error => XeibeError::new_err(error.to_string()),
    }
}

/// No input holds a feature, as a read or a scan reports it.
fn is_no_features(error: &xeibe_arrow::Error) -> bool {
    matches!(
        error,
        xeibe_arrow::Error::Core(xeibe_core::Error::NoFeatures(_))
            | xeibe_arrow::Error::Schema(xeibe_schema::Error::Core(xeibe_core::Error::NoFeatures(
                _
            )))
    )
}

/// `scan(paths, sample=None, options=None) -> Scan`
#[pyfunction]
#[pyo3(signature = (paths, sample = None, options = None))]
fn scan(
    py: Python<'_>,
    paths: Vec<String>,
    sample: Option<u64>,
    options: Option<Bound<'_, PyAny>>,
) -> PyResult<Scan> {
    let options = match &options {
        Some(options) => parse_options(options)?,
        None => ReadOptions::default(),
    };
    let extent = match sample {
        Some(max_features) => ScanExtent::Sample { max_features },
        None => ScanExtent::Full,
    };
    let inner = py.detach(|| {
        let sources = resolve(&paths)?;
        xeibe_arrow::scan(sources, extent, &options).map_err(read_error)
    })?;
    Ok(Scan { inner })
}

/// `read(paths, layer, schema=None, options=None) ->
/// arro3.core.RecordBatchReader`, a one-shot stream.
/// `schema`: an Arrow schema (anything with `__arrow_c_schema__`), or
/// settings (see [`load_settings`]): a whole settings file, or the columns
/// of this one layer.
/// `options`: a dict or JSON text (the `options` section) or settings.
///
/// Whole settings given as `schema` also supply the options when `options`
/// is `None`.
///
/// Sources without features (not GML) are skipped, and each one skipped is a
/// [`SkippedSourceWarning`] when the stream ends. If every source is, the
/// read fails with a [`NoFeaturesError`]. Without a schema, the layer's first
/// features are sampled before this returns.
#[pyfunction]
#[pyo3(signature = (paths, layer, schema = None, options = None))]
fn read<'py>(
    py: Python<'py>,
    paths: Vec<String>,
    layer: &str,
    schema: Option<Bound<'py, PyAny>>,
    options: Option<Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    read_layer(py, paths, layer, schema, options, false)
}

/// `read_or_empty(paths, layer, schema, options=None)`: [`read`], but an input
/// in which every source is skipped is an empty stream of `schema` instead
/// of a [`NoFeaturesError`]. For data sources (`xeibe.sedona`) that must
/// answer for every file they are given, GML or not. Not public API.
#[pyfunction]
#[pyo3(signature = (paths, layer, schema, options = None))]
fn read_or_empty<'py>(
    py: Python<'py>,
    paths: Vec<String>,
    layer: &str,
    schema: Bound<'py, PyAny>,
    options: Option<Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    read_layer(py, paths, layer, Some(schema), options, true)
}

fn read_layer<'py>(
    py: Python<'py>,
    paths: Vec<String>,
    layer: &str,
    schema: Option<Bound<'py, PyAny>>,
    options: Option<Bound<'py, PyAny>>,
    allow_empty: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let mut settings_options = None;
    let schema = match &schema {
        None => None,
        Some(schema) if schema.hasattr("__arrow_c_schema__")? => Some(import_schema(schema)?),
        Some(settings) => match load_settings(settings)? {
            Loaded::Settings(settings) => {
                let schema = settings.schema(layer).map_err(read_error)?;
                settings_options = Some(settings.options);
                Some(schema)
            }
            Loaded::Columns(columns) => Some(columns_schema(columns, layer)?),
        },
    };
    let options = match (&options, settings_options) {
        (Some(options), _) => parse_options(options)?,
        (None, Some(options)) => options,
        (None, None) => ReadOptions::default(),
    };
    let given = schema.clone();
    let reader = py.detach(|| {
        let sources = resolve(&paths)?;
        Ok::<_, PyErr>(xeibe_arrow::read(sources, layer, schema, &options))
    })?;
    let reader = match (reader, given) {
        (Ok(reader), _) => reader,
        // Found while the read sampled its first chunks.
        (Err(error), Some(schema)) if allow_empty && is_no_features(&error) => {
            warn_skipped(&[format!("skipped: {error}")])?;
            let empty = arrow_array::RecordBatchIterator::new(Vec::new(), schema);
            return PyRecordBatchReader::new(Box::new(empty)).into_arro3(py);
        }
        (Err(error), _) => return Err(read_error(error)),
    };
    let stream = Stream {
        reader,
        allow_empty,
        finished: false,
    };
    PyRecordBatchReader::new(Box::new(stream)).into_arro3(py)
}

/// The stream of a read: at its end, the sources it skipped become
/// [`SkippedSourceWarning`]s, and with `allow_empty` a
/// [`xeibe_core::Error::NoFeatures`] ends it without an error.
struct Stream {
    reader: xeibe_arrow::LayerReader,
    allow_empty: bool,
    finished: bool,
}

impl Stream {
    /// Warn once about every skipped source. A warning filter that makes the
    /// warning an error makes it the stream's error.
    fn finish(&mut self) -> Option<Result<arrow_array::RecordBatch, arrow_schema::ArrowError>> {
        self.finished = true;
        let skipped: Vec<String> = self
            .reader
            .report()
            .warnings
            .into_iter()
            .filter(|warning| warning.kind == xeibe_arrow::report::WarningKind::SkippedSource)
            .map(|warning| warning.message)
            .collect();
        let error = warn_skipped(&skipped).err()?;
        Some(Err(arrow_schema::ArrowError::ExternalError(Box::new(
            error,
        ))))
    }
}

impl Iterator for Stream {
    type Item = Result<arrow_array::RecordBatch, arrow_schema::ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.reader.next() {
            None => self.finish(),
            Some(Err(arrow_schema::ArrowError::ExternalError(error)))
                if self.allow_empty
                    && error
                        .downcast_ref::<xeibe_arrow::Error>()
                        .is_some_and(is_no_features) =>
            {
                self.finish()
            }
            item => item,
        }
    }
}

impl RecordBatchReader for Stream {
    fn schema(&self) -> SchemaRef {
        self.reader.schema()
    }
}

/// One [`SkippedSourceWarning`] per message.
fn warn_skipped(messages: &[String]) -> PyResult<()> {
    if messages.is_empty() {
        return Ok(());
    }
    Python::attach(|py| {
        let category = py.get_type::<SkippedSourceWarning>();
        for message in messages {
            let message = std::ffi::CString::new(message.as_str()).map_err(error)?;
            PyErr::warn(py, &category, &message, 1)?;
        }
        Ok(())
    })
}

fn resolve(paths: &[String]) -> PyResult<Vec<Source>> {
    xeibe_io::resolve_sources(paths, &IoOptions::default()).map_err(error)
}

// Not public API: helpers for `xeibe.sedona`, left out of the package's
// top level (`python/xeibe/__init__.py`).

/// `read_options(settings=None, pairs=None) -> dict`: the read options of
/// `settings` (as [`parse_options`] takes them; default options if `None`)
/// with `key=value` pairs applied over them (`preset`, `axis_order`, `crs`,
/// `sample_features`, `batch_size`, `threads`), as the `options` dict
/// `scan()` and `read()` take.
#[pyfunction]
#[pyo3(signature = (settings = None, pairs = None))]
fn read_options<'py>(
    py: Python<'py>,
    settings: Option<Bound<'py, PyAny>>,
    pairs: Option<Bound<'py, PyDict>>,
) -> PyResult<Bound<'py, PyAny>> {
    let mut options = match &settings {
        Some(settings) => parse_options(settings)?,
        None => ReadOptions::default(),
    };
    for (key, value) in pairs.iter().flat_map(|pairs| pairs.iter()) {
        let key: String = key.extract()?;
        let value = value.str()?.to_string();
        options
            .set(&key, &value)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
    }
    let json = serde_json::to_string(&options).map_err(error)?;
    py.import("json")?.call_method1("loads", (json,))
}

/// `layer_schema(settings, layer) -> arro3.core.Schema | None`: the layer's
/// schema from settings (see [`load_settings`]); `None` if they are a whole
/// settings file without that layer.
#[pyfunction]
fn layer_schema<'py>(
    py: Python<'py>,
    settings: Bound<'py, PyAny>,
    layer: &str,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let schema = match load_settings(&settings)? {
        Loaded::Settings(settings) => match settings.schema(layer) {
            Ok(schema) => schema,
            Err(xeibe_arrow::Error::Schema(xeibe_schema::Error::UnknownLayer(_))) => {
                return Ok(None);
            }
            Err(e) => return Err(error(e)),
        },
        Loaded::Columns(columns) => columns_schema(columns, layer)?,
    };
    to_arro3(py, schema).map(Some)
}

/// `to_wkb(schema) -> arro3.core.Schema`: native GeoArrow columns as
/// `geoarrow.wkb`, CRS and `gml:path` kept.
#[pyfunction]
fn to_wkb<'py>(py: Python<'py>, schema: Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let schema = xeibe_arrow::to_wkb(&*import_schema(&schema)?).map_err(error)?;
    to_arro3(py, Arc::new(schema))
}

/// Options: a dict or JSON text (the `options` section of a settings file),
/// or whole settings (see [`load_settings`]).
fn parse_options(options: &Bound<'_, PyAny>) -> PyResult<ReadOptions> {
    match json_text(options)? {
        Some(json) => {
            let value: serde_json::Value = serde_json::from_str(&json)
                .map_err(|e| PyValueError::new_err(format!("options: {e}")))?;
            if value.get("format_version").is_some() {
                return Ok(Settings::parse(&json, "options").map_err(error)?.options);
            }
            serde_json::from_value(value)
                .map_err(|e| PyValueError::new_err(format!("options: {e}")))
        }
        None => Ok(settings_file(options)?.options),
    }
}

/// Settings as [`load_settings`] finds them.
enum Loaded {
    /// A whole settings file: it has a `format_version`.
    Settings(Box<Settings>),
    /// The columns of one layer: `{"name": "type" | {"type": …, "path": …}}`.
    Columns(serde_json::Value),
}

/// Settings from a dict, JSON text (a string starting with `{`) or a file
/// path.
fn load_settings(settings: &Bound<'_, PyAny>) -> PyResult<Loaded> {
    let Some(json) = json_text(settings)? else {
        return Ok(Loaded::Settings(Box::new(settings_file(settings)?)));
    };
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| PyValueError::new_err(format!("settings: {e}")))?;
    if value.get("format_version").is_some() {
        return Ok(Loaded::Settings(Box::new(
            Settings::parse(&json, "settings").map_err(error)?,
        )));
    }
    Ok(Loaded::Columns(value))
}

/// The schema of one layer's columns.
fn columns_schema(columns: serde_json::Value, layer: &str) -> PyResult<SchemaRef> {
    let mut settings = Settings::new(ReadOptions::default());
    let columns = serde_json::from_value(columns)
        .map_err(|e| PyValueError::new_err(format!("schema of {layer}: {e}")))?;
    settings.layers.insert(layer.to_string(), columns);
    settings.schema(layer).map_err(error)
}

/// A dict as JSON, a string starting with `{` as it is; `None` for a path.
fn json_text(object: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    if object.is_instance_of::<PyDict>() {
        let json = object
            .py()
            .import("json")?
            .call_method1("dumps", (object,))?
            .extract()?;
        return Ok(Some(json));
    }
    if let Ok(text) = object.extract::<String>()
        && text.trim_start().starts_with('{')
    {
        return Ok(Some(text));
    }
    Ok(None)
}

fn settings_file(path: &Bound<'_, PyAny>) -> PyResult<Settings> {
    let path: PathBuf = path.extract()?;
    Settings::load(&path).map_err(error)
}

/// A schema from anything with `__arrow_c_schema__` (the PyCapsule interface).
fn import_schema(object: &Bound<'_, PyAny>) -> PyResult<SchemaRef> {
    Ok(object.extract::<PySchema>()?.into_inner())
}

/// A schema as an `arro3.core.Schema`.
fn to_arro3(py: Python<'_>, schema: SchemaRef) -> PyResult<Bound<'_, PyAny>> {
    PySchema::new(schema).into_arro3(py)
}

#[pyclass(module = "xeibe")]
pub struct Scan {
    inner: xeibe_arrow::ScanResult,
}

#[pymethods]
impl Scan {
    /// List of dicts: name, feature_count, geometry_columns, crs, extent.
    /// `name` is the local name, `{uri}local` in `qname`.
    #[getter]
    fn layers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let layers = PyList::empty(py);
        for layer in self.inner.layers() {
            let info = PyDict::new(py);
            info.set_item("name", &*layer.name.local)?;
            info.set_item("qname", layer.name.to_clark())?;
            info.set_item("feature_count", layer.feature_count)?;
            info.set_item("geometry_columns", layer.geometry_columns)?;
            info.set_item("crs", layer.crs)?;
            info.set_item("extent", layer.extent.map(|[a, b, c, d]| (a, b, c, d)))?;
            layers.append(info)?;
        }
        Ok(layers.into_any())
    }

    /// `False` for a sampled scan: layers that start later may be missing.
    #[getter]
    fn is_complete(&self) -> bool {
        self.inner.is_complete()
    }

    /// Names of the sources skipped because they hold no feature collection
    /// or feature member (ISO metadata next to the GML in a zip, say).
    #[getter]
    fn skipped_sources(&self) -> Vec<String> {
        self.inner.skipped_sources().to_vec()
    }

    /// `(xmin, ymin, xmax, ymax)` the collections declare in their
    /// `boundedBy`, as written, for all layers; `None` if none does.
    #[getter]
    fn extent(&self) -> Option<(f64, f64, f64, f64)> {
        self.inner.extent().map(|[a, b, c, d]| (a, b, c, d))
    }

    /// The layer's schema as an `arro3.core.Schema`.
    fn schema<'py>(&self, py: Python<'py>, layer: &str) -> PyResult<Bound<'py, PyAny>> {
        to_arro3(py, self.inner.arrow_schema(layer).map_err(read_error)?)
    }

    fn explain(&self, layer: &str) -> PyResult<String> {
        self.inner.explain(layer).map_err(error)
    }

    /// Write the settings file.
    fn save(&self, path: PathBuf) -> PyResult<()> {
        self.inner
            .to_settings()
            .and_then(|settings| settings.save(&path))
            .map_err(error)
    }
}

#[pymodule]
fn _xeibe(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(scan, m)?)?;
    m.add_function(wrap_pyfunction!(read, m)?)?;
    m.add_function(wrap_pyfunction!(read_or_empty, m)?)?;
    m.add_function(wrap_pyfunction!(read_options, m)?)?;
    m.add_function(wrap_pyfunction!(layer_schema, m)?)?;
    m.add_function(wrap_pyfunction!(to_wkb, m)?)?;
    m.add_class::<Scan>()?;
    m.add("XeibeError", m.py().get_type::<XeibeError>())?;
    m.add("UnknownLayerError", m.py().get_type::<UnknownLayerError>())?;
    m.add("NoFeaturesError", m.py().get_type::<NoFeaturesError>())?;
    m.add(
        "SkippedSourceWarning",
        m.py().get_type::<SkippedSourceWarning>(),
    )?;
    Ok(())
}
