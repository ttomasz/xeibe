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
use crate::grid::Grids;
use crate::grid::store::GridFile;
use crate::operation::{self, Method};
use crate::registry::registry;
use crate::transform::{from_geographic, to_geographic};

pub(crate) mod candidate;
pub(crate) mod crs;
pub(crate) mod filter;
pub(crate) mod search;

use candidate::{Candidate, SingleKind};
use crs::CrsCache;
use crs::CrsRef;

/// One step of a runnable pipeline.
#[derive(Debug, Clone)]
enum Exec {
    /// Move the coordinates into this CRS, on the same datum.
    Convert(Box<Crs>),
    /// Keep the horizontal position (PROJ's `push +v_1 +v_2`).
    SaveHorizontal,
    /// Put it back (`pop +v_1 +v_2`), with the CRS it was in.
    RestoreHorizontal,
    /// Apply an operation between two CRSs, either way.
    Apply {
        method: Arc<dyn Method>,
        from: Box<Crs>,
        to: Box<Crs>,
        inverse: bool,
        height: Height,
    },
}

/// What a step does to the height.
#[derive(Debug, Clone, Copy)]
enum Height {
    /// It comes back as it went in (PROJ's `push`/`pop +v_3`).
    Kept,
    /// The operation gives the new one.
    Changed,
    /// A datum shift that models the ellipsoidal height: an ellipsoidal
    /// height changes with it, a gravity-related height (or none) doesn't.
    /// PROJ decides this from the dimensions of the operation's CRSs as the
    /// route uses them, which comes to the same on all of EPSG.
    Ellipsoidal,
}

impl Height {
    /// Whether the height is kept, run on `input`'s coordinates with the
    /// height now ellipsoidal or not.
    fn keeps(self, input: &Crs, ellipsoidal: bool) -> bool {
        match self {
            Height::Kept => true,
            Height::Changed => false,
            // On geocentric coordinates the third value is Z, never kept.
            Height::Ellipsoidal => {
                matches!(input.horizontal().kind, Kind::Geographic { .. }) && !ellipsoidal
            }
        }
    }
}

/// One of the operations a transform chooses among: a candidate of the
/// selection, as [`crate::Transform::operation_info`] lists them.
#[derive(Debug, Clone, PartialEq)]
pub struct OperationInfo {
    /// PROJ's name for it, e.g. `Inverse of ETRS89 to WGS 84 (1) + Poland
    /// CS92`.
    pub name: String,
    /// The EPSG operation it is, and whether it runs inverted, when it is
    /// one (a single or concatenated operation); `None` for a route the
    /// search put together.
    pub code: Option<(u32, bool)>,
    /// The EPSG transformations it runs, in order, each with whether it
    /// runs inverted. Map projections and the changes on one datum are left
    /// out.
    pub steps: Vec<(u32, bool)>,
    /// In metres; `None` when unknown.
    pub accuracy: Option<f64>,
    /// The area of use in degrees: west, south, east, north (east < west
    /// across the antimeridian). `None` when it has none.
    pub area: Option<[f64; 4]>,
    /// EPSG's description of that area.
    pub area_name: Option<String>,
    /// PROJ-data's names of the grid files it reads.
    pub grids: Vec<String>,
    /// A step is time-dependent: it needs the coordinates' epoch.
    pub time_dependent: bool,
    /// EPSG's scope of the operation, when it is one EPSG operation (see
    /// `code`). Its remarks: `xeibe_crs::operation_remarks`.
    pub scope: Option<String>,
}

impl OperationInfo {
    fn of(c: &Candidate, steps: &[Exec]) -> OperationInfo {
        fn transformations(steps: &[candidate::Single], out: &mut Vec<(u32, bool)>) {
            for s in steps {
                match &s.kind {
                    SingleKind::Registry { code, inverse } => out.push((*code, *inverse)),
                    SingleKind::Composite { parts, .. } => transformations(parts, out),
                    _ => {}
                }
            }
        }
        let mut codes = Vec::new();
        transformations(&c.steps, &mut codes);
        let mut grids: Vec<String> = Vec::new();
        for step in steps {
            if let Exec::Apply { method, .. } = step {
                for f in method.grid_files() {
                    if !grids.contains(&f.name) {
                        grids.push(f.name.clone());
                    }
                }
            }
        }
        OperationInfo {
            name: c.name.clone(),
            code: c.registry,
            steps: codes,
            accuracy: (c.accuracy >= 0.0).then_some(c.accuracy),
            area: c
                .extent
                .as_ref()
                .map(|e| [e.west, e.south, e.east, e.north]),
            area_name: c.extent.as_ref().and_then(|e| e.name.clone()),
            grids,
            time_dependent: c.requires_time,
            scope: c
                .registry
                .and_then(|(code, _)| registry().ops.get(&code))
                .and_then(|op| op.scope.clone()),
        }
    }
}

/// A candidate ready to run, with what the per-point choice needs.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub(crate) name: String,
    /// Which candidate this is, counted from the best; a candidate across
    /// the antimeridian is prepared twice, with one index.
    index: usize,
    candidate: Arc<Candidate>,
    info: Arc<OperationInfo>,
    steps: Vec<Exec>,
    /// The candidate's area of use in the source CRS's internal coordinates
    /// (or in degrees of longitude and latitude for a geocentric source):
    /// min x, min y, max x, max y.
    bbox: [f64; 4],
    /// The area of use in degrees (west, south, east, north), the part on
    /// this side of the antimeridian.
    area: [f64; 4],
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
    /// One candidate only: PROJ then returns the operation itself, which
    /// applies to every coordinate whatever its area.
    single: bool,
    /// The grid files the candidates read, each once.
    grid_files: Vec<Arc<GridFile>>,
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
            CrsRef::Compound(h, v) => Crs::compound(&Crs::from_epsg(h)?, &Crs::from_epsg(v)?)?,
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
        src_ref: CrsRef,
        dst_ref: CrsRef,
        grids: &Grids,
    ) -> Result<Candidates, Error> {
        let (sorted, _) = search::create_operations(src_ref, dst_ref, grids.clone());
        let mut builder = CrsBuilder::default();
        let src_geographic = matches!(src.kind, Kind::Geographic { .. });
        let src_geocentric = matches!(src.kind, Kind::Geocentric { .. });
        let mut list = Vec::new();
        let mut operations = 0;
        let mut first: Option<(Arc<Candidate>, Vec<Exec>, Arc<OperationInfo>)> = None;
        for c in sorted {
            if !filter::is_exportable(&c) {
                continue;
            }
            let Ok(steps) = executable(&c, &mut builder) else {
                continue;
            };
            let index = operations;
            operations += 1;
            let info = Arc::new(OperationInfo::of(&c, &steps));
            let c = Arc::new(c);
            if first.is_none() {
                first = Some((c.clone(), steps.clone(), info.clone()));
            }
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
                    index,
                    candidate: c.clone(),
                    info: info.clone(),
                    steps: steps.clone(),
                    bbox,
                    area: [west, s, east, n],
                    accuracy: c.accuracy,
                    pseudo_area,
                    offshore: area_name.contains("- offshore"),
                    unknown_area_name: area_name.is_empty() || area_name == "unknown",
                    priority: is_priority(&c.name),
                    has_grids: c.has_grids(),
                });
            }
        }
        // A single operation whose area doesn't convert into the source CRS
        // still serves, as PROJ returns it directly.
        if list.is_empty()
            && operations == 1
            && let Some((c, steps, info)) = first
        {
            list.push(Prepared::everywhere(c, steps, info));
        }
        if list.is_empty() {
            return Err(Error::NoTransformation {
                src: src.label(),
                dst: dst.label(),
            });
        }
        // PROJ's `proj_create_crs_to_crs`: a single operation, or a single
        // prepared one, is returned as the transformation itself.
        let single = operations == 1 || list.len() == 1;
        Ok(Candidates {
            grid_files: grid_files(&list),
            list,
            src_geographic,
            src_geocentric,
            single,
        })
    }

    /// The grid files the candidates read.
    pub(crate) fn grid_files(&self) -> &[Arc<GridFile>] {
        &self.grid_files
    }

    /// The grid files of the candidates whose area of use meets `area`
    /// (degrees: west, south, east, north; east < west across the
    /// antimeridian): only those can serve coordinates there.
    pub(crate) fn grid_files_within(
        &self,
        area: [f64; 4],
        internal: Option<[f64; 4]>,
    ) -> Vec<Arc<GridFile>> {
        if self.single {
            return self.grid_files.clone();
        }
        let [w, s, e, n] = area;
        let e = if e < w { e + 360.0 } else { e };
        let meets = |a: &[f64; 4]| {
            a[1] <= n
                && a[3] >= s
                && [-360.0, 0.0, 360.0]
                    .iter()
                    .any(|shift| a[0] + shift <= e && a[2] + shift >= w)
        };
        // The per-point choice compares the coordinates with each
        // candidate's box in the source CRS, which for a projected CRS
        // reaches beyond the area of use.
        let in_box = |b: &[f64; 4]| {
            internal.is_some_and(|[x0, y0, x1, y1]| {
                b[0] <= x1 && b[2] >= x0 && b[1] <= y1 && b[3] >= y0
            })
        };
        let near: Vec<Prepared> = self
            .list
            .iter()
            .filter(|p| meets(&p.area) || in_box(&p.bbox))
            .cloned()
            .collect();
        grid_files(&near)
    }

    /// One registry operation, forced (`docs/reprojection.md`: the optional
    /// override of the automatic choice). It runs in whichever direction
    /// joins the two CRSs' datums, for every coordinate, whatever its area.
    pub(crate) fn with_operation(
        src: &Crs,
        dst: &Crs,
        src_ref: CrsRef,
        dst_ref: CrsRef,
        code: u32,
    ) -> Result<Candidates, Error> {
        let not_joining = || Error::NoTransformation {
            src: src.label(),
            dst: dst.label(),
        };
        let r = registry();
        let op = r.ops.get(&code).ok_or_else(not_joining)?;
        let mut cache = crs::CrsCache::default();
        // The geodetic, vertical and engineering datums each end of the
        // operation names must be those of the CRS on that side.
        let engineering = |c: CrsRef| match c {
            CrsRef::Epsg(code) => Crs::from_epsg(code)
                .ok()
                .and_then(|c| c.engineering_datum().map(|d| d.key.clone())),
            _ => None,
        };
        let mut datums = |c: CrsRef| (cache.datum(c), cache.vertical_datum(c), engineering(c));
        let (us, ud) = (datums(src_ref), datums(dst_ref));
        let (os, od) = (
            datums(CrsRef::Epsg(op.src.unwrap_or(0))),
            datums(CrsRef::Epsg(op.dst.unwrap_or(0))),
        );
        type Datums = (Option<u32>, Option<u32>, Option<String>);
        let fits = |o: &Datums, u: &Datums| {
            (o.0.is_some() || o.1.is_some() || o.2.is_some())
                && (o.0.is_none() || o.0 == u.0)
                && (o.1.is_none() || o.1 == u.1)
                && o.2 == u.2
        };
        let inverse = if fits(&os, &us) && fits(&od, &ud) {
            false
        } else if fits(&os, &ud) && fits(&od, &us) {
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
        Candidates::one(c, src)
    }

    /// One candidate, run for every coordinate whatever its area.
    fn one(c: Candidate, src: &Crs) -> Result<Candidates, Error> {
        let steps = executable(&c, &mut CrsBuilder::default())?;
        let info = Arc::new(OperationInfo::of(&c, &steps));
        let list = vec![Prepared::everywhere(Arc::new(c), steps, info)];
        Ok(Candidates {
            grid_files: grid_files(&list),
            list,
            src_geographic: matches!(src.kind, Kind::Geographic { .. }),
            src_geocentric: matches!(src.kind, Kind::Geocentric { .. }),
            single: true,
        })
    }

    /// Candidate `index` only (see [`Candidates::info`]), for every
    /// coordinate: one member of PROJ's `TransformerGroup`.
    pub(crate) fn only(&self, index: usize, src: &Crs) -> Option<Result<Candidates, Error>> {
        let p = self.list.iter().find(|p| p.index == index)?;
        Some(Candidates::one((*p.candidate).clone(), src))
    }

    /// The first candidate the other way, for every coordinate: the
    /// inverse of [`Candidates::only`]. `src` is the inverse's source.
    pub(crate) fn inverse_of_first(&self, src: &Crs) -> Result<Candidates, Error> {
        let inverse = self.list[0].candidate.inverse(&mut CrsCache::default());
        Candidates::one(inverse, src)
    }

    /// Names of the candidates, best first.
    pub(crate) fn names(&self) -> Vec<&str> {
        self.list.iter().map(|p| p.name.as_str()).collect()
    }

    /// The candidates' descriptions, best first, each once.
    pub(crate) fn info(&self) -> Vec<OperationInfo> {
        let mut out: Vec<OperationInfo> = Vec::new();
        let mut last = None;
        for p in &self.list {
            if last != Some(p.index) {
                out.push((*p.info).clone());
                last = Some(p.index);
            }
        }
        out
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
        if self.single {
            return run(&self.list[0].steps, src, dst, internal).map(|out| (out, 0));
        }
        let point = self.choice_point(src, internal)?;
        let mut excluded = Vec::new();
        let mut last_error = CoordinateError::OutsideDomain;
        for _ in 0..3 {
            let Some(best) = self.suggest(point, &excluded) else {
                break;
            };
            match run(&self.list[best].steps, src, dst, internal) {
                Ok(out) => return Ok((out, best)),
                // A grid that isn't loaded or can't be read is an error, not
                // a reason to try a less accurate operation (PROJ returns
                // at once on a network error).
                Err(e @ (CoordinateError::GridNotLoaded | CoordinateError::InvalidGrid)) => {
                    return Err(e);
                }
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

    /// Index (see [`Candidates::info`]) of the candidate a coordinate goes
    /// through.
    pub(crate) fn used_index(&self, src: &Crs, dst: &Crs, internal: [f64; 3]) -> Option<usize> {
        self.run_with(src, dst, internal)
            .ok()
            .map(|(_, i)| self.list[i].index)
    }
}

impl Prepared {
    /// A candidate for every coordinate, whatever its area.
    fn everywhere(
        candidate: Arc<Candidate>,
        steps: Vec<Exec>,
        info: Arc<OperationInfo>,
    ) -> Prepared {
        Prepared {
            name: candidate.name.clone(),
            index: 0,
            accuracy: candidate.accuracy,
            has_grids: candidate.has_grids(),
            candidate,
            info,
            steps,
            bbox: [-f64::MAX, -f64::MAX, f64::MAX, f64::MAX],
            area: [-180.0, -90.0, 180.0, 90.0],
            pseudo_area: 0.0,
            offshore: false,
            unknown_area_name: false,
            priority: false,
        }
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
    let pm = crs.datum().map_or(0.0, |d| d.prime_meridian);
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

fn grid_files(list: &[Prepared]) -> Vec<Arc<GridFile>> {
    let mut out: Vec<Arc<GridFile>> = Vec::new();
    for p in list {
        for step in &p.steps {
            if let Exec::Apply { method, .. } = step {
                for f in method.grid_files() {
                    if !out.iter().any(|g| Arc::ptr_eq(g, &f)) {
                        out.push(f);
                    }
                }
            }
        }
    }
    out
}

/// A candidate's operations in the order they run, with the markers of
/// combined operations that keep the horizontal position.
enum Flat {
    Step(candidate::Single),
    Save,
    Restore,
}

fn flatten(steps: &[candidate::Single], out: &mut Vec<Flat>) {
    for s in steps {
        match &s.kind {
            SingleKind::Composite {
                parts,
                save_horizontal,
            } => {
                if *save_horizontal {
                    out.push(Flat::Save);
                }
                flatten(parts, out);
                if *save_horizontal {
                    out.push(Flat::Restore);
                }
            }
            _ => out.push(Flat::Step(s.clone())),
        }
    }
}

/// Drops neighbouring registry operations that undo each other in PROJ's
/// pipeline: the same method and parameters, one inverse, with the same
/// ellipsoid on both outer ends. PROJ's pipeline optimiser removes such
/// pairs symbolically; running them would leave the error of the
/// small-angle Helmert inverse (millimetres).
fn cancel_inverse_pairs(steps: Vec<Flat>) -> Vec<Flat> {
    let r = registry();
    let ellipsoid = |code: u32| {
        r.geodetic
            .get(&code)
            .and_then(|g| r.datums.get(&g.datum))
            .map(|d| d.ellipsoid)
    };
    let mut out: Vec<Flat> = Vec::new();
    for flat in steps {
        let Flat::Step(step) = &flat else {
            out.push(flat);
            continue;
        };
        // The operation before, past changes between geographic CRSs on one
        // datum, which PROJ writes as no step.
        let geographic = |c: CrsRef| {
            matches!(
                registry().geodetic.get(&c.code()).map(|g| g.kind),
                Some(crate::registry::GeodeticKind::Geographic2d)
                    | Some(crate::registry::GeodeticKind::Geographic3d)
            )
        };
        let prev_index = out.iter().rposition(|f| match f {
            Flat::Step(p) => {
                !(p.kind == SingleKind::SameDatum && geographic(p.src) && geographic(p.dst))
            }
            _ => true,
        });
        if let (
            Some(Flat::Step(prev)),
            SingleKind::Registry {
                code: b,
                inverse: ib,
            },
        ) = (prev_index.map(|i| &out[i]), &step.kind)
            && let SingleKind::Registry {
                code: a,
                inverse: ia,
            } = prev.kind
            && ia != *ib
        {
            let (oa, ob) = (&r.ops[&a], &r.ops[b]);
            let crs_code = CrsRef::code;
            // A vertical grid changes only the height: run backwards then
            // forwards it cancels whatever the CRSs (PROJ drops the pair of
            // `vgridshift` steps).
            let vertical_grid = |o: &crate::registry::Operation| {
                operation::is_geoid_grid(o.method)
                    || operation::offset_grid_sign(o.method).is_some()
            };
            if same_math(oa, ob)
                && oa.table != crate::registry::Table::Conversion
                && ((ellipsoid(crs_code(prev.src)).is_some()
                    && ellipsoid(crs_code(prev.src)) == ellipsoid(crs_code(step.dst)))
                    || (vertical_grid(oa) && vertical_grid(ob)))
            {
                out.remove(prev_index.expect("found above"));
                continue;
            }
        }
        out.push(flat);
    }
    out
}

/// Two operations with the same math: the same method and parameters, or
/// grid steps reading the same PROJ-data file the same way (EPSG names one
/// grid differently in different operations).
fn same_math(a: &crate::registry::Operation, b: &crate::registry::Operation) -> bool {
    if a.method == b.method && a.params == b.params {
        return true;
    }
    let file = |o: &crate::registry::Operation| {
        let names = o.grid_names();
        let [name] = names.as_slice() else {
            return None;
        };
        crate::grid::lookup(name, &o.name).map(|e| (e.file, e.inverse))
    };
    a.method == b.method && file(a).is_some() && file(a) == file(b)
}

/// The runnable steps of a candidate.
fn executable(c: &Candidate, builder: &mut CrsBuilder) -> Result<Vec<Exec>, Error> {
    let mut steps = Vec::new();
    let mut flat = Vec::new();
    flatten(&c.steps, &mut flat);
    for f in cancel_inverse_pairs(flat) {
        let s = match f {
            Flat::Step(s) => s,
            Flat::Save => {
                steps.push(Exec::SaveHorizontal);
                continue;
            }
            Flat::Restore => {
                steps.push(Exec::RestoreHorizontal);
                continue;
            }
        };
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
                let height = if operation::shifts_ellipsoidal_height(op.method) {
                    Height::Ellipsoidal
                } else if method.keeps_height() {
                    Height::Kept
                } else {
                    Height::Changed
                };
                steps.push(Exec::Apply {
                    method,
                    from: Box::new(from),
                    to: Box::new(to),
                    inverse: *inverse,
                    height,
                });
            }
            SingleKind::Projection { .. }
            | SingleKind::SameDatum
            | SingleKind::EnsembleMember
            | SingleKind::VerticalConversion
            | SingleKind::BallparkVertical => {
                steps.push(Exec::Convert(Box::new(builder.get(s.dst)?)));
            }
            SingleKind::Composite { .. } => unreachable!("flattened"),
        }
    }
    Ok(steps)
}

/// Coordinates of one CRS to another on the same datum, or, after a pair of
/// operations cancelled, across datums on one ellipsoid (PROJ then keeps
/// longitude, latitude and height too).
fn convert(a: &Crs, b: &Crs, c: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
    crate::transform::on_one_datum(a, b, c)
}

/// Whether the next operation after step `i` (past conversions between
/// geographic CRSs, which add no PROJ step) is a geog2D-domain one, so the
/// `pop`/`push` pair between them cancels in PROJ's pipeline.
fn next_keeps_height(steps: &[Exec], i: usize, ellipsoidal: bool) -> bool {
    for step in &steps[i + 1..] {
        match step {
            Exec::Convert(to) => {
                if !matches!(to.kind, Kind::Geographic { .. }) {
                    return false;
                }
            }
            Exec::Apply {
                from,
                to,
                inverse,
                height,
                ..
            } => return height.keeps(if *inverse { to } else { from }, ellipsoidal),
            Exec::SaveHorizontal | Exec::RestoreHorizontal => return false,
        }
    }
    false
}

/// A vertical CRS: an operation on heights reads the horizontal position in
/// the CRS the coordinates are in (its interpolation CRS, by construction),
/// so a vertical CRS never becomes the current one.
fn is_vertical(crs: &Crs) -> bool {
    matches!(crs.kind, Kind::Vertical { .. })
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
    // Horizontal positions `push +v_1 +v_2` saved, with their CRS.
    let mut horizontal: Vec<([f64; 2], &Crs)> = Vec::new();
    // Whether the height is ellipsoidal: from a geographic 3D or geocentric
    // source, and after a vertical operation back from a vertical CRS (a
    // geoid model backwards); not after one into a vertical CRS.
    let mut ellipsoidal = src.dimension() == 3
        && matches!(src.kind, Kind::Geographic { .. } | Kind::Geocentric { .. });
    for (i, step) in steps.iter().enumerate() {
        match step {
            Exec::SaveHorizontal => horizontal.push(([c[0], c[1]], cur)),
            Exec::RestoreHorizontal => {
                if let Some((xy, crs)) = horizontal.pop() {
                    // The height went on through the CRSs since: carry it
                    // back to the saved CRS's frame first.
                    let h = convert(cur, crs, c)?[2];
                    c = [xy[0], xy[1], h];
                    cur = crs;
                }
            }
            Exec::Convert(to) => {
                if !is_vertical(to) {
                    c = convert(cur, to, c)?;
                    cur = to;
                }
            }
            Exec::Apply {
                method,
                from,
                to,
                inverse,
                height,
            } => {
                let (input, output) = if *inverse { (to, from) } else { (from, to) };
                let keeps_height = height.keeps(input, ellipsoidal);
                if !is_vertical(input) {
                    c = convert(cur, input, c)?;
                }
                if keeps_height {
                    saved.get_or_insert(c[2]);
                    c = if *inverse {
                        method.inverse_3d(c)?
                    } else {
                        method.forward_3d(c)?
                    };
                    if !next_keeps_height(steps, i, ellipsoidal)
                        && let Some(h) = saved.take()
                    {
                        c[2] = h;
                    }
                } else {
                    // Its height goes through (`forward_3d`: a geog2D-domain
                    // Helmert between 3D CRSs carries it).
                    c = if *inverse {
                        method.inverse_3d(c)?
                    } else {
                        method.forward_3d(c)?
                    };
                }
                if !c.iter().all(|v| v.is_finite()) {
                    return Err(CoordinateError::OutsideDomain);
                }
                if is_vertical(output) {
                    ellipsoidal = false;
                } else if is_vertical(input) {
                    ellipsoidal = true;
                }
                if !is_vertical(output) {
                    cur = output;
                }
            }
        }
    }
    convert(cur, dst, c)
}
