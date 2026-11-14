//! The Helmert family: geocentric translations, Position Vector and
//! Coordinate Frame rotations, and Molodensky-Badekas, each in its geocentric,
//! geog2D and geog3D domain.
//!
//! Ported from PROJ 9.8.1 `src/transformations/helmert.cpp` (MIT, see
//! PROJ-NOTICE.md), with the small-angle rotation matrix PROJ uses unless
//! `+exact` is given, and wrapped the way PROJ exports these EPSG methods: a
//! geographic-domain operation goes to geocentric on the source ellipsoid,
//! applies the Helmert step and comes back on the target ellipsoid. In the
//! geog2D domain the height is carried through unchanged (PROJ's
//! `push`/`pop +v_3`).
//!
//! The time-dependent methods (1053 to 1058) carry a rate for each of the
//! seven parameters and a reference epoch. For a coordinate with an epoch
//! (a decimal year; PROJ's fourth value, `t`) the parameters are those at
//! that epoch, `P + rate × (epoch − reference epoch)`, as PROJ's
//! `update_parameters` computes them; without one they are those at the
//! reference epoch, the rates unused, which is what PROJ does too.
//!
//! The time-specific methods (1065, 1066), which PROJ imports but can't run,
//! are a plain geocentric Helmert that holds at one epoch, the
//! transformation reference epoch, and run only when forced
//! (`operation::only_forced`). EPSG moves a point there and back by its
//! own velocities (Guidance Note 7-2), which a coordinate here doesn't
//! carry: a coordinate without an epoch is taken to be at it, and one at
//! another epoch fails (`CoordinateError::EpochOutOfRange`), so the choice
//! goes on to the next operation.
//!
//! A geographic-domain Helmert of zeros between ellipsoids within
//! [`SAME_ELLIPSOID_M`] of each other, on one prime meridian, changes the CRS
//! only: "ETRS89 to WGS 84 (1)", "NAD83 to WGS 84 (1)" and a zero `TOWGS84`
//! state that the two datums coincide, and through geocentric coordinates
//! they would move latitude and height by the 0.1 mm between GRS 1980 and
//! WGS 84 (`docs/reprojection.md`, deviations).

use std::sync::Arc;

use super::{Method, MethodError, YEAR, param, required};
use crate::crs::{Crs, Kind};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;
use crate::geocentric::{cartesian, geodetic};
use crate::registry::{Operation, ParamValue};

/// How far apart, in each semi-axis, two ellipsoids may be for a Helmert of
/// zeros between them to keep the coordinates. Of EPSG's ellipsoids only
/// GRS 1980 and CGCS2000 against WGS 84 and GEM 10C are this close and not
/// equal (0.105 mm); next are the Clarke 1880 (Arc) and (RGS) at 0.19 mm.
pub(crate) const SAME_ELLIPSOID_M: f64 = 1.5e-4;

/// EPSG method codes handled here.
pub(super) const METHODS: &[u32] = &[
    // Geocentric translations: geocentric, geog2D, geog3D domain.
    1031, 9603, 1035, //
    // Position Vector.
    1033, 9606, 1037, //
    // Coordinate Frame.
    1032, 9607, 1038, //
    // Molodensky-Badekas, Coordinate Frame convention.
    1034, 9636, 1039, //
    // Molodensky-Badekas, Position Vector convention.
    1061, 1063, 1062, //
    // Time-dependent Position Vector and Coordinate Frame, applied at the
    // coordinate's epoch, or without one at their reference epoch, as in
    // PROJ.
    1053, 1054, 1055, 1056, 1057, 1058, //
    // Time-specific Position Vector and Coordinate Frame (geocentric),
    // at their one epoch.
    1065, 1066, //
    // Coordinate Frame with the full (exact) rotation matrix.
    1132, 1133, 1140, 1149,
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Domain {
    Geocentric,
    Geog2d,
    Geog3d,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind7 {
    Translation,
    PositionVector,
    CoordinateFrame,
    BadekasCoordinateFrame,
    BadekasPositionVector,
}

fn classify(method: u32) -> Option<(Kind7, Domain)> {
    use Domain::*;
    use Kind7::*;
    Some(match method {
        1031 => (Translation, Geocentric),
        9603 => (Translation, Geog2d),
        1035 => (Translation, Geog3d),
        1033 => (PositionVector, Geocentric),
        9606 => (PositionVector, Geog2d),
        1037 => (PositionVector, Geog3d),
        1032 => (CoordinateFrame, Geocentric),
        9607 => (CoordinateFrame, Geog2d),
        1038 => (CoordinateFrame, Geog3d),
        1034 => (BadekasCoordinateFrame, Geocentric),
        9636 => (BadekasCoordinateFrame, Geog2d),
        1039 => (BadekasCoordinateFrame, Geog3d),
        1061 => (BadekasPositionVector, Geocentric),
        1063 => (BadekasPositionVector, Geog2d),
        1062 => (BadekasPositionVector, Geog3d),
        1053 => (PositionVector, Geocentric),
        1054 => (PositionVector, Geog2d),
        1055 => (PositionVector, Geog3d),
        1056 => (CoordinateFrame, Geocentric),
        1057 => (CoordinateFrame, Geog2d),
        1058 => (CoordinateFrame, Geog3d),
        1065 => (PositionVector, Geocentric),
        1066 => (CoordinateFrame, Geocentric),
        1132 => (CoordinateFrame, Geocentric),
        1133 => (CoordinateFrame, Geog2d),
        1140 => (CoordinateFrame, Geog3d),
        // Coordinate Frame (geog3D to compound): the height goes into the
        // compound CRS's vertical part.
        1149 => (CoordinateFrame, Geog3d),
        _ => return None,
    })
}

/// The 3D Helmert step on geocentric coordinates (PROJ's `helmert` and
/// `molobadekas` with constant parameters).
#[derive(Debug, Clone)]
pub(crate) struct Helmert {
    /// Translation; for Molodensky-Badekas it includes the reference point,
    /// as in PROJ.
    t: [f64; 3],
    /// Rotation matrix, small-angle approximation, already transposed for
    /// the Position Vector convention.
    r: [[f64; 3]; 3],
    /// Scale difference as a fraction (ppm × 1e-6).
    s: f64,
    /// Molodensky-Badekas reference point.
    refp: [f64; 3],
    no_rotation: bool,
}

impl Helmert {
    /// `rot` in radians, `s` as a fraction, `position_vector` picks the
    /// rotation convention, `exact` the full rotation matrix instead of the
    /// small-angle one.
    pub(crate) fn new(
        t: [f64; 3],
        rot: [f64; 3],
        s: f64,
        position_vector: bool,
        refp: [f64; 3],
        exact: bool,
    ) -> Self {
        let [f, th, p] = rot;
        let mut r = if exact {
            let (sf, cf) = f.sin_cos();
            let (st, ct) = th.sin_cos();
            let (sp, cp) = p.sin_cos();
            [
                [ct * cp, cf * sp + sf * st * cp, sf * sp - cf * st * cp],
                [-ct * sp, cf * cp - sf * st * sp, sf * cp + cf * st * sp],
                [st, -sf * ct, cf * ct],
            ]
        } else {
            [[1.0, p, -th], [-p, 1.0, f], [th, -f, 1.0]]
        };
        if position_vector {
            r = [
                [r[0][0], r[1][0], r[2][0]],
                [r[0][1], r[1][1], r[2][1]],
                [r[0][2], r[1][2], r[2][2]],
            ];
        }
        Helmert {
            t: [t[0] + refp[0], t[1] + refp[1], t[2] + refp[2]],
            r,
            s,
            refp,
            no_rotation: rot == [0.0; 3],
        }
    }

    pub(crate) fn forward(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        if self.no_rotation && self.s == 0.0 {
            return [x + self.t[0], y + self.t[1], z + self.t[2]];
        }
        let scale = 1.0 + self.s;
        let (x, y, z) = (x - self.refp[0], y - self.refp[1], z - self.refp[2]);
        let r = &self.r;
        [
            scale * (r[0][0] * x + r[0][1] * y + r[0][2] * z) + self.t[0],
            scale * (r[1][0] * x + r[1][1] * y + r[1][2] * z) + self.t[1],
            scale * (r[2][0] * x + r[2][1] * y + r[2][2] * z) + self.t[2],
        ]
    }

    /// PROJ's reverse: undo translation and scale, then apply the transposed
    /// rotation matrix (exact for the matrix PROJ builds only to first order,
    /// which is what PROJ does).
    pub(crate) fn inverse(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        if self.no_rotation && self.s == 0.0 {
            return [x - self.t[0], y - self.t[1], z - self.t[2]];
        }
        let scale = 1.0 + self.s;
        let (x, y, z) = (
            (x - self.t[0]) / scale,
            (y - self.t[1]) / scale,
            (z - self.t[2]) / scale,
        );
        let r = &self.r;
        [
            r[0][0] * x + r[1][0] * y + r[2][0] * z + self.refp[0],
            r[0][1] * x + r[1][1] * y + r[2][1] * z + self.refp[1],
            r[0][2] * x + r[1][2] * y + r[2][2] * z + self.refp[2],
        ]
    }
}

/// Whether the method is one of the Helmert family handled here.
pub(crate) fn is_helmert(method: u32) -> bool {
    classify(method).is_some()
}

/// A geographic CRS's ellipsoid and prime meridian, or an error naming why
/// the CRS isn't one.
fn geographic(crs: &Crs) -> Result<(Ellipsoid, f64), MethodError> {
    match &crs.horizontal().kind {
        Kind::Geographic { datum } => Ok((datum.ellipsoid, datum.prime_meridian)),
        _ => Err(MethodError::WrongCrs("a geographic CRS")),
    }
}

/// A time-dependent Helmert's parameters at its reference epoch and their
/// rates per year, from which [`Rates::at`] makes the step for an epoch.
#[derive(Debug, Clone)]
struct Rates {
    /// Translation, rotation (radians) and scale (a fraction) at the
    /// reference epoch.
    t: [f64; 3],
    rot: [f64; 3],
    s: f64,
    /// Their rates, per year.
    dt: [f64; 3],
    drot: [f64; 3],
    ds: f64,
    /// The reference epoch, a decimal year.
    epoch: f64,
    position_vector: bool,
}

impl Rates {
    /// The rates of a time-dependent method's operation; `None` for the
    /// other methods.
    fn read(
        op: &Operation,
        t: [f64; 3],
        rot: [f64; 3],
        s: f64,
        position_vector: bool,
    ) -> Result<Option<Rates>, MethodError> {
        if !matches!(op.method, 1053..=1058) {
            return Ok(None);
        }
        // Stored in SI units: per second, and the epoch in seconds.
        let per_year = |code| required(op, code).map(|v| v * YEAR);
        Ok(Some(Rates {
            t,
            rot,
            s,
            dt: [
                per_year(param::X_TRANSLATION_RATE)?,
                per_year(param::Y_TRANSLATION_RATE)?,
                per_year(param::Z_TRANSLATION_RATE)?,
            ],
            drot: [
                per_year(param::X_ROTATION_RATE)?,
                per_year(param::Y_ROTATION_RATE)?,
                per_year(param::Z_ROTATION_RATE)?,
            ],
            ds: per_year(param::SCALE_DIFFERENCE_RATE)?,
            epoch: required(op, param::REFERENCE_EPOCH)? / YEAR,
            position_vector,
        }))
    }

    /// The Helmert step for coordinates at `epoch`.
    fn at(&self, epoch: f64) -> Helmert {
        let years = epoch - self.epoch;
        let moved = |p: [f64; 3], rate: [f64; 3]| {
            [
                p[0] + rate[0] * years,
                p[1] + rate[1] * years,
                p[2] + rate[2] * years,
            ]
        };
        Helmert::new(
            moved(self.t, self.dt),
            moved(self.rot, self.drot),
            self.s + self.ds * years,
            self.position_vector,
            [0.0; 3],
            false,
        )
    }
}

#[derive(Debug)]
struct HelmertOperation {
    /// The step with the operation's parameters as given: for a
    /// time-dependent method, those at its reference epoch.
    helmert: Helmert,
    /// A time-dependent method's rates.
    rates: Option<Rates>,
    domain: Domain,
    /// Source and target ellipsoid and prime meridian, for the geographic
    /// domains.
    src: Option<(Ellipsoid, f64)>,
    dst: Option<(Ellipsoid, f64)>,
    /// Zeros between two geographic CRSs that [`same_place`] holds for:
    /// the coordinates go through unchanged.
    relabel: bool,
    /// A time-specific method's epoch, a decimal year.
    only_at: Option<f64>,
}

/// "Transformation reference epoch" of the time-specific methods.
pub(crate) const TRANSFORMATION_REFERENCE_EPOCH: u32 = 1049;
/// How near a coordinate's epoch must be to a time-specific transformation's:
/// the same thousandth of a year (PROJ's `getRoundedEpochInDecimalYear`).
const SAME_EPOCH_YEARS: f64 = 5e-4;

/// Whether two geographic CRSs' ellipsoids and prime meridians are one for
/// a Helmert of zeros between them.
fn same_place((a, pa): (Ellipsoid, f64), (b, pb): (Ellipsoid, f64)) -> bool {
    pa == pb && (a.a - b.a).abs() <= SAME_ELLIPSOID_M && (a.b - b.b).abs() <= SAME_ELLIPSOID_M
}

pub(super) fn build(op: &Operation, src: &Crs, dst: &Crs) -> Result<Arc<dyn Method>, MethodError> {
    let (kind, domain) = classify(op.method).ok_or(MethodError::NotImplemented)?;
    let t = [
        required(op, param::X_TRANSLATION)?,
        required(op, param::Y_TRANSLATION)?,
        required(op, param::Z_TRANSLATION)?,
    ];
    let (rot, s) = if kind == Kind7::Translation {
        ([0.0; 3], 0.0)
    } else {
        (
            [
                required(op, param::X_ROTATION)?,
                required(op, param::Y_ROTATION)?,
                required(op, param::Z_ROTATION)?,
            ],
            required(op, param::SCALE_DIFFERENCE)?,
        )
    };
    let refp = match kind {
        Kind7::BadekasCoordinateFrame | Kind7::BadekasPositionVector => [
            required(op, param::ORDINATE_1_EVALUATION_POINT)?,
            required(op, param::ORDINATE_2_EVALUATION_POINT)?,
            required(op, param::ORDINATE_3_EVALUATION_POINT)?,
        ],
        _ => [0.0; 3],
    };
    let position_vector = matches!(kind, Kind7::PositionVector | Kind7::BadekasPositionVector);
    let exact = matches!(op.method, 1132 | 1133 | 1140);
    let helmert = Helmert::new(t, rot, s, position_vector, refp, exact);
    let rates = Rates::read(op, t, rot, s, position_vector)?;
    let (src, dst) = match domain {
        Domain::Geocentric => {
            for crs in [src, dst] {
                if !matches!(crs.kind, Kind::Geocentric { .. }) {
                    return Err(MethodError::WrongCrs("a geocentric CRS"));
                }
            }
            (None, None)
        }
        Domain::Geog2d | Domain::Geog3d => (Some(geographic(src)?), Some(geographic(dst)?)),
    };
    let zeros = op.params.iter().all(|(_, v)| *v == ParamValue::Number(0.0));
    let relabel = zeros && src.zip(dst).is_some_and(|(s, d)| same_place(s, d));
    let only_at = match op.method {
        1065 | 1066 => Some(required(op, TRANSFORMATION_REFERENCE_EPOCH)? / YEAR),
        _ => None,
    };
    Ok(Arc::new(HelmertOperation {
        helmert,
        rates,
        domain,
        src,
        dst,
        relabel,
        only_at,
    }))
}

impl HelmertOperation {
    /// Geographic coordinates on `from` to geocentric, the Helmert step
    /// (`step`), and back to geographic on `to`.
    fn through_geocentric(
        &self,
        [lon, lat, h]: [f64; 3],
        from: (Ellipsoid, f64),
        to: (Ellipsoid, f64),
        step: impl Fn([f64; 3]) -> [f64; 3],
        keep_height: bool,
    ) -> Result<[f64; 3], CoordinateError> {
        let xyz = cartesian([lon + from.1, lat, h], &from.0);
        let [lon2, lat2, h2] = geodetic(step(xyz), &to.0);
        let h = if keep_height { h } else { h2 };
        Ok([lon2 - to.1, lat2, h])
    }
}

impl HelmertOperation {
    fn apply(
        &self,
        c: [f64; 3],
        inverse: bool,
        keep_height: bool,
    ) -> Result<[f64; 3], CoordinateError> {
        self.apply_with(&self.helmert, c, inverse, keep_height)
    }

    /// [`HelmertOperation::apply`] for a coordinate at `epoch`: a
    /// time-dependent method's parameters are those at that epoch.
    fn apply_at(
        &self,
        c: [f64; 3],
        epoch: f64,
        inverse: bool,
        keep_height: bool,
    ) -> Result<[f64; 3], CoordinateError> {
        if let Some(at) = self.only_at
            && (epoch - at).abs() > SAME_EPOCH_YEARS
        {
            return Err(CoordinateError::EpochOutOfRange);
        }
        match &self.rates {
            // PROJ rebuilds its parameters only when the epoch differs
            // from the one they are at: the same numbers either way.
            Some(rates) if epoch != rates.epoch => {
                self.apply_with(&rates.at(epoch), c, inverse, keep_height)
            }
            _ => self.apply(c, inverse, keep_height),
        }
    }

    fn apply_with(
        &self,
        helmert: &Helmert,
        c: [f64; 3],
        inverse: bool,
        keep_height: bool,
    ) -> Result<[f64; 3], CoordinateError> {
        if self.relabel {
            return Ok(c);
        }
        match (self.src, self.dst, inverse) {
            (Some(from), Some(to), false) => {
                self.through_geocentric(c, from, to, |x| helmert.forward(x), keep_height)
            }
            (Some(from), Some(to), true) => {
                self.through_geocentric(c, to, from, |x| helmert.inverse(x), keep_height)
            }
            (_, _, false) => Ok(helmert.forward(c)),
            (_, _, true) => Ok(helmert.inverse(c)),
        }
    }
}

impl Method for HelmertOperation {
    fn forward(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, false, self.domain == Domain::Geog2d)
    }

    fn inverse(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, true, self.domain == Domain::Geog2d)
    }

    fn keeps_height(&self) -> bool {
        self.domain == Domain::Geog2d
    }

    fn forward_3d(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, false, false)
    }

    fn inverse_3d(&self, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
        self.apply(c, true, false)
    }

    fn forward_3d_at(&self, c: [f64; 3], epoch: f64) -> Result<[f64; 3], CoordinateError> {
        self.apply_at(c, epoch, false, false)
    }

    fn inverse_3d_at(&self, c: [f64; 3], epoch: f64) -> Result<[f64; 3], CoordinateError> {
        self.apply_at(c, epoch, true, false)
    }

    fn is_identity(&self) -> bool {
        self.relabel
    }
}
