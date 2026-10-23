//! Bounding boxes, converted by densifying their edges.
//!
//! Ported from PROJ 9.8.1 `src/trans_bounds.cpp` (`proj_trans_bounds`; MIT,
//! see PROJ-NOTICE.md), with its handling of the antimeridian and the poles
//! for geographic output. PROJ works in degrees; here the thresholds scale
//! with the output CRS's angular unit.

use crate::error::Error;
use crate::projection::Projection;
use crate::transform::{AxisOrder, Transform};

/// Points added along each edge by default, as PROJ recommends.
pub const DEFAULT_DENSIFY: usize = 21;

/// A conversion of one point, `None` when it fails.
pub(crate) type PointFn<'a> = Box<dyn Fn(f64, f64) -> Option<(f64, f64)> + 'a>;

/// A point conversion as [`convert_bounds`] sees it.
pub(crate) trait PointConversion {
    /// A full turn in the input's and the output's angular unit, for each
    /// side that is geographic.
    fn turns(&self) -> (Option<f64>, Option<f64>);
    /// Whether the input and the output come longitude first.
    fn longitude_first(&self) -> (bool, bool);
    /// The input CRS, for errors.
    fn label(&self) -> String;
    /// One point, or `None` when it fails.
    fn point(&self, x: f64, y: f64) -> Option<(f64, f64)>;
    /// The conversion back, for finding the poles inside the box.
    fn reverse(&self) -> Option<PointFn<'_>>;
}

impl PointConversion for Transform {
    fn turns(&self) -> (Option<f64>, Option<f64>) {
        (
            self.source().longitude_turn(),
            self.target().longitude_turn(),
        )
    }

    fn longitude_first(&self) -> (bool, bool) {
        let lon_first = |crs: &crate::Crs| {
            self.order() == AxisOrder::EastFirst || !crs.axes.first_is_northing()
        };
        (lon_first(self.source()), lon_first(self.target()))
    }

    fn label(&self) -> String {
        self.source().label()
    }

    fn point(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        self.convert(x, y, None).ok().map(|(x, y, _)| (x, y))
    }

    fn reverse(&self) -> Option<PointFn<'_>> {
        let inverse = self.inverse().ok()?;
        Some(Box::new(move |x, y| {
            inverse.convert(x, y, None).ok().map(|(x, y, _)| (x, y))
        }))
    }
}

impl PointConversion for Projection {
    fn turns(&self) -> (Option<f64>, Option<f64>) {
        let crs = self.crs_is_angular().then_some(360.0);
        if self.is_inverse() {
            (crs, Some(360.0))
        } else {
            (Some(360.0), crs)
        }
    }

    fn longitude_first(&self) -> (bool, bool) {
        (true, true)
    }

    fn label(&self) -> String {
        self.crs().label()
    }

    fn point(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        self.convert(x, y, None).ok().map(|(x, y, _)| (x, y))
    }

    fn reverse(&self) -> Option<PointFn<'_>> {
        let inverse = self.inverse();
        Some(Box::new(move |x, y| {
            inverse.convert(x, y, None).ok().map(|(x, y, _)| (x, y))
        }))
    }
}

impl Transform {
    /// Convert a bounding box `[min_x, min_y, max_x, max_y]`, in this
    /// transform's axis order, by converting `densify` extra points along
    /// each edge (and, unless the result crosses the antimeridian, a grid
    /// inside) and taking the outermost values. [`DEFAULT_DENSIFY`] is 21.
    ///
    /// For a geographic target with longitude first, `max_x < min_x` in the
    /// result means the box crosses the antimeridian: it is the union of
    /// `[min_x, min_y, 180, max_y]` and `[-180, min_y, max_x, max_y]`
    /// (latitude first: the same on y). A box containing a pole spans every
    /// longitude and reaches 90°. A geographic source box may cross the
    /// antimeridian the same way.
    ///
    /// Points that fail to convert are skipped; the box is an error only when
    /// none converts.
    pub fn convert_bounds(&self, bounds: [f64; 4], densify: usize) -> Result<[f64; 4], Error> {
        convert_bounds(self, bounds, densify)
    }
}

impl Projection {
    /// A bounding box through the projection, as
    /// [`Transform::convert_bounds`] converts one.
    pub fn convert_bounds(&self, bounds: [f64; 4], densify: usize) -> Result<[f64; 4], Error> {
        convert_bounds(self, bounds, densify)
    }
}

/// PROJ's `proj_trans_bounds`.
pub(crate) fn convert_bounds<C: PointConversion>(
    c: &C,
    bounds: [f64; 4],
    densify: usize,
) -> Result<[f64; 4], Error> {
    let [xmin, ymin, xmax, ymax] = bounds;
    let (input_turn, output_turn) = c.turns();
    let degree_input = input_turn.is_some();
    let degree_output = output_turn.is_some();
    let invalid = |message: &str| Error::Projjson {
        crs: c.label(),
        message: message.to_owned(),
    };
    if degree_output && densify < 2 {
        return Err(invalid(
            "densify must be at least 2 when the output is geographic",
        ));
    }
    let (input_lon_first, output_lon_first) = c.longitude_first();
    let input_lon_lat_order = degree_input && input_lon_first;
    let output_lon_lat_order = degree_output && output_lon_first;
    let turn_out = output_turn.unwrap_or(360.0);

    let (mut north_pole_in_bounds, mut south_pole_in_bounds) = (false, false);
    if degree_output {
        let quarter = turn_out / 4.0;
        let reverse = c.reverse();
        let contains = |lat: f64| {
            let (px, py) = if output_lon_lat_order {
                (0.0, lat)
            } else {
                (lat, 0.0)
            };
            let Some(reverse) = &reverse else {
                return false;
            };
            match reverse(px, py) {
                Some((x, y)) => xmin < x && x < xmax && ymax > y && y > ymin,
                None => false,
            }
        };
        north_pole_in_bounds = contains(quarter);
        south_pole_in_bounds = contains(-quarter);
    }

    let side_pts = densify + 1;
    let turn_in = input_turn.unwrap_or(360.0);
    let delta_x = if degree_input && xmax < xmin {
        if !input_lon_lat_order {
            return Err(invalid("latitude max < latitude min"));
        }
        (xmax - xmin + turn_in) / side_pts as f64
    } else {
        (xmax - xmin) / side_pts as f64
    };
    let delta_y = if degree_input && ymax < ymin {
        if input_lon_lat_order {
            return Err(invalid("latitude max < latitude min"));
        }
        (ymax - ymin + turn_in) / side_pts as f64
    } else {
        (ymax - ymin) / side_pts as f64
    };

    // The densified box as a linear ring: the antimeridian logic needs it.
    let boundary_len = side_pts * 4;
    let mut xs = vec![0.0; boundary_len];
    let mut ys = vec![0.0; boundary_len];
    for i in 0..side_pts {
        let f = i as f64;
        (xs[i], ys[i]) = (xmin, ymax - f * delta_y);
        (xs[i + side_pts], ys[i + side_pts]) = (xmin + f * delta_x, ymin);
        (xs[i + side_pts * 2], ys[i + side_pts * 2]) = (xmax, ymin + f * delta_y);
        (xs[i + side_pts * 3], ys[i + side_pts * 3]) = (xmax - f * delta_x, ymax);
    }
    let mut converted = 0;
    for (x, y) in xs.iter_mut().zip(ys.iter_mut()) {
        match c.point(*x, *y) {
            Some((cx, cy)) if cx.is_finite() && cy.is_finite() => {
                (*x, *y) = (cx, cy);
                converted += 1;
            }
            _ => (*x, *y) = (f64::INFINITY, f64::INFINITY),
        }
    }
    if converted == 0 {
        return Err(invalid("no point of the bounding box could be converted"));
    }
    if degree_output && !output_lon_lat_order {
        // Longitude first for the antimeridian logic.
        std::mem::swap(&mut xs, &mut ys);
    }

    let half = turn_out / 2.0;
    let quarter = turn_out / 4.0;
    let mut crossed_antimeridian = false;
    let [mut out_xmin, mut out_ymin, mut out_xmax, mut out_ymax] = if !degree_output {
        [
            simple_min(&xs),
            simple_min(&ys),
            simple_max(&xs),
            simple_max(&ys),
        ]
    } else if north_pole_in_bounds {
        [-half, simple_min(&ys), half, quarter]
    } else if south_pole_in_bounds {
        [-half, -quarter, half, simple_max(&ys)]
    } else {
        let threshold = turn_out * 200.0 / 360.0;
        let lo = antimeridian_min(&xs, threshold, half);
        let hi = antimeridian_max(&xs, threshold, half);
        crossed_antimeridian = lo > hi;
        [lo, simple_min(&ys), hi, simple_max(&ys)]
    };
    if degree_output && !output_lon_lat_order {
        // Back to the CRS's axis order.
        std::mem::swap(&mut out_xmin, &mut out_ymin);
        std::mem::swap(&mut out_xmax, &mut out_ymax);
    }

    if !crossed_antimeridian {
        // Points inside the box, for extremes that are not on its edges.
        for j in 1..side_pts.saturating_sub(1) {
            for i in 0..side_pts {
                let x = xmin + i as f64 * delta_x;
                let y = ymin + j as f64 * delta_y;
                if let Some((x, y)) = c.point(x, y)
                    && x.is_finite()
                    && y.is_finite()
                {
                    out_xmin = out_xmin.min(x);
                    out_xmax = out_xmax.max(x);
                    out_ymin = out_ymin.min(y);
                    out_ymax = out_ymax.max(y);
                }
            }
        }
    }
    Ok([out_xmin, out_ymin, out_xmax, out_ymax])
}

/// Smallest value; failed points are +inf and never win.
fn simple_min(data: &[f64]) -> f64 {
    data.iter().copied().fold(f64::INFINITY, f64::min)
}

/// Largest value, skipping failed points.
fn simple_max(data: &[f64]) -> f64 {
    data.iter()
        .copied()
        .filter(|v| *v != f64::INFINITY)
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Index of the nearest earlier point that converted, wrapping around.
fn previous_index(i: usize, data: &[f64]) -> usize {
    let n = data.len();
    let mut prev = (i + n - 1) % n;
    while data[prev] == f64::INFINITY && prev != i {
        prev = (prev + n - 1) % n;
    }
    prev
}

/// Minimum longitude of a ring that may cross the antimeridian. A jump of
/// more than `threshold` between neighbours is a crossing; two crossings mean
/// the minimum is on the positive side, four that the ring spans every
/// longitude.
fn antimeridian_min(data: &[f64], threshold: f64, half: f64) -> f64 {
    let mut positive_min = f64::INFINITY;
    let mut min_value = f64::INFINITY;
    let mut crossed = 0;
    let mut positive_meridian = false;
    for (i, &value) in data.iter().enumerate() {
        if value == f64::INFINITY {
            continue;
        }
        let delta = data[previous_index(i, data)] - value;
        if delta >= threshold && delta != f64::INFINITY {
            // 180 -> -180
            if crossed == 0 {
                positive_min = min_value;
            }
            crossed += 1;
            positive_meridian = false;
        } else if delta <= -threshold && delta != f64::INFINITY {
            // -180 -> 180
            if crossed == 0 {
                positive_min = value;
            }
            crossed += 1;
            positive_meridian = true;
        }
        if positive_meridian && value < positive_min {
            positive_min = value;
        }
        if value < min_value {
            min_value = value;
        }
    }
    match crossed {
        2 => positive_min,
        4 => -half,
        _ => min_value,
    }
}

/// Maximum longitude of a ring that may cross the antimeridian; see
/// [`antimeridian_min`].
fn antimeridian_max(data: &[f64], threshold: f64, half: f64) -> f64 {
    let mut negative_max = f64::NEG_INFINITY;
    let mut max_value = f64::NEG_INFINITY;
    let mut crossed = 0;
    let mut negative_meridian = false;
    for (i, &value) in data.iter().enumerate() {
        if value == f64::INFINITY {
            continue;
        }
        let delta = data[previous_index(i, data)] - value;
        if delta >= threshold && delta != f64::INFINITY {
            // 180 -> -180
            if crossed == 0 {
                negative_max = value;
            }
            crossed += 1;
            negative_meridian = true;
        } else if delta <= -threshold && delta != f64::INFINITY {
            // -180 -> 180
            if crossed == 0 {
                negative_max = max_value;
            }
            negative_meridian = false;
            crossed += 1;
        }
        if negative_meridian && value > negative_max {
            negative_max = value;
        }
        if value > max_value {
            max_value = value;
        }
    }
    match crossed {
        2 => negative_max,
        4 => half,
        _ => max_value,
    }
}
