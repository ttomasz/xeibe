//! Interrupted Mollweide, land (`imoll`) or ocean (`imoll_o`): Mollweide
//! in six lobes, two or three north of the equator and three or four
//! south.
//!
//! Ported from PROJ 9.8.1 `src/projections/imoll.cpp` and `imoll_o.cpp`
//! (MIT, see PROJ-NOTICE.md), on the unit sphere.

use std::f64::consts::{FRAC_PI_2, SQRT_2};

use super::sphere::Unit;
use super::world::World;
use crate::error::CoordinateError;

const DEG: f64 = std::f64::consts::PI / 180.0;
/// Slack on the edges of the lobes.
const EPSLN: f64 = 1e-10;

/// The meridians that cut a row of lobes, in degrees, each with whether a
/// longitude on it belongs to the lobe west of it.
type Cuts = &'static [(f64, bool)];

/// The lobes matched to another lobe (1 to 6), each with that lobe and
/// the meridian where they meet, in degrees.
type Matches = &'static [(usize, usize, f64)];

#[derive(Debug)]
pub(crate) struct InterruptedMollweide {
    moll: World,
    /// Each lobe's central meridian and false easting.
    zones: Vec<(f64, f64)>,
    north: Cuts,
    south: Cuts,
    /// The cuts' x, north row then south.
    boundaries: Vec<f64>,
}

impl InterruptedMollweide {
    pub(crate) fn new(ocean: bool) -> Self {
        // The lobes' central meridians, north row first; the cuts; and
        // which lobe each is matched to and where, in PROJ's order.
        let (meridians, north, south, matches): (&[f64], Cuts, Cuts, Matches) = if ocean {
            (
                &[-140.0, -10.0, 130.0, -110.0, 20.0, 150.0],
                &[(-90.0, true), (60.0, false)],
                &[(-60.0, true), (90.0, false)],
                &[
                    (2, 1, -90.0),
                    (3, 2, 60.0),
                    (4, 1, -180.0),
                    (5, 2, -60.0),
                    (6, 3, 90.0),
                ],
            )
        } else {
            (
                &[-100.0, 30.0, -160.0, -60.0, 20.0, 140.0],
                &[(-40.0, true)],
                &[(-100.0, true), (-20.0, true), (80.0, true)],
                &[
                    (3, 1, -160.0),
                    (2, 1, -40.0),
                    (4, 1, -100.0),
                    (5, 2, -20.0),
                    (6, 2, 80.0),
                ],
            )
        };
        let mut p = InterruptedMollweide {
            moll: World::mollweide(FRAC_PI_2),
            zones: meridians.iter().map(|&m| (m * DEG, m * DEG)).collect(),
            north,
            south,
            boundaries: Vec::new(),
        };
        let row_phi = |z: usize| if z <= north.len() + 1 { EPSLN } else { -EPSLN };
        for &(z, to, lam) in matches {
            let x = |z: usize| {
                let (lam0, x0) = p.zones[z - 1];
                p.moll
                    .fwd(lam * DEG - lam0, row_phi(z))
                    .map_or(f64::NAN, |(x, _)| x + x0)
            };
            let offset = x(to) - x(z);
            p.zones[z - 1].1 += offset;
        }
        let boundary = |lam: f64, phi: f64| {
            let x = |lam| p.fwd(lam, phi).map_or(f64::NAN, |(x, _)| x);
            (x(lam - EPSLN) + x(lam + EPSLN)) / 2.0
        };
        let boundaries = north
            .iter()
            .map(|&(cut, _)| boundary(cut * DEG, EPSLN))
            .chain(south.iter().map(|&(cut, _)| boundary(cut * DEG, -EPSLN)))
            .collect();
        p.boundaries = boundaries;
        p
    }

    /// The lobe (1 to 6) of `v` in its row, by the cuts' `at` values.
    fn pick(cuts: Cuts, at: impl Fn(usize) -> f64, v: f64, first: usize) -> usize {
        for (i, &(_, west_inclusive)) in cuts.iter().enumerate() {
            let cut = at(i);
            if if west_inclusive { v <= cut } else { v < cut } {
                return first + i;
            }
        }
        first + cuts.len()
    }

    fn zone(&self, lam: f64, phi: f64) -> usize {
        if phi >= 0.0 {
            Self::pick(self.north, |i| self.north[i].0 * DEG, lam, 1)
        } else {
            Self::pick(
                self.south,
                |i| self.south[i].0 * DEG,
                lam,
                self.north.len() + 2,
            )
        }
    }
}

impl Unit for InterruptedMollweide {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let (lam0, x0) = self.zones[self.zone(lam, phi) - 1];
        let (x, y) = self.moll.fwd(lam - lam0, phi)?;
        Ok((x + x0, y))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        // Beyond the poles, with slack both ways: PROJ's `-y90 + EPSLN`
        // refuses the south pole itself.
        if y.abs() > SQRT_2 + EPSLN {
            return Err(CoordinateError::OutsideDomain);
        }
        let n_north = self.north.len();
        let (z, cuts, first) = if y >= 0.0 {
            let z = Self::pick(self.north, |i| self.boundaries[i], x, 1);
            (z, self.north, 1)
        } else {
            let z = Self::pick(self.south, |i| self.boundaries[n_north + i], x, n_north + 2);
            (z, self.south, n_north + 2)
        };
        let (lam0, x0) = self.zones[z - 1];
        let (lam, phi) = self.moll.inv(x - x0, y)?;
        let lam = lam + lam0;
        // Projectable: within the lobe's meridians and its hemisphere.
        let i = z - first;
        let west = if i == 0 { -180.0 } else { cuts[i - 1].0 };
        let east = cuts.get(i).map_or(180.0, |c| c.0);
        let in_row = if first == 1 {
            phi >= -EPSLN
        } else {
            phi <= EPSLN
        };
        if lam >= west * DEG - EPSLN && lam <= east * DEG + EPSLN && in_row {
            Ok((lam, phi))
        } else {
            Err(CoordinateError::OutsideDomain)
        }
    }
}
