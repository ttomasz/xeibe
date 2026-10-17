//! Choosing and running datum transformations (`docs/reprojection.md`,
//! level 2), the way PROJ's `proj_create_crs_to_crs` and `proj_trans` do.
//!
//! The search (`search.rs`) lists candidate operations, filtered and sorted
//! (`filter.rs`). Each candidate's area of use is then converted into the
//! source CRS, and every coordinate goes through the most accurate candidate
//! whose area contains it. Ported from PROJ 9.8.1 `src/crs_to_crs.cpp`,
//! `src/coord_operation.cpp` and `src/trans.cpp` (MIT, see PROJ-NOTICE.md).

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Arc;

use crate::crs::{Crs, Kind};
use crate::error::{CoordinateError, Error};
use crate::operation::{self, Method};
use crate::registry::registry;
use crate::transform::{from_geographic, to_geographic};

pub(crate) mod candidate;
pub(crate) mod crs;
pub(crate) mod filter;
pub(crate) mod search;

use candidate::{Candidate, SingleKind};
use crs::CrsRef;

/// One step of a runnable pipeline.
#[derive(Debug, Clone)]
enum Exec {
    /// Move the coordinates into this CRS, on the same datum.
    Convert(Box<Crs>),
    /// Apply an operation between two CRSs, either way.
    Apply {
        method: Arc<dyn Method>,
        from: Box<Crs>,
        to: Box<Crs>,
        inverse: bool,
    },
}

/// A candidate ready to run, with what the per-point choice needs.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub(crate) name: String,
    steps: Vec<Exec>,
    /// The candidate's area of use in the source CRS's internal coordinates
    /// (or in degrees of longitude and latitude for a geocentric source):
    /// min x, min y, max x, max y.
    bbox: [f64; 4],
    accuracy: f64,
    pseudo_area: f64,
    offshore: bool,
    unknown_area_name: bool,
    priority: bool,
    has_grids: bool,
}

/// The candidates for one source and target CRS.
#[derive(Debug, Clone)]
pub(crate) struct Candidates {
    list: Vec<Prepared>,
    /// The source CRS is geographic: longitudes may need normalising.
    src_geographic: bool,
    /// The source CRS is geocentric: compare in degrees on its ellipsoid.
    src_geocentric: bool,
}

/// Builds `Crs` values for the CRSs candidates pass through.
#[derive(Default)]
struct CrsBuilder {
    cache: HashMap<CrsRef, Crs>,
}

impl CrsBuilder {
    fn get(&mut self, c: CrsRef) -> Result<Crs, Error> {
        if let Some(crs) = self.cache.get(&c) {
            return Ok(crs.clone());
        }
        let crs = match c {
            CrsRef::Epsg(code) => Crs::from_epsg(code)?,
            CrsRef::Promoted(code) => Crs::from_epsg(code)?.promoted_to_3d(),
        };
        self.cache.insert(c, crs.clone());
        Ok(crs)
    }
}

fn is_priority(name: &str) -> bool {
    [
        "NAD83 to NAD83(HARN) (47)",
        "NAD83 to NAD83(HARN) (48)",
        "NAD83 to NAD83(HARN) (49)",
        "NAD83 to NAD83(HARN) (50)",
        "GDA94 to WGS 84 (1)",
        "GDA2020 to WGS 84 (2)",
    ]
    .iter()
    .any(|p| name.contains(p))
}

impl Candidates {
    /// The candidates from `src` to `dst` (EPSG codes), as PROJ prepares
    /// them. Errors when none can be used.
    pub(crate) fn new(
        src: &Crs,
        dst: &Crs,
        src_code: u32,
        dst_code: u32,
    ) -> Result<Candidates, Error> {
        let (sorted, _) = search::create_operations(CrsRef::Epsg(src_code), CrsRef::Epsg(dst_code));
        let mut builder = CrsBuilder::default();
        let src_geographic = matches!(src.kind, Kind::Geographic { .. });
        let src_geocentric = matches!(src.kind, Kind::Geocentric { .. });
        let mut list = Vec::new();
        for c in sorted {
            if !filter::is_exportable(&c) {
                continue;
            }
            let Ok(steps) = executable(&c, &mut builder) else {
                continue;
            };
            let extent = c.extent.clone().unwrap_or_else(crate::extent::world);
            let area_name = extent.name.clone().unwrap_or_default();
            let parts = if extent.west <= extent.east {
                vec![(extent.west, extent.east)]
            } else {
                vec![(extent.west, 180.0), (-180.0, extent.east)]
            };
            for (west, east) in parts {
                let (s, n) = (extent.south, extent.north);
                let (mut w, mut e) = (west.to_radians(), east.to_radians());
                if w > e {
                    e += 2.0 * PI;
                }
                let _ = &mut w;
                let pseudo_area = (e - w) * (n.to_radians().sin() - s.to_radians().sin());
                let bbox = if src_geocentric {
                    [west, s, east, n]
                } else {
                    reproject_bbox(src, west, s, east, n)
                };
                if bbox[0] > bbox[2] {
                    continue;
                }
                list.push(Prepared {
                    name: c.name.clone(),
                    steps: steps.clone(),
                    bbox,
                    accuracy: c.accuracy,
                    pseudo_area,
                    offshore: area_name.contains("- offshore"),
                    unknown_area_name: area_name.is_empty() || area_name == "unknown",
                    priority: is_priority(&c.name),
                    has_grids: c.has_grids(),
                });
            }
        }
        if list.is_empty() {
            return Err(Error::NoTransformation {
                src: src.label(),
                dst: dst.label(),
            });
        }
        Ok(Candidates {
            list,
            src_geographic,
            src_geocentric,
        })
    }

    /// One registry operation, forced (`docs/reprojection.md`: the optional
    /// override of the automatic choice). It runs in whichever direction
    /// joins the two CRSs' datums, for every coordinate, whatever its area.
    pub(crate) fn with_operation(
        src: &Crs,
        dst: &Crs,
        src_code: u32,
        dst_code: u32,
        code: u32,
    ) -> Result<Candidates, Error> {
        let not_joining = || Error::NoTransformation {
            src: src.label(),
            dst: dst.label(),
        };
        let r = registry();
        let op = r.ops.get(&code).ok_or_else(not_joining)?;
        let mut cache = crs::CrsCache::default();
        let (ds, dd) = (
            cache.datum(CrsRef::Epsg(src_code)),
            cache.datum(CrsRef::Epsg(dst_code)),
        );
        let (os, od) = (
            cache.datum(CrsRef::Epsg(op.src.unwrap_or(0))),
            cache.datum(CrsRef::Epsg(op.dst.unwrap_or(0))),
        );
        let inverse = if os == ds && od == dd {
            false
        } else if os == dd && od == ds {
            true
        } else {
            return Err(not_joining());
        };
        let c = candidate::registry_candidate(code, inverse, &mut cache).ok_or_else(not_joining)?;
        if !filter::is_exportable(&c) {
            return Err(Error::Unsupported {
                crs: format!("EPSG operation {code}"),
                what: "its method".into(),
            });
        }
        let steps = executable(&c, &mut CrsBuilder::default())?;
        Ok(Candidates {
            list: vec![Prepared {
                name: c.name.clone(),
                steps,
                bbox: [-f64::MAX, -f64::MAX, f64::MAX, f64::MAX],
                accuracy: c.accuracy,
                pseudo_area: 0.0,
                offshore: false,
                unknown_area_name: false,
                priority: false,
                has_grids: false,
            }],
            src_geographic: matches!(src.kind, Kind::Geographic { .. }),
            src_geocentric: matches!(src.kind, Kind::Geocentric { .. }),
        })
    }

    /// Names of the candidates, best first.
    pub(crate) fn names(&self) -> Vec<&str> {
        self.list.iter().map(|p| p.name.as_str()).collect()
    }

    /// `pj_get_suggested_operation` for a coordinate in the source CRS's
    /// internal form.
    fn suggest(&self, point: [f64; 2], excluded: &[usize]) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_accuracy = f64::MAX;
        for (i, alt) in self.list.iter().enumerate() {
            if excluded.contains(&i) {
                continue;
            }
            let [x, y] = point;
            let [minx, miny, maxx, maxy] = alt.bbox;
            let mut inside = x >= minx && y >= miny && x <= maxx && y <= maxy;
            if !inside && self.src_geographic && y >= miny && y <= maxy {
                let lon = normalize_longitude(x);
                inside = lon >= minx && lon <= maxx;
            }
            if !inside {
                continue;
            }
            let replace = match best {
                None => true,
                Some(b) => {
                    let cur = &self.list[b];
                    ((alt.accuracy >= 0.0 && alt.accuracy < best_accuracy)
                        || (alt.accuracy == best_accuracy
                            && alt.pseudo_area < cur.pseudo_area
                            && !(alt.unknown_area_name && !cur.unknown_area_name)
                            && !cur.priority))
                        && !alt.offshore
                }
            };
            if replace {
                best = Some(i);
                best_accuracy = alt.accuracy;
            }
        }
        best
    }

    /// The point the per-point choice looks at: internal coordinates, or
    /// degrees on the ellipsoid for a geocentric source.
    fn choice_point(&self, src: &Crs, internal: [f64; 3]) -> Result<[f64; 2], CoordinateError> {
        if self.src_geocentric {
            let [lon, lat, _] = to_geographic(src, internal)?;
            Ok([lon.to_degrees(), lat.to_degrees()])
        } else {
            Ok([internal[0], internal[1]])
        }
    }

    /// Run the best candidate for this coordinate (PROJ's `proj_trans` with
    /// alternatives), returning the target's internal coordinates and the
    /// candidate used.
    pub(crate) fn run_with(
        &self,
        src: &Crs,
        dst: &Crs,
        internal: [f64; 3],
    ) -> Result<([f64; 3], usize), CoordinateError> {
        let point = self.choice_point(src, internal)?;
        let mut excluded = Vec::new();
        let mut last_error = CoordinateError::OutsideDomain;
        for _ in 0..3 {
            let Some(best) = self.suggest(point, &excluded) else {
                break;
            };
            match run(&self.list[best].steps, src, dst, internal) {
                Ok(out) => return Ok((out, best)),
                Err(e) => last_error = e,
            }
            excluded.push(best);
        }
        // No candidate covers the point: PROJ falls back to the first one
        // that uses no grid, whatever its area.
        if let Some(i) = self.list.iter().position(|p| !p.has_grids) {
            return run(&self.list[i].steps, src, dst, internal).map(|out| (out, i));
        }
        Err(last_error)
    }

    pub(crate) fn run(
        &self,
        src: &Crs,
        dst: &Crs,
        internal: [f64; 3],
    ) -> Result<[f64; 3], CoordinateError> {
        self.run_with(src, dst, internal).map(|(out, _)| out)
    }

    /// Name of the candidate a coordinate goes through.
    pub(crate) fn used(&self, src: &Crs, dst: &Crs, internal: [f64; 3]) -> Option<&str> {
        self.run_with(src, dst, internal)
            .ok()
            .map(|(_, i)| self.list[i].name.as_str())
    }
}

fn normalize_longitude(x: f64) -> f64 {
    // Radians here; PROJ does the same in degrees.
    let mut x = x;
    if x > PI {
        x -= 2.0 * PI;
        if x > PI {
            x = (x + PI) % (2.0 * PI) - PI;
        }
    } else if x < -PI {
        x += 2.0 * PI;
        if x < -PI {
            x = (x + PI) % (2.0 * PI) - PI;
        }
    }
    x
}

/// PROJ's `reproject_bbox`: 21 points per edge of a box in degrees
/// (Greenwich) converted into the CRS's internal coordinates, min and max of
/// those that convert. The whole world stays unbounded.
fn reproject_bbox(crs: &Crs, west: f64, south: f64, east: f64, north: f64) -> [f64; 4] {
    if west == -180.0 && east == 180.0 && south == -90.0 && north == 90.0 {
        return [-f64::MAX, -f64::MAX, f64::MAX, f64::MAX];
    }
    const N_STEPS: usize = 20;
    let pm = crs.datum().prime_meridian;
    let step_lon = (east - west) / N_STEPS as f64;
    let step_lat = (north - south) / N_STEPS as f64;
    let mut out = [f64::MAX, f64::MAX, -f64::MAX, -f64::MAX];
    let mut add = |lon: f64, lat: f64| {
        let g = [lon.to_radians() - pm, lat.to_radians(), 0.0];
        if let Ok(p) = from_geographic(crs, g)
            && p[0].is_finite()
            && p[1].is_finite()
        {
            out[0] = out[0].min(p[0]);
            out[1] = out[1].min(p[1]);
            out[2] = out[2].max(p[0]);
            out[3] = out[3].max(p[1]);
        }
    };
    for j in 0..=N_STEPS {
        let x = west + j as f64 * step_lon;
        let y = south + j as f64 * step_lat;
        add(x, south);
        add(x, north);
        add(west, y);
        add(east, y);
    }
    out
}

/// Drops neighbouring registry operations that undo each other in PROJ's
/// pipeline: the same method and parameters, one inverse, with the same
/// ellipsoid on both outer ends. PROJ's pipeline optimiser removes such
/// pairs symbolically; running them would leave the error of the
/// small-angle Helmert inverse (millimetres).
fn cancel_inverse_pairs(steps: &[candidate::Single]) -> Vec<candidate::Single> {
    let r = registry();
    let ellipsoid = |code: u32| {
        r.geodetic
            .get(&code)
            .and_then(|g| r.datums.get(&g.datum))
            .map(|d| d.ellipsoid)
    };
    let mut out: Vec<candidate::Single> = Vec::new();
    for step in steps {
        if let (
            Some(prev),
            SingleKind::Registry {
                code: b,
                inverse: ib,
            },
        ) = (out.last(), &step.kind)
            && let SingleKind::Registry {
                code: a,
                inverse: ia,
            } = prev.kind
            && ia != *ib
        {
            let (oa, ob) = (&r.ops[&a], &r.ops[b]);
            let crs_code = |c: CrsRef| match c {
                CrsRef::Epsg(c) | CrsRef::Promoted(c) => c,
            };
            if oa.method == ob.method
                && oa.params == ob.params
                && oa.table != crate::registry::Table::Conversion
                && ellipsoid(crs_code(prev.src)).is_some()
                && ellipsoid(crs_code(prev.src)) == ellipsoid(crs_code(step.dst))
            {
                out.pop();
                continue;
            }
        }
        out.push(step.clone());
    }
    out
}

/// The runnable steps of a candidate.
fn executable(c: &Candidate, builder: &mut CrsBuilder) -> Result<Vec<Exec>, Error> {
    let mut steps = Vec::new();
    for s in &cancel_inverse_pairs(&c.steps) {
        match &s.kind {
            SingleKind::Registry { code, inverse } => {
                let op = &registry().ops[code];
                let from = builder.get(CrsRef::Epsg(op.src.unwrap_or(0)))?;
                let to = builder.get(CrsRef::Epsg(op.dst.unwrap_or(0)))?;
                if op.table == crate::registry::Table::Conversion {
                    // A conversion step inside a concatenated operation.
                    steps.push(Exec::Convert(Box::new(if *inverse { from } else { to })));
                    continue;
                }
                let method = operation::build(op, &from, &to).map_err(|e| Error::Unsupported {
                    crs: format!("EPSG operation {code}"),
                    what: format!("{e:?}"),
                })?;
                steps.push(Exec::Apply {
                    method,
                    from: Box::new(from),
                    to: Box::new(to),
                    inverse: *inverse,
                });
            }
            SingleKind::Projection { .. } | SingleKind::SameDatum | SingleKind::EnsembleMember => {
                steps.push(Exec::Convert(Box::new(builder.get(s.dst)?)));
            }
        }
    }
    Ok(steps)
}

/// Coordinates of one CRS to another on the same datum, or, after a pair of
/// operations cancelled, across datums on one ellipsoid (PROJ then keeps
/// longitude, latitude and height too).
fn convert(a: &Crs, b: &Crs, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
    from_geographic(b, to_geographic(a, c)?)
}

/// Whether the next operation after step `i` (past conversions between
/// geographic CRSs, which add no PROJ step) is a geog2D-domain one, so the
/// `pop`/`push` pair between them cancels in PROJ's pipeline.
fn next_keeps_height(steps: &[Exec], i: usize) -> bool {
    for step in &steps[i + 1..] {
        match step {
            Exec::Convert(to) => {
                if !matches!(to.kind, Kind::Geographic { .. }) {
                    return false;
                }
            }
            Exec::Apply { method, .. } => return method.keeps_height(),
        }
    }
    false
}

fn run(
    steps: &[Exec],
    src: &Crs,
    dst: &Crs,
    internal: [f64; 3],
) -> Result<[f64; 3], CoordinateError> {
    let mut cur = src;
    let mut c = internal;
    // The height `push +v_3` saved, while a run of geog2D-domain operations
    // is under way.
    let mut saved: Option<f64> = None;
    for (i, step) in steps.iter().enumerate() {
        match step {
            Exec::Convert(to) => {
                c = convert(cur, to, c)?;
                cur = to;
            }
            Exec::Apply {
                method,
                from,
                to,
                inverse,
            } => {
                let (input, output) = if *inverse { (to, from) } else { (from, to) };
                c = convert(cur, input, c)?;
                if method.keeps_height() {
                    saved.get_or_insert(c[2]);
                    c = if *inverse {
                        method.inverse_3d(c)?
                    } else {
                        method.forward_3d(c)?
                    };
                    if !next_keeps_height(steps, i)
                        && let Some(h) = saved.take()
                    {
                        c[2] = h;
                    }
                } else {
                    c = if *inverse {
                        method.inverse(c)?
                    } else {
                        method.forward(c)?
                    };
                }
                if !c.iter().all(|v| v.is_finite()) {
                    return Err(CoordinateError::OutsideDomain);
                }
                cur = output;
            }
        }
    }
    convert(cur, dst, c)
}
