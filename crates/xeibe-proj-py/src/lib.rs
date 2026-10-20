//! The native part of the Python package `xeibe_proj` (`xeibe_proj._native`).
//!
//! Kept narrow (`docs/pyproj-compat.md`, Naming and layout): EPSG records and
//! PROJJSON from `xeibe-crs`, and `xeibe-reproject`'s transforms. The
//! pyproj-shaped API is Python over PROJJSON. Conversion works in place on
//! float64 buffers the Python side has already copied, and releases the GIL.
//! Errors are `xeibe_proj.exceptions`' classes.

use pyo3::buffer::PyBuffer;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use xeibe_reproject::{AxisOrder, Crs, Error, Grids};

/// `xeibe_proj.exceptions.<name>(message)`.
fn exception(py: Python<'_>, name: &str, message: String) -> PyErr {
    match py
        .import("xeibe_proj.exceptions")
        .and_then(|m| m.getattr(name))
        .and_then(|class| class.call1((message,)))
    {
        Ok(error) => PyErr::from_value(error),
        Err(error) => error,
    }
}

/// A CRS that can't be read is a `CRSError`, anything else a `ProjError`.
fn error(py: Python<'_>, error: Error) -> PyErr {
    let class = match error {
        Error::UnknownCode(_) | Error::Projjson { .. } | Error::Unsupported { .. } => "CRSError",
        _ => "ProjError",
    };
    exception(py, class, error.to_string())
}

/// The EPSG Dataset version of the tables, e.g. `"13.106"`.
#[pyfunction]
fn epsg_version() -> &'static str {
    xeibe_crs::EPSG_VERSION
}

/// The publication date of that EPSG Dataset version.
#[pyfunction]
fn epsg_date() -> &'static str {
    xeibe_crs::EPSG_DATE
}

/// The PROJJSON of an EPSG CRS, or `None`.
#[pyfunction]
fn projjson(code: u32) -> Option<&'static str> {
    xeibe_crs::projjson(code)
}

/// The code an EPSG alias names, or `None`. Case is ignored.
#[pyfunction]
fn alias(name: &str) -> Option<u32> {
    xeibe_crs::alias(name)
}

type Record = (
    u32,
    &'static str,
    &'static str,
    u8,
    bool,
    Option<(f64, f64, f64, f64)>,
);

/// The f64 of an f32's shortest decimal form: EPSG's `14.14`, not
/// `14.140000343322754`.
fn f(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(value as f64)
}

/// Every EPSG CRS: `(code, name, kind, dimension, deprecated, area)`, with
/// the area as `(west, south, east, north)` in degrees.
#[pyfunction]
fn crs_records() -> Vec<Record> {
    use xeibe_crs::CrsKind;
    xeibe_crs::CRS
        .iter()
        .map(|r| {
            let kind = match r.kind {
                CrsKind::Geographic2d => "geographic_2d",
                CrsKind::Geographic3d => "geographic_3d",
                CrsKind::Projected => "projected",
                CrsKind::Geocentric => "geocentric",
                CrsKind::Vertical => "vertical",
                CrsKind::Compound => "compound",
                CrsKind::Engineering => "engineering",
                CrsKind::Derived => "derived",
            };
            let area = r
                .area_wgs84
                .map(|[south, west, north, east]| (f(west), f(south), f(east), f(north)));
            (r.code, r.name, kind, r.dimension, r.deprecated, area)
        })
        .collect()
}

/// Forget the grid data loaded so far (shared by every transform).
#[pyfunction]
fn clear_grids() {
    xeibe_reproject::clear_grids();
}

/// A writable, C-contiguous float64 buffer: its start and length. The
/// pointer is passed as an integer so the conversion can run without the
/// GIL; the caller keeps `buffer` alive meanwhile.
fn writable(buffer: &PyBuffer<f64>, name: &str) -> PyResult<(usize, usize)> {
    if buffer.readonly() {
        return Err(PyTypeError::new_err(format!(
            "{name}: the buffer is read-only"
        )));
    }
    if !buffer.is_c_contiguous() {
        return Err(PyTypeError::new_err(format!(
            "{name}: the buffer is not C-contiguous"
        )));
    }
    Ok((buffer.buf_ptr() as usize, buffer.item_count()))
}

/// # Safety
/// `ptr` and `len` come from [`writable`] on a buffer that is still held.
unsafe fn slice<'a>((ptr, len): (usize, usize)) -> &'a mut [f64] {
    if len == 0 {
        return &mut [];
    }
    unsafe { std::slice::from_raw_parts_mut(ptr as *mut f64, len) }
}

/// Multiply every value by `factor`.
fn scale(values: &mut [f64], factor: f64) {
    values.iter_mut().for_each(|v| *v *= factor);
}

/// A conversion between two CRSs (`xeibe_reproject::Transform`).
#[pyclass(frozen, module = "xeibe_proj._native")]
struct Transform {
    inner: xeibe_reproject::Transform,
}

#[pymethods]
impl Transform {
    /// `Transform(source, target, always_xy, grids)`: the CRSs as PROJJSON
    /// text. `grids=False` keeps to operations without grids.
    #[new]
    fn new(
        py: Python<'_>,
        source: &str,
        target: &str,
        always_xy: bool,
        grids: bool,
    ) -> PyResult<Self> {
        let order = if always_xy {
            AxisOrder::EastFirst
        } else {
            AxisOrder::Authority
        };
        let grids = if grids { Grids::ProjData } else { Grids::None };
        let inner = py
            .detach(|| {
                let src = Crs::from_projjson(source)?;
                let dst = Crs::from_projjson(target)?;
                xeibe_reproject::Transform::with_grids(&src, &dst, order, grids)
            })
            .map_err(|e| error(py, e))?;
        Ok(Transform { inner })
    }

    /// The conversion the other way.
    fn inverse(&self, py: Python<'_>) -> PyResult<Transform> {
        let inner = py
            .detach(|| self.inner.inverse())
            .map_err(|e| error(py, e))?;
        Ok(Transform { inner })
    }

    /// Names of the candidate operations, best first; empty on one datum.
    fn operations(&self) -> Vec<String> {
        self.inner
            .operations()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    /// The name of the operation one coordinate goes through.
    #[pyo3(signature = (x, y, z = None))]
    fn operation_for(&self, x: f64, y: f64, z: Option<f64>) -> Option<String> {
        self.inner.operation_for(x, y, z).map(str::to_owned)
    }

    /// PROJ-data's names of the grid files this transform may read.
    fn grids(&self) -> Vec<String> {
        self.inner.grids().into_iter().map(str::to_owned).collect()
    }

    /// The grid ranges still to load for coordinates within `bounds`
    /// (`(min_x, min_y, max_x, max_y)` in the source CRS), or anywhere for
    /// `None`: `(file, offset, length)`, `length` `None` for the rest of the
    /// file.
    #[pyo3(signature = (bounds = None))]
    fn grid_requests(&self, bounds: Option<[f64; 4]>) -> Vec<(String, u64, Option<u64>)> {
        self.inner
            .grid_requests(bounds)
            .into_iter()
            .map(|r| (r.file, r.offset, r.length))
            .collect()
    }

    /// Hand over the bytes of grid `file` read from `offset`.
    fn add_grid_data(
        &self,
        py: Python<'_>,
        file: &str,
        offset: u64,
        data: &Bound<'_, PyBytes>,
    ) -> PyResult<()> {
        let data = data.as_bytes().to_vec();
        py.detach(|| self.inner.add_grid_data(file, offset, data))
            .map_err(|e| error(py, e))
    }

    /// Convert coordinates in place: `x`, `y` and optionally `z` are
    /// writable float64 buffers of one length. `radians_in` takes x and y
    /// in radians for a source CRS in degrees; `radians_out` returns them in
    /// radians for a target CRS in degrees. Stops at the first coordinate
    /// that fails, with a `ProjError` naming it.
    #[pyo3(signature = (x, y, z = None, radians_in = false, radians_out = false))]
    fn convert(
        &self,
        py: Python<'_>,
        x: &Bound<'_, PyAny>,
        y: &Bound<'_, PyAny>,
        z: Option<&Bound<'_, PyAny>>,
        radians_in: bool,
        radians_out: bool,
    ) -> PyResult<()> {
        let bx = PyBuffer::<f64>::get(x)?;
        let by = PyBuffer::<f64>::get(y)?;
        let bz = z.map(PyBuffer::<f64>::get).transpose()?;
        let rx = writable(&bx, "x")?;
        let ry = writable(&by, "y")?;
        let rz = bz.as_ref().map(|b| writable(b, "z")).transpose()?;
        if rx.1 != ry.1 || rz.is_some_and(|rz| rz.1 != rx.1) {
            return Err(PyValueError::new_err(
                "the coordinate arrays differ in length",
            ));
        }
        if rx.0 == ry.0 || rz.is_some_and(|rz| rz.0 == rx.0 || rz.0 == ry.0) {
            return Err(PyValueError::new_err("the coordinate arrays share memory"));
        }
        py.detach(|| {
            // SAFETY: the buffers are held until this returns, and they don't
            // overlap (checked above for the starts; each is its own copy).
            let (xs, ys) = unsafe { (slice(rx), slice(ry)) };
            if radians_in {
                scale(xs, 180.0 / std::f64::consts::PI);
                scale(ys, 180.0 / std::f64::consts::PI);
            }
            match rz {
                Some(rz) => self.inner.convert_xyz(xs, ys, unsafe { slice(rz) })?,
                None => self.inner.convert_xy(xs, ys)?,
            }
            if radians_out {
                scale(xs, std::f64::consts::PI / 180.0);
                scale(ys, std::f64::consts::PI / 180.0);
            }
            Ok(())
        })
        .map_err(|e| error(py, e))
    }

    /// Convert a box (`Transform::convert_bounds`): `(min_x, min_y, max_x,
    /// max_y)` in, the outermost box of its densified edges out.
    fn convert_bounds(
        &self,
        py: Python<'_>,
        bounds: [f64; 4],
        densify: usize,
    ) -> PyResult<(f64, f64, f64, f64)> {
        let [a, b, c, d] = py
            .detach(|| self.inner.convert_bounds(bounds, densify))
            .map_err(|e| error(py, e))?;
        Ok((a, b, c, d))
    }
}

/// The box around the finite coordinates of two float64 buffers, or `None`
/// when there is none: what grid loading asks for.
#[pyfunction]
fn extent(
    py: Python<'_>,
    x: &Bound<'_, PyAny>,
    y: &Bound<'_, PyAny>,
) -> PyResult<Option<(f64, f64, f64, f64)>> {
    let bx = PyBuffer::<f64>::get(x)?;
    let by = PyBuffer::<f64>::get(y)?;
    let (Some(xs), Some(ys)) = (bx.as_slice(py), by.as_slice(py)) else {
        return Err(PyTypeError::new_err("the buffers are not C-contiguous"));
    };
    let mut out = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for (x, y) in xs.iter().zip(ys) {
        let (x, y) = (x.get(), y.get());
        if x.is_finite() && y.is_finite() {
            out = [out[0].min(x), out[1].min(y), out[2].max(x), out[3].max(y)];
        }
    }
    Ok(out[0]
        .is_finite()
        .then_some((out[0], out[1], out[2], out[3])))
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(epsg_version, m)?)?;
    m.add_function(wrap_pyfunction!(epsg_date, m)?)?;
    m.add_function(wrap_pyfunction!(projjson, m)?)?;
    m.add_function(wrap_pyfunction!(alias, m)?)?;
    m.add_function(wrap_pyfunction!(crs_records, m)?)?;
    m.add_function(wrap_pyfunction!(clear_grids, m)?)?;
    m.add_function(wrap_pyfunction!(extent, m)?)?;
    m.add_class::<Transform>()?;
    Ok(())
}
