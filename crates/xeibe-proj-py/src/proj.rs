//! The legacy `Proj` (`docs/pyproj-phase2.md`, `Proj`): a CRS's own
//! projection, with the methods `xeibe_proj.transformer` calls on a
//! native transform, so that `Proj` is a `Transformer`.

use pyo3::prelude::*;
use pyo3::types::PyTuple;
use xeibe_reproject::Error;

use crate::convert;
use crate::json::Source;
use crate::{error, read_crs};

/// A CRS's projection, forwards or backwards.
#[pyclass(frozen, module = "xeibe_proj._native")]
pub(crate) struct Projection {
    inner: xeibe_reproject::Projection,
    calls: convert::Calls,
}

impl Projection {
    /// Degrees on the geographic side, and on the CRS's when it is
    /// geographic, as PROJ's angular input and output.
    fn with(inner: xeibe_reproject::Projection) -> Projection {
        let crs = inner.crs_is_angular();
        let degrees = if inner.is_inverse() {
            (crs, true)
        } else {
            (true, crs)
        };
        Projection {
            inner,
            calls: convert::Calls::new(degrees, false),
        }
    }
}

impl convert::Convert for xeibe_reproject::Projection {
    fn convert_xy(&self, x: &mut [f64], y: &mut [f64]) -> Result<(), Error> {
        xeibe_reproject::Projection::convert_xy(self, x, y)
    }

    fn convert_xyz(&self, x: &mut [f64], y: &mut [f64], z: &mut [f64]) -> Result<(), Error> {
        xeibe_reproject::Projection::convert_xyz(self, x, y, z)
    }

    fn convert_bounds(&self, bounds: [f64; 4], densify: usize) -> Result<[f64; 4], Error> {
        xeibe_reproject::Projection::convert_bounds(self, bounds, densify)
    }

    /// A projection reads no grids.
    fn grids_missing(&self, _bounds: Option<[f64; 4]>) -> bool {
        false
    }
}

#[pymethods]
impl Projection {
    /// The projection of the CRS given as PROJJSON, a dict or text
    /// ([`Source`]), or WKT, with lengths
    /// in metres when `metres`.
    #[new]
    #[pyo3(signature = (text, metres = false))]
    fn new(py: Python<'_>, text: Source, metres: bool) -> PyResult<Self> {
        let crs = read_crs(&text).map_err(|e| error(py, e))?;
        let inner = xeibe_reproject::Projection::new(&crs).map_err(|e| error(py, e))?;
        Ok(Projection::with(if metres {
            inner.in_metres()
        } else {
            inner
        }))
    }

    fn inverse(&self) -> Projection {
        Projection::with(self.inner.inverse())
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

    /// As `Transform.transform`.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (xx, yy, zz = None, tt = None, radians = false, check_grids = true))]
    fn transform<'py>(
        &self,
        py: Python<'py>,
        xx: &Bound<'py, PyAny>,
        yy: &Bound<'py, PyAny>,
        zz: Option<&Bound<'py, PyAny>>,
        tt: Option<&Bound<'py, PyAny>>,
        radians: bool,
        check_grids: bool,
    ) -> PyResult<Option<Bound<'py, PyTuple>>> {
        convert::transform(
            &self.inner,
            &self.calls,
            py,
            xx,
            yy,
            zz,
            tt,
            radians,
            check_grids,
        )
    }

    /// Convert in place, as `Transform.convert`.
    #[pyo3(signature = (x, y, z = None, radians = false, check_grids = true))]
    fn convert(
        &self,
        py: Python<'_>,
        x: &Bound<'_, PyAny>,
        y: &Bound<'_, PyAny>,
        z: Option<&Bound<'_, PyAny>>,
        radians: bool,
        check_grids: bool,
    ) -> PyResult<()> {
        convert::convert(&self.inner, &self.calls, py, x, y, z, radians, check_grids)
    }

    /// A box, as `Transform.convert_bounds`.
    #[pyo3(signature = (bounds, densify, radians = false, check_grids = true))]
    fn convert_bounds(
        &self,
        py: Python<'_>,
        bounds: [f64; 4],
        densify: usize,
        radians: bool,
        check_grids: bool,
    ) -> PyResult<(f64, f64, f64, f64)> {
        convert::convert_bounds(
            &self.inner,
            &self.calls,
            py,
            bounds,
            densify,
            radians,
            check_grids,
        )
    }

    /// As `Transform.last_point`.
    fn last_point(&self) -> Option<(u64, f64, f64, Option<f64>)> {
        self.calls.last_point()
    }
}
