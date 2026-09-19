//! Python module `xeibe`: `scan()` and `read()`. Readers implement
//! `__arrow_c_stream__`, so they can be passed to PyArrow, GeoPandas
//! (`from_arrow`), DuckDB, Polars or SedonaDB.

// Skeleton phase: signatures only, bodies are `todo!()`.
#![allow(dead_code, unused_variables)]

use pyo3::prelude::*;
use pyo3::types::PyCapsule;

/// `scan(paths, sample=None, options=None) -> Scan`
#[pyfunction]
#[pyo3(signature = (paths, sample = None, options = None))]
fn scan(paths: Vec<String>, sample: Option<u64>, options: Option<Bound<'_, PyAny>>) -> PyResult<Scan> {
    todo!()
}

/// `read(paths, layer, schema=None, options=None) -> LayerStream`.
/// `schema`: a `pyarrow.Schema` (anything with `__arrow_c_schema__`) or a settings-file path.
/// `options`: a dict or a settings-file path.
#[pyfunction]
#[pyo3(signature = (paths, layer, schema = None, options = None))]
fn read(
    paths: Vec<String>,
    layer: &str,
    schema: Option<Bound<'_, PyAny>>,
    options: Option<Bound<'_, PyAny>>,
) -> PyResult<LayerStream> {
    todo!()
}

#[pyclass(module = "xeibe")]
pub struct Scan {
    inner: xeibe_arrow::ScanResult,
}

#[pymethods]
impl Scan {
    /// List of dicts: name, feature_count, geometry_columns, crs, extent.
    #[getter]
    fn layers<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        todo!()
    }

    /// The layer's schema as a `pyarrow.Schema`.
    fn schema<'py>(&self, py: Python<'py>, layer: &str) -> PyResult<Bound<'py, PyAny>> {
        todo!()
    }

    fn explain(&self, layer: &str) -> PyResult<String> {
        todo!()
    }

    /// Write the settings file.
    fn save(&self, path: &str) -> PyResult<()> {
        todo!()
    }
}

/// A one-shot stream of record batches for one layer.
#[pyclass(module = "xeibe")]
pub struct LayerStream {
    reader: Option<xeibe_arrow::LayerReader>,
}

#[pymethods]
impl LayerStream {
    #[pyo3(signature = (requested_schema = None))]
    fn __arrow_c_stream__<'py>(
        &mut self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        todo!()
    }
}

#[pymodule]
fn xeibe(m: &Bound<'_, PyModule>) -> PyResult<()> {
    todo!()
}
