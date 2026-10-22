//! A CRS read from PROJJSON, reduced to what conversion needs.
//!
//! Only the subset of PROJJSON that `xeibe-crs` produces from the EPSG dataset
//! is read: geographic, geocentric, projected, vertical (plain or derived by
//! a change of unit or of direction), compound and engineering CRSs. Units can
//! be given as a
//! name (`"metre"`, `"degree"`, `"unity"`) or as an object with a
//! `conversion_factor`, and values as a number or as `{value, unit}`.

use std::f64::consts::PI;
use std::sync::Arc;

use serde_json::Value;

use crate::ellipsoid::Ellipsoid;
use crate::error::Error;
use crate::methods::{self, Params, Projection};

/// A coordinate reference system that conversions can start or end in.
#[derive(Debug, Clone)]
pub struct Crs {
    pub(crate) name: String,
    pub(crate) code: Option<u32>,
    pub(crate) kind: Kind,
    pub(crate) axes: Axes,
    /// The horizontal axes are WKT1's defaults, not stated (a `GEOGCS` or
    /// `PROJCS` without `AXIS`): identification compares them by unit only.
    pub(crate) implicit_axes: bool,
}

/// What [`Crs::matches`] leaves out of equivalence, for identification.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Relax {
    /// The two horizontal axes in either order.
    pub(crate) axis_order: bool,
    /// The horizontal axes by unit only, not by direction or order.
    pub(crate) axis_directions: bool,
    /// Geodetic datums by ellipsoid and prime meridian only, not by code
    /// or name.
    pub(crate) datum_identity: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum Kind {
    /// Longitude and latitude, plus ellipsoidal height in 3D.
    Geographic { datum: Datum },
    /// Earth-centred X, Y, Z.
    Geocentric { datum: Datum },
    /// A map projection of a geographic base CRS.
    Projected {
        datum: Datum,
        projection: Arc<dyn Projection>,
        method: String,
        /// The method's EPSG code, and the parameters the projection was
        /// built from: what equivalence compares.
        method_code: Option<u32>,
        params: Params,
    },
    /// A gravity-related height or depth. Internally a height in metres,
    /// up, as the third value; the first two carry the horizontal position
    /// from the CRS before it in a pipeline, which vertical operations read.
    Vertical { datum: VerticalDatum },
    /// A horizontal CRS with a vertical one: the horizontal CRS's internal
    /// values, and the vertical CRS's height in metres, up.
    Compound {
        horizontal: Box<Crs>,
        vertical: Box<Crs>,
    },
    /// A local grid (a site, a plant, an airport), on an engineering datum
    /// with no tie to the Earth's shape. Internally (east, north) in metres
    /// in the grid's own plane, as its axes name them.
    Engineering { datum: EngineeringDatum },
}

/// The datum of an engineering CRS.
#[derive(Debug, Clone)]
pub(crate) struct EngineeringDatum {
    /// `EPSG:9301`, or the name when there is no id.
    pub(crate) key: String,
    #[allow(dead_code)] // kept for diagnostics
    pub(crate) name: String,
}

/// The datum (or datum ensemble) of a vertical CRS.
#[derive(Debug, Clone)]
pub(crate) struct VerticalDatum {
    /// `EPSG:5181`, or the name when there is no id.
    pub(crate) key: String,
    pub(crate) name: String,
}

/// The datum a CRS is on. Two CRSs can be converted between without a datum
/// transformation only when their datums are the same.
#[derive(Debug, Clone)]
pub(crate) struct Datum {
    /// `EPSG:6258`, or the name when there is no id.
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) ellipsoid: Ellipsoid,
    /// Prime meridian, radians east of Greenwich.
    pub(crate) prime_meridian: f64,
}

/// How the CRS's axes relate to the internal order, and their units.
///
/// The internal order is (longitude, latitude, height) in radians and metres
/// for geographic CRSs, (easting, northing, height) in metres for projected
/// ones and (X, Y, Z) in metres for geocentric ones. Longitudes are counted
/// from the datum's prime meridian.
///
/// Each horizontal axis is a direction in the (east, north) plane: ±east or
/// ±north for most CRSs, and for the polar ones an axis along a meridian
/// ("south along 45°E"), which is a rotation. The two horizontal axes form a
/// 2 × 2 matrix, inverted once here.
#[derive(Debug, Clone)]
pub(crate) struct Axes {
    pub(crate) axes: Vec<Axis>,
    /// Internal (east, north) from the two horizontal values, in SI units
    /// and in the order the axes come.
    inverse: [[f64; 2]; 2],
    /// The first axis's direction is `north`: xeibe-crs's `NorthOrLat`.
    first_is_north: bool,
}

/// Equal to PROJ's relative tolerance for measures (1e-10).
fn close(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-10 * a.abs().max(b.abs())
}

/// Two datums named by key (`EPSG:6258`, or the name without one), or by
/// name as PROJ compares names (`Ordnance_Survey_of_Great_Britain_1936` is
/// "Ordnance Survey of Great Britain 1936"): one CRS may come with
/// identifiers and the other without. Three
/// ensembles also match their traditional datum names, which WKT2 2015
/// (with no ensembles) gives them: PROJ's `ensembleNameToNonEnsembleName`.
fn same_key(key_a: &str, name_a: &str, key_b: &str, name_b: &str) -> bool {
    fn traditional(name: &str) -> &str {
        match name.trim() {
            "World Geodetic System 1984 ensemble" => "World Geodetic System 1984",
            "European Terrestrial Reference System 1989 ensemble" => {
                "European Terrestrial Reference System 1989"
            }
            "Greenland Reference 1996 ensemble" => "Greenland 1996",
            other => other,
        }
    }
    key_a == key_b || crate::name::equivalent(traditional(name_a), traditional(name_b))
}

impl Datum {
    fn same(&self, other: &Datum, relax: Relax) -> bool {
        (relax.datum_identity || same_key(&self.key, &self.name, &other.key, &other.name))
            && close(self.ellipsoid.a, other.ellipsoid.a)
            && close(self.ellipsoid.b, other.ellipsoid.b)
            && same_meridian(self.prime_meridian, other.prime_meridian)
    }
}

/// A projection method's code, with Krovak's north-orientated variants
/// (1041, 1043) as Krovak (9819, 1042): they differ in their axes only, so
/// where the axes are unknown (WKT1 without `AXIS`) they are one method.
pub(crate) fn oriented_alike(method: u32) -> u32 {
    match method {
        1041 => 9819,
        1043 => 1042,
        other => other,
    }
}

/// Prime meridians within 1e-8 relative, PROJ's tolerance for them
/// (measured: it accepts EPSG's own WKT for Paris, rounded to 0.040792344
/// radian, 9.6e-9 from the database's 2.5969213 grad, and rejects 1.2e-8).
fn same_meridian(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= 1e-8 * a.abs().max(b.abs())
}

impl Axis {
    fn same(&self, other: &Axis) -> bool {
        // Directions are unit vectors (or ±1): compare them absolutely.
        let near = |a: f64, b: f64| (a - b).abs() <= 1e-12;
        near(self.along[0], other.along[0])
            && near(self.along[1], other.along[1])
            && near(self.vertical, other.vertical)
            && close(self.factor, other.factor)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Axis {
    /// For a horizontal axis, the unit vector it points along in the
    /// (east, north) plane; zero for the third axis.
    along: [f64; 2],
    /// For the third axis (height, Z), +1 or −1 (down); zero otherwise.
    vertical: f64,
    /// Size of the unit in radians or metres.
    pub(crate) factor: f64,
}

impl Axes {
    pub(crate) fn dimension(&self) -> usize {
        self.axes.len()
    }

    /// The same axes, in the same order (or as `relax` allows, the two
    /// horizontal ones in either order, or in any directions with the same
    /// units).
    fn same(&self, other: &Axes, relax: Relax) -> bool {
        if self.axes.len() != other.axes.len() {
            return false;
        }
        if relax.axis_directions {
            return self.axes.iter().zip(&other.axes).all(|(x, y)| {
                close(x.factor, y.factor) && (x.vertical == 0.0) == (y.vertical == 0.0) && {
                    x.vertical == 0.0 || x.same(y)
                }
            });
        }
        let pairwise = |a: &[Axis], b: &[Axis]| a.iter().zip(b).all(|(x, y)| x.same(y));
        if pairwise(&self.axes, &other.axes) {
            return true;
        }
        if !relax.axis_order || self.axes.len() < 2 {
            return false;
        }
        let (a, b) = (&self.axes, &other.axes);
        a[0].same(&b[1]) && a[1].same(&b[0]) && pairwise(&a[2..], &b[2..])
    }

    /// Whether the first axis points north, so data in xeibe's east-first
    /// order has its first two values swapped against this CRS. Matches
    /// `xeibe_crs::FirstAxis::NorthOrLat`: a southing or westing first axis
    /// is not swapped.
    pub(crate) fn first_is_northing(&self) -> bool {
        self.first_is_north
    }

    /// The same axes with an ellipsoidal height in metres added, for a 2D CRS.
    pub(crate) fn with_height(&self) -> Axes {
        let mut axes = self.clone();
        if axes.axes.len() == 2 {
            axes.axes.push(Axis {
                along: [0.0, 0.0],
                vertical: 1.0,
                factor: 1.0,
            });
        }
        axes
    }

    /// Size of the longitude (east or west) axis's unit, for a geographic CRS.
    pub(crate) fn east_factor(&self) -> Option<f64> {
        self.axes
            .iter()
            .find(|axis| axis.along[1] == 0.0 && axis.along[0] != 0.0)
            .map(|axis| axis.factor)
    }

    /// Values in this CRS's axis order to the internal order, in SI units.
    /// A 2D CRS passes the third value through.
    pub(crate) fn to_internal(&self, values: [f64; 3]) -> [f64; 3] {
        let mut horizontal = [0.0; 2];
        let mut out = [0.0, 0.0, values[2]];
        let mut h = 0;
        for (axis, value) in self.axes.iter().zip(values) {
            if axis.vertical != 0.0 {
                out[2] = value * axis.factor * axis.vertical;
            } else {
                horizontal[h] = value * axis.factor;
                h += 1;
            }
        }
        let m = self.inverse;
        out[0] = m[0][0] * horizontal[0] + m[0][1] * horizontal[1];
        out[1] = m[1][0] * horizontal[0] + m[1][1] * horizontal[1];
        out
    }

    /// The reverse of [`Axes::to_internal`].
    pub(crate) fn to_crs(&self, v: [f64; 3]) -> [f64; 3] {
        let mut out = [0.0, 0.0, v[2]];
        for (i, axis) in self.axes.iter().enumerate() {
            out[i] = if axis.vertical != 0.0 {
                v[2] * axis.vertical / axis.factor
            } else {
                (axis.along[0] * v[0] + axis.along[1] * v[1]) / axis.factor
            };
        }
        out
    }
}

impl Crs {
    /// The CRS with this EPSG code, from the PROJJSON in `xeibe-crs`.
    pub fn from_epsg(code: u32) -> Result<Self, Error> {
        let json = xeibe_crs::projjson(code).ok_or(Error::UnknownCode(code))?;
        Self::from_projjson(json)
    }

    /// A CRS from a PROJJSON document.
    pub fn from_projjson(json: &str) -> Result<Self, Error> {
        let value: Value = serde_json::from_str(json).map_err(|e| Error::Projjson {
            crs: "PROJJSON".into(),
            message: e.to_string(),
        })?;
        Self::from_value(&value)
    }

    /// A CRS from WKT: see [`crate::wkt`] for what is read.
    pub fn from_wkt(text: &str) -> Result<Self, Error> {
        let (value, implicit_axes) = crate::wkt::read(text)?;
        let mut crs = Self::from_value(&value)?;
        crs.implicit_axes = implicit_axes;
        Ok(crs)
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self, Error> {
        let name = value["name"].as_str().unwrap_or("unnamed CRS").to_owned();
        let code = epsg_id(value);
        let label = match code {
            Some(code) => format!("EPSG:{code} ({name})"),
            None => name.clone(),
        };
        let ctx = Ctx { crs: &label };
        let kind_name = value["type"].as_str().unwrap_or("");
        let (kind, axes) = match kind_name {
            "GeographicCRS" => {
                let datum = ctx.datum(value)?;
                let axes = ctx.axes(&value["coordinate_system"], CsKind::Ellipsoidal, None)?;
                (Kind::Geographic { datum }, axes)
            }
            "GeodeticCRS" => {
                let datum = ctx.datum(value)?;
                let cs = &value["coordinate_system"];
                match cs["subtype"].as_str() {
                    Some("Cartesian") => {
                        if datum.prime_meridian != 0.0 {
                            return Err(ctx.unsupported(
                                "a geocentric CRS with a prime meridian other than Greenwich",
                            ));
                        }
                        let axes = ctx.axes(cs, CsKind::Geocentric, None)?;
                        (Kind::Geocentric { datum }, axes)
                    }
                    Some("ellipsoidal") => {
                        let axes = ctx.axes(cs, CsKind::Ellipsoidal, None)?;
                        (Kind::Geographic { datum }, axes)
                    }
                    other => {
                        return Err(ctx.unsupported(&format!(
                            "a geodetic CRS with a {} coordinate system",
                            other.unwrap_or("missing")
                        )));
                    }
                }
            }
            "ProjectedCRS" => {
                let base = &value["base_crs"];
                let datum = ctx.datum(base)?;
                let conversion = &value["conversion"];
                let method = conversion["method"]["name"]
                    .as_str()
                    .unwrap_or("unnamed method")
                    .to_owned();
                let method_code = method_id(&conversion["method"]);
                let mut params = ctx.params(conversion)?;
                methods::complete(method_code, &mut params);
                let projection = methods::build(method_code, &method, &params, &datum.ellipsoid)
                    .map_err(|e| match e {
                        methods::BuildError::UnknownMethod => {
                            ctx.unsupported(&format!("the projection method {method}"))
                        }
                        methods::BuildError::MissingParameter(code) => {
                            ctx.invalid(&format!("{method}: parameter EPSG:{code} is missing"))
                        }
                        methods::BuildError::Invalid(message) => {
                            ctx.invalid(&format!("{method}: {message}"))
                        }
                    })?;
                // EPSG's longitude-of-origin parameters, in the order methods use them.
                let origin = [8833, 8802, 8822, 8812]
                    .into_iter()
                    .find_map(|code| params.get(code).ok());
                let axes = ctx.axes(&value["coordinate_system"], CsKind::Projected, origin)?;
                (
                    Kind::Projected {
                        datum,
                        projection,
                        method,
                        method_code,
                        params,
                    },
                    axes,
                )
            }
            "VerticalCRS" => {
                let datum = ctx.vertical_datum(value)?;
                let axes = ctx.axes(&value["coordinate_system"], CsKind::Vertical, None)?;
                (Kind::Vertical { datum }, axes)
            }
            "DerivedVerticalCRS" => {
                // A change of unit or of direction (height to depth) of its
                // base: both are in the axis, so only the datum comes from
                // the base.
                let method = method_id(&value["conversion"]["method"]);
                if !matches!(method, Some(1068 | 1069 | 1104)) {
                    let name = value["conversion"]["method"]["name"].as_str().unwrap_or("");
                    return Err(ctx.unsupported(&format!("a vertical CRS derived by {name}")));
                }
                let datum = ctx.vertical_datum(&value["base_crs"])?;
                let axes = ctx.axes(&value["coordinate_system"], CsKind::Vertical, None)?;
                (Kind::Vertical { datum }, axes)
            }
            "EngineeringCRS" => {
                let datum = ctx.engineering_datum(value)?;
                let cs = &value["coordinate_system"];
                if cs["subtype"].as_str() != Some("Cartesian") {
                    return Err(ctx.unsupported(&format!(
                        "an engineering CRS with a {} coordinate system",
                        cs["subtype"].as_str().unwrap_or("missing")
                    )));
                }
                let axes = ctx.axes(cs, CsKind::Engineering, None)?;
                (Kind::Engineering { datum }, axes)
            }
            "CompoundCRS" => {
                let components = value["components"].as_array().map(Vec::as_slice);
                let Some([h, v]) = components else {
                    return Err(ctx.unsupported("a compound CRS without two components"));
                };
                return Crs::compound(&Crs::from_value(h)?, &Crs::from_value(v)?).map(|mut c| {
                    c.name = name;
                    c.code = code;
                    c
                });
            }
            other => {
                return Err(ctx.unsupported(&format!(
                    "a CRS of type {}",
                    if other.is_empty() { "unknown" } else { other }
                )));
            }
        };
        Ok(Crs {
            name,
            code,
            kind,
            axes,
            implicit_axes: false,
        })
    }

    /// A compound CRS of a horizontal CRS (geographic 2D or projected) and a
    /// vertical one, as GML names it with both codes
    /// (`urn:ogc:def:crs,crs:EPSG::4258,crs:EPSG::7837`). It has no code of
    /// its own; its name is PROJ's, "horizontal + vertical".
    pub fn compound(horizontal: &Crs, vertical: &Crs) -> Result<Crs, Error> {
        let name = format!("{} + {}", horizontal.name, vertical.name);
        let unsupported = |what: &str| Error::Unsupported {
            crs: name.clone(),
            what: what.to_owned(),
        };
        let two_d = horizontal.dimension() == 2
            && matches!(
                horizontal.kind,
                Kind::Geographic { .. } | Kind::Projected { .. }
            );
        if !two_d {
            return Err(unsupported(
                "a compound CRS whose first part is not a 2D geographic or projected CRS",
            ));
        }
        if !matches!(vertical.kind, Kind::Vertical { .. }) {
            return Err(unsupported(
                "a compound CRS whose second part is not a vertical CRS",
            ));
        }
        let mut axes = horizontal.axes.clone();
        axes.axes.extend(vertical.axes.axes.iter().copied());
        Ok(Crs {
            name,
            code: None,
            kind: Kind::Compound {
                horizontal: Box::new(horizontal.clone()),
                vertical: Box::new(vertical.clone()),
            },
            axes,
            implicit_axes: horizontal.implicit_axes,
        })
    }

    /// EPSG's name for the CRS.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The EPSG code, when the PROJJSON has one.
    pub fn code(&self) -> Option<u32> {
        self.code
    }

    /// Number of axes, 2 or 3.
    pub fn dimension(&self) -> usize {
        self.axes.dimension()
    }

    /// EPSG's name for the projection method, for a projected CRS (or a
    /// compound CRS whose horizontal part is one).
    pub fn method(&self) -> Option<&str> {
        match &self.horizontal().kind {
            Kind::Projected { method, .. } => Some(method),
            _ => None,
        }
    }

    /// Whether both CRSs define the same coordinates: the same kind of CRS,
    /// on the same datum (by EPSG code, else by name) with the same
    /// ellipsoid and prime meridian, the same projection method and
    /// parameters, and the same axes in the same units. Names and codes
    /// aren't compared, as PROJ's `proj_is_equivalent_to` doesn't, and
    /// numbers agree to PROJ's relative 1e-10. With `ignore_axis_order`,
    /// CRSs whose two horizontal axes differ only in order are equivalent
    /// too (EPSG:4326 and OGC:CRS84).
    pub fn equivalent(&self, other: &Crs, ignore_axis_order: bool) -> bool {
        self.matches(
            other,
            Relax {
                axis_order: ignore_axis_order,
                ..Relax::default()
            },
        )
    }

    /// [`Crs::equivalent`], leaving out what `relax` says. The axis
    /// relaxations apply to the horizontal axes; a compound CRS's vertical
    /// part, and a vertical or engineering datum, are always compared in
    /// full.
    pub(crate) fn matches(&self, other: &Crs, relax: Relax) -> bool {
        let same_axes = self.axes.same(&other.axes, relax);
        match (&self.kind, &other.kind) {
            (Kind::Geographic { datum: a }, Kind::Geographic { datum: b })
            | (Kind::Geocentric { datum: a }, Kind::Geocentric { datum: b }) => {
                a.same(b, relax) && same_axes
            }
            (
                Kind::Projected {
                    datum: a,
                    method: ma,
                    method_code: ca,
                    params: pa,
                    ..
                },
                Kind::Projected {
                    datum: b,
                    method: mb,
                    method_code: cb,
                    params: pb,
                    ..
                },
            ) => {
                // By name too: EPSG names two deprecated methods as their
                // replacements (9823 and 1029, 9842 and 1028), so a method
                // read by name has the replacement's code.
                let same_method = ma.eq_ignore_ascii_case(mb)
                    || match (ca, cb) {
                        (Some(x), Some(y)) if relax.axis_directions => {
                            oriented_alike(*x) == oriented_alike(*y)
                        }
                        (Some(x), Some(y)) => x == y,
                        _ => false,
                    };
                a.same(b, relax) && same_method && pa.same(pb) && same_axes
            }
            (Kind::Vertical { datum: a }, Kind::Vertical { datum: b }) => {
                same_key(&a.key, &a.name, &b.key, &b.name) && same_axes
            }
            (Kind::Engineering { datum: a }, Kind::Engineering { datum: b }) => {
                same_key(&a.key, &a.name, &b.key, &b.name) && same_axes
            }
            (
                Kind::Compound {
                    horizontal: ha,
                    vertical: va,
                },
                Kind::Compound {
                    horizontal: hb,
                    vertical: vb,
                },
            ) => ha.matches(hb, relax) && va.equivalent(vb, false),
            _ => false,
        }
    }

    /// PROJ's synthetic 3D twin of a geographic 2D CRS: the same CRS with an
    /// ellipsoidal height, and no EPSG code of its own.
    pub(crate) fn promoted_to_3d(&self) -> Crs {
        let mut crs = self.clone();
        crs.axes = self.axes.with_height();
        crs.code = None;
        crs
    }

    /// A full turn in the unit of the longitude axis, for a geographic CRS.
    pub(crate) fn longitude_turn(&self) -> Option<f64> {
        match self.horizontal().kind {
            Kind::Geographic { .. } => self
                .axes
                .east_factor()
                .map(|factor| std::f64::consts::TAU / factor),
            _ => None,
        }
    }

    /// The geodetic datum, of the horizontal part for a compound CRS; none
    /// for a vertical or an engineering CRS.
    pub(crate) fn datum(&self) -> Option<&Datum> {
        match &self.kind {
            Kind::Geographic { datum } | Kind::Geocentric { datum } => Some(datum),
            Kind::Projected { datum, .. } => Some(datum),
            Kind::Compound { horizontal, .. } => horizontal.datum(),
            Kind::Vertical { .. } | Kind::Engineering { .. } => None,
        }
    }

    /// The datum of an engineering CRS.
    pub(crate) fn engineering_datum(&self) -> Option<&EngineeringDatum> {
        match &self.kind {
            Kind::Engineering { datum } => Some(datum),
            _ => None,
        }
    }

    /// +1 when the height axis points up, −1 for a depth axis; +1 without one.
    pub(crate) fn height_direction(&self) -> f64 {
        self.axes
            .axes
            .iter()
            .find(|a| a.vertical != 0.0)
            .map_or(1.0, |a| a.vertical)
    }

    /// The horizontal CRS: the CRS itself, or a compound CRS's first part.
    pub(crate) fn horizontal(&self) -> &Crs {
        match &self.kind {
            Kind::Compound { horizontal, .. } => horizontal,
            _ => self,
        }
    }

    /// The vertical datum, of a vertical CRS or a compound CRS's second part.
    pub(crate) fn vertical_datum(&self) -> Option<&VerticalDatum> {
        match &self.kind {
            Kind::Vertical { datum } => Some(datum),
            Kind::Compound { vertical, .. } => vertical.vertical_datum(),
            _ => None,
        }
    }

    pub(crate) fn label(&self) -> String {
        match self.code {
            Some(code) => format!("EPSG:{code}"),
            None => self.name.clone(),
        }
    }
}

fn epsg_id(value: &Value) -> Option<u32> {
    let id = &value["id"];
    if id["authority"].as_str() == Some("EPSG") {
        id["code"]
            .as_u64()
            .and_then(|code| u32::try_from(code).ok())
    } else {
        None
    }
}

/// A method's EPSG code: its id, else its EPSG name, for definitions such
/// as WKT that may name it without one.
fn method_id(method: &Value) -> Option<u32> {
    epsg_id(method).or_else(|| method["name"].as_str().and_then(xeibe_crs::method_code))
}

/// The components of a diagonal direction (northEast and the like).
const DIAGONAL: f64 = std::f64::consts::FRAC_1_SQRT_2;

#[derive(Clone, Copy, PartialEq)]
enum CsKind {
    Ellipsoidal,
    Projected,
    Geocentric,
    Vertical,
    /// A local grid: compass directions name the grid's own axes, the
    /// diagonal ones included, and a platform's axes are its own.
    Engineering,
}

#[derive(Clone, Copy)]
enum UnitKind {
    Angle,
    Length,
    Scale,
}

struct Ctx<'a> {
    crs: &'a str,
}

impl Ctx<'_> {
    fn invalid(&self, message: &str) -> Error {
        Error::Projjson {
            crs: self.crs.to_owned(),
            message: message.to_owned(),
        }
    }

    fn unsupported(&self, what: &str) -> Error {
        Error::Unsupported {
            crs: self.crs.to_owned(),
            what: what.to_owned(),
        }
    }

    fn engineering_datum(&self, crs: &Value) -> Result<EngineeringDatum, Error> {
        let datum = &crs["datum"];
        if !datum.is_object() {
            return Err(self.invalid("no datum"));
        }
        let name = datum["name"].as_str().unwrap_or("unnamed datum").to_owned();
        let key = match epsg_id(datum) {
            Some(code) => format!("EPSG:{code}"),
            None => name.clone(),
        };
        Ok(EngineeringDatum { key, name })
    }

    fn vertical_datum(&self, crs: &Value) -> Result<VerticalDatum, Error> {
        let datum = match (&crs["datum"], &crs["datum_ensemble"]) {
            (Value::Object(_), _) => &crs["datum"],
            (_, Value::Object(_)) => &crs["datum_ensemble"],
            _ => return Err(self.invalid("no datum or datum ensemble")),
        };
        let name = datum["name"].as_str().unwrap_or("unnamed datum").to_owned();
        let key = match epsg_id(datum) {
            Some(code) => format!("EPSG:{code}"),
            None => name.clone(),
        };
        Ok(VerticalDatum { key, name })
    }

    fn datum(&self, crs: &Value) -> Result<Datum, Error> {
        let datum = match (&crs["datum"], &crs["datum_ensemble"]) {
            (Value::Object(_), _) => &crs["datum"],
            (_, Value::Object(_)) => &crs["datum_ensemble"],
            _ => return Err(self.invalid("no datum or datum ensemble")),
        };
        let name = datum["name"].as_str().unwrap_or("unnamed datum").to_owned();
        let key = match epsg_id(datum) {
            Some(code) => format!("EPSG:{code}"),
            None => name.clone(),
        };
        let ellipsoid = self.ellipsoid(&datum["ellipsoid"])?;
        // A datum ensemble carries no prime meridian; EPSG's are all Greenwich.
        let prime_meridian = match &datum["prime_meridian"] {
            Value::Null => 0.0,
            pm => self.value(&pm["longitude"], UnitKind::Angle)?,
        };
        Ok(Datum {
            key,
            name,
            ellipsoid,
            prime_meridian,
        })
    }

    fn ellipsoid(&self, value: &Value) -> Result<Ellipsoid, Error> {
        if value.is_null() {
            return Err(self.invalid("no ellipsoid"));
        }
        if !value["radius"].is_null() {
            return Ok(Ellipsoid::sphere(
                self.value(&value["radius"], UnitKind::Length)?,
            ));
        }
        let a = self.value(&value["semi_major_axis"], UnitKind::Length)?;
        if !value["inverse_flattening"].is_null() {
            let rf = self.value(&value["inverse_flattening"], UnitKind::Scale)?;
            Ok(Ellipsoid::from_inverse_flattening(a, rf))
        } else if !value["semi_minor_axis"].is_null() {
            let b = self.value(&value["semi_minor_axis"], UnitKind::Length)?;
            Ok(Ellipsoid::from_semi_axes(a, b))
        } else {
            Err(self.invalid("the ellipsoid has neither a flattening nor a semi-minor axis"))
        }
    }

    /// A number, or `{value, unit}`, in SI units (radians, metres).
    fn value(&self, value: &Value, kind: UnitKind) -> Result<f64, Error> {
        match value {
            Value::Number(n) => {
                let n = n.as_f64().unwrap_or(f64::NAN);
                // A bare number takes PROJJSON's default unit for its kind.
                Ok(match kind {
                    UnitKind::Angle => n * PI / 180.0,
                    UnitKind::Length | UnitKind::Scale => n,
                })
            }
            Value::Object(_) => {
                let n = value["value"]
                    .as_f64()
                    .ok_or_else(|| self.invalid("a value without a number"))?;
                Ok(n * self.unit(&value["unit"], kind)?)
            }
            _ => Err(self.invalid("a missing number")),
        }
    }

    /// Size of a unit in radians, metres or as a plain factor.
    fn unit(&self, unit: &Value, kind: UnitKind) -> Result<f64, Error> {
        match unit {
            Value::String(name) => match name.as_str() {
                "degree" => Ok(PI / 180.0),
                "metre" | "unity" => Ok(1.0),
                "radian" => Ok(1.0),
                other => Err(self.unsupported(&format!("the unit {other}"))),
            },
            Value::Object(_) => unit["conversion_factor"]
                .as_f64()
                .ok_or_else(|| self.invalid("a unit without a conversion factor")),
            // PROJJSON's default for the kind.
            Value::Null => Ok(match kind {
                UnitKind::Angle => PI / 180.0,
                UnitKind::Length | UnitKind::Scale => 1.0,
            }),
            _ => Err(self.invalid("a malformed unit")),
        }
    }

    fn params(&self, conversion: &Value) -> Result<Params, Error> {
        let mut params = Params::default();
        for param in conversion["parameters"].as_array().into_iter().flatten() {
            // By name when the definition gives no id, as WKT may.
            let code = epsg_id(param)
                .or_else(|| param["name"].as_str().and_then(xeibe_crs::parameter_code));
            let Some(code) = code else {
                let name = param["name"].as_str().unwrap_or("unnamed");
                return Err(self.unsupported(&format!("the parameter {name} without an EPSG id")));
            };
            let raw = param["value"]
                .as_f64()
                .ok_or_else(|| self.invalid("a parameter without a number"))?;
            let factor = self.unit(&param["unit"], UnitKind::Scale)?;
            params.push(code, raw * factor);
        }
        Ok(params)
    }

    /// `origin` is the projection's longitude of origin, for axes given
    /// along a meridian.
    fn axes(&self, cs: &Value, kind: CsKind, origin: Option<f64>) -> Result<Axes, Error> {
        let list = cs["axis"]
            .as_array()
            .ok_or_else(|| self.invalid("a coordinate system without axes"))?;
        let mut axes = Vec::with_capacity(list.len());
        for axis in list {
            let direction = axis["direction"].as_str().unwrap_or("");
            let meridian = &axis["meridian"];
            let (along, vertical) = if !meridian.is_null() && kind == CsKind::Projected {
                // Polar CRSs: an axis pointing along a meridian, away from
                // the north pole ("south") or from the south pole ("north").
                let m = self.value(&meridian["longitude"], UnitKind::Angle)?;
                let d = m - origin.ok_or_else(|| {
                    self.unsupported("an axis along a meridian without a longitude of origin")
                })?;
                match direction {
                    "south" => ([d.sin(), -d.cos()], 0.0),
                    "north" => ([d.sin(), d.cos()], 0.0),
                    _ => {
                        return Err(self.unsupported(&format!(
                            "the axis direction {direction} along a meridian"
                        )));
                    }
                }
            } else {
                match (kind, direction) {
                    (CsKind::Geocentric, "geocentricX") => ([1.0, 0.0], 0.0),
                    (CsKind::Geocentric, "geocentricY") => ([0.0, 1.0], 0.0),
                    (CsKind::Geocentric, "geocentricZ") => ([0.0, 0.0], 1.0),
                    (CsKind::Ellipsoidal | CsKind::Projected, "east") => ([1.0, 0.0], 0.0),
                    (CsKind::Ellipsoidal | CsKind::Projected, "west") => ([-1.0, 0.0], 0.0),
                    (CsKind::Ellipsoidal | CsKind::Projected, "north") => ([0.0, 1.0], 0.0),
                    (CsKind::Ellipsoidal | CsKind::Projected, "south") => ([0.0, -1.0], 0.0),
                    (CsKind::Ellipsoidal | CsKind::Projected, "up") => ([0.0, 0.0], 1.0),
                    (CsKind::Ellipsoidal | CsKind::Projected, "down") => ([0.0, 0.0], -1.0),
                    (CsKind::Engineering, "east") => ([1.0, 0.0], 0.0),
                    (CsKind::Engineering, "west") => ([-1.0, 0.0], 0.0),
                    (CsKind::Engineering, "north") => ([0.0, 1.0], 0.0),
                    (CsKind::Engineering, "south") => ([0.0, -1.0], 0.0),
                    (CsKind::Engineering, "northEast") => ([DIAGONAL, DIAGONAL], 0.0),
                    (CsKind::Engineering, "northWest") => ([-DIAGONAL, DIAGONAL], 0.0),
                    (CsKind::Engineering, "southEast") => ([DIAGONAL, -DIAGONAL], 0.0),
                    (CsKind::Engineering, "southWest") => ([-DIAGONAL, -DIAGONAL], 0.0),
                    // A moving platform's frame, right-handed: starboard,
                    // forward, up.
                    (CsKind::Engineering, "starboard") => ([1.0, 0.0], 0.0),
                    (CsKind::Engineering, "port") => ([-1.0, 0.0], 0.0),
                    (CsKind::Engineering, "forward") => ([0.0, 1.0], 0.0),
                    (CsKind::Engineering, "aft") => ([0.0, -1.0], 0.0),
                    (CsKind::Engineering, "up") => ([0.0, 0.0], 1.0),
                    (CsKind::Engineering, "down") => ([0.0, 0.0], -1.0),
                    (CsKind::Vertical, "up") => ([0.0, 0.0], 1.0),
                    (CsKind::Vertical, "down") => ([0.0, 0.0], -1.0),
                    _ => {
                        return Err(self.unsupported(&format!("the axis direction {direction}")));
                    }
                }
            };
            let unit_kind = if kind == CsKind::Ellipsoidal && vertical == 0.0 {
                UnitKind::Angle
            } else {
                UnitKind::Length
            };
            let factor = self.unit(&axis["unit"], unit_kind)?;
            axes.push(Axis {
                along,
                vertical,
                factor,
            });
        }
        let horizontal: Vec<[f64; 2]> = axes
            .iter()
            .filter(|axis| axis.vertical == 0.0)
            .map(|axis| axis.along)
            .collect();
        let verticals = axes.len() - horizontal.len();
        if kind == CsKind::Vertical {
            if axes.len() != 1 || verticals != 1 {
                return Err(self.unsupported("a vertical coordinate system without one axis"));
            }
            return Ok(Axes {
                axes,
                inverse: [[1.0, 0.0], [0.0, 1.0]],
                first_is_north: false,
            });
        }
        if horizontal.len() != 2 || verticals > 1 {
            return Err(self.unsupported("a coordinate system without two horizontal axes"));
        }
        let [[a, b], [c, d]] = [horizontal[0], horizontal[1]];
        let det = a * d - b * c;
        if det.abs() < 1e-9 {
            return Err(self.invalid("two axes in the same direction"));
        }
        // The matrix maps (east, north) to the two values; invert it.
        let inverse = [[d / det, -b / det], [-c / det, a / det]];
        let first_is_north = list
            .first()
            .is_some_and(|axis| axis["direction"].as_str() == Some("north"));
        Ok(Axes {
            axes,
            inverse,
            first_is_north,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AxisOrder::EastFirst` must swap exactly the CRSs whose coordinates a
    /// read writes swapped, i.e. those `xeibe-crs` marks `NorthOrLat`.
    #[test]
    fn east_first_swaps_what_xeibe_crs_marks_north_first() {
        let mut checked = 0;
        for record in xeibe_crs::CRS {
            let Ok(crs) = Crs::from_epsg(record.code) else {
                continue;
            };
            let north = record.first_axis == xeibe_crs::FirstAxis::NorthOrLat;
            assert_eq!(crs.axes.first_is_northing(), north, "EPSG:{}", record.code);
            checked += 1;
        }
        assert!(checked > 8000, "{checked}");
    }
}
