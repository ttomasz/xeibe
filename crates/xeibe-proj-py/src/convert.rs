//! What `Transform` and `Projection` share for `Transformer.transform`,
//! `itransform` and `transform_bounds`: plain numbers converted without
//! Python's containers, `radians=True` for the sides in degrees, the check
//! that the grids a call needs are loaded, and the last point each thread
//! converted (`Transformer.get_last_used_operation`).

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::ThreadId;

use pyo3::buffer::PyBuffer;
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyFloat, PyInt, PyTuple};
use xeibe_reproject::{CoordinateError, Error};

use crate::{error, scale, slice, writable};

pyo3::create_exception!(
    _native,
    GridsNeeded,
    PyException,
    "Grid data the call needs isn't loaded: its only argument is the box \
     (min_x, min_y, max_x, max_y) to load for, or None for everything. \
     Load it and call again with check_grids=False."
);

/// A conversion the Python side calls.
pub(crate) trait Convert: Sync {
    fn convert_xy(&self, x: &mut [f64], y: &mut [f64]) -> Result<(), Error>;
    fn convert_xyz(&self, x: &mut [f64], y: &mut [f64], z: &mut [f64]) -> Result<(), Error>;
    fn convert_bounds(&self, bounds: [f64; 4], densify: usize) -> Result<[f64; 4], Error>;
    /// Whether grid data is still missing for coordinates within `bounds`.
    fn grids_missing(&self, bounds: Option<[f64; 4]>) -> bool;
}

/// Orders the last points of all conversions, so that of a transformer's
/// two directions the one used last is known.
static STAMP: AtomicU64 = AtomicU64::new(0);

/// The last point a thread converted: its stamp, and x, y (in degrees for
/// a side in degrees, as `radians=True` turns them) and z as they went in.
type Last = (u64, f64, f64, Option<f64>);

/// A conversion's state on the Python side.
pub(crate) struct Calls {
    /// Whether `radians=True` applies to the values going in and coming
    /// out: a side geographic in degrees.
    pub(crate) degrees: (bool, bool),
    /// Whether any candidate reads grids.
    grids: bool,
    last: Mutex<HashMap<ThreadId, Last>>,
}

impl Calls {
    pub(crate) fn new(degrees: (bool, bool), grids: bool) -> Calls {
        Calls {
            degrees,
            grids,
            last: Mutex::new(HashMap::new()),
        }
    }

    /// The factors that take the values going in to degrees and the
    /// values coming out back to radians, for `radians`.
    fn factors(&self, radians: bool) -> (Option<f64>, Option<f64>) {
        (
            (radians && self.degrees.0).then_some(180.0 / PI),
            (radians && self.degrees.1).then_some(PI / 180.0),
        )
    }

    fn record(&self, x: f64, y: f64, z: Option<f64>) {
        let stamp = STAMP.fetch_add(1, Ordering::Relaxed);
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        last.insert(std::thread::current().id(), (stamp, x, y, z));
    }

    /// The last point this thread converted, with its stamp.
    pub(crate) fn last_point(&self) -> Option<Last> {
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        last.get(&std::thread::current().id()).copied()
    }

    /// `GridsNeeded` when grid data for `bounds` is missing.
    fn check_grids<C: Convert>(&self, c: &C, bounds: Option<[f64; 4]>) -> Result<(), Missing> {
        if self.grids && c.grids_missing(bounds) {
            Err(Missing(bounds))
        } else {
            Ok(())
        }
    }
}

/// Grid data missing for a box.
struct Missing(Option<[f64; 4]>);

impl Missing {
    fn into_err(self) -> PyErr {
        GridsNeeded::new_err((self.0.map(|[a, b, c, d]| (a, b, c, d)),))
    }
}

/// The box around the finite points, or `None` when there are none.
fn extent(xs: &[f64], ys: &[f64]) -> Option<[f64; 4]> {
    let mut out = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for (&x, &y) in xs.iter().zip(ys) {
        if x.is_finite() && y.is_finite() {
            out = [out[0].min(x), out[1].min(y), out[2].max(x), out[3].max(y)];
        }
    }
    out[0].is_finite().then_some(out)
}

/// A Python `float` or `int`, not a subclass: other values (`bool`, numpy's
/// scalars) go through the containers, which decide what comes back.
fn plain_number(value: &Bound<'_, PyAny>) -> bool {
    value.is_exact_instance_of::<PyFloat>() || value.is_exact_instance_of::<PyInt>()
}

/// `Transformer.transform` for plain numbers: `(x, y)`, with `z` and `t`
/// when given, as floats. `None` for anything else, which the Python side
/// converts in buffers.
/// `GridsNeeded` (with `check_grids`) when the point needs grid data that
/// isn't loaded.
#[allow(clippy::too_many_arguments)]
pub(crate) fn transform<'py, C: Convert>(
    c: &C,
    calls: &Calls,
    py: Python<'py>,
    xx: &Bound<'py, PyAny>,
    yy: &Bound<'py, PyAny>,
    zz: Option<&Bound<'py, PyAny>>,
    tt: Option<&Bound<'py, PyAny>>,
    radians: bool,
    check_grids: bool,
) -> PyResult<Option<Bound<'py, PyTuple>>> {
    if !(plain_number(xx)
        && plain_number(yy)
        && zz.is_none_or(plain_number)
        && tt.is_none_or(plain_number))
    {
        return Ok(None);
    }
    let (mut x, mut y) = ([xx.extract::<f64>()?], [yy.extract::<f64>()?]);
    let z = zz.map(|v| v.extract::<f64>()).transpose()?;
    let t = tt.map(|v| v.extract::<f64>()).transpose()?;
    let (into, out) = calls.factors(radians);
    if let Some(f) = into {
        scale(&mut x, f);
        scale(&mut y, f);
    }
    let went_in = (x[0], y[0], z);
    let converted = match z {
        Some(z) => {
            let mut zs = [z];
            c.convert_xyz(&mut x, &mut y, &mut zs).map(|()| Some(zs[0]))
        }
        None => c.convert_xy(&mut x, &mut y).map(|()| None),
    };
    // Converted first and checked only when that fails: asking for the
    // grid requests of the point every time took most of a call once the
    // grids were loaded. A failed point is left as it went in.
    let z = match converted {
        Err(Error::Coordinate {
            reason: CoordinateError::GridNotLoaded,
            ..
        }) if check_grids => {
            return Err(Missing(extent(&x, &y)).into_err());
        }
        converted => converted.map_err(|e| error(py, e))?,
    };
    calls.record(went_in.0, went_in.1, went_in.2);
    if let Some(f) = out {
        scale(&mut x, f);
        scale(&mut y, f);
    }
    let mut values = vec![x[0], y[0]];
    values.extend(z);
    values.extend(t);
    PyTuple::new(py, values).map(Some)
}

/// Why a conversion in buffers stopped.
enum Stop {
    Grids(Missing),
    Failed(Error),
}

/// Convert buffers in place: `x`, `y` and optionally `z` are writable
/// float64 buffers of one length, in the order the conversion takes. Stops
/// at the first coordinate that fails, with a `ProjError` naming it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn convert<C: Convert>(
    c: &C,
    calls: &Calls,
    py: Python<'_>,
    x: &Bound<'_, PyAny>,
    y: &Bound<'_, PyAny>,
    z: Option<&Bound<'_, PyAny>>,
    radians: bool,
    check_grids: bool,
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
    let (into, out) = calls.factors(radians);
    let done = py.detach(|| {
        // SAFETY: the buffers are held until this returns, and they don't
        // overlap (checked above for the starts; each is its own copy).
        let (xs, ys) = unsafe { (slice(rx), slice(ry)) };
        let zs = rz.map(|rz| unsafe { slice(rz) });
        if check_grids {
            // The box in degrees, before the buffers change.
            let bounds = extent(xs, ys).map(|b| b.map(|v| v * into.unwrap_or(1.0)));
            calls.check_grids(c, bounds).map_err(Stop::Grids)?;
        }
        let went_in = (
            xs.last().map(|x| x * into.unwrap_or(1.0)),
            ys.last().map(|y| y * into.unwrap_or(1.0)),
            zs.as_deref().and_then(|zs| zs.last().copied()),
        );
        if let Some(f) = into {
            scale(xs, f);
            scale(ys, f);
        }
        match zs {
            Some(zs) => c.convert_xyz(xs, ys, zs),
            None => c.convert_xy(xs, ys),
        }
        .map_err(Stop::Failed)?;
        if let Some(f) = out {
            scale(xs, f);
            scale(ys, f);
        }
        Ok(went_in)
    });
    match done {
        Ok((Some(x), Some(y), z)) => {
            calls.record(x, y, z);
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(Stop::Grids(missing)) => Err(missing.into_err()),
        Err(Stop::Failed(e)) => Err(error(py, e)),
    }
}

/// Convert a box: `(min_x, min_y, max_x, max_y)` in, the outermost box of
/// its densified edges out.
pub(crate) fn convert_bounds<C: Convert>(
    c: &C,
    calls: &Calls,
    py: Python<'_>,
    bounds: [f64; 4],
    densify: usize,
    radians: bool,
    check_grids: bool,
) -> PyResult<(f64, f64, f64, f64)> {
    let (into, out) = calls.factors(radians);
    let bounds = bounds.map(|v| v * into.unwrap_or(1.0));
    if check_grids {
        calls
            .check_grids(c, Some(bounds))
            .map_err(Missing::into_err)?;
    }
    let [a, b, d, e] = py
        .detach(|| c.convert_bounds(bounds, densify))
        .map_err(|e| error(py, e))?
        .map(|v| v * out.unwrap_or(1.0));
    Ok((a, b, d, e))
}
