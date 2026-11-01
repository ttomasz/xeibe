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

use std::sync::Arc;

use super::{Method, MethodError, param, required};
use crate::crs::{Crs, Kind};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;
use crate::geocentric::{cartesian, geodetic};
use crate::registry::Operation;

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
    // Time-dependent Position Vector and Coordinate Frame, applied at their
    // reference epoch: without a coordinate epoch PROJ does the same.
    1053, 1054, 1055, 1056, 1057, 1058, //
    // Not the time-specific variants (1065, 1066): PROJ can't run them
    // either, so they never compete in its choice.
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

#[derive(Debug)]
struct HelmertOperation {
    helmert: Helmert,
    domain: Domain,
    /// Source and target ellipsoid and prime meridian, for the geographic
    /// domains.
    src: Option<(Ellipsoid, f64)>,
    dst: Option<(Ellipsoid, f64)>,
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
    Ok(Arc::new(HelmertOperation {
        helmert,
        domain,
        src,
        dst,
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
        match (self.src, self.dst, inverse) {
            (Some(from), Some(to), false) => {
                self.through_geocentric(c, from, to, |x| self.helmert.forward(x), keep_height)
            }
            (Some(from), Some(to), true) => {
                self.through_geocentric(c, to, from, |x| self.helmert.inverse(x), keep_height)
            }
            (_, _, false) => Ok(self.helmert.forward(c)),
            (_, _, true) => Ok(self.helmert.inverse(c)),
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
}
