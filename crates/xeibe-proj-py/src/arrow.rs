//! GeoArrow arrays in and out of `xeibe_proj.arrow`
//! (`docs/pyproj-phase3.md`, GeoArrow), through the Arrow C Data and C
//! Stream Interfaces (`__arrow_c_array__`, `__arrow_c_stream__`), so no
//! Python Arrow library is imported. The conversion itself is
//! `xeibe_reproject::arrow`.

use std::collections::{HashMap, VecDeque};
use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::Arc;

use arrow_array::ffi::{FFI_ArrowArray, FFI_ArrowSchema, from_ffi};
use arrow_array::ffi_stream::FFI_ArrowArrayStream;
use arrow_array::{ArrayRef, make_array};
use arrow_schema::{Field, FieldRef};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyCapsule;
use xeibe_reproject::arrow::{self as convert_arrays, ArrayError};

use crate::convert::Missing;
use crate::{Transform, exception};

/// A GeoArrow array (or chunked array) and its field, as imported.
#[pyclass(frozen, module = "xeibe_proj._native")]
pub(crate) struct ArrowData {
    field: FieldRef,
    chunks: Vec<ArrayRef>,
}

/// Why a conversion stopped.
enum Stop {
    /// Grid data is missing for the box.
    Grids(Missing),
    Failed(ArrayError),
}

impl Stop {
    fn into_err(self, py: Python<'_>) -> PyErr {
        match self {
            Stop::Grids(missing) => missing.into_err(),
            Stop::Failed(e @ ArrayError::Layout(_)) => PyValueError::new_err(e.to_string()),
            Stop::Failed(e @ ArrayError::Convert { .. }) => {
                exception(py, "ProjError", e.to_string())
            }
        }
    }
}

fn arrow_error(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(format!("Arrow: {e}"))
}

#[pymethods]
impl ArrowData {
    /// Import an array from the two capsules of `__arrow_c_array__`.
    #[staticmethod]
    fn from_array(schema: &Bound<'_, PyCapsule>, array: &Bound<'_, PyCapsule>) -> PyResult<Self> {
        let schema_ptr = schema.pointer_checked(Some(c"arrow_schema"))?;
        let array_ptr = array.pointer_checked(Some(c"arrow_array"))?;
        // SAFETY: the capsules hold the structs their names say. The schema
        // stays the capsule's to release; the array is moved out, leaving a
        // released one behind, as the PyCapsule interface asks.
        let schema = unsafe { &*(schema_ptr.as_ptr() as *const FFI_ArrowSchema) };
        let array = unsafe { FFI_ArrowArray::from_raw(array_ptr.as_ptr() as *mut FFI_ArrowArray) };
        let field = Field::try_from(schema).map_err(arrow_error)?;
        // SAFETY: the array was made for this schema.
        let data = unsafe { from_ffi(array, schema) }.map_err(arrow_error)?;
        Ok(ArrowData {
            field: Arc::new(field),
            chunks: vec![make_array(data)],
        })
    }

    /// Import every chunk of the capsule of `__arrow_c_stream__`.
    #[staticmethod]
    fn from_stream(stream: &Bound<'_, PyCapsule>) -> PyResult<Self> {
        let ptr = stream.pointer_checked(Some(c"arrow_array_stream"))?;
        // SAFETY: moved out of the capsule, which is left a released stream.
        let mut stream =
            unsafe { FFI_ArrowArrayStream::from_raw(ptr.as_ptr() as *mut FFI_ArrowArrayStream) };
        let (Some(get_schema), Some(get_next)) = (stream.get_schema, stream.get_next) else {
            return Err(arrow_error("the stream is released"));
        };
        let mut schema = FFI_ArrowSchema::empty();
        // SAFETY: the stream's own callbacks, on the stream.
        let rc = unsafe { get_schema(&mut stream, &mut schema) };
        if rc != 0 {
            return Err(arrow_error(stream_error(&mut stream, rc)));
        }
        let field = Field::try_from(&schema).map_err(arrow_error)?;
        let mut chunks = Vec::new();
        loop {
            let mut array = FFI_ArrowArray::empty();
            let rc = unsafe { get_next(&mut stream, &mut array) };
            if rc != 0 {
                return Err(arrow_error(stream_error(&mut stream, rc)));
            }
            if array.is_released() {
                break;
            }
            // SAFETY: the stream's arrays have the stream's schema.
            let data = unsafe { from_ffi(array, &schema) }.map_err(arrow_error)?;
            chunks.push(make_array(data));
        }
        Ok(ArrowData {
            field: Arc::new(field),
            chunks,
        })
    }

    /// The field's metadata (`ARROW:extension:name` and
    /// `ARROW:extension:metadata` among them).
    #[getter]
    fn metadata(&self) -> HashMap<String, String> {
        self.field.metadata().clone()
    }

    /// The number of chunks.
    #[getter]
    fn num_chunks(&self) -> usize {
        self.chunks.len()
    }

    fn __len__(&self) -> usize {
        self.chunks.iter().map(|c| c.len()).sum()
    }

    /// The same data with the field's metadata replaced.
    fn with_metadata(&self, metadata: HashMap<String, String>) -> ArrowData {
        ArrowData {
            field: Arc::new(self.field.as_ref().clone().with_metadata(metadata)),
            chunks: self.chunks.clone(),
        }
    }

    fn __arrow_c_schema__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyCapsule>> {
        let schema = FFI_ArrowSchema::try_from(self.field.as_ref()).map_err(arrow_error)?;
        PyCapsule::new_with_value(py, schema, c"arrow_schema")
    }

    /// The one chunk; a `TypeError` for chunked data. `requested_schema`
    /// is ignored, as the interface allows.
    #[pyo3(signature = (requested_schema = None))]
    fn __arrow_c_array__<'py>(
        &self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyAny>>,
    ) -> PyResult<(Bound<'py, PyCapsule>, Bound<'py, PyCapsule>)> {
        let _ = requested_schema;
        let [chunk] = self.chunks.as_slice() else {
            return Err(PyTypeError::new_err("chunked data: use __arrow_c_stream__"));
        };
        let schema = self.__arrow_c_schema__(py)?;
        let array = FFI_ArrowArray::new(&chunk.to_data());
        Ok((
            schema,
            PyCapsule::new_with_value(py, array, c"arrow_array")?,
        ))
    }

    /// The chunks as a stream. `requested_schema` is ignored.
    #[pyo3(signature = (requested_schema = None))]
    fn __arrow_c_stream__<'py>(
        &self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        let _ = requested_schema;
        let stream = export_stream(self.field.clone(), self.chunks.clone());
        PyCapsule::new_with_value(py, stream, c"arrow_array_stream")
    }

    /// The data converted by `native` (a `Transform`), easting first. With
    /// `check_grids`, `GridsNeeded` when grid data for its extent is
    /// missing; a `ProjError` naming the geometry when a coordinate fails.
    #[pyo3(signature = (native, check_grids = true))]
    fn convert(
        &self,
        py: Python<'_>,
        native: &Bound<'_, Transform>,
        check_grids: bool,
    ) -> PyResult<ArrowData> {
        let native = native.get();
        let chunks = py
            .detach(|| {
                if check_grids {
                    let bounds = convert_arrays::extent(&self.chunks).map_err(Stop::Failed)?;
                    native
                        .calls
                        .check_grids(&native.inner, bounds)
                        .map_err(Stop::Grids)?;
                }
                convert_arrays::convert_chunks(&self.chunks, &native.inner).map_err(Stop::Failed)
            })
            .map_err(|stop| stop.into_err(py))?;
        Ok(ArrowData {
            field: self.field.clone(),
            chunks,
        })
    }
}

// The C Stream Interface, for arrays -------------------------------------------
//
// arrow-rs exports streams of record batches only; a chunked geometry
// column is a stream of plain arrays.

struct Exported {
    field: FieldRef,
    chunks: VecDeque<ArrayRef>,
    error: Option<CString>,
}

const EIO: c_int = 5;

fn export_stream(field: FieldRef, chunks: Vec<ArrayRef>) -> FFI_ArrowArrayStream {
    let private = Box::new(Exported {
        field,
        chunks: chunks.into(),
        error: None,
    });
    FFI_ArrowArrayStream {
        get_schema: Some(stream_get_schema),
        get_next: Some(stream_get_next),
        get_last_error: Some(stream_get_last_error),
        release: Some(stream_release),
        private_data: Box::into_raw(private) as *mut c_void,
    }
}

/// # Safety
/// `stream` is a live stream made by `export_stream`.
unsafe fn exported<'a>(stream: *mut FFI_ArrowArrayStream) -> &'a mut Exported {
    unsafe { &mut *((*stream).private_data as *mut Exported) }
}

unsafe extern "C" fn stream_get_schema(
    stream: *mut FFI_ArrowArrayStream,
    out: *mut FFI_ArrowSchema,
) -> c_int {
    let private = unsafe { exported(stream) };
    match FFI_ArrowSchema::try_from(private.field.as_ref()) {
        Ok(schema) => {
            unsafe { std::ptr::write(out, schema) };
            0
        }
        Err(e) => {
            private.error = CString::new(e.to_string()).ok();
            EIO
        }
    }
}

unsafe extern "C" fn stream_get_next(
    stream: *mut FFI_ArrowArrayStream,
    out: *mut FFI_ArrowArray,
) -> c_int {
    let private = unsafe { exported(stream) };
    let array = match private.chunks.pop_front() {
        Some(chunk) => FFI_ArrowArray::new(&chunk.to_data()),
        None => FFI_ArrowArray::empty(),
    };
    unsafe { std::ptr::write(out, array) };
    0
}

unsafe extern "C" fn stream_get_last_error(stream: *mut FFI_ArrowArrayStream) -> *const c_char {
    let private = unsafe { exported(stream) };
    private
        .error
        .as_ref()
        .map_or(std::ptr::null(), |e| e.as_ptr())
}

unsafe extern "C" fn stream_release(stream: *mut FFI_ArrowArrayStream) {
    if stream.is_null() {
        return;
    }
    let stream = unsafe { &mut *stream };
    if !stream.private_data.is_null() {
        drop(unsafe { Box::from_raw(stream.private_data as *mut Exported) });
    }
    stream.private_data = std::ptr::null_mut();
    stream.get_schema = None;
    stream.get_next = None;
    stream.get_last_error = None;
    stream.release = None;
}

/// The message of a stream's failed call.
fn stream_error(stream: &mut FFI_ArrowArrayStream, rc: c_int) -> String {
    let message = stream.get_last_error.and_then(|get| {
        // SAFETY: the stream's own callback; the string lives until the
        // next call on the stream.
        let ptr = unsafe { get(stream) };
        (!ptr.is_null()).then(|| {
            unsafe { std::ffi::CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned()
        })
    });
    message.unwrap_or_else(|| format!("the stream failed with error {rc}"))
}
