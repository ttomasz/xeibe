//! Interpolation in grids and the iterative inverses, as PROJ does them.
//!
//! Ported from PROJ 9.8.1 (MIT, see PROJ-NOTICE.md): `src/grids.cpp`
//! (`pj_hgrid_interpolate`, `pj_hgrid_apply_internal`, `pj_hgrid_apply`,
//! `GTiffGrid::valuesAt`, `pj_bilinear_interpolation_three_samples`) and
//! `src/transformations/gridshift.cpp` (`grid_interpolate`,
//! `grid_apply_internal`, `apply`); `read_vgrid_value` for `vgridshift`.
//!
//! Coordinates are longitude and latitude in radians from Greenwich.
//! `Ok(None)` is PROJ's `HUGE_VAL`: the point is outside the grids, or the
//! inverse didn't converge; the caller may try another operation.
//! `Err(NotLoaded)` means a grid block wasn't loaded.

use std::f64::consts::PI;

use super::set::{
    GInfo, HInfo, Node, REL_TOLERANCE, VInfo, generic_grid_at, generic_grid_at_type,
    generic_metadata, horizontal_grid_at, vertical_grid_at,
};
use super::tiff::{Image, NotLoaded};
use crate::methods::adjlon;

const MAX_ITERATIONS: usize = 10;
const TOL: f64 = 1e-12;

/// Cell index and fraction along one axis, with PROJ's tolerance at the
/// edges; `None` when outside.
fn cell(t: f64, size: usize) -> Option<(usize, f64)> {
    let mut index = if t.is_nan() {
        0
    } else {
        t.floor().round() as i64
    };
    let mut frac = t - index as f64;
    let size = size as i64;
    if index < 0 {
        if index == -1 && frac > 1.0 - 10.0 * REL_TOLERANCE {
            index += 1;
            frac = 0.0;
        } else {
            return None;
        }
    } else if index + 1 >= size {
        if index + 1 == size && frac < 10.0 * REL_TOLERANCE {
            index -= 1;
            frac = 1.0;
        } else {
            return None;
        }
    }
    Some((index as usize, frac))
}

/// Bilinear weights m00, m10, m01, m11, in PROJ's order of operations.
fn weights(fx: f64, fy: f64) -> [f64; 4] {
    let mut m10 = fx;
    let mut m11 = m10;
    let mut m01 = 1.0 - fx;
    let mut m00 = m01;
    m11 *= fy;
    m01 *= fy;
    let fy = 1.0 - fy;
    m00 *= fy;
    m10 *= fy;
    [m00, m10, m01, m11]
}

/// `GTiffHGrid::valueAt`: longitude and latitude shifts in radians,
/// rounded to single precision as PROJ does.
fn h_value(image: &Image, info: &HInfo, x: usize, y: usize) -> Result<(f32, f32), NotLoaded> {
    let lat = image.value(info.lat, x, y)?;
    let lon = image.value(info.lon, x, y)?;
    let lat = (f64::from(lat) * info.to_radians) as f32;
    let mut lon = (f64::from(lon) * info.to_radians) as f32;
    if !info.positive_east {
        lon = -lon;
    }
    Ok((lon, lat))
}

/// `pj_hgrid_interpolate`: `t` relative to the grid's south-west node.
fn h_interpolate(
    t: (f64, f64),
    image: &Image,
    info: &HInfo,
) -> Result<Option<(f64, f64)>, NotLoaded> {
    let extent = &image.extent;
    let Some((ix, fx)) = cell(t.0 / extent.res_x, image.width) else {
        return Ok(None);
    };
    let Some((iy, fy)) = cell(t.1 / extent.res_y, image.height) else {
        return Ok(None);
    };
    let f00 = h_value(image, info, ix, iy)?;
    let f10 = h_value(image, info, ix + 1, iy)?;
    let f01 = h_value(image, info, ix, iy + 1)?;
    let f11 = h_value(image, info, ix + 1, iy + 1)?;
    let [m00, m10, m01, m11] = weights(fx, fy);
    let lon = m00 * f64::from(f00.0)
        + m10 * f64::from(f10.0)
        + m01 * f64::from(f01.0)
        + m11 * f64::from(f11.0);
    let lat = m00 * f64::from(f00.1)
        + m10 * f64::from(f10.1)
        + m01 * f64::from(f01.1)
        + m11 * f64::from(f11.1);
    Ok(Some((lon, lat)))
}

/// The input relative to a grid's origin, with longitude wrapping.
fn relative(lp: (f64, f64), image: &Image) -> (f64, f64) {
    let extent = &image.extent;
    let eps = extent.epsilon();
    let mut lam = lp.0 - extent.west;
    if lam + eps < 0.0 {
        lam += 2.0 * PI;
    } else if lam - eps > extent.east - extent.west {
        lam -= 2.0 * PI;
    }
    (lam, lp.1 - extent.south)
}

/// `pj_hgrid_apply`: an `hgridshift` step.
pub(crate) fn hgrid_apply(
    top: &[Node<HInfo>],
    images: &[Image],
    lp: (f64, f64),
    forward: bool,
) -> Result<Option<(f64, f64)>, NotLoaded> {
    let Some(mut grid) = horizontal_grid_at(top, images, lp.0, lp.1) else {
        return Ok(None);
    };
    let mut image = &images[grid.image];
    let mut tb = relative(lp, image);
    let Some(t) = h_interpolate(tb, image, &grid.info)? else {
        return Ok(None);
    };
    if forward {
        return Ok(Some((lp.0 + t.0, lp.1 + t.1)));
    }
    let mut t = (tb.0 - t.0, tb.1 - t.1);
    let mut del: Option<(f64, f64)>;
    let mut i = MAX_ITERATIONS;
    loop {
        del = h_interpolate(t, image, &grid.info)?;
        let dif;
        match del {
            None => {
                // Possibly gone outside the first grid: try the grid the
                // estimate falls in.
                let extent = &image.extent;
                let point = (t.0 + extent.west, t.1 + extent.south);
                let Some(new_grid) = horizontal_grid_at(top, images, point.0, point.1) else {
                    break;
                };
                if std::ptr::eq(new_grid, grid) {
                    break;
                }
                grid = new_grid;
                image = &images[grid.image];
                let extent = &image.extent;
                t = (point.0 - extent.west, point.1 - extent.south);
                tb = relative(lp, image);
                dif = (f64::MAX, f64::MAX);
            }
            Some(d) => {
                dif = (t.0 + d.0 - tb.0, t.1 + d.1 - tb.1);
                t = (t.0 - dif.0, t.1 - dif.1);
            }
        }
        i -= 1;
        if i == 0 || dif.0 * dif.0 + dif.1 * dif.1 <= TOL * TOL {
            break;
        }
    }
    if i == 0 {
        return Ok(None);
    }
    let extent = &image.extent;
    Ok(Some((adjlon(t.0 + extent.west), t.1 + extent.south)))
}

/// `read_vgrid_value` without the multiplier: the vertical shift at a
/// longitude and latitude (radians from Greenwich), bilinear over the valid
/// nodes around it; `None` outside the grids or where all four are nodata.
pub(crate) fn vgrid_value(
    top: &[Node<VInfo>],
    images: &[Image],
    lon: f64,
    lat: f64,
) -> Result<Option<f64>, NotLoaded> {
    if lon.is_nan() || lat.is_nan() {
        return Ok(None);
    }
    let Some(grid) = vertical_grid_at(top, images, lon, lat) else {
        return Ok(None);
    };
    let image = &images[grid.image];
    let extent = &image.extent;
    if !extent.is_geographic {
        return Ok(None);
    }
    let width = image.width as f64;
    let mut grid_x = (lon - extent.west) * extent.inv_res_x;
    let wrap = |x: f64| ((x + width) % width + width) % width;
    if lon < extent.west {
        grid_x = if extent.full_world_longitude() {
            wrap(grid_x)
        } else {
            (lon + 2.0 * PI - extent.west) * extent.inv_res_x
        };
    } else if lon > extent.east {
        grid_x = if extent.full_world_longitude() {
            wrap(grid_x)
        } else {
            (lon - 2.0 * PI - extent.west) * extent.inv_res_x
        };
    }
    let mut grid_y = (lat - extent.south) * extent.inv_res_y;
    let ix = grid_x.floor().round() as i64;
    let iy = grid_y.floor().round() as i64;
    if ix < 0 || ix >= image.width as i64 || iy < 0 || iy >= image.height as i64 {
        return Ok(None);
    }
    let (ix, iy) = (ix as usize, iy as usize);
    grid_x -= ix as f64;
    grid_y -= iy as f64;
    let ix2 = if ix + 1 >= image.width {
        if extent.full_world_longitude() {
            0
        } else {
            image.width - 1
        }
    } else {
        ix + 1
    };
    let iy2 = (iy + 1).min(image.height - 1);
    let s = grid.info.sample;
    let values = [
        image.value(s, ix, iy)?,
        image.value(s, ix2, iy)?,
        image.value(s, ix, iy2)?,
        image.value(s, ix2, iy2)?,
    ];
    let xy = grid_x * grid_y;
    let weights = [1.0 - grid_x - grid_y + xy, grid_x - xy, grid_y - xy, xy];
    let mut value = 0.0;
    let mut total = 0.0;
    let mut valid = 0;
    for (v, w) in values.iter().zip(weights) {
        if !image.is_nodata(*v) {
            value += f64::from(*v) * w;
            total += w;
            valid += 1;
        }
    }
    Ok(match valid {
        0 => None,
        4 => Some(value),
        _ => Some(value / total),
    })
}

/// Which samples a generic grid's interpolation reads, and how.
#[derive(Debug, Clone)]
struct GridInfo {
    x: Option<usize>,
    y: Option<usize>,
    z: Option<usize>,
    bilinear: bool,
    swap_xy: bool,
    /// The samples in the order the values are read: x, y (or swapped), z.
    order: Vec<usize>,
}

/// The cached part of `grid_interpolate`: samples and interpolation.
fn grid_info(images: &[Image], node: &Node<GInfo>, ty: &str) -> Result<GridInfo, String> {
    let image = &images[node.image];
    let projected = !image.extent.is_geographic;
    let meta = |name: &str, s: usize| generic_metadata(images, node, name, s as i32);
    let (mut x, mut y, mut z) = (None, None, None);
    let mut easting_northing = false;
    for i in 0..image.samples {
        let desc = meta("DESCRIPTION", i);
        let unit = meta("UNITTYPE", i);
        if !projected && desc == "latitude_offset" {
            y = Some(i);
            if !unit.is_empty() && unit != "arc-second" {
                return Err("gridshift: only arc-second offsets".into());
            }
        } else if !projected && desc == "longitude_offset" {
            x = Some(i);
            if !unit.is_empty() && unit != "arc-second" {
                return Err("gridshift: only arc-second offsets".into());
            }
        } else if projected && (desc == "easting_offset" || desc == "northing_offset") {
            easting_northing = true;
            if desc == "easting_offset" {
                x = Some(i);
            } else {
                y = Some(i);
            }
            if !unit.is_empty() && unit != "metre" {
                return Err("gridshift: only metre offsets".into());
            }
        } else if matches!(
            desc,
            "ellipsoidal_height_offset" | "geoid_undulation" | "hydroid_height" | "vertical_offset"
        ) {
            z = Some(i);
            if !unit.is_empty() && unit != "metre" {
                return Err("gridshift: only metre offsets".into());
            }
        }
    }
    if image.samples >= 2 && x.is_none() && y.is_none() && ty == "HORIZONTAL_OFFSET" {
        if projected {
            (x, y) = (Some(0), Some(1));
        } else {
            (x, y) = (Some(1), Some(0));
        }
    }
    if projected && easting_northing {
        return Err("gridshift: projected grids are not supported".into());
    }
    if (ty == "HORIZONTAL_OFFSET" || ty == "GEOGRAPHIC_3D_OFFSET") && (x.is_none() || y.is_none()) {
        return Err("gridshift: grid has not the expected samples".into());
    }
    if matches!(
        ty,
        "ELLIPSOIDAL_HEIGHT_OFFSET"
            | "VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL"
            | "VERTICAL_OFFSET_VERTICAL_TO_VERTICAL"
            | "GEOGRAPHIC_3D_OFFSET"
    ) && z.is_none()
    {
        return Err("gridshift: grid has not the expected samples".into());
    }
    let interpolation = match generic_metadata(images, node, "interpolation_method", -1) {
        "" => "bilinear",
        i => i,
    };
    if interpolation != "bilinear" && interpolation != "biquadratic" {
        return Err("gridshift: unsupported interpolation_method".into());
    }
    let swap_xy = x == Some(1) && y == Some(0);
    let mut order = match (x, y) {
        _ if swap_xy => vec![0, 1],
        (Some(x), Some(y)) => vec![x, y],
        _ => vec![],
    };
    if let Some(z) = z {
        order.push(z);
    }
    Ok(GridInfo {
        x,
        y,
        z,
        bilinear: interpolation == "bilinear" || image.width < 3 || image.height < 3,
        swap_xy,
        order,
    })
}

/// `GTiffGrid::valuesAt`: the samples at a block of nodes, south to north,
/// west to east, and whether a nodata value was among them. (PROJ's fast
/// path for 256×256 single-precision blocks skips the check, which only
/// matters for NaN; a NaN is nodata here too.)
fn values_at(
    image: &Image,
    x0: usize,
    y0: usize,
    nx: usize,
    ny: usize,
    samples: &[usize],
) -> Result<(Vec<f32>, bool), NotLoaded> {
    let mut out = Vec::with_capacity(nx * ny * samples.len());
    let mut nodata = false;
    for y in y0..y0 + ny {
        for x in x0..x0 + nx {
            for &s in samples {
                let v = image.value(s, x, y)?;
                if image.is_nodata(v) {
                    nodata = true;
                }
                out.push(v);
            }
        }
    }
    Ok((out, nodata))
}

fn quadratic(x: f64, f0: f64, f1: f64, f2: f64) -> f64 {
    let df0 = f1 - f0;
    let df1 = f2 - f1;
    let d2f0 = df1 - df0;
    f0 + x * df0 + 0.5 * x * (x - 1.0) * d2f0
}

/// `gridshiftData::grid_interpolate`: the shift at `xy` (absolute), and
/// whether it was biquadratic.
fn grid_interpolate(
    images: &[Image],
    node: &Node<GInfo>,
    info: &GridInfo,
    xy: (f64, f64),
) -> Result<Option<([f64; 3], bool)>, NotLoaded> {
    let image = &images[node.image];
    let extent = &image.extent;
    let Some((mut ix, mut fx)) = cell((xy.0 - extent.west) / extent.res_x, image.width) else {
        return Ok(None);
    };
    let Some((mut iy, mut fy)) = cell((xy.1 - extent.south) / extent.res_y, image.height) else {
        return Ok(None);
    };
    let has_xy = info.x.is_some() && info.y.is_some();
    let mut val = [0.0f64; 3];
    let samples = if has_xy {
        info.order.clone()
    } else {
        info.z.into_iter().collect()
    };
    let n = samples.len();
    if n == 0 {
        return Ok(Some((val, !info.bilinear)));
    }
    if info.bilinear {
        let (v, nodata) = values_at(image, ix, iy, 2, 2, &samples)?;
        if nodata {
            return Ok(None);
        }
        let w = weights(fx, fy);
        let mix = |k: usize| {
            w[0] * f64::from(v[k])
                + w[1] * f64::from(v[n + k])
                + w[2] * f64::from(v[2 * n + k])
                + w[3] * f64::from(v[3 * n + k])
        };
        if has_xy {
            val[0] = mix(0);
            val[1] = mix(1);
            if n == 3 {
                val[2] = mix(2);
            }
        } else {
            val[2] = mix(0);
        }
    } else {
        if (fx <= 0.5 && ix > 0) || ix + 2 == image.width {
            ix -= 1;
            fx += 1.0;
        }
        if (fy <= 0.5 && iy > 0) || iy + 2 == image.height {
            iy -= 1;
            fy += 1.0;
        }
        let (v, nodata) = values_at(image, ix, iy, 3, 3, &samples)?;
        if nodata {
            return Ok(None);
        }
        // Along x in each of the three rows, then along y.
        let at = |row: usize, col: usize, k: usize| f64::from(v[(row * 3 + col) * n + k]);
        let interp = |k: usize| {
            let rows: Vec<f64> = (0..3)
                .map(|r| quadratic(fx, at(r, 0, k), at(r, 1, k), at(r, 2, k)))
                .collect();
            quadratic(fy, rows[0], rows[1], rows[2])
        };
        if has_xy {
            val[0] = interp(0);
            val[1] = interp(1);
            if n == 3 {
                val[2] = interp(2);
            }
        } else {
            val[2] = interp(0);
        }
    }
    if has_xy {
        let factor = 1.0 / 3600.0 / 180.0 * PI;
        val[0] *= factor;
        val[1] *= factor;
    }
    if info.swap_xy {
        val.swap(0, 1);
    }
    Ok(Some((val, !info.bilinear)))
}

/// `normalizeX`.
fn normalize_x(image: &Image, x: f64) -> f64 {
    let extent = &image.extent;
    if !extent.is_geographic {
        return x;
    }
    let eps = extent.epsilon();
    if x < extent.west - eps {
        x + 2.0 * PI
    } else if x > extent.east + eps {
        x - 2.0 * PI
    } else {
        x
    }
}

/// How the iteration of a reverse grid shift ended.
enum End {
    Converged,
    Exhausted,
    LeftGrids,
}

/// A `gridshift` step's grids: PROJ's main and auxiliary grid types.
#[derive(Debug)]
pub(crate) struct GenericShift {
    main_type: &'static str,
    aux_type: Option<&'static str>,
    has_horizontal: bool,
    has_geographic_3d: bool,
}

const HORIZONTAL_OFFSET: &str = "HORIZONTAL_OFFSET";

impl GenericShift {
    /// `checkGridTypes`.
    pub(crate) fn new(images: &[Image], top: &[Node<GInfo>]) -> Result<GenericShift, String> {
        let (mut horizontal, mut geog3d, mut eht, mut v2v, mut g2v) =
            (false, false, false, false, false);
        let mut constant_offset = false;
        let mut error = None;
        // Only the top-level grids, as PROJ's `gridset->grids()`.
        for n in top {
            match generic_metadata(images, n, "TYPE", -1) {
                "HORIZONTAL_OFFSET" => {
                    horizontal = true;
                    if !generic_metadata(images, n, "constant_offset", 0).is_empty()
                        || !generic_metadata(images, n, "constant_offset", 1).is_empty()
                    {
                        constant_offset = true;
                    }
                }
                "GEOGRAPHIC_3D_OFFSET" => geog3d = true,
                "ELLIPSOIDAL_HEIGHT_OFFSET" => eht = true,
                "VERTICAL_OFFSET_VERTICAL_TO_VERTICAL" => v2v = true,
                "VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL" => g2v = true,
                "" => error = Some("missing TYPE metadata item in grid(s)"),
                _ => error = Some("unhandled value for TYPE metadata item"),
            }
        }
        if let Some(e) = error {
            return Err(e.into());
        }
        if constant_offset {
            return Err("gridshift: constant offsets are not supported".into());
        }
        if usize::from(eht) + usize::from(v2v) + usize::from(g2v) > 1 {
            return Err("unsupported mix of grid types".into());
        }
        let main_type = if geog3d {
            "GEOGRAPHIC_3D_OFFSET"
        } else if !horizontal {
            if eht {
                "ELLIPSOIDAL_HEIGHT_OFFSET"
            } else if g2v {
                "VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL"
            } else {
                "VERTICAL_OFFSET_VERTICAL_TO_VERTICAL"
            }
        } else {
            HORIZONTAL_OFFSET
        };
        let aux_type = if !horizontal {
            None
        } else if eht {
            Some("ELLIPSOIDAL_HEIGHT_OFFSET")
        } else if g2v {
            Some("VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL")
        } else if v2v {
            Some("VERTICAL_OFFSET_VERTICAL_TO_VERTICAL")
        } else {
            None
        };
        Ok(GenericShift {
            main_type,
            aux_type,
            has_horizontal: horizontal,
            has_geographic_3d: geog3d,
        })
    }

    /// `grid_apply_internal`.
    #[allow(clippy::too_many_arguments)]
    fn apply_internal<'a>(
        &self,
        images: &[Image],
        top: &'a [Node<GInfo>],
        ty: &str,
        vertical_only: bool,
        input: [f64; 3],
        forward: bool,
        mut grid: &'a Node<GInfo>,
    ) -> Result<Result<Option<[f64; 3]>, String>, NotLoaded> {
        let info = |g: &Node<GInfo>| grid_info(images, g, ty);
        let mut gi = match info(grid) {
            Ok(i) => i,
            Err(e) => return Ok(Err(e)),
        };
        let mut normalized = (normalize_x(&images[grid.image], input[0]), input[1]);
        let Some((mut shift, biquadratic)) = grid_interpolate(images, grid, &gi, normalized)?
        else {
            return Ok(Ok(None));
        };
        if forward {
            return Ok(Ok(Some([
                input[0] + shift[0],
                input[1] + shift[1],
                input[2] + shift[2],
            ])));
        }
        if vertical_only {
            return Ok(Ok(Some([input[0], input[1], input[2] - shift[2]])));
        }
        // EPSG iterates the reverse of both interpolations "until the
        // difference between results from successive iterations is
        // insignificant". PROJ, like NOAA's NCAT, keeps the first estimate
        // for a biquadratic grid, which is up to decimetres off NADCON5's
        // forward. A biquadratic surface steps at half cells, where the
        // iteration may not settle: then the estimate that came closest.
        let first = ((normalized.0 - shift[0], normalized.1 - shift[1]), shift);
        let mut guess = first.0;
        let mut best: Option<(f64, (f64, f64), [f64; 3])> = None;
        let mut end = End::Exhausted;
        for _ in 0..MAX_ITERATIONS {
            let Some((s, _)) = grid_interpolate(images, grid, &gi, guess)? else {
                // Possibly gone outside the first grid: try the grid the
                // estimate falls in.
                match generic_grid_at_type(top, images, ty, guess.0, guess.1) {
                    Some(new_grid) if !std::ptr::eq(new_grid, grid) => {
                        grid = new_grid;
                        gi = match info(grid) {
                            Ok(i) => i,
                            Err(e) => return Ok(Err(e)),
                        };
                        normalized = (normalize_x(&images[grid.image], input[0]), input[1]);
                        continue;
                    }
                    _ => {
                        end = End::LeftGrids;
                        break;
                    }
                }
            };
            let diff = (guess.0 + s[0] - normalized.0, guess.1 + s[1] - normalized.1);
            let residual = diff.0 * diff.0 + diff.1 * diff.1;
            if best.is_none_or(|(r, _, _)| residual < r) {
                best = Some((residual, guess, s));
            }
            guess = (guess.0 - diff.0, guess.1 - diff.1);
            shift = s;
            if residual <= TOL * TOL {
                end = End::Converged;
                break;
            }
        }
        (guess, shift) = match end {
            End::Converged => (guess, shift),
            _ if biquadratic => best.map_or(first, |(_, g, s)| (g, s)),
            End::Exhausted => return Ok(Ok(None)),
            // PROJ keeps the estimate so far, with no height shift.
            End::LeftGrids => (guess, [0.0; 3]),
        };
        let extent = &images[grid.image].extent;
        Ok(Ok(Some([
            if extent.is_geographic {
                adjlon(guess.0)
            } else {
                guess.0
            },
            guess.1,
            input[2] - shift[2],
        ])))
    }

    /// `gridshiftData::apply`.
    pub(crate) fn apply(
        &self,
        images: &[Image],
        top: &[Node<GInfo>],
        xyz: [f64; 3],
        forward: bool,
    ) -> Result<Result<Option<[f64; 3]>, String>, NotLoaded> {
        let mut ty = self.main_type;
        let mut found_geog3d = false;
        let grid = match generic_grid_at_type(top, images, ty, xyz[0], xyz[1]) {
            Some(g) => {
                if self.has_geographic_3d {
                    found_geog3d = true;
                }
                Some(g)
            }
            None if self.has_geographic_3d && self.has_horizontal => {
                ty = HORIZONTAL_OFFSET;
                generic_grid_at_type(top, images, ty, xyz[0], xyz[1])
            }
            None => None,
        };
        let Some(grid) = grid else {
            return Ok(Ok(None));
        };
        let out = match self.apply_internal(
            images,
            top,
            ty,
            !(self.has_geographic_3d || self.has_horizontal),
            xyz,
            forward,
            grid,
        )? {
            Ok(Some(out)) => out,
            other => return Ok(other),
        };
        if found_geog3d {
            return Ok(Ok(Some(out)));
        }
        let Some(aux) = self.aux_type else {
            return Ok(Ok(Some(out)));
        };
        let Some(grid) = generic_grid_at_type(top, images, aux, out[0], out[1]) else {
            return Ok(Ok(None));
        };
        self.apply_internal(images, top, aux, true, out, forward, grid)
    }
}

/// `pj_bilinear_interpolation_three_samples` with the grid found by
/// `pj_find_generic_grid`: three values (`xyzgridshift`'s translations,
/// `deformation`'s velocities), with the unit PROJ accepts for them.
pub(crate) fn three_samples(
    images: &[Image],
    top: &[Node<GInfo>],
    lp: (f64, f64),
    names: [&str; 3],
    unit_name: &str,
) -> Result<Result<Option<[f64; 3]>, String>, NotLoaded> {
    let Some(grid) = generic_grid_at(top, images, lp.0, lp.1) else {
        return Ok(Ok(None));
    };
    let image = &images[grid.image];
    if image.samples < 3 {
        return Ok(Err("xyzgridshift: grid has not enough samples".into()));
    }
    let mut idx = [0, 1, 2];
    for i in 0..image.samples {
        let desc = generic_metadata(images, grid, "DESCRIPTION", i as i32);
        for (k, name) in names.iter().enumerate() {
            if desc == *name {
                idx[k] = i;
            }
        }
    }
    let unit = generic_metadata(images, grid, "UNITTYPE", idx[0] as i32);
    if !unit.is_empty() && unit != unit_name {
        return Ok(Err(format!("only {unit_name} handled")));
    }
    let extent = &image.extent;
    if !extent.is_geographic {
        return Ok(Err("grids must be referenced in a geographic CRS".into()));
    }
    let mut grid_x = (lp.0 - extent.west) * extent.inv_res_x;
    if lp.0 < extent.west {
        grid_x = (lp.0 + 2.0 * PI - extent.west) * extent.inv_res_x;
    } else if lp.0 > extent.east {
        grid_x = (lp.0 - 2.0 * PI - extent.west) * extent.inv_res_x;
    }
    let grid_y = (lp.1 - extent.south) * extent.inv_res_y;
    // C++ truncates towards zero; indices outside the grid fail the read.
    let (ix, iy) = (grid_x as i64, grid_y as i64);
    if ix < 0 || iy < 0 || ix as usize >= image.width || iy as usize >= image.height {
        return Ok(Ok(None));
    }
    let (ix, iy) = (ix as usize, iy as usize);
    let ix2 = (ix + 1).min(image.width - 1);
    let iy2 = (iy + 1).min(image.height - 1);
    let read = |x: usize, y: usize| -> Result<[f32; 3], NotLoaded> {
        Ok([
            image.value(idx[0], x, y)?,
            image.value(idx[1], x, y)?,
            image.value(idx[2], x, y)?,
        ])
    };
    let (v1, v2, v3, v4) = (
        read(ix, iy)?,
        read(ix2, iy)?,
        read(ix, iy2)?,
        read(ix2, iy2)?,
    );
    let w = weights(grid_x - ix as f64, grid_y - iy as f64);
    let mix = |k: usize| {
        w[0] * f64::from(v1[k])
            + w[1] * f64::from(v2[k])
            + w[2] * f64::from(v3[k])
            + w[3] * f64::from(v4[k])
    };
    Ok(Ok(Some([mix(0), mix(1), mix(2)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_snap_at_the_edges() {
        assert_eq!(cell(2.25, 5), Some((2, 0.25)));
        assert_eq!(cell(-0.000_001, 5), Some((0, 0.0)));
        assert_eq!(cell(-0.5, 5), None);
        assert_eq!(cell(4.000_001, 5), Some((3, 1.0)));
        assert_eq!(cell(4.5, 5), None);
    }

    #[test]
    fn quadratic_through_three_points() {
        // f(x) = x² at 0, 1, 2.
        assert_eq!(quadratic(1.5, 0.0, 1.0, 4.0), 2.25);
    }
}

/// Every way PROJ-data encodes a GeoTIFF (`scripts/gen_grid_fixtures.py`):
/// the reader and `hgridshift` against PROJ on each re-encoding.
#[cfg(test)]
mod fixtures {
    use super::super::set::horizontal;
    use super::super::tiff::{Ranges, parse};
    use super::*;

    #[test]
    fn variants_match_proj() {
        let dir = xeibe_testkit::data_dir().join("reprojection/grids");
        let text = std::fs::read_to_string(dir.join("variants.jsonl")).unwrap();
        let mut files = std::collections::HashMap::new();
        let mut checked = 0;
        for line in text.lines().skip(1) {
            let r: serde_json::Value = serde_json::from_str(line).unwrap();
            let name = r["file"].as_str().unwrap().to_string();
            let images = files.entry(name.clone()).or_insert_with(|| {
                let mut ranges = Ranges::default();
                ranges.insert(0, std::fs::read(dir.join(&name)).unwrap());
                let images = parse(&ranges).unwrap();
                for image in &images {
                    for b in 0..image.block_count() {
                        image.decode(b, &ranges).unwrap();
                    }
                }
                images
            });
            let top = horizontal(images).unwrap();
            let p: Vec<f64> = serde_json::from_value(r["in"].clone()).unwrap();
            let forward = r["forward"].as_bool().unwrap();
            let got = hgrid_apply(
                &top,
                images,
                (p[0].to_radians(), p[1].to_radians()),
                forward,
            )
            .unwrap()
            .map(|(x, y)| [x.to_degrees(), y.to_degrees()]);
            let want: Option<Vec<f64>> = serde_json::from_value(r["out"].clone()).unwrap();
            match (got, want) {
                (Some(g), Some(w)) => {
                    // PROJ normalises the longitude it hands back.
                    let dx = (g[0] - w[0] + 540.0).rem_euclid(360.0) - 180.0;
                    assert!(
                        dx.abs() < 1e-11 && (g[1] - w[1]).abs() < 1e-11,
                        "{name} {forward} {p:?}: {g:?}, PROJ {w:?}"
                    );
                }
                (None, None) => {}
                (g, w) => panic!("{name} {forward} {p:?}: {g:?}, PROJ {w:?}"),
            }
            checked += 1;
        }
        assert_eq!(files.len(), 8, "the source grid and its 7 variants");
        assert!(checked > 800);
    }
}
