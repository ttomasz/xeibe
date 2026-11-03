//! Geodesics for `xeibe_proj.geod` (`docs/pyproj-phase2.md`, Geodesics):
//! pyproj's `_geod.pyx` loops over `xeibe_reproject::geodesic`, in place on
//! float64 buffers the Python side has already copied, without the GIL, and
//! its point functions for plain numbers.

use std::f64::consts::PI;

use pyo3::buffer::PyBuffer;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use xeibe_reproject::geodesic::{self, Mask};
use xeibe_reproject::proj_tables::{self, Shape};

use crate::convert::plain_number;
use crate::{exception, slice, writable};

const RAD2DG: f64 = 180.0 / PI;
const DG2RAD: f64 = PI / 180.0;

/// pyproj's `_reverse_azimuth`: the azimuth turned by a half turn.
fn reverse(azi: f64, half_turn: f64) -> f64 {
    if azi > 0.0 {
        azi - half_turn
    } else {
        azi + half_turn
    }
}

/// Buffers held for a call, and their starts and lengths.
type Held<const N: usize> = (Vec<Option<PyBuffer<f64>>>, [(usize, usize); N]);

/// The buffers of one call, checked: writable, C-contiguous, of one length.
fn buffers<'py, const N: usize>(
    py: Python<'py>,
    objects: [&Bound<'py, PyAny>; N],
) -> PyResult<Held<N>> {
    let mut held = Vec::with_capacity(N);
    let mut raw = [(0, 0); N];
    for (i, o) in objects.iter().enumerate() {
        let (buffer, r) = writable(o, "array")?;
        held.push(buffer);
        raw[i] = r;
    }
    if raw.iter().any(|r| r.1 != raw[0].1) {
        return Err(exception(
            py,
            "GeodError",
            "Array lengths are not the same.".into(),
        ));
    }
    let shared = (0..N).any(|i| (i + 1..N).any(|j| raw[i].0 == raw[j].0 && raw[i].1 > 0));
    if shared {
        return Err(PyValueError::new_err("the arrays share memory"));
    }
    Ok((held, raw))
}

/// An ellipsoid to solve geodesic problems on.
#[pyclass(frozen, module = "xeibe_proj._native")]
pub(crate) struct Geodesic {
    inner: geodesic::Geodesic,
}

impl Geodesic {
    /// One direct problem as `fwd` solves it: the longitude, latitude and
    /// azimuth at the end, turned by 180° with `back_azimuth`, in radians
    /// with `radians`.
    fn direct(
        &self,
        lon: f64,
        lat: f64,
        azi: f64,
        dist: f64,
        radians: bool,
        back_azimuth: bool,
    ) -> (f64, f64, f64) {
        let k = if radians { RAD2DG } else { 1.0 };
        let (lat2, lon2, azi2) = self.inner.direct(k * lat, k * lon, k * azi, dist);
        let azi2 = if back_azimuth {
            reverse(azi2, 180.0)
        } else {
            azi2
        };
        let k = if radians { DG2RAD } else { 1.0 };
        (k * lon2, k * lat2, k * azi2)
    }

    /// One inverse problem as `inv` solves it: the forward azimuth, the
    /// azimuth at the end (turned by 180° with `back_azimuth`) and the
    /// distance.
    fn inverse(
        &self,
        lon1: f64,
        lat1: f64,
        lon2: f64,
        lat2: f64,
        radians: bool,
        back_azimuth: bool,
    ) -> (f64, f64, f64) {
        let k = if radians { RAD2DG } else { 1.0 };
        let (s12, azi1, azi2) = self.inner.inverse(k * lat1, k * lon1, k * lat2, k * lon2);
        let azi2 = if back_azimuth {
            reverse(azi2, 180.0)
        } else {
            azi2
        };
        let k = if radians { DG2RAD } else { 1.0 };
        (k * azi1, k * azi2, s12)
    }
}

#[pymethods]
impl Geodesic {
    #[new]
    fn new(a: f64, f: f64) -> Self {
        Geodesic {
            inner: geodesic::Geodesic::new(a, f),
        }
    }

    /// The direct problem in place (pyproj's `Geod._fwd`): longitudes,
    /// latitudes and azimuths become those at the end points, from the
    /// distances; `back_azimuth` turns the end azimuths by 180°.
    #[allow(clippy::too_many_arguments)]
    fn fwd(
        &self,
        py: Python<'_>,
        lons: &Bound<'_, PyAny>,
        lats: &Bound<'_, PyAny>,
        azis: &Bound<'_, PyAny>,
        dists: &Bound<'_, PyAny>,
        radians: bool,
        back_azimuth: bool,
    ) -> PyResult<()> {
        let (_held, raw) = buffers(py, [lons, lats, azis, dists])?;
        py.detach(|| {
            // SAFETY: the buffers are held until this returns, and each is
            // its own copy.
            let [lons, lats, azis, dists] = raw.map(|r| unsafe { slice(r) });
            for i in 0..lons.len() {
                (lons[i], lats[i], azis[i]) =
                    self.direct(lons[i], lats[i], azis[i], dists[i], radians, back_azimuth);
            }
        });
        Ok(())
    }

    /// The inverse problem in place (pyproj's `Geod._inv`): `lons1` becomes
    /// the forward azimuths, `lats1` the azimuths at the end points (turned
    /// by 180° with `back_azimuth`) and `lons2` the distances.
    #[allow(clippy::too_many_arguments)]
    fn inv(
        &self,
        py: Python<'_>,
        lons1: &Bound<'_, PyAny>,
        lats1: &Bound<'_, PyAny>,
        lons2: &Bound<'_, PyAny>,
        lats2: &Bound<'_, PyAny>,
        radians: bool,
        back_azimuth: bool,
    ) -> PyResult<()> {
        let (_held, raw) = buffers(py, [lons1, lats1, lons2, lats2])?;
        py.detach(|| {
            // SAFETY: as in `fwd`.
            let [lons1, lats1, lons2, lats2] = raw.map(|r| unsafe { slice(r) });
            for i in 0..lons1.len() {
                (lons1[i], lats1[i], lons2[i]) = self.inverse(
                    lons1[i],
                    lats1[i],
                    lons2[i],
                    lats2[i],
                    radians,
                    back_azimuth,
                );
            }
        });
        Ok(())
    }

    /// `Geod.fwd` for plain numbers (pyproj's `_fwd_point`): `fwd`'s
    /// three results as floats, or `None` for anything else, which the
    /// Python side solves in buffers.
    fn fwd_point(
        &self,
        lon: &Bound<'_, PyAny>,
        lat: &Bound<'_, PyAny>,
        azi: &Bound<'_, PyAny>,
        dist: &Bound<'_, PyAny>,
        radians: bool,
        back_azimuth: bool,
    ) -> PyResult<Option<(f64, f64, f64)>> {
        if ![lon, lat, azi, dist].into_iter().all(plain_number) {
            return Ok(None);
        }
        Ok(Some(self.direct(
            lon.extract()?,
            lat.extract()?,
            azi.extract()?,
            dist.extract()?,
            radians,
            back_azimuth,
        )))
    }

    /// `Geod.inv` for plain numbers (pyproj's `_inv_point`), as
    /// [`Geodesic::fwd_point`] is for `fwd`.
    fn inv_point(
        &self,
        lon1: &Bound<'_, PyAny>,
        lat1: &Bound<'_, PyAny>,
        lon2: &Bound<'_, PyAny>,
        lat2: &Bound<'_, PyAny>,
        radians: bool,
        back_azimuth: bool,
    ) -> PyResult<Option<(f64, f64, f64)>> {
        if ![lon1, lat1, lon2, lat2].into_iter().all(plain_number) {
            return Ok(None);
        }
        Ok(Some(self.inverse(
            lon1.extract()?,
            lat1.extract()?,
            lon2.extract()?,
            lat2.extract()?,
            radians,
            back_azimuth,
        )))
    }

    /// The geodesic line from (`lat1`, `lon1`) along `azi1`, in degrees
    /// (`geod_lineinit`).
    fn line(&self, lat1: f64, lon1: f64, azi1: f64) -> GeodesicLine {
        GeodesicLine {
            inner: self.inner.line(lat1, lon1, azi1, Mask::NONE),
        }
    }

    /// The geodesic line between two points, in degrees, with its distance
    /// (`geod_inverseline`).
    fn inverse_line(&self, lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> GeodesicLine {
        GeodesicLine {
            inner: self.inner.inverse_line(lat1, lon1, lat2, lon2, Mask::NONE),
        }
    }

    /// The total length of the line through the points (pyproj's
    /// `Geod._line_length`); `lons` becomes the segments' lengths, and a
    /// single point's is 0.
    fn line_length(
        &self,
        py: Python<'_>,
        lons: &Bound<'_, PyAny>,
        lats: &Bound<'_, PyAny>,
        radians: bool,
    ) -> PyResult<f64> {
        let (_held, raw) = buffers(py, [lons, lats])?;
        Ok(py.detach(|| {
            // SAFETY: as in `fwd`.
            let [lons, lats] = raw.map(|r| unsafe { slice(r) });
            if lons.len() == 1 {
                lons[0] = 0.0;
                return 0.0;
            }
            let k = if radians { RAD2DG } else { 1.0 };
            let mut total = 0.0;
            for i in 0..lons.len().saturating_sub(1) {
                let (s12, _, _) =
                    self.inner
                        .inverse(k * lats[i], k * lons[i], k * lats[i + 1], k * lons[i + 1]);
                lons[i] = s12;
                total += s12;
            }
            total
        }))
    }

    /// The area (counter-clockwise positive) and perimeter of the polygon
    /// (pyproj's `Geod._polygon_area_perimeter`, `geod_polygonarea`).
    fn polygon_area_perimeter(
        &self,
        py: Python<'_>,
        lons: &Bound<'_, PyAny>,
        lats: &Bound<'_, PyAny>,
        radians: bool,
    ) -> PyResult<(f64, f64)> {
        let (_held, raw) = buffers(py, [lons, lats])?;
        Ok(py.detach(|| {
            // SAFETY: as in `fwd`.
            let [lons, lats] = raw.map(|r| unsafe { slice(r) });
            if radians {
                lons.iter_mut().for_each(|v| *v *= RAD2DG);
                lats.iter_mut().for_each(|v| *v *= RAD2DG);
            }
            self.inner.polygon_area(lats, lons)
        }))
    }
}

/// A geodesic line, for pyproj's intermediate points.
#[pyclass(frozen, module = "xeibe_proj._native")]
pub(crate) struct GeodesicLine {
    inner: geodesic::GeodesicLine,
}

#[pymethods]
impl GeodesicLine {
    /// The distance to the end point of an inverse line (`s13`).
    #[getter]
    fn distance(&self) -> f64 {
        self.inner.distance()
    }

    /// Fill `lons`, `lats` and, when given, `azis` with the first
    /// `len(lons)` points `(i + initial_idx) * del_s` along the line
    /// (pyproj's `_inv_or_fwd_intermediate` loop). With `radians` the
    /// longitudes and latitudes are in radians, the azimuths stay in
    /// degrees, as in pyproj.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (lons, lats, azis, initial_idx, del_s, radians, back_azimuth))]
    fn fill(
        &self,
        py: Python<'_>,
        lons: &Bound<'_, PyAny>,
        lats: &Bound<'_, PyAny>,
        azis: Option<&Bound<'_, PyAny>>,
        initial_idx: i64,
        del_s: f64,
        radians: bool,
        back_azimuth: bool,
    ) -> PyResult<()> {
        let (_held, raw) = buffers(py, [lons, lats])?;
        let azis = azis.map(|a| writable(a, "azis")).transpose()?;
        let raw_azis = azis.as_ref().map(|b| b.1);
        if raw_azis.is_some_and(|r| r.1 != raw[0].1) {
            return Err(exception(
                py,
                "GeodError",
                "Array lengths are not the same.".into(),
            ));
        }
        if raw_azis.is_some_and(|r| r.1 > 0 && (r.0 == raw[0].0 || r.0 == raw[1].0)) {
            return Err(PyValueError::new_err("the arrays share memory"));
        }
        py.detach(|| {
            // SAFETY: as in `Geodesic::fwd`.
            let [lons, lats] = raw.map(|r| unsafe { slice(r) });
            let mut azis = raw_azis.map(|r| unsafe { slice(r) });
            for i in 0..lons.len() {
                let s12 = (i as i64 + initial_idx) as f64 * del_s;
                let (lat2, lon2, azi2) = self.inner.position(s12);
                let k = if radians { DG2RAD } else { 1.0 };
                lats[i] = k * lat2;
                lons[i] = k * lon2;
                if let Some(azis) = azis.as_mut() {
                    azis[i] = if back_azimuth {
                        reverse(azi2, 180.0)
                    } else {
                        azi2
                    };
                }
            }
        });
        Ok(())
    }
}

/// Turn every azimuth in place by a half turn (pyproj's
/// `reverse_azimuth`).
#[pyfunction]
pub(crate) fn reverse_azimuth(azis: &Bound<'_, PyAny>, radians: bool) -> PyResult<()> {
    let (_buffer, raw) = writable(azis, "azi")?;
    // SAFETY: the buffer is held until this returns.
    let azis = unsafe { slice(raw) };
    let half_turn = if radians { PI } else { 180.0 };
    azis.iter_mut().for_each(|a| *a = reverse(*a, half_turn));
    Ok(())
}

/// PROJ's named ellipsoids: (id, a, "rf" or "b", its value, description).
#[pyfunction]
pub(crate) fn ellipsoids() -> Vec<(&'static str, f64, &'static str, f64, &'static str)> {
    proj_tables::ELLIPSOIDS
        .iter()
        .map(|e| {
            let (key, value) = match e.shape {
                Shape::InverseFlattening(rf) => ("rf", rf),
                Shape::SemiMinor(b) => ("b", b),
            };
            (e.id, e.a, key, value, e.description)
        })
        .collect()
}

/// PROJ's named prime meridians: (id, definition).
#[pyfunction]
pub(crate) fn prime_meridians() -> Vec<(&'static str, &'static str)> {
    proj_tables::PRIME_MERIDIANS.to_vec()
}
