//! Spherical Cross-Track Height: along-track and cross-track distances on
//! a sphere that fits the ellipsoid at a peg point along a heading, as for
//! radar imagery.
//!
//! Ported from PROJ 9.8.1 `src/projections/sch.cpp` (MIT, see
//! PROJ-NOTICE.md). PROJ's `sch` is 3D, its height above the local sphere;
//! a 2D CRS gives it a height of 0 and keeps x and y.

use super::{BuildError, Params, Projection, proj_codes};
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;
use crate::geocentric::{cartesian, geodetic};

#[derive(Debug)]
pub(crate) struct Sch {
    ellipsoid: Ellipsoid,
    /// The local sphere, of radius `rcurv`.
    sphere: Ellipsoid,
    rcurv: f64,
    trans: [f64; 9],
    xyzoff: [f64; 3],
}

impl Sch {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let plat = params.get(proj_codes::PEG_POINT_LATITUDE)?;
        let plon = params.get(proj_codes::PEG_POINT_LONGITUDE)?;
        let phdg = params.get(proj_codes::PEG_POINT_HEADING)?;
        let h0 = params.get(proj_codes::PEG_POINT_HEIGHT).unwrap_or(0.0);
        let (slt, clt) = plat.sin_cos();
        let (slo, clo) = plon.sin_cos();
        let (a, es) = (ellipsoid.a, ellipsoid.es);
        // The radius of curvature along the heading at the peg.
        let temp = (1.0 - es * slt * slt).sqrt();
        let reast = a / temp;
        let rnorth = a * (1.0 - es) / temp.powi(3);
        let (shdg, chdg) = phdg.sin_cos();
        let rcurv = h0 + (reast * rnorth) / (reast * chdg * chdg + rnorth * shdg * shdg);
        let trans = [
            clt * clo,
            -shdg * slo - slt * clo * chdg,
            slo * chdg - slt * clo * shdg,
            clt * slo,
            clo * shdg - slt * slo * chdg,
            -clo * chdg - slt * slo * shdg,
            slt,
            clt * chdg,
            clt * shdg,
        ];
        let [x, y, z] = cartesian([plon, plat, h0], ellipsoid);
        Ok(Sch {
            ellipsoid: *ellipsoid,
            sphere: Ellipsoid::sphere(rcurv),
            rcurv,
            trans,
            xyzoff: [
                x - rcurv * clt * clo,
                y - rcurv * clt * slo,
                z - rcurv * slt,
            ],
        })
    }
}

impl Projection for Sch {
    fn forward(&self, lon: f64, lat: f64) -> Result<(f64, f64), CoordinateError> {
        let [x, y, z] = cartesian([lon, lat, 0.0], &self.ellipsoid);
        let (x, y, z) = (x - self.xyzoff[0], y - self.xyzoff[1], z - self.xyzoff[2]);
        let m = &self.trans;
        let xyz = [
            m[0] * x + m[3] * y + m[6] * z,
            m[1] * x + m[4] * y + m[7] * z,
            m[2] * x + m[5] * y + m[8] * z,
        ];
        let [lam, phi, _] = geodetic(xyz, &self.sphere);
        // PROJ scales by rcurv / a, then by a.
        let a = self.ellipsoid.a;
        Ok((lam * (self.rcurv / a) * a, phi * (self.rcurv / a) * a))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let ra = 1.0 / self.ellipsoid.a;
        let scale = self.ellipsoid.a / self.rcurv;
        let [x, y, z] = cartesian([x * ra * scale, y * ra * scale, 0.0], &self.sphere);
        let m = &self.trans;
        let xyz = [
            m[0] * x + m[1] * y + m[2] * z + self.xyzoff[0],
            m[3] * x + m[4] * y + m[5] * z + self.xyzoff[1],
            m[6] * x + m[7] * y + m[8] * z + self.xyzoff[2],
        ];
        let [lam, phi, _] = geodetic(xyz, &self.ellipsoid);
        Ok((lam, phi))
    }
}
