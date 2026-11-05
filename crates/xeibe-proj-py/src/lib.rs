//! The native part of the Python package `xeibe_proj` (`xeibe_proj._native`).
//!
//! Kept narrow (`docs/pyproj-compat.md`, Naming and layout): EPSG records and
//! PROJJSON from `xeibe-crs`, `xeibe-reproject`'s transforms, WKT,
//! identification and reading of CRS text in pyproj's forms. The
//! pyproj-shaped API is Python over PROJJSON. Conversion takes plain
//! numbers, or works in place on float64 buffers the Python side has
//! already copied and releases the GIL (`convert`), or on GeoArrow arrays
//! through the Arrow C Data Interface (`arrow`). Errors are
//! `xeibe_proj.exceptions`' classes.

mod arrow;
mod convert;
mod geod;
mod json;
mod proj;

use pyo3::buffer::PyBuffer;
use pyo3::exceptions::{PyBufferError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};
use xeibe_reproject::wkt::{self, AxisRule, Version};
use xeibe_reproject::{AxisOrder, Crs, Error, Grids};

use crate::json::Source;

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
        Error::UnknownCode(_)
        | Error::Projjson { .. }
        | Error::Wkt { .. }
        | Error::ProjString { .. }
        | Error::UserInput { .. }
        | Error::Unsupported { .. } => "CRSError",
        _ => "ProjError",
    };
    exception(py, class, error.to_string())
}

/// The package's version: the crate's, which maturin gives the wheel.
/// Read here, not through `importlib.metadata`, which takes about half of
/// `import xeibe_proj` (22 ms) to import.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
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

/// The code an EPSG alias names, or `None`, compared by `name_key`.
#[pyfunction]
fn alias(name: &str) -> Option<u32> {
    xeibe_crs::alias(name)
}

/// A name as name searches compare it: lowercase, without accents,
/// typographic apostrophes and dashes as ASCII ones, whitespace collapsed
/// (`xeibe_crs::name_key`).
#[pyfunction]
fn name_key(name: &str) -> String {
    xeibe_crs::name_key(name)
}

/// EPSG's coordinate operations as `xeibe-crs` exports them: one JSON
/// document (`xeibe_crs::operations_json`), for describing operations.
#[pyfunction]
fn operations_json() -> &'static str {
    xeibe_crs::operations_json()
}

/// The CRSs EPSG gives as replacements of a deprecated one, in EPSG's order.
#[pyfunction]
fn replacements(code: u32) -> Vec<u32> {
    xeibe_crs::replacements(code).collect()
}

/// An EPSG object's kind by the name the Python side uses.
fn object_kind(kind: &str) -> PyResult<xeibe_crs::ObjectKind> {
    Ok(match kind {
        "ellipsoid" => xeibe_crs::ObjectKind::Ellipsoid,
        "prime_meridian" => xeibe_crs::ObjectKind::PrimeMeridian,
        "datum" => xeibe_crs::ObjectKind::Datum,
        "conversion" => xeibe_crs::ObjectKind::Conversion,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown object kind {other:?}"
            )));
        }
    })
}

/// The PROJJSON of an EPSG ellipsoid, prime meridian, datum or conversion
/// on its own (`kind` is one of those, as `"prime_meridian"`), or `None`.
#[pyfunction]
fn object_json(kind: &str, code: u32) -> PyResult<Option<&'static str>> {
    Ok(xeibe_crs::object(object_kind(kind)?, code).map(|o| o.projjson))
}

/// The EPSG objects of a kind with this name or alias, as PROJ's name
/// search orders them: `(code, PROJJSON)`.
#[pyfunction]
fn objects_by_name(kind: &str, name: &str) -> PyResult<Vec<(u32, &'static str)>> {
    Ok(xeibe_crs::objects_by_name(object_kind(kind)?, name)
        .into_iter()
        .map(|o| (o.code, o.projjson))
        .collect())
}

/// Whether two parts of CRSs on their own, PROJJSON dicts or text, are
/// equivalent (`xeibe_reproject::equivalent_parts`): `None` for parts it
/// doesn't compare.
#[pyfunction]
fn equivalent_parts(py: Python<'_>, a: Source, b: Source) -> PyResult<Option<bool>> {
    let value = |s: Source| match s {
        Source::Value(v) => Ok(v),
        Source::Text(t) => serde_json::from_str(&t).map_err(|e| {
            error(
                py,
                Error::Projjson {
                    crs: "PROJJSON".into(),
                    message: e.to_string(),
                },
            )
        }),
    };
    xeibe_reproject::equivalent_parts(&value(a)?, &value(b)?).map_err(|e| error(py, e))
}

/// PROJ-data's file for a grid EPSG names in operation `op_name`, and
/// whether `cdn.proj.org` serves it openly, or `None`.
#[pyfunction]
fn proj_data_file(epsg_name: &str, op_name: &str) -> Option<(&'static str, bool)> {
    xeibe_reproject::proj_data_file(epsg_name, op_name)
}

/// The PROJJSON of a part of a CRS given as WKT2 on its own (an
/// ellipsoid, prime meridian, datum, ensemble or conversion).
#[pyfunction]
fn wkt_object_to_projjson(py: Python<'_>, text: &str) -> PyResult<String> {
    wkt::object_to_projjson(text).map_err(|e| error(py, e))
}

/// The PROJJSON of a CRS given as WKT (any version PROJ reads but WKT2's
/// derived geographic, parametric and temporal CRSs).
#[pyfunction]
fn wkt_to_projjson(py: Python<'_>, text: &str) -> PyResult<String> {
    wkt::to_projjson(text).map_err(|e| error(py, e))
}

/// A CRS's PROJJSON, or a part's, as a dict or text ([`Source`]), as WKT:
/// `version` `"WKT2_2019"` or `"WKT1_GDAL"`, `axes` pyproj's
/// `output_axis_rule`.
#[pyfunction]
#[pyo3(signature = (projjson, version, pretty = false, axes = None))]
fn to_wkt(
    py: Python<'_>,
    projjson: Source,
    version: &str,
    pretty: bool,
    axes: Option<bool>,
) -> PyResult<String> {
    let version = match version {
        "WKT2_2019" => Version::Wkt2_2019,
        "WKT2_2015" => Version::Wkt2_2015,
        "WKT2_2019_SIMPLIFIED" => Version::Wkt2_2019Simplified,
        "WKT2_2015_SIMPLIFIED" => Version::Wkt2_2015Simplified,
        "WKT1_GDAL" => Version::Wkt1Gdal,
        other => return Err(PyValueError::new_err(format!("not written: {other}"))),
    };
    let axes = match axes {
        None => AxisRule::Auto,
        Some(true) => AxisRule::Always,
        Some(false) => AxisRule::Never,
    };
    match projjson {
        Source::Value(v) => wkt::from_projjson_value(&v, version, pretty, axes),
        Source::Text(t) => wkt::from_projjson_with(&t, version, pretty, axes),
    }
    .map_err(|e| error(py, e))
}

/// The PROJJSON of a CRS given as a PROJ string, as pyproj prepares it
/// (`+type=crs` included).
#[pyfunction]
fn proj_string_to_projjson(py: Python<'_>, text: &str) -> PyResult<String> {
    xeibe_reproject::proj_string::to_projjson(text).map_err(|e| error(py, e))
}

/// What a CRS given as text names (`xeibe_reproject::user_input::parse`),
/// as `(kind, codes, ogc)`: `("epsg", [code], None)`, `("compound",
/// [horizontal, vertical], None)`, `("ogc", [], "CRS84")`, or `("projjson"
/// | "wkt" | "proj", [], None)` for text read as it is. A code of another
/// authority is `NotImplementedError` (`_unsupported.out`), anything else
/// that isn't read a `CRSError`.
#[pyfunction]
fn user_input(
    py: Python<'_>,
    text: &str,
) -> PyResult<(&'static str, Vec<u32>, Option<&'static str>)> {
    use xeibe_reproject::user_input::{self, UserInput};
    let input = user_input::parse(text).map_err(|e| match e {
        Error::Unsupported { crs, what } => match py
            .import("xeibe_proj._unsupported")
            .and_then(|m| m.getattr("out"))
            .and_then(|out| out.call1((crs, what)))
        {
            Ok(error) => PyErr::from_value(error),
            Err(error) => error,
        },
        e => error(py, e),
    })?;
    Ok(match input {
        UserInput::Epsg(code) => ("epsg", vec![code], None),
        UserInput::Compound(horizontal, vertical) => ("compound", vec![horizontal, vertical], None),
        UserInput::Ogc(code) => ("ogc", vec![], Some(code)),
        UserInput::Projjson(_) => ("projjson", vec![], None),
        UserInput::Wkt(_) => ("wkt", vec![], None),
        UserInput::ProjString(_) => ("proj", vec![], None),
    })
}

/// A CRS's PROJJSON, as a dict or text ([`Source`]), as a PROJ string, as
/// PROJ writes it.
#[pyfunction]
fn to_proj4(py: Python<'_>, projjson: Source) -> PyResult<String> {
    match projjson {
        Source::Value(v) => xeibe_reproject::proj_string::from_projjson_value(&v),
        Source::Text(t) => xeibe_reproject::proj_string::from_projjson(&t),
    }
    .map_err(|e| error(py, e))
}

/// A CRS from a PROJJSON dict, or text: PROJJSON, a PROJ string (`proj=`
/// or `init=`, as pyproj prepares one), or else WKT.
fn read_crs(source: &Source) -> Result<Crs, Error> {
    let text = match source {
        Source::Value(v) => return Crs::from_projjson_value(v),
        Source::Text(t) => &**t,
    };
    let s = text.trim_start();
    if s.starts_with('{') {
        Crs::from_projjson(text)
    } else if !s.contains('[') && (s.contains("proj=") || s.contains("init=")) {
        Crs::from_proj_string(text)
    } else {
        Crs::from_wkt(text)
    }
}

type Record = (
    u32,
    &'static str,
    &'static str,
    u8,
    bool,
    Option<(f64, f64, f64, f64)>,
    Option<u32>,
);

/// The f64 of an f32's shortest decimal form: EPSG's `14.14`, not
/// `14.140000343322754`.
fn f(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(value as f64)
}

/// EPSG's remarks on a CRS, or `None`.
#[pyfunction]
fn crs_remarks(code: u32) -> Option<&'static str> {
    xeibe_crs::crs_remarks(code)
}

/// EPSG's remarks on a coordinate operation, or `None`.
#[pyfunction]
fn operation_remarks(code: u32) -> Option<&'static str> {
    xeibe_crs::operation_remarks(code)
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
            (
                r.code,
                r.name,
                kind,
                r.dimension,
                r.deprecated,
                area,
                r.datum,
            )
        })
        .collect()
}

/// A unit as `get_units_map` lists it: (authority, code, name, kind,
/// factor to SI, PROJ's short name, deprecated).
type UnitRow = (
    &'static str,
    String,
    &'static str,
    &'static str,
    Option<f64>,
    Option<&'static str>,
    bool,
);

/// EPSG's units of measure with PROJ's short names, then PROJ's own
/// units, ordered by authority and code as PROJ's database lists them.
#[pyfunction]
pub(crate) fn units() -> Vec<UnitRow> {
    use xeibe_crs::UnitKind;
    let epsg = xeibe_crs::UNITS_OF_MEASURE.iter().map(|u| {
        let kind = match u.kind {
            UnitKind::Length => "length",
            UnitKind::Angle => "angle",
            UnitKind::Scale => "scale",
            UnitKind::Time => "time",
        };
        let short = xeibe_reproject::proj_tables::unit_short_name(u.code);
        (
            "EPSG",
            u.code.to_string(),
            u.name,
            kind,
            u.factor,
            short,
            u.deprecated,
        )
    });
    let proj = xeibe_reproject::proj_tables::PROJ_UNITS.iter().map(|u| {
        (
            "PROJ",
            u.code.to_string(),
            u.name,
            "length",
            Some(u.factor),
            Some(u.short_name),
            false,
        )
    });
    epsg.chain(proj).collect()
}

/// Forget the grid data loaded so far (shared by every transform).
#[pyfunction]
fn clear_grids() {
    xeibe_reproject::clear_grids();
}

/// A buffer to hold while its memory is used (`None` for an empty one
/// without memory), and its start and length.
type Writable = (Option<PyBuffer<f64>>, (usize, usize));

/// A writable, C-contiguous float64 buffer of `object`. The pointer is passed as an integer
/// so the conversion can run without the GIL. An empty `array.array` has
/// no memory, whose null pointer PyO3 rejects as misaligned: it gives no
/// buffer and length 0.
fn writable(object: &Bound<'_, PyAny>, name: &str) -> PyResult<Writable> {
    let buffer = match PyBuffer::<f64>::get(object) {
        Ok(buffer) => buffer,
        Err(e)
            if e.is_instance_of::<PyBufferError>(object.py())
                && object.len().is_ok_and(|n| n == 0) =>
        {
            return Ok((None, (0, 0)));
        }
        Err(e) => return Err(e),
    };
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
    let raw = (buffer.buf_ptr() as usize, buffer.item_count());
    Ok((Some(buffer), raw))
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

/// A CRS read by `xeibe-reproject`, for comparing definitions.
#[pyclass(frozen, name = "Crs", module = "xeibe_proj._native")]
struct ParsedCrs {
    inner: Crs,
}

#[pymethods]
impl ParsedCrs {
    /// `Crs(text)`, PROJJSON as a dict or text ([`Source`]), or WKT; a
    /// `CRSError` when `xeibe-reproject` can't read it. WKT1 without `AXIS` is identified by its axes' unit
    /// only, which reading the PROJJSON made from it can't know.
    #[new]
    fn new(py: Python<'_>, text: Source) -> PyResult<Self> {
        let inner = read_crs(&text).map_err(|e| error(py, e))?;
        Ok(ParsedCrs { inner })
    }

    /// `Crs::identify`: EPSG codes with PROJ's confidence, best first.
    fn identify(&self, py: Python<'_>) -> Vec<(u32, u8)> {
        py.detach(|| self.inner.identify())
    }

    /// `Crs::equivalent`: the same definition, whatever the names and ids.
    #[pyo3(signature = (other, ignore_axis_order = false))]
    fn equivalent(&self, other: &ParsedCrs, ignore_axis_order: bool) -> bool {
        self.inner.equivalent(&other.inner, ignore_axis_order)
    }
}

/// One of the operations a transform chooses among
/// (`xeibe_reproject::OperationInfo`).
#[pyclass(frozen, get_all, module = "xeibe_proj._native")]
struct OperationInfo {
    /// PROJ's name for it.
    name: String,
    /// The EPSG operation it is, with whether it runs inverted.
    code: Option<(u32, bool)>,
    /// The EPSG transformations it runs, in order.
    steps: Vec<(u32, bool)>,
    /// Metres, or `None`.
    accuracy: Option<f64>,
    /// `(west, south, east, north)` in degrees, or `None`.
    area: Option<(f64, f64, f64, f64)>,
    area_name: Option<String>,
    /// PROJ-data's names of its grid files.
    grids: Vec<String>,
    time_dependent: bool,
    /// EPSG's scope, when it is one EPSG operation.
    scope: Option<String>,
    /// Every step, as (name, kind, code, inverse, source, target,
    /// accuracy); see [`step`].
    chain: Vec<Step>,
}

/// A CRS of a step: ("epsg" or "promoted", code, `None`) or ("compound",
/// horizontal code, vertical code).
type StepCrs = (&'static str, u32, Option<u32>);

type Step = (
    String,
    &'static str,
    Option<u32>,
    bool,
    StepCrs,
    StepCrs,
    Option<f64>,
);

/// A step of `xeibe_reproject::OperationInfo::chain` for Python. The kind
/// is "epsg" (the code an EPSG operation's), "ensemble_identity" (the code
/// EPSG's method), "projection" (the code the projected CRS's),
/// "same_datum", "ensemble_member", "vertical_conversion" or
/// "ballpark_vertical".
fn step(s: xeibe_reproject::OperationStep) -> Step {
    use xeibe_reproject::{StepCrs as C, StepKind as K};
    let crs = |c: C| match c {
        C::Epsg(code) => ("epsg", code, None),
        C::Promoted(code) => ("promoted", code, None),
        C::Compound(h, v) => ("compound", h, Some(v)),
    };
    let (kind, code, inverse) = match s.kind {
        K::Epsg { code, inverse } => ("epsg", Some(code), inverse),
        K::EnsembleIdentity { method, inverse } => ("ensemble_identity", Some(method), inverse),
        K::Projection { crs, inverse } => ("projection", Some(crs), inverse),
        K::SameDatum => ("same_datum", None, false),
        K::EnsembleMember => ("ensemble_member", None, false),
        K::VerticalConversion => ("vertical_conversion", None, false),
        K::BallparkVertical => ("ballpark_vertical", None, false),
    };
    (
        s.name,
        kind,
        code,
        inverse,
        crs(s.source),
        crs(s.target),
        s.accuracy,
    )
}

impl From<xeibe_reproject::OperationInfo> for OperationInfo {
    fn from(o: xeibe_reproject::OperationInfo) -> Self {
        OperationInfo {
            name: o.name,
            code: o.code,
            steps: o.steps,
            accuracy: o.accuracy,
            area: o.area.map(|[w, s, e, n]| (w, s, e, n)),
            area_name: o.area_name,
            grids: o.grids,
            time_dependent: o.time_dependent,
            scope: o.scope,
            chain: o.chain.into_iter().map(step).collect(),
        }
    }
}

/// A conversion between two CRSs (`xeibe_reproject::Transform`).
#[pyclass(frozen, module = "xeibe_proj._native")]
struct Transform {
    inner: xeibe_reproject::Transform,
    calls: convert::Calls,
}

impl Transform {
    fn with(inner: xeibe_reproject::Transform, degrees: (bool, bool)) -> Transform {
        let grids = !inner.grids().is_empty();
        Transform {
            inner,
            calls: convert::Calls::new(degrees, grids),
        }
    }
}

impl convert::Convert for xeibe_reproject::Transform {
    fn convert_xy(&self, x: &mut [f64], y: &mut [f64]) -> Result<(), Error> {
        xeibe_reproject::Transform::convert_xy(self, x, y)
    }

    fn convert_xyz(&self, x: &mut [f64], y: &mut [f64], z: &mut [f64]) -> Result<(), Error> {
        xeibe_reproject::Transform::convert_xyz(self, x, y, z)
    }

    fn convert_bounds(&self, bounds: [f64; 4], densify: usize) -> Result<[f64; 4], Error> {
        xeibe_reproject::Transform::convert_bounds(self, bounds, densify)
    }

    fn grids_missing(&self, bounds: Option<[f64; 4]>) -> bool {
        !self.grid_requests(bounds).is_empty()
    }
}

#[pymethods]
impl Transform {
    /// `Transform(source, target, always_xy, grids, local=None,
    /// area_of_interest=None, accuracy=None, degrees=(False, False))`: the
    /// CRSs as PROJJSON, a dict or text ([`Source`]), or WKT. `always_xy` takes and gives the values
    /// easting first (PROJ's `mustAxisOrderBeSwitchedForVisualization`).
    /// `grids=True` may use PROJ-data's grids on the CDN; with `False`, only
    /// those in `local` (PROJ-data's file names), or none.
    /// `area_of_interest` (west, south, east, north in degrees) and
    /// `accuracy` (metres) limit the operations, as PROJ's do. `degrees`
    /// says which sides `radians=True` applies to: the source and the
    /// target when geographic in degrees.
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (source, target, always_xy, grids, local = None, area_of_interest = None, accuracy = None, degrees = (false, false)))]
    fn new(
        py: Python<'_>,
        source: Source,
        target: Source,
        always_xy: bool,
        grids: bool,
        local: Option<Vec<String>>,
        area_of_interest: Option<[f64; 4]>,
        accuracy: Option<f64>,
        degrees: (bool, bool),
    ) -> PyResult<Self> {
        let order = if always_xy {
            AxisOrder::EastFirst
        } else {
            AxisOrder::Authority
        };
        let grids = match (grids, local) {
            (true, _) => Grids::ProjData,
            (false, Some(files)) if !files.is_empty() => Grids::local(files),
            (false, _) => Grids::None,
        };
        let inner = py
            .detach(|| {
                let src = read_crs(&source)?;
                let dst = read_crs(&target)?;
                let selection = xeibe_reproject::Selection {
                    area_of_interest,
                    accuracy,
                };
                xeibe_reproject::Transform::with_selection(&src, &dst, order, grids, selection)
            })
            .map_err(|e| error(py, e))?;
        Ok(Transform::with(inner, degrees))
    }

    /// The conversion the other way.
    fn inverse(&self, py: Python<'_>) -> PyResult<Transform> {
        let inner = py
            .detach(|| self.inner.inverse())
            .map_err(|e| error(py, e))?;
        let (into, out) = self.calls.degrees;
        Ok(Transform::with(inner, (out, into)))
    }

    /// Whether `always_xy` swaps the source's and the target's first two
    /// values.
    #[getter]
    fn swaps(&self) -> (bool, bool) {
        let east_first = self.inner.order() == AxisOrder::EastFirst;
        (
            east_first && self.inner.source().swapped_east_first(),
            east_first && self.inner.target().swapped_east_first(),
        )
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

    /// The operations this transform chooses among, best first.
    fn operation_info(&self) -> Vec<OperationInfo> {
        self.inner
            .operation_info()
            .into_iter()
            .map(OperationInfo::from)
            .collect()
    }

    /// The index in `operation_info()` of the operation one coordinate
    /// (in the source CRS's axis order) goes through.
    #[pyo3(signature = (x, y, z = None))]
    fn operation_index_for(&self, x: f64, y: f64, z: Option<f64>) -> Option<usize> {
        self.inner.operation_index_for(x, y, z)
    }

    /// The transform through operation `index` of `operation_info()` only,
    /// for every coordinate; `None` when there is no such operation.
    fn with_operation_index(&self, py: Python<'_>, index: usize) -> PyResult<Option<Transform>> {
        match self.inner.with_operation_index(index) {
            None => Ok(None),
            Some(Ok(inner)) => Ok(Some(Transform::with(inner, self.calls.degrees))),
            Some(Err(e)) => Err(error(py, e)),
        }
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

    /// `Transformer.transform` for plain numbers (`float` or `int`): the
    /// converted `(x, y)`, with `z` and `t` when given; `None` for anything
    /// else. `GridsNeeded` when grid data is missing (with `check_grids`).
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

    /// Convert coordinates in place: `x`, `y` and optionally `z` are
    /// writable float64 buffers of one length. `radians` takes and gives
    /// the sides in degrees in radians. Stops at the first coordinate that
    /// fails, with a `ProjError` naming it; `GridsNeeded` when grid data is
    /// missing (with `check_grids`).
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

    /// Convert a box (`Transform::convert_bounds`): `(min_x, min_y, max_x,
    /// max_y)` in, the outermost box of its densified edges out.
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

    /// The last point this thread converted: `(stamp, x, y, z)`, x and y in
    /// degrees for a side in degrees, the stamp ordering it among all
    /// conversions' last points.
    fn last_point(&self) -> Option<(u64, f64, f64, Option<f64>)> {
        self.calls.last_point()
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(epsg_version, m)?)?;
    m.add_function(wrap_pyfunction!(epsg_date, m)?)?;
    m.add_function(wrap_pyfunction!(projjson, m)?)?;
    m.add_function(wrap_pyfunction!(alias, m)?)?;
    m.add_function(wrap_pyfunction!(crs_records, m)?)?;
    m.add_function(wrap_pyfunction!(crs_remarks, m)?)?;
    m.add_function(wrap_pyfunction!(operation_remarks, m)?)?;
    m.add_function(wrap_pyfunction!(clear_grids, m)?)?;
    m.add_function(wrap_pyfunction!(replacements, m)?)?;
    m.add_function(wrap_pyfunction!(operations_json, m)?)?;
    m.add_function(wrap_pyfunction!(wkt_to_projjson, m)?)?;
    m.add_function(wrap_pyfunction!(wkt_object_to_projjson, m)?)?;
    m.add_function(wrap_pyfunction!(object_json, m)?)?;
    m.add_function(wrap_pyfunction!(objects_by_name, m)?)?;
    m.add_function(wrap_pyfunction!(name_key, m)?)?;
    m.add_function(wrap_pyfunction!(equivalent_parts, m)?)?;
    m.add_function(wrap_pyfunction!(proj_data_file, m)?)?;
    m.add_function(wrap_pyfunction!(to_wkt, m)?)?;
    m.add_function(wrap_pyfunction!(proj_string_to_projjson, m)?)?;
    m.add_function(wrap_pyfunction!(user_input, m)?)?;
    m.add_function(wrap_pyfunction!(to_proj4, m)?)?;
    m.add_class::<Transform>()?;
    m.add_class::<arrow::ArrowData>()?;
    m.add("GridsNeeded", m.py().get_type::<convert::GridsNeeded>())?;
    m.add_class::<OperationInfo>()?;
    m.add_class::<ParsedCrs>()?;
    m.add_class::<geod::Geodesic>()?;
    m.add_class::<geod::GeodesicLine>()?;
    m.add_class::<proj::Projection>()?;
    m.add_function(wrap_pyfunction!(geod::reverse_azimuth, m)?)?;
    m.add_function(wrap_pyfunction!(geod::ellipsoids, m)?)?;
    m.add_function(wrap_pyfunction!(geod::prime_meridians, m)?)?;
    m.add_function(wrap_pyfunction!(units, m)?)?;
    Ok(())
}
