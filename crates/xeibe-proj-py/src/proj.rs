//! The legacy `Proj` (`docs/pyproj-phase2.md`, `Proj`): a CRS's own
//! projection, with the methods `xeibe_proj.transformer` calls on a
//! native transform, so that `Proj` is a `Transformer`.

use pyo3::buffer::PyBuffer;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::{error, read_crs, scale, slice, writable};

/// A CRS's projection, forwards or backwards.
#[pyclass(frozen, module = "xeibe_proj._native")]
pub(crate) struct Projection {
    inner: xeibe_reproject::Projection,
}

#[pymethods]
impl Projection {
    /// The projection of the CRS given as PROJJSON or WKT, with lengths
    /// in metres when `metres`.
    #[new]
    #[pyo3(signature = (text, metres = false))]
    fn new(py: Python<'_>, text: &str, metres: bool) -> PyResult<Self> {
        let crs = read_crs(text).map_err(|e| error(py, e))?;
        let inner = xeibe_reproject::Projection::new(&crs).map_err(|e| error(py, e))?;
        Ok(Projection {
            inner: if metres { inner.in_metres() } else { inner },
        })
    }

    fn inverse(&self) -> Projection {
        Projection {
            inner: self.inner.inverse(),
        }
    }

    /// Whether the CRS's side is angular (a geographic CRS).
    #[getter]
    fn crs_is_angular(&self) -> bool {
        self.inner.crs_is_angular()
    }

    /// A projection runs no operations and reads no grids.
    fn operations(&self) -> Vec<String> {
        Vec::new()
    }

    fn operation_info(&self) -> Vec<crate::OperationInfo> {
        Vec::new()
    }

    fn grids(&self) -> Vec<String> {
        Vec::new()
    }

    #[pyo3(signature = (bounds = None))]
    fn grid_requests(&self, bounds: Option<[f64; 4]>) -> Vec<(String, u64, Option<u64>)> {
        let _ = bounds;
        Vec::new()
    }

    fn add_grid_data(&self, file: &str, offset: u64, data: &[u8]) {
        let _ = (file, offset, data);
    }

    #[pyo3(signature = (x, y, z = None))]
    fn operation_index_for(&self, x: f64, y: f64, z: Option<f64>) -> Option<usize> {
        let _ = (x, y, z);
        None
    }

    fn with_operation_index(&self, index: usize) -> Option<Projection> {
        let _ = index;
        None
    }

    /// Convert in place, as `Transform.convert`.
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

    /// A box, as `Transform.convert_bounds`.
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
