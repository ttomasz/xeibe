//! Python module `xeibe`: `scan()` and `read()`. Readers implement
//! `__arrow_c_stream__`, so they can be passed to PyArrow, GeoPandas
//! (`from_arrow`), DuckDB, Polars or SedonaDB.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use arrow_array::RecordBatchReader;
use arrow_array::ffi_stream::FFI_ArrowArrayStream;
use arrow_schema::ffi::FFI_ArrowSchema;
use arrow_schema::{Schema, SchemaRef};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyCapsule, PyCapsuleMethods, PyDict, PyList};
use xeibe_arrow::{ReadOptions, Settings};
use xeibe_core::Source;
use xeibe_io::IoOptions;
use xeibe_schema::ScanExtent;

create_exception!(xeibe, XeibeError, PyException, "A GML input could not be scanned or read.");

fn error(error: impl std::fmt::Display) -> PyErr {
    XeibeError::new_err(error.to_string())
}

/// `scan(paths, sample=None, options=None) -> Scan`
#[pyfunction]
#[pyo3(signature = (paths, sample = None, options = None))]
fn scan(py: Python<'_>, paths: Vec<String>, sample: Option<u64>, options: Option<Bound<'_, PyAny>>) -> PyResult<Scan> {
    let options = match &options {
        Some(options) => read_options(options)?,
        None => ReadOptions::default(),
    };
    let extent = match sample {
        Some(max_features) => ScanExtent::Sample { max_features },
        None => ScanExtent::Full,
    };
    let inner = py.detach(|| {
        let sources = resolve(&paths)?;
        xeibe_arrow::scan(sources, extent, &options).map_err(error)
    })?;
    Ok(Scan { inner })
}

/// `read(paths, layer, schema=None, options=None) -> LayerStream`.
/// `schema`: a `pyarrow.Schema` (anything with `__arrow_c_schema__`) or a settings-file path.
/// `options`: a dict or a settings-file path.
///
/// A settings file given as `schema` also supplies the options when
/// `options` is `None`. Without a schema, the layer's first features are
/// sampled before this returns.
#[pyfunction]
#[pyo3(signature = (paths, layer, schema = None, options = None))]
fn read(
    py: Python<'_>,
    paths: Vec<String>,
    layer: &str,
    schema: Option<Bound<'_, PyAny>>,
    options: Option<Bound<'_, PyAny>>,
) -> PyResult<LayerStream> {
    let mut settings_options = None;
    let schema = match &schema {
        None => None,
        Some(schema) if schema.hasattr("__arrow_c_schema__")? => Some(import_schema(schema)?),
        Some(path) => {
            let settings = load_settings(path)?;
            let schema = settings.schema(layer).map_err(error)?;
            settings_options = Some(settings.options);
            Some(schema)
        }
    };
    let options = match (&options, settings_options) {
        (Some(options), _) => read_options(options)?,
        (None, Some(options)) => options,
        (None, None) => ReadOptions::default(),
    };
    let reader = py.detach(|| {
        let sources = resolve(&paths)?;
        xeibe_arrow::read(sources, layer, schema, &options).map_err(error)
    })?;
    Ok(LayerStream { schema: reader.schema(), reader: Mutex::new(Some(reader)) })
}

fn resolve(paths: &[String]) -> PyResult<Vec<Source>> {
    xeibe_io::resolve_sources(paths, &IoOptions::default()).map_err(error)
}

/// A dict (the `options` section of a settings file) or a settings-file path.
fn read_options(options: &Bound<'_, PyAny>) -> PyResult<ReadOptions> {
    if options.is_instance_of::<PyDict>() {
        let json: String = options.py().import("json")?.call_method1("dumps", (options,))?.extract()?;
        return serde_json::from_str(&json).map_err(|e| PyValueError::new_err(format!("options: {e}")));
    }
    Ok(load_settings(options)?.options)
}

fn load_settings(path: &Bound<'_, PyAny>) -> PyResult<Settings> {
    let path: PathBuf = path.extract()?;
    Settings::load(&path).map_err(error)
}

/// A schema from anything with `__arrow_c_schema__` (the PyCapsule interface).
fn import_schema(object: &Bound<'_, PyAny>) -> PyResult<SchemaRef> {
    let capsule = object.call_method0("__arrow_c_schema__")?;
    let capsule = capsule.cast::<PyCapsule>()?;
    let pointer = capsule.pointer_checked(Some(c"arrow_schema"))?;
    // SAFETY: an `arrow_schema` capsule holds an `ArrowSchema`, owned (and
    // released) by the capsule, which outlives this borrow.
    let ffi = unsafe { pointer.cast::<FFI_ArrowSchema>().as_ref() };
    Ok(Arc::new(Schema::try_from(ffi).map_err(error)?))
}

fn schema_capsule<'py>(py: Python<'py>, schema: &Schema) -> PyResult<Bound<'py, PyCapsule>> {
    let ffi = FFI_ArrowSchema::try_from(schema).map_err(error)?;
    PyCapsule::new_with_value(py, ffi, c"arrow_schema")
}

/// A schema as a `pyarrow.Schema`.
fn to_pyarrow<'py>(py: Python<'py>, schema: SchemaRef) -> PyResult<Bound<'py, PyAny>> {
    let schema = Bound::new(py, ArrowSchema { schema })?;
    py.import("pyarrow")?.call_method1("schema", (schema,))
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

    /// `(xmin, ymin, xmax, ymax)` the collections declare in their
    /// `boundedBy`, as written, for all layers; `None` if none does.
    #[getter]
    fn extent(&self) -> Option<(f64, f64, f64, f64)> {
        self.inner.extent().map(|[a, b, c, d]| (a, b, c, d))
    }

    /// The layer's schema as a `pyarrow.Schema`.
    fn schema<'py>(&self, py: Python<'py>, layer: &str) -> PyResult<Bound<'py, PyAny>> {
        to_pyarrow(py, self.inner.arrow_schema(layer).map_err(error)?)
    }

    fn explain(&self, layer: &str) -> PyResult<String> {
        self.inner.explain(layer).map_err(error)
    }

    /// Write the settings file.
    fn save(&self, path: PathBuf) -> PyResult<()> {
        self.inner.to_settings().and_then(|settings| settings.save(&path)).map_err(error)
    }
}

/// A one-shot stream of record batches for one layer.
#[pyclass(module = "xeibe")]
pub struct LayerStream {
    schema: SchemaRef,
    reader: Mutex<Option<xeibe_arrow::LayerReader>>,
}

#[pymethods]
impl LayerStream {
    /// Hands the reader to the consumer; a second call fails. The requested
    /// schema is ignored (the protocol allows it): the layer's schema is fixed.
    #[pyo3(signature = (requested_schema = None))]
    fn __arrow_c_stream__<'py>(
        &mut self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        let _ = requested_schema;
        let reader = self
            .reader
            .get_mut()
            .map_err(error)?
            .take()
            .ok_or_else(|| error("the stream has already been consumed"))?;
        let stream = FFI_ArrowArrayStream::new(Box::new(reader));
        PyCapsule::new_with_value(py, stream, c"arrow_array_stream")
    }

    fn __arrow_c_schema__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyCapsule>> {
        schema_capsule(py, &self.schema)
    }

    /// The layer's schema as a `pyarrow.Schema`.
    #[getter]
    fn schema<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_pyarrow(py, self.schema.clone())
    }
}

/// A schema behind `__arrow_c_schema__`, to hand to PyArrow.
#[pyclass(module = "xeibe", frozen)]
struct ArrowSchema {
    schema: SchemaRef,
}

#[pymethods]
impl ArrowSchema {
    fn __arrow_c_schema__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyCapsule>> {
        schema_capsule(py, &self.schema)
    }
}

#[pymodule]
fn xeibe(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(scan, m)?)?;
    m.add_function(wrap_pyfunction!(read, m)?)?;
    m.add_class::<Scan>()?;
    m.add_class::<LayerStream>()?;
    m.add("XeibeError", m.py().get_type::<XeibeError>())?;
    Ok(())
}
