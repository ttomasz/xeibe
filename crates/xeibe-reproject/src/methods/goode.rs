//! Goode Homolosine and its interrupted forms, land and ocean: Sinusoidal
//! near the equator and Mollweide nearer the poles, in up to twelve lobes.
//!
//! Ported from PROJ 9.8.1 `src/projections/goode.cpp`, `igh.cpp` and
//! `igh_o.cpp` (MIT, see PROJ-NOTICE.md), on the unit sphere.

use super::sphere::Unit;
use super::world::World;
use crate::error::CoordinateError;

const OUTSIDE: CoordinateError = CoordinateError::OutsideDomain;
const DEG: f64 = std::f64::consts::PI / 180.0;

/// Goode Homolosine, uninterrupted.
#[derive(Debug)]
pub(crate) struct Goode {
    sinu: World,
    moll: World,
}

impl Goode {
    pub(crate) fn new() -> Self {
        Goode {
            sinu: World::sinusoidal(0.0, 1.0),
            moll: World::mollweide(std::f64::consts::FRAC_PI_2),
        }
    }
}

const Y_COR: f64 = 0.05280;
const PHI_LIM: f64 = 0.710_930_781_979_023_6;

impl Unit for Goode {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        if phi.abs() <= PHI_LIM {
            self.sinu.fwd(lam, phi)
        } else {
            let (x, y) = self.moll.fwd(lam, phi)?;
            Ok((x, y - if phi >= 0.0 { Y_COR } else { -Y_COR }))
        }
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        if y.abs() <= PHI_LIM {
            self.sinu.inv(x, y)
        } else {
            self.moll.inv(x, y + if y >= 0.0 { Y_COR } else { -Y_COR })
        }
    }
}

/// One lobe: Sinusoidal or Mollweide around its own meridian, offset.
#[derive(Debug)]
struct Zone {
    projection: World,
    x0: f64,
    y0: f64,
    lam0: f64,
}

/// The latitude where Sinusoidal gives way to Mollweide, 40°44′11.8″.
const PHI_BOUNDARY: f64 = (40.0 + 44.0 / 60.0 + 11.8 / 3600.0) * DEG;
/// Slack on the edges of the lobes.
const EPSLN: f64 = 1e-10;

/// Interrupted Goode Homolosine, land (`igh`) or ocean (`igh_o`).
#[derive(Debug)]
pub(crate) struct Interrupted {
    zones: Vec<Zone>,
    dy0: f64,
    ocean: bool,
}

impl Interrupted {
    pub(crate) fn new(ocean: bool) -> Self {
        let sinu = |lon: f64| Zone {
            projection: World::sinusoidal(0.0, 1.0),
            x0: lon * DEG,
            y0: 0.0,
            lam0: lon * DEG,
        };
        let moll = |lon: f64, y0: f64| Zone {
            projection: World::mollweide(std::f64::consts::FRAC_PI_2),
            x0: lon * DEG,
            y0,
            lam0: lon * DEG,
        };
        // The lobes' central meridians, zones 1 to 12: Mollweide above
        // the boundary (1 to 3) and below it (9 or 10 to 12), Sinusoidal
        // between.
        let (north, middle_n, middle_s, south): (&[f64], &[f64], &[f64], &[f64]) = if ocean {
            (
                &[-140.0, -10.0, 130.0],
                &[-140.0, -10.0, 130.0],
                &[-110.0, 20.0, 150.0],
                &[-110.0, 20.0, 150.0],
            )
        } else {
            (
                &[-100.0, 30.0],
                &[-100.0, 30.0],
                &[-160.0, -60.0, 20.0, 140.0],
                &[-160.0, -60.0, 20.0, 140.0],
            )
        };
        // y0 makes the first Mollweide lobe meet the Sinusoidal one below it
        // at the boundary.
        let probe = |z: &Zone| z.projection.fwd(0.0, PHI_BOUNDARY).map_or(0.0, |(_, y)| y);
        let dy0 = probe(&sinu(north[0])) - probe(&moll(north[0], 0.0));
        let mut zones = Vec::new();
        zones.extend(north.iter().map(|&l| moll(l, dy0)));
        zones.extend(middle_n.iter().map(|&l| sinu(l)));
        zones.extend(middle_s.iter().map(|&l| sinu(l)));
        zones.extend(south.iter().map(|&l| moll(l, -dy0)));
        Interrupted { zones, dy0, ocean }
    }

    /// The lobe (1 to 12) of a longitude and latitude, or of x and y: by
    /// row, then by the meridians that interrupt it.
    fn zone(&self, lam: f64, phi: f64) -> usize {
        let pick = |cuts: &[(f64, bool)], first: usize| {
            // Each cut: the meridian, and whether a value on it belongs to
            // the lobe west of it (`<=`) or east (`>=` in `igh_o`).
            let mut z = first;
            for &(cut, west_inclusive) in cuts {
                let west = if west_inclusive {
                    lam <= cut
                } else {
                    lam < cut
                };
                if west {
                    return z;
                }
                z += 1;
            }
            z
        };
        if self.ocean {
            let (north, south) = (
                [(-90.0 * DEG, true), (60.0 * DEG, false)],
                [(-60.0 * DEG, true), (90.0 * DEG, false)],
            );
            if phi >= PHI_BOUNDARY {
                pick(&north, 1)
            } else if phi >= 0.0 {
                pick(&north, 4)
            } else if phi >= -PHI_BOUNDARY {
                pick(&south, 7)
            } else {
                pick(&south, 10)
            }
        } else {
            let north = [(-40.0 * DEG, true)];
            let south = [
                (-100.0 * DEG, true),
                (-20.0 * DEG, true),
                (80.0 * DEG, true),
            ];
            if phi >= PHI_BOUNDARY {
                pick(&north, 1)
            } else if phi >= 0.0 {
                pick(&north, 3)
            } else if phi >= -PHI_BOUNDARY {
                pick(&south, 5)
            } else {
                pick(&south, 9)
            }
        }
    }

    /// Whether a longitude and latitude found in lobe `z` belong to it.
    fn projectable(&self, z: usize, lam: f64, phi: f64) -> bool {
        let within = |a: f64, b: f64| lam >= a * DEG - EPSLN && lam <= b * DEG + EPSLN;
        let lat = |a: f64, b: f64| phi >= a * DEG - EPSLN && phi <= b * DEG + EPSLN;
        if self.ocean {
            match z {
                1 => within(-180.0, -90.0) || (within(160.0, 180.0) && lat(50.0, 90.0)),
                2 | 5 => within(-90.0, 60.0),
                3 => within(60.0, 180.0) || (within(-180.0, -160.0) && lat(50.0, 90.0)),
                4 => within(-180.0, -90.0),
                6 => within(60.0, 180.0),
                7 | 10 => within(-180.0, -60.0),
                8 => within(-60.0, 90.0),
                9 | 12 => within(90.0, 180.0),
                11 => within(-60.0, 90.0) || (within(90.0, 100.0) && lat(-90.0, -40.0)),
                _ => false,
            }
        } else {
            match z {
                1 => within(-180.0, -40.0) || (within(-40.0, -10.0) && lat(60.0, 90.0)),
                2 => {
                    within(-40.0, 180.0)
                        || (within(-180.0, -160.0) && lat(50.0, 90.0))
                        || (within(-50.0, -40.0) && lat(60.0, 90.0))
                }
                3 => within(-180.0, -40.0),
                4 => within(-40.0, 180.0),
                5 | 9 => within(-180.0, -100.0),
                6 | 10 => within(-100.0, -20.0),
                7 | 11 => within(-20.0, 80.0),
                8 | 12 => within(80.0, 180.0),
                _ => false,
            }
        }
    }
}

impl Unit for Interrupted {
    fn fwd(&self, lam: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let z = &self.zones[self.zone(lam, phi) - 1];
        let (x, y) = z.projection.fwd(lam - z.lam0, phi)?;
        Ok((x + z.x0, y + z.y0))
    }

    fn inv(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        // Latitude 90° is at y = dy0 + √2.
        let y90 = self.dy0 + std::f64::consts::SQRT_2;
        // With slack both ways: PROJ's `-y90 + EPSLN` refuses the south
        // pole itself.
        if y.abs() > y90 + EPSLN {
            return Err(OUTSIDE);
        }
        let n = self.zone(x, y);
        let z = &self.zones[n - 1];
        let (lam, phi) = z.projection.inv(x - z.x0, y - z.y0)?;
        let lam = lam + z.lam0;
        if !self.projectable(n, lam, phi) {
            return Err(OUTSIDE);
        }
        Ok((lam, phi))
    }
}
