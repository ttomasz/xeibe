//! Triangulated transformations (PROJ's `tinshift`): a JSON file of
//! vertices with their source and target positions (or a vertical offset),
//! and triangles; a point moves by barycentric interpolation in the
//! triangle it falls in.
//!
//! Ported from PROJ 9.8.1 `src/transformations/tinshift_impl.hpp` (MIT, see
//! PROJ-NOTICE.md). PROJ finds the triangle through a quadtree; here a
//! uniform grid of buckets does the same job. Either returns the first
//! triangle that contains the point, which only matters on a shared edge,
//! where both give the same value.

use std::sync::OnceLock;

use serde_json::Value;

const EPS: f64 = 1e-10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fallback {
    None,
    NearestSide,
    NearestCentroid,
}

/// A parsed TIN file.
#[derive(Debug)]
pub(crate) struct Tin {
    horizontal: bool,
    vertical: bool,
    /// Per vertex: source x, y; target x, y when horizontal; z offset when
    /// vertical.
    vertices: Vec<f64>,
    columns: usize,
    triangles: Vec<[usize; 3]>,
    fallback: Fallback,
    forward_index: OnceLock<Index>,
    inverse_index: OnceLock<Index>,
}

fn string<'a>(j: &'a Value, key: &str, optional: bool) -> Result<Option<&'a str>, String> {
    match j.get(key) {
        None if optional => Ok(None),
        None => Err(format!("missing \"{key}\"")),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("\"{key}\" should be a string")),
    }
}

fn array<'a>(j: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    match j.get(key) {
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(format!("\"{key}\" should be an array")),
        None => Err(format!("missing \"{key}\"")),
    }
}

impl Tin {
    /// `TINShiftFile::parse`.
    pub(crate) fn parse(text: &[u8]) -> Result<Tin, String> {
        let j: Value = serde_json::from_slice(text).map_err(|e| e.to_string())?;
        if !j.is_object() {
            return Err("not an object".into());
        }
        string(&j, "file_type", false)?;
        let version = string(&j, "format_version", false)?.unwrap_or("");
        let fallback = match string(&j, "fallback_strategy", true)? {
            None => Fallback::None,
            Some(_) if version != "1.1" => {
                return Err("fallback_strategy needs format_version 1.1".into());
            }
            Some("nearest_side") => Fallback::NearestSide,
            Some("nearest_centroid") => Fallback::NearestCentroid,
            Some("none") => Fallback::None,
            Some(_) => return Err("invalid fallback_strategy".into()),
        };
        let (mut horizontal, mut vertical) = (false, false);
        for c in array(&j, "transformed_components")? {
            match c.as_str() {
                Some("horizontal") => horizontal = true,
                Some("vertical") => vertical = true,
                _ => return Err("unhandled transformed_components[] item".into()),
            }
        }
        let names = array(&j, "vertices_columns")?;
        let col = |name: &str| names.iter().position(|c| c.as_str() == Some(name));
        let (sx, sy) = (
            col("source_x").ok_or("source_x missing")?,
            col("source_y").ok_or("source_y missing")?,
        );
        let (tx, ty) = (col("target_x"), col("target_y"));
        let (sz, tz, oz) = (col("source_z"), col("target_z"), col("offset_z"));
        if horizontal && (tx.is_none() || ty.is_none()) {
            return Err("target_x and target_y must be given".into());
        }
        if vertical && oz.is_none() && (sz.is_none() || tz.is_none()) {
            return Err("offset_z, or source_z and target_z, must be given".into());
        }
        let columns = 2 + if horizontal { 2 } else { 0 } + usize::from(vertical);
        let raw = array(&j, "vertices")?;
        let mut vertices = Vec::with_capacity(raw.len() * columns);
        for v in raw {
            let v = v.as_array().ok_or("vertices[] item is not an array")?;
            if v.len() != names.len() {
                return Err("vertices[] item has not the expected length".into());
            }
            let num = |i: usize| v[i].as_f64().ok_or("vertices[][] item is not a number");
            vertices.push(num(sx)?);
            vertices.push(num(sy)?);
            if let (true, Some(tx), Some(ty)) = (horizontal, tx, ty) {
                vertices.push(num(tx)?);
                vertices.push(num(ty)?);
            }
            if vertical {
                match (oz, sz, tz) {
                    (Some(oz), _, _) => vertices.push(num(oz)?),
                    (None, Some(sz), Some(tz)) => vertices.push(num(tz)? - num(sz)?),
                    _ => unreachable!("checked above"),
                }
            }
        }
        let tnames = array(&j, "triangles_columns")?;
        let tcol = |name: &str| tnames.iter().position(|c| c.as_str() == Some(name));
        let idx = [
            tcol("idx_vertex1").ok_or("idx_vertex1 missing")?,
            tcol("idx_vertex2").ok_or("idx_vertex2 missing")?,
            tcol("idx_vertex3").ok_or("idx_vertex3 missing")?,
        ];
        let mut triangles = Vec::new();
        for t in array(&j, "triangles")? {
            let t = t.as_array().ok_or("triangles[] item is not an array")?;
            if t.len() != tnames.len() {
                return Err("triangles[] item has not the expected length".into());
            }
            let mut tri = [0usize; 3];
            for (k, &i) in idx.iter().enumerate() {
                let v = t[i]
                    .as_u64()
                    .ok_or("triangles[][] item is not an integer")?;
                if v as usize >= raw.len() {
                    return Err("invalid vertex index".into());
                }
                tri[k] = v as usize;
            }
            triangles.push(tri);
        }
        Ok(Tin {
            horizontal,
            vertical,
            vertices,
            columns,
            triangles,
            fallback,
            forward_index: OnceLock::new(),
            inverse_index: OnceLock::new(),
        })
    }

    /// Column of x and y: the source positions, or the target ones for the
    /// inverse of a horizontal TIN.
    fn xy_columns(&self, forward: bool) -> (usize, usize) {
        if self.horizontal && !forward {
            (2, 3)
        } else {
            (0, 1)
        }
    }

    fn corner(&self, vertex: usize, forward: bool) -> (f64, f64) {
        let (ix, iy) = self.xy_columns(forward);
        let base = vertex * self.columns;
        (self.vertices[base + ix], self.vertices[base + iy])
    }

    fn index(&self, forward: bool) -> &Index {
        let cell = if forward {
            &self.forward_index
        } else {
            &self.inverse_index
        };
        cell.get_or_init(|| Index::new(self, forward))
    }

    /// `FindTriangle`: the triangle and barycentric weights.
    fn find(&self, x: f64, y: f64, forward: bool) -> Option<(usize, [f64; 3])> {
        let weights = |t: &[usize; 3]| {
            let (x1, y1) = self.corner(t[0], forward);
            let (x2, y2) = self.corner(t[1], forward);
            let (x3, y3) = self.corner(t[2], forward);
            let det = (y2 - y3) * (x1 - x3) + (x3 - x2) * (y1 - y3);
            let l1 = ((y2 - y3) * (x - x3) + (x3 - x2) * (y - y3)) / det;
            let l2 = ((y3 - y1) * (x - x3) + (x1 - x3) * (y - y3)) / det;
            (det, l1, l2)
        };
        for &i in self.index(forward).candidates(x, y) {
            let (_, l1, l2) = weights(&self.triangles[i]);
            if (-EPS..=1.0 + EPS).contains(&l1) && (-EPS..=1.0 + EPS).contains(&l2) {
                let l3 = 1.0 - l1 - l2;
                if l3 >= 0.0 {
                    return Some((i, [l1, l2, l3]));
                }
            }
        }
        if self.fallback == Fallback::None {
            return None;
        }
        let sq = |a: f64, b: f64, c: f64, d: f64| (a - c).powi(2) + (b - d).powi(2);
        let segment = |x1: f64, y1: f64, x2: f64, y2: f64, d12: f64| {
            let t = ((x - x1) * (x2 - x1) + (y - y1) * (y2 - y1)) / d12;
            if t <= 0.0 {
                sq(x, y, x1, y1)
            } else if t >= 1.0 {
                sq(x, y, x2, y2)
            } else {
                sq(x, y, x1 + t * (x2 - x1), y1 + t * (y2 - y1))
            }
        };
        let (mut closest, mut closest2, mut best) = (f64::INFINITY, f64::INFINITY, 0);
        for (i, t) in self.triangles.iter().enumerate() {
            let (x1, y1) = self.corner(t[0], forward);
            let (x2, y2) = self.corner(t[1], forward);
            let (x3, y3) = self.corner(t[2], forward);
            if x + closest < x1.min(x2).min(x3)
                || x - closest > x1.max(x2).max(x3)
                || y + closest < y1.min(y2).min(y3)
                || y - closest > y1.max(y2).max(y3)
            {
                continue;
            }
            let (d12, d23, d13) = (sq(x1, y1, x2, y2), sq(x2, y2, x3, y3), sq(x1, y1, x3, y3));
            if d12 < EPS || d23 < EPS || d13 < EPS {
                continue;
            }
            let distances = match self.fallback {
                Fallback::NearestSide => vec![
                    segment(x1, y1, x2, y2, d12),
                    segment(x2, y2, x3, y3, d23),
                    segment(x1, y1, x3, y3, d13),
                ],
                _ => vec![sq(x, y, (x1 + x2 + x3) / 3.0, (y1 + y2 + y3) / 3.0)],
            };
            for d2 in distances {
                if d2 < closest2 {
                    closest2 = d2;
                    closest = d2.sqrt();
                    best = i;
                }
            }
        }
        if closest.is_infinite() {
            return None;
        }
        let (det, l1, l2) = weights(&self.triangles[best]);
        if det.abs() < EPS {
            return None;
        }
        Some((best, [l1, l2, 1.0 - l1 - l2]))
    }

    fn interpolate(&self, t: usize, l: [f64; 3], column: usize) -> f64 {
        let tri = &self.triangles[t];
        self.vertices[tri[0] * self.columns + column] * l[0]
            + self.vertices[tri[1] * self.columns + column] * l[1]
            + self.vertices[tri[2] * self.columns + column] * l[2]
    }

    /// `Evaluator::forward`; `None` outside the triangulation.
    pub(crate) fn forward(&self, x: f64, y: f64, z: f64) -> Option<[f64; 3]> {
        let (t, l) = self.find(x, y, true)?;
        let (x, y) = if self.horizontal {
            (self.interpolate(t, l, 2), self.interpolate(t, l, 3))
        } else {
            (x, y)
        };
        let z = if self.vertical {
            z + self.interpolate(t, l, if self.horizontal { 4 } else { 2 })
        } else {
            z
        };
        Some([x, y, z])
    }

    /// `Evaluator::inverse`.
    pub(crate) fn inverse(&self, x: f64, y: f64, z: f64) -> Option<[f64; 3]> {
        // A vertical-only TIN is searched on its source positions.
        let forward_positions = !self.horizontal && self.vertical;
        let (t, l) = self.find(x, y, forward_positions)?;
        let (x, y) = if self.horizontal {
            (self.interpolate(t, l, 0), self.interpolate(t, l, 1))
        } else {
            (x, y)
        };
        let z = if self.vertical {
            z - self.interpolate(t, l, if self.horizontal { 4 } else { 2 })
        } else {
            z
        };
        Some([x, y, z])
    }
}

/// Triangles by the buckets of a uniform grid their boxes overlap.
#[derive(Debug)]
struct Index {
    min: (f64, f64),
    cell: (f64, f64),
    size: (usize, usize),
    buckets: Vec<Vec<usize>>,
}

impl Index {
    fn new(tin: &Tin, forward: bool) -> Index {
        let n = tin.vertices.len() / tin.columns;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for v in 0..n {
            let (x, y) = tin.corner(v, forward);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let side = ((tin.triangles.len() as f64).sqrt().ceil() as usize).clamp(1, 1024);
        let cell = (
            ((x1 - x0) / side as f64).max(f64::MIN_POSITIVE),
            ((y1 - y0) / side as f64).max(f64::MIN_POSITIVE),
        );
        let mut index = Index {
            min: (x0, y0),
            cell,
            size: (side, side),
            buckets: vec![Vec::new(); side * side],
        };
        for (i, t) in tin.triangles.iter().enumerate() {
            let corners = t.map(|v| tin.corner(v, forward));
            let bx0 = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min);
            let by0 = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min);
            let bx1 = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max);
            let by1 = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max);
            let (c0, r0) = index.cell_of(bx0, by0);
            let (c1, r1) = index.cell_of(bx1, by1);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    index.buckets[r * side + c].push(i);
                }
            }
        }
        index
    }

    fn cell_of(&self, x: f64, y: f64) -> (usize, usize) {
        let c = ((x - self.min.0) / self.cell.0).floor();
        let r = ((y - self.min.1) / self.cell.1).floor();
        (
            (c.max(0.0) as usize).min(self.size.0 - 1),
            (r.max(0.0) as usize).min(self.size.1 - 1),
        )
    }

    /// The triangles whose boxes overlap the point's bucket. (A point
    /// within PROJ's 1e-10 tolerance outside a triangle's box, in the next
    /// bucket, misses that triangle; its neighbour gives the same value.)
    fn candidates(&self, x: f64, y: f64) -> &[usize] {
        let (c, r) = self.cell_of(x, y);
        &self.buckets[r * self.size.0 + c]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two triangles over the unit square, moving every point by (+1, +2)
    /// and lifting it by the x coordinate.
    const SQUARE: &str = r#"{
        "file_type": "triangulation_file", "format_version": "1.0",
        "transformed_components": ["horizontal", "vertical"],
        "vertices_columns": ["source_x", "source_y", "target_x", "target_y", "offset_z"],
        "triangles_columns": ["idx_vertex1", "idx_vertex2", "idx_vertex3"],
        "vertices": [[0, 0, 1, 2, 0], [1, 0, 2, 2, 1], [1, 1, 2, 3, 1], [0, 1, 1, 3, 0]],
        "triangles": [[0, 1, 2], [0, 2, 3]]
    }"#;

    #[test]
    fn interpolates_inside_and_inverts() {
        let tin = Tin::parse(SQUARE.as_bytes()).unwrap();
        let p = tin.forward(0.25, 0.5, 10.0).unwrap();
        assert!((p[0] - 1.25).abs() < 1e-15 && (p[1] - 2.5).abs() < 1e-15);
        assert!((p[2] - 10.25).abs() < 1e-15);
        let q = tin.inverse(p[0], p[1], p[2]).unwrap();
        assert!((q[0] - 0.25).abs() < 1e-15 && (q[1] - 0.5).abs() < 1e-15);
        assert!(tin.forward(1.5, 0.5, 0.0).is_none());
    }
}
