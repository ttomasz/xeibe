//! California Cooperative Oceanic Fisheries Investigations lines and
//! stations (`calcofi`): a line and station number from the Mercator
//! projection, turned 30° about point O (line 80, station 60).
//!
//! Ported from PROJ 9.8.1 `src/projections/calcofi.cpp` (MIT, see
//! PROJ-NOTICE.md). PROJ ignores `lon_0`, `x_0`, `y_0` and the radius: the
//! output is the line and station numbers themselves.

// PROJ's constants, digit for digit.
#![allow(clippy::excessive_precision)]

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use super::Projection;
use super::common::{phi2, tsfn};
use crate::error::CoordinateError;

const DEG_TO_LINE: f64 = 5.0;
const DEG_TO_STATION: f64 = 15.0;
const LINE_TO_RAD: f64 = 0.0034906585039886592;
const STATION_TO_RAD: f64 = 0.0011635528346628863;
const PT_O_LINE: f64 = 80.0;
const PT_O_STATION: f64 = 60.0;
const PT_O_LAMBDA: f64 = -2.1144663887911301;
const PT_O_PHI: f64 = 0.59602993955606354;
const ROTATION_ANGLE: f64 = 0.52359877559829882;

#[derive(Debug)]
pub(crate) struct Calcofi {
    e: f64,
}

impl Calcofi {
    pub(crate) fn new(e: f64) -> Self {
        Calcofi { e }
    }

    /// Mercator's y on the unit sphere or ellipsoid.
    fn merc_y(&self, phi: f64) -> f64 {
        if self.e != 0.0 {
            -tsfn(phi, phi.sin(), self.e).ln()
        } else {
            (FRAC_PI_4 + 0.5 * phi).tan().ln()
        }
    }
}

impl Projection for Calcofi {
    fn forward(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        if (phi.abs() - FRAC_PI_2).abs() <= 1e-10 {
            return Err(CoordinateError::OutsideDomain);
        }
        let y = self.merc_y(phi);
        let oy = self.merc_y(PT_O_PHI);
        let l1 = (y - oy) * ROTATION_ANGLE.tan();
        let l2 = -lam - l1 + PT_O_LAMBDA;
        let ry = l2 * ROTATION_ANGLE.cos() * ROTATION_ANGLE.sin() + y;
        let ry = if self.e != 0.0 {
            phi2((-ry).exp(), self.e)?
        } else {
            FRAC_PI_2 - 2.0 * (-ry).exp().atan()
        };
        Ok((
            PT_O_LINE - (ry - PT_O_PHI).to_degrees() * DEG_TO_LINE / ROTATION_ANGLE.cos(),
            PT_O_STATION + (ry - phi).to_degrees() * DEG_TO_STATION / ROTATION_ANGLE.sin(),
        ))
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let ry = PT_O_PHI - LINE_TO_RAD * (x - PT_O_LINE) * ROTATION_ANGLE.cos();
        let phi = ry - STATION_TO_RAD * (y - PT_O_STATION) * ROTATION_ANGLE.sin();
        let oymctr = self.merc_y(PT_O_PHI);
        let rymctr = self.merc_y(ry);
        let xymctr = self.merc_y(phi);
        let l1 = (xymctr - oymctr) * ROTATION_ANGLE.tan();
        let l2 = (rymctr - xymctr) / (ROTATION_ANGLE.cos() * ROTATION_ANGLE.sin());
        Ok((PT_O_LAMBDA - (l1 + l2), phi))
    }
}
