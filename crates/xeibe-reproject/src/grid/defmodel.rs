//! Deformation models (PROJ's `defmodel`): a JSON master file listing
//! components, each a grid of displacements (a GeoTIFF of its own) scaled
//! by a function of time. A point at an epoch moves by the sum of its
//! components' displacements there and then. Land Information New
//! Zealand's NZGD2000 deformation model is one: the secular motion of the
//! plates as a velocity, and a patch for each earthquake since 2000.
//!
//! Ported from PROJ 9.8.1 `src/transformations/defmodel_impl.hpp` and
//! `defmodel.cpp` (MIT, see PROJ-NOTICE.md). Of the format, what New
//! Zealand's files use is read: a geographic definition CRS, offsets in
//! metres added to longitude and latitude (`"horizontal_offset_method":
//! "addition"`), and bilinear interpolation. A model in degrees, with
//! geocentric addition or geocentric bilinear interpolation, or defined in
//! a projected CRS, is refused as invalid: PROJ-data has none to check one
//! against.

use std::f64::consts::PI;
use std::sync::{Arc, OnceLock};

use serde_json::Value;

use super::set::{GInfo, Node, generic, generic_grid_at, generic_metadata};
use super::store::{self, GridFile, GridState};
use super::tiff::Image;
use crate::ellipsoid::Ellipsoid;
use crate::error::CoordinateError;

/// A box in radians of longitude and latitude.
#[derive(Debug, Clone, Copy)]
struct Bbox {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Displacement {
    None,
    Horizontal,
    Vertical,
    ThreeD,
}

/// How a piecewise function continues before its first epoch and after its
/// last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Beyond {
    Zero,
    Constant,
    Linear,
}

/// The factor a component's displacement is scaled by at an epoch. Epochs
/// are decimal years.
#[derive(Debug, Clone, PartialEq)]
enum TimeFunction {
    Constant,
    Velocity {
        reference: f64,
    },
    Step {
        epoch: f64,
    },
    ReverseStep {
        epoch: f64,
    },
    Piecewise {
        before: Beyond,
        after: Beyond,
        /// Epoch and scale factor.
        model: Vec<(f64, f64)>,
    },
    Exponential {
        reference: f64,
        end: Option<f64>,
        relaxation: f64,
        before: f64,
        initial: f64,
        last: f64,
    },
}

impl TimeFunction {
    /// `TimeFunction::evaluateAt`.
    fn at(&self, t: f64) -> f64 {
        match self {
            TimeFunction::Constant => 1.0,
            TimeFunction::Velocity { reference } => t - reference,
            TimeFunction::Step { epoch } => {
                if t < *epoch {
                    0.0
                } else {
                    1.0
                }
            }
            TimeFunction::ReverseStep { epoch } => {
                if t < *epoch {
                    -1.0
                } else {
                    0.0
                }
            }
            TimeFunction::Piecewise {
                before,
                after,
                model,
            } => {
                let (Some(&(t1, f1)), Some(&(tn, f_n))) = (model.first(), model.last()) else {
                    return 0.0;
                };
                let line = |(ta, fa): (f64, f64), (tb, fb): (f64, f64)| {
                    (fa * (tb - t) + fb * (t - ta)) / (tb - ta)
                };
                if t < t1 {
                    return match before {
                        Beyond::Zero => 0.0,
                        Beyond::Constant => f1,
                        Beyond::Linear if model.len() == 1 || model[1].0 == t1 => f1,
                        Beyond::Linear => line(model[0], model[1]),
                    };
                }
                for pair in model.windows(2) {
                    if t < pair[1].0 {
                        return line(pair[0], pair[1]);
                    }
                }
                match after {
                    Beyond::Zero => 0.0,
                    Beyond::Constant => f_n,
                    Beyond::Linear if model.len() == 1 => f_n,
                    Beyond::Linear => {
                        let previous = model[model.len() - 2];
                        if previous.0 == tn {
                            f_n
                        } else {
                            line(previous, (tn, f_n))
                        }
                    }
                }
            }
            TimeFunction::Exponential {
                reference,
                end,
                relaxation,
                before,
                initial,
                last,
            } => {
                if t < *reference {
                    return *before;
                }
                let t = end.map_or(t, |end| t.min(end));
                initial + (last - initial) * (1.0 - (-(t - reference) / relaxation).exp())
            }
        }
    }
}

/// The samples of a grid a component reads.
#[derive(Debug, Clone, Copy)]
struct Samples {
    /// East and north offsets, when the grid can give them.
    horizontal: Result<(usize, usize), ()>,
    vertical: Result<usize, ()>,
}

#[derive(Debug)]
struct Component {
    displacement: Displacement,
    extent: Bbox,
    file: Arc<GridFile>,
    set: OnceLock<Result<Vec<Node<GInfo>>, String>>,
    /// Per image of the file.
    samples: OnceLock<Vec<OnceLock<Samples>>>,
    time: TimeFunction,
}

/// A parsed master file.
#[derive(Debug)]
pub(crate) struct Model {
    extent: Bbox,
    /// First and last epoch the model is valid at.
    time_extent: (f64, f64),
    components: Vec<Component>,
    /// The components' grid files, each once.
    files: Vec<Arc<GridFile>>,
}

fn string<'a>(j: &'a Value, key: &str) -> Result<&'a str, String> {
    match j.get(key) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(format!("\"{key}\" should be a string")),
        None => Err(format!("missing \"{key}\"")),
    }
}

fn optional<'a>(j: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match j.get(key) {
        None => Ok(None),
        Some(_) => string(j, key).map(Some),
    }
}

fn number(j: &Value, key: &str) -> Result<f64, String> {
    match j.get(key) {
        Some(Value::Number(n)) => n
            .as_f64()
            .ok_or_else(|| format!("\"{key}\" is out of range")),
        Some(_) => Err(format!("\"{key}\" should be a number")),
        None => Err(format!("missing \"{key}\"")),
    }
}

fn object<'a>(j: &'a Value, key: &str) -> Result<&'a Value, String> {
    match j.get(key) {
        Some(v @ Value::Object(_)) => Ok(v),
        Some(_) => Err(format!("\"{key}\" should be an object")),
        None => Err(format!("missing \"{key}\"")),
    }
}

/// `ISO8601ToDecimalYear`: `YYYY-MM-DDTHH:MM:SSZ` as a decimal year, with
/// leap years and without leap seconds.
fn decimal_year(text: &str) -> Result<f64, String> {
    let invalid = || format!("invalid date-time {text}");
    let b = text.as_bytes();
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    if b.len() != 20 || b[19] != b'Z' || separators.iter().any(|&(i, c)| b[i] != c) {
        return Err(invalid());
    }
    let field = |from: usize, to: usize| -> Result<u32, String> {
        text.get(from..to)
            .filter(|s| s.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|s| s.parse().ok())
            .ok_or_else(invalid)
    };
    let (year, month, day) = (field(0, 4)?, field(5, 7)?, field(8, 10)?);
    let (hour, minute, second) = (field(11, 13)?, field(14, 16)?, field(17, 19)?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let mut days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if leap {
        days[1] = 29;
    }
    // 1582: the start of the Gregorian calendar.
    if year < 1582
        || !(1..=12).contains(&month)
        || day < 1
        || day > days[month as usize - 1]
        || hour >= 24
        || minute >= 60
        || second >= 61
    {
        return Err(invalid());
    }
    let day_in_year: u32 = days[..month as usize - 1].iter().sum::<u32>() + day - 1;
    let seconds = f64::from(day_in_year * 86_400 + hour * 3600 + minute * 60 + second);
    let year_seconds = if leap {
        86_400.0 * 366.0
    } else {
        86_400.0 * 365.0
    };
    Ok(f64::from(year) + seconds / year_seconds)
}

fn bbox(j: &Value) -> Result<Bbox, String> {
    if string(j, "type")? != "bbox" {
        return Err("unsupported type of extent".into());
    }
    let values: Vec<f64> = match object(j, "parameters")?.get("bbox") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_f64).collect(),
        _ => Vec::new(),
    };
    let [min_x, min_y, max_x, max_y] = values[..] else {
        return Err("bbox is not an array of 4 numbers".into());
    };
    Ok(Bbox {
        min_x: min_x.to_radians(),
        min_y: min_y.to_radians(),
        max_x: max_x.to_radians(),
        max_y: max_y.to_radians(),
    })
}

fn time_function(j: &Value) -> Result<TimeFunction, String> {
    let kind = string(j, "type")?;
    if kind == "constant" {
        return Ok(TimeFunction::Constant);
    }
    let p = object(j, "parameters")?;
    let epoch = |key: &str| string(p, key).and_then(decimal_year);
    Ok(match kind {
        "velocity" => TimeFunction::Velocity {
            reference: epoch("reference_epoch")?,
        },
        "step" => TimeFunction::Step {
            epoch: epoch("step_epoch")?,
        },
        "reverse_step" => TimeFunction::ReverseStep {
            epoch: epoch("step_epoch")?,
        },
        "piecewise" => {
            let beyond = |key: &str| match string(p, key)? {
                "zero" => Ok(Beyond::Zero),
                "constant" => Ok(Beyond::Constant),
                "linear" => Ok(Beyond::Linear),
                _ => Err(format!("unsupported value for {key}")),
            };
            let Some(Value::Array(model)) = p.get("model") else {
                return Err("missing \"model\"".into());
            };
            TimeFunction::Piecewise {
                before: beyond("before_first")?,
                after: beyond("after_last")?,
                model: model
                    .iter()
                    .map(|m| {
                        Ok((
                            string(m, "epoch").and_then(decimal_year)?,
                            number(m, "scale_factor")?,
                        ))
                    })
                    .collect::<Result<_, String>>()?,
            }
        }
        "exponential" => {
            let relaxation = number(p, "relaxation_constant")?;
            if relaxation <= 0.0 {
                return Err("invalid value for relaxation_constant".into());
            }
            TimeFunction::Exponential {
                reference: epoch("reference_epoch")?,
                end: optional(p, "end_epoch")?
                    .filter(|e| !e.is_empty())
                    .map(decimal_year)
                    .transpose()?,
                relaxation,
                before: number(p, "before_scale_factor")?,
                initial: number(p, "initial_scale_factor")?,
                last: number(p, "final_scale_factor")?,
            }
        }
        other => return Err(format!("unsupported type of time function: {other}")),
    })
}

impl Model {
    /// Whether a JSON file is a deformation model's master file (and not a
    /// triangulation).
    pub(crate) fn is_master_file(text: &[u8]) -> bool {
        serde_json::from_slice::<Value>(text).is_ok_and(|j| {
            j.get("file_type").and_then(Value::as_str) == Some("deformation_model_master_file")
        })
    }

    /// `MasterFile::parse`, and the checks of `Evaluator`'s constructor.
    pub(crate) fn parse(text: &[u8]) -> Result<Model, String> {
        let j: Value = serde_json::from_slice(text).map_err(|e| e.to_string())?;
        if !j.is_object() {
            return Err("not an object".into());
        }
        string(&j, "file_type")?;
        string(&j, "format_version")?;
        if string(&j, "source_crs")? != string(&j, "definition_crs")? {
            return Err("source_crs != definition_crs is not supported".into());
        }
        string(&j, "target_crs")?;
        match optional(&j, "horizontal_offset_unit")? {
            None | Some("" | "metre") => {}
            Some(unit) => return Err(format!("horizontal offsets in {unit} are not supported")),
        }
        match optional(&j, "vertical_offset_unit")? {
            None | Some("" | "metre") => {}
            Some(_) => return Err("unsupported value for vertical_offset_unit".into()),
        }
        match optional(&j, "horizontal_offset_method")? {
            None | Some("" | "addition") => {}
            Some(method) => {
                return Err(format!(
                    "horizontal_offset_method {method} is not supported"
                ));
            }
        }
        let time = object(&j, "time_extent")?;
        let time_extent = (
            string(time, "first").and_then(decimal_year)?,
            string(time, "last").and_then(decimal_year)?,
        );
        let Some(Value::Array(list)) = j.get("components") else {
            return Err("missing \"components\"".into());
        };
        let mut components = Vec::new();
        let mut files: Vec<Arc<GridFile>> = Vec::new();
        for c in list {
            let displacement = match string(c, "displacement_type")? {
                "none" => Displacement::None,
                "horizontal" => Displacement::Horizontal,
                "vertical" => Displacement::Vertical,
                "3d" => Displacement::ThreeD,
                _ => return Err("unsupported value for displacement_type".into()),
            };
            string(c, "uncertainty_type")?;
            let spatial = object(c, "spatial_model")?;
            string(spatial, "type")?;
            match string(spatial, "interpolation_method")? {
                "bilinear" => {}
                method => return Err(format!("interpolation_method {method} is not supported")),
            }
            let file = store::file(string(spatial, "filename")?);
            if !files.iter().any(|f| Arc::ptr_eq(f, &file)) {
                files.push(file.clone());
            }
            components.push(Component {
                displacement,
                extent: bbox(object(c, "extent")?)?,
                file,
                set: OnceLock::new(),
                samples: OnceLock::new(),
                time: time_function(object(c, "time_function")?)?,
            });
        }
        Ok(Model {
            extent: bbox(object(&j, "extent")?)?,
            time_extent,
            components,
            files,
        })
    }

    /// The grid files of the components.
    pub(crate) fn files(&self) -> &[Arc<GridFile>] {
        &self.files
    }

    /// `Evaluator::forward`: from the model's source CRS to its target CRS
    /// at epoch `t`, for longitude and latitude in radians and a height.
    /// `ellipsoid` is the definition CRS's.
    pub(crate) fn forward(
        &self,
        [x_in, y, z]: [f64; 3],
        t: f64,
        ellipsoid: &Ellipsoid,
    ) -> Result<[f64; 3], CoordinateError> {
        self.shifted([x_in, y, z], t, false, ellipsoid)
    }

    fn shifted(
        &self,
        [x_in, y, z]: [f64; 3],
        t: f64,
        for_inverse: bool,
        ellipsoid: &Ellipsoid,
    ) -> Result<[f64; 3], CoordinateError> {
        const EPS: f64 = 1e-10;
        // The longitude as the model's extent has it (New Zealand's runs
        // to 194°E).
        let mut x = x_in;
        while x < self.extent.min_x - EPS {
            x += 2.0 * PI;
        }
        while x > self.extent.max_x + EPS {
            x -= 2.0 * PI;
        }
        let mut y_checked = y;
        if !bbox_check(
            &mut x,
            &mut y_checked,
            for_inverse,
            &self.extent,
            EPS,
            0.1_f64.to_radians(),
        ) {
            return Err(CoordinateError::OutsideGrid);
        }
        let y = y_checked;
        if t < self.time_extent.0 || t > self.time_extent.1 {
            return Err(CoordinateError::EpochOutOfRange);
        }
        let (mut de, mut dn, mut dz) = (0.0, 0.0, 0.0);
        for component in &self.components {
            if component.displacement == Displacement::None {
                continue;
            }
            let (mut gx, mut gy) = (x, y);
            if !bbox_check(&mut gx, &mut gy, for_inverse, &component.extent, EPS, 0.0) {
                continue;
            }
            let gx = gx.clamp(component.extent.min_x, component.extent.max_x);
            let gy = gy.clamp(component.extent.min_y, component.extent.max_y);
            let factor = component.time.at(t);
            if factor == 0.0 {
                continue;
            }
            let Some([e, n, u]) = component.offsets(gx, gy)? else {
                continue;
            };
            de += factor * e;
            dn += factor * n;
            dz += factor * u;
        }
        // `DeltaEastingNorthingToLongLat`: metres east and north as
        // longitude and latitude, by the radii of curvature at the point.
        let (a, b) = (ellipsoid.a, ellipsoid.b);
        let es = 1.0 - (b * b) / (a * a);
        let cos_phi = y.cos();
        let w2 = 1.0 - es * (1.0 - cos_phi * cos_phi);
        let w = w2.sqrt();
        let d_lam = de * w / (a * cos_phi);
        let d_phi = dn * a * w * w2 / (b * b);
        Ok([x_in + d_lam, y + d_phi, z + dz])
    }

    /// `Evaluator::inverse`: the point `forward` moves to this one, by
    /// iteration.
    pub(crate) fn inverse(
        &self,
        [x, y, z]: [f64; 3],
        t: f64,
        ellipsoid: &Ellipsoid,
    ) -> Result<[f64; 3], CoordinateError> {
        const EPS_HORIZONTAL: f64 = 1e-12;
        const EPS_VERTICAL: f64 = 1e-3;
        let mut out = [x, y, z];
        for _ in 0..10 {
            let new = self.shifted(out, t, true, ellipsoid)?;
            let (dx, dy, dz) = (new[0] - x, new[1] - y, new[2] - z);
            out = [out[0] - dx, out[1] - dy, out[2] - dz];
            if dx.abs().max(dy.abs()) < EPS_HORIZONTAL && dz.abs() < EPS_VERTICAL {
                return Ok(out);
            }
        }
        Err(CoordinateError::NoConvergence)
    }
}

/// `bboxCheck`: whether the point is in the box; for the inverse, a point
/// within `margin` outside is moved onto its edge.
fn bbox_check(
    x: &mut f64,
    y: &mut f64,
    for_inverse: bool,
    b: &Bbox,
    eps: f64,
    margin: f64,
) -> bool {
    let inside = |v: f64, min: f64, max: f64| v >= min - eps && v <= max + eps;
    if inside(*x, b.min_x, b.max_x) && inside(*y, b.min_y, b.max_y) {
        return true;
    }
    if !for_inverse {
        return false;
    }
    let snap = |v: &mut f64, min: f64, max: f64| {
        if inside(*v, min, max) {
            true
        } else if *v > min - margin && *v < min {
            *v = min;
            true
        } else if *v < max + margin && *v > max {
            *v = max;
            true
        } else {
            false
        }
    };
    // Both, as PROJ moves each onto its edge before looking at the other.
    let x_ok = snap(x, b.min_x, b.max_x);
    let y_ok = snap(y, b.min_y, b.max_y);
    x_ok && y_ok
}

impl Component {
    /// The east, north and up offsets at a point, in metres, bilinearly
    /// between the four nodes around it; `None` where the file has no grid.
    fn offsets(&self, x: f64, y: f64) -> Result<Option<[f64; 3]>, CoordinateError> {
        const EPS: f64 = 1e-10;
        let images = self.file.images().map_err(|s| match s {
            GridState::NotLoaded => CoordinateError::GridNotLoaded,
            GridState::Invalid(_) => CoordinateError::InvalidGrid,
        })?;
        let Ok(top) = self.set.get_or_init(|| generic(images)) else {
            return Err(CoordinateError::InvalidGrid);
        };
        let Some(node) = generic_grid_at(top, images, x, y) else {
            return Ok(None);
        };
        let image = &images[node.image];
        if image.width < 2 || image.height < 2 {
            return Err(CoordinateError::InvalidGrid);
        }
        let extent = &image.extent;
        let ix_d = (x - extent.west) / extent.res_x;
        let iy_d = (y - extent.south) / extent.res_y;
        if ix_d < -EPS
            || iy_d < -EPS
            || ix_d + 1.0 >= image.width as f64 + EPS
            || iy_d + 1.0 >= image.height as f64 + EPS
        {
            return Ok(None);
        }
        // C++'s cast truncates towards zero.
        let ix0 = (ix_d.max(0.0) as usize).min(image.width - 2);
        let iy0 = (iy_d.max(0.0) as usize).min(image.height - 2);
        let (fx, fy) = (ix_d - ix0 as f64, iy_d - iy0 as f64);
        let weights = [
            ((ix0, iy0), (1.0 - fx) * (1.0 - fy)),
            ((ix0 + 1, iy0), fx * (1.0 - fy)),
            ((ix0, iy0 + 1), (1.0 - fx) * fy),
            ((ix0 + 1, iy0 + 1), fx * fy),
        ];
        let samples = self
            .samples
            .get_or_init(|| images.iter().map(|_| OnceLock::new()).collect())[node.image]
            .get_or_init(|| samples(images, node));
        let mix = |sample: usize| -> Result<f64, CoordinateError> {
            let mut sum = 0.0;
            for ((ix, iy), weight) in weights {
                let v = image
                    .value(sample, ix, iy)
                    .map_err(|_| CoordinateError::GridNotLoaded)?;
                sum += weight * f64::from(v);
            }
            Ok(sum)
        };
        let invalid = |()| CoordinateError::InvalidGrid;
        let mut out = [0.0; 3];
        if self.displacement != Displacement::Vertical {
            let (east, north) = samples.horizontal.map_err(invalid)?;
            out[0] = mix(east)?;
            out[1] = mix(north)?;
        }
        if self.displacement != Displacement::Horizontal {
            out[2] = mix(samples.vertical.map_err(invalid)?)?;
        }
        Ok(Some(out))
    }
}

/// `Grid::checkHorizontal` and the start of `Grid::getZOffset`: the samples
/// by their descriptions, in metres.
fn samples(images: &[Image], node: &Node<GInfo>) -> Samples {
    let count = images[node.image].samples;
    let description =
        |i: usize| generic_metadata(images, node, "DESCRIPTION", i as i32).to_string();
    let metres = |sample: usize| {
        let unit = generic_metadata(images, node, "UNITTYPE", sample as i32);
        unit.is_empty() || unit == "metre"
    };
    let described = (0..count).any(|i| !description(i).is_empty());
    let find = |name: &str| (0..count).find(|&i| description(i) == name);

    let horizontal = (|| {
        if count < 2 {
            return Err(());
        }
        let (east, north) = match (find("east_offset"), find("north_offset")) {
            (Some(e), Some(n)) => (e, n),
            _ if described => return Err(()),
            (e, n) => (e.unwrap_or(0), n.unwrap_or(1)),
        };
        if metres(east) {
            Ok((east, north))
        } else {
            Err(())
        }
    })();
    let vertical = (|| {
        let default = match count {
            1 => 0,
            2 => return Err(()),
            _ => 2,
        };
        let up = match find("vertical_offset") {
            Some(u) => u,
            None if described => return Err(()),
            None => default,
        };
        if metres(up) { Ok(up) } else { Err(()) }
    })();
    Samples {
        horizontal,
        vertical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_decimal_years() {
        assert_eq!(decimal_year("2000-01-01T00:00:00Z"), Ok(2000.0));
        // 2016 is a leap year: 366 days.
        assert_eq!(decimal_year("2016-07-02T00:00:00Z"), Ok(2016.5));
        assert_eq!(decimal_year("2017-07-02T12:00:00Z"), Ok(2017.5));
        for bad in [
            "2000-01-01",
            "2000-13-01T00:00:00Z",
            "2001-02-29T00:00:00Z",
            "1500-01-01T00:00:00Z",
            "2000-01-01T24:00:00Z",
            "2000-01-01 00:00:00Z",
        ] {
            assert!(decimal_year(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn time_functions() {
        assert_eq!(TimeFunction::Constant.at(1990.0), 1.0);
        assert_eq!(
            TimeFunction::Velocity { reference: 2000.0 }.at(2010.5),
            10.5
        );
        let step = TimeFunction::Step { epoch: 2010.0 };
        assert_eq!((step.at(2009.9), step.at(2010.0)), (0.0, 1.0));
        let reverse = TimeFunction::ReverseStep { epoch: 2010.0 };
        assert_eq!((reverse.at(2009.9), reverse.at(2010.0)), (-1.0, 0.0));

        let piecewise = |before, after| TimeFunction::Piecewise {
            before,
            after,
            model: vec![(2010.0, 1.0), (2012.0, 3.0), (2016.0, 5.0)],
        };
        let f = piecewise(Beyond::Zero, Beyond::Zero);
        assert_eq!(
            (f.at(2009.0), f.at(2011.0), f.at(2014.0), f.at(2016.0)),
            (0.0, 2.0, 4.0, 0.0)
        );
        let f = piecewise(Beyond::Constant, Beyond::Constant);
        assert_eq!((f.at(2009.0), f.at(2020.0)), (1.0, 5.0));
        let f = piecewise(Beyond::Linear, Beyond::Linear);
        assert_eq!((f.at(2009.0), f.at(2018.0)), (0.0, 6.0));

        let f = TimeFunction::Exponential {
            reference: 2010.0,
            end: Some(2012.0),
            relaxation: 2.0,
            before: -1.0,
            initial: 1.0,
            last: 3.0,
        };
        assert_eq!(f.at(2009.0), -1.0);
        assert_eq!(f.at(2010.0), 1.0);
        let at_end = 1.0 + 2.0 * (1.0 - (-1.0_f64).exp());
        assert_eq!((f.at(2012.0), f.at(2030.0)), (at_end, at_end));
    }

    #[test]
    fn what_new_zealand_does_not_use_is_refused() {
        let model = |change: &str| {
            let text = r#"{"file_type":"deformation_model_master_file","format_version":"1.0",
                "source_crs":"EPSG:4959","target_crs":"EPSG:7907","definition_crs":"EPSG:4959",
                "horizontal_offset_unit":"metre","horizontal_offset_method":"addition",
                "extent":{"type":"bbox","parameters":{"bbox":[165,-48,180,-32]}},
                "time_extent":{"first":"1900-01-01T00:00:00Z","last":"2050-01-01T00:00:00Z"},
                "components":[{"displacement_type":"horizontal","uncertainty_type":"none",
                    "extent":{"type":"bbox","parameters":{"bbox":[165,-48,180,-32]}},
                    "spatial_model":{"type":"GeoTIFF","interpolation_method":"bilinear",
                        "filename":"defmodel-test-grid.tif"},
                    "time_function":{"type":"velocity",
                        "parameters":{"reference_epoch":"2000-01-01T00:00:00Z"}}}]}"#;
            let (from, to) = change.split_once("=>").unwrap_or(("", ""));
            Model::parse(text.replace(from, to).as_bytes())
        };
        let ok = model("").unwrap();
        assert_eq!(ok.files().len(), 1);
        assert_eq!(ok.time_extent, (1900.0, 2050.0));
        for change in [
            r#""horizontal_offset_unit":"metre"=>"horizontal_offset_unit":"degree""#,
            r#""addition"=>"geocentric""#,
            r#""bilinear"=>"geocentric_bilinear""#,
            r#""definition_crs":"EPSG:4959"=>"definition_crs":"EPSG:2193""#,
            r#""velocity"=>"spline""#,
        ] {
            assert!(model(change).is_err(), "{change}");
        }
    }
}
