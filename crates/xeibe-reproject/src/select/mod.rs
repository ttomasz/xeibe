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
use crate::operation::{self, Method, PointMotion};
use crate::registry::{PROJ_CODES, registry};
use crate::transform::{from_geographic, to_geographic};

pub(crate) mod candidate;
pub(crate) mod crs;
pub(crate) mod explain;
pub(crate) mod filter;
pub(crate) mod search;
pub(crate) mod trace;

use candidate::{Candidate, SingleKind};
use crs::CrsCache;
use crs::CrsRef;
use explain::{Explain, Explained, Explanation, Outcome, PassReason, Recorder};
use trace::Event;

/// One step of a runnable pipeline.
#[derive(Debug, Clone)]
enum Exec {
    /// Move the coordinates into this CRS, on the same datum.
    Convert(Box<Crs>),
    /// Keep the horizontal position (PROJ's `push +v_1 +v_2`).
    SaveHorizontal,
    /// Put it back (`pop +v_1 +v_2`), with the CRS it was in.
    RestoreHorizontal,
    /// Apply an operation between two CRSs, either way: registry
    /// operation `code`.
    Apply {
        code: u32,
        method: Arc<dyn Method>,
        from: Box<Crs>,
        to: Box<Crs>,
        inverse: bool,
        height: Height,
    },
    /// Move the coordinates in time within `crs`, from one coordinate epoch
    /// to another, by registry point motion operation `code`. The
    /// operations after it run at the epoch it moved to.
    Move {
        code: u32,
        motion: Arc<PointMotion>,
        crs: Box<Crs>,
        from: f64,
        to: f64,
    },
}

impl Exec {
    /// The grid files the step reads.
    fn grid_files(&self) -> Vec<Arc<GridFile>> {
        match self {
            Exec::Apply { method, .. } => method.grid_files(),
            Exec::Move { motion, .. } => motion.grid_files(),
            _ => Vec::new(),
        }
    }
}

/// A change of coordinate epoch a transform asks for
/// (`Transform::with_epochs` with two different epochs), and the point
/// motion operation that makes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Motion {
    /// The registry's point motion operation.
    pub(crate) operation: u32,
    /// It is the source CRS's datum's: the coordinates are moved first, and
    /// the operations to the target run at the target's epoch. Else it is
    /// the target's, and they are moved last. PROJ looks in this order
    /// (`createOperationsWithDatumPivot`).
    pub(crate) at_source: bool,
    /// Decimal years.
    pub(crate) from: f64,
    pub(crate) to: f64,
}

impl Motion {
    /// The point motion operation for a change of epoch between two CRSs:
    /// the source's datum's, else the target's (EPSG datum codes).
    pub(crate) fn between(
        src_datum: Option<u32>,
        dst_datum: Option<u32>,
        from: f64,
        to: f64,
    ) -> Option<Motion> {
        let of = |datum: Option<u32>| datum.and_then(operation::point_motion_for);
        let (op, at_source) = match (of(src_datum), of(dst_datum)) {
            (Some(op), _) => (op, true),
            (None, Some(op)) => (op, false),
            (None, None) => return None,
        };
        Some(Motion {
            operation: op.code,
            at_source,
            from,
            to,
        })
    }

    /// The operation as a step, named as PROJ's `cloneWithEpochs` names it.
    fn single(&self) -> candidate::Single {
        let mut step = candidate::registry_single(self.operation, false);
        step.name = format!(
            "{} from epoch {} to epoch {}",
            step.name, self.from, self.to
        );
        step
    }

    /// The steps between `crs` and the CRS the operation is defined on, on
    /// one datum, as far as PROJ names them: a projection, a change between
    /// geodetic CRSs. What they leave out `run` converts anyway.
    fn bridge(crs: CrsRef, to: CrsRef, back: bool, cache: &mut CrsCache) -> Vec<candidate::Single> {
        let mut steps = Vec::new();
        let mut cur = crs;
        if cache.kind(cur) == crs::CrsKind::Projected
            && let CrsRef::Epsg(code) = cur
            && let Some(p) = candidate::projection(cache, code, true)
        {
            cur = p.dst;
            steps.push(p);
        }
        if cur != to
            && let Some(s) = candidate::same_datum(cache, cur, to)
        {
            steps.push(s);
        }
        if back {
            steps = steps.iter().rev().map(|s| s.inverse(cache)).collect();
        }
        steps
    }

    /// A step between CRSs on one datum, which a change of epoch can go
    /// before or after alike.
    fn converts(step: &candidate::Single) -> bool {
        matches!(
            step.kind,
            candidate::SingleKind::SameDatum | candidate::SingleKind::Projection { .. }
        )
    }

    /// Where `c`'s first step between datums is: after the conversions
    /// from the source CRS (PROJ's `opFirst`).
    fn first_transformation(c: &Candidate) -> usize {
        c.steps
            .iter()
            .position(|s| !Motion::converts(s))
            .unwrap_or(0)
    }

    /// One past `c`'s last step between datums: before the conversions to
    /// the target CRS (PROJ's `opThird`).
    fn last_transformation(c: &Candidate) -> usize {
        c.steps
            .iter()
            .rposition(|s| !Motion::converts(s))
            .map_or(c.steps.len(), |i| i + 1)
    }

    /// `c`, an operation from `src` to `dst` (`None` between CRSs on one
    /// datum), with the coordinates moved in time before or after it.
    fn wrap(
        &self,
        c: Option<&Candidate>,
        src: CrsRef,
        dst: CrsRef,
        cache: &mut CrsCache,
    ) -> Option<Candidate> {
        let step = self.single();
        let frame = step.src;
        let single = |s: candidate::Single| Candidate::single(s);
        let around = |anchor: CrsRef, cache: &mut CrsCache| {
            let mut parts: Vec<Candidate> = Motion::bridge(anchor, frame, false, cache)
                .into_iter()
                .map(single)
                .collect();
            parts.push(single(step.clone()));
            parts
        };
        let parts = match c {
            // On one datum: to the operation's CRS, through it, on to the
            // target.
            None => {
                let mut parts = around(src, cache);
                parts.extend(
                    Motion::bridge(dst, frame, true, cache)
                        .into_iter()
                        .map(single),
                );
                parts
            }
            // Before the operations, or after them.
            Some(c) if self.at_source && Motion::first_transformation(c) == 0 => {
                let mut parts = around(src, cache);
                parts.extend(
                    Motion::bridge(src, frame, true, cache)
                        .into_iter()
                        .map(single),
                );
                parts.push(c.clone());
                parts
            }
            Some(c) if !self.at_source && Motion::last_transformation(c) == c.steps.len() => {
                let mut parts = vec![c.clone()];
                parts.extend(around(dst, cache));
                parts.extend(
                    Motion::bridge(dst, frame, true, cache)
                        .into_iter()
                        .map(single),
                );
                parts
            }
            // Between the conversions at that end and the transformation,
            // in the CRS the transformation works on, as PROJ does
            // (`createOperationsWithDatumPivot`): from ITRF2014 (geog2D)
            // the Helmert's geocentric result is moved, not the target's
            // geographic 2D one.
            Some(c) => {
                let at = if self.at_source {
                    Motion::first_transformation(c)
                } else {
                    Motion::last_transformation(c)
                };
                let anchor = if self.at_source {
                    c.steps[at].src
                } else {
                    c.steps[at - 1].dst
                };
                let mut parts: Vec<Candidate> = c.steps[..at].iter().cloned().map(single).collect();
                parts.extend(around(anchor, cache));
                parts.extend(
                    Motion::bridge(anchor, frame, true, cache)
                        .into_iter()
                        .map(single),
                );
                parts.extend(c.steps[at..].iter().cloned().map(single));
                parts
            }
        };
        let mut out = candidate::concatenate(parts, false, cache)?;
        out.src = src;
        out.dst = dst;
        // The result depends on the two epochs.
        out.requires_time = true;
        Some(out)
    }
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
    /// runs inverted. Map projections, the changes on one datum and PROJ's
    /// null operations between a datum ensemble and its members are left
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
    /// A step is time-dependent: its result depends on the coordinates'
    /// epoch (`Transform::with_epochs`, `Transform::convert_at`), or on
    /// the transform's two epochs (a change of coordinate epoch). Without
    /// an epoch a time-dependent Helmert runs at its reference epoch; see
    /// `needs_epoch`.
    pub time_dependent: bool,
    /// A step works only on a coordinate with an epoch: a deformation
    /// model (New Zealand's), which says where a point was at a given
    /// time. A coordinate without one fails
    /// (`CoordinateError::MissingEpoch`), as in PROJ.
    pub needs_epoch: bool,
    /// Through a time-specific Helmert, which holds at this one epoch (a
    /// decimal year): only a coordinate at it, its own epoch or the
    /// transform's, goes this way; others, and those without an epoch, go
    /// through the other operations. Where there are none, they fail, and
    /// a coordinate at the epoch goes this way whatever the area of use, as
    /// through any only operation.
    pub only_at_epoch: Option<f64>,
    /// EPSG's scope of the operation, when it is one EPSG operation (see
    /// `code`). Its remarks: `xeibe_crs::operation_remarks`.
    pub scope: Option<String>,
    /// Every step it runs, in order, as PROJ lists them: what `steps`
    /// leaves out too. A step PROJ builds from others (a horizontal and a
    /// vertical operation) comes as its parts.
    pub chain: Vec<OperationStep>,
}

/// One step of an operation ([`OperationInfo::chain`]).
#[derive(Debug, Clone, PartialEq)]
pub struct OperationStep {
    /// PROJ's name for it.
    pub name: String,
    pub kind: StepKind,
    pub source: StepCrs,
    pub target: StepCrs,
    /// In metres; `None` when unknown.
    pub accuracy: Option<f64>,
}

/// What a step of an operation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// An EPSG operation, run inverted or not.
    Epsg { code: u32, inverse: bool },
    /// One of the null operations PROJ's database adds from a datum
    /// ensemble's CRS to a member's: EPSG's method `method` (geocentric
    /// translations or a vertical offset) with zero parameters.
    EnsembleIdentity { method: u32, inverse: bool },
    /// The map projection of the projected CRS `crs`, or its inverse.
    Projection { crs: u32, inverse: bool },
    /// Between geodetic CRSs on one datum: a null geographic offset (2D ↔
    /// 3D), a geographic ↔ geocentric conversion or a null geocentric
    /// translation, by the CRSs' kinds.
    SameDatum,
    /// Between a datum ensemble's CRS and a member's, where PROJ's database
    /// has no operation: coordinates pass unchanged.
    EnsembleMember,
    /// Between vertical CRSs on one datum: another unit, height to depth.
    VerticalConversion,
    /// A height passed unchanged between two vertical datums.
    BallparkVertical,
}

/// A CRS a step starts or ends in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepCrs {
    /// An EPSG CRS.
    Epsg(u32),
    /// The geographic 2D EPSG CRS promoted to 3D, where EPSG has no twin.
    Promoted(u32),
    /// The compound CRS of a horizontal and a vertical EPSG CRS.
    Compound(u32, u32),
}

impl From<CrsRef> for StepCrs {
    fn from(r: CrsRef) -> Self {
        match r {
            CrsRef::Epsg(c) => StepCrs::Epsg(c),
            CrsRef::Promoted(c) => StepCrs::Promoted(c),
            CrsRef::Compound(h, v) => StepCrs::Compound(h, v),
        }
    }
}

fn chain(steps: &[candidate::Single], out: &mut Vec<OperationStep>) {
    for s in steps {
        let kind = match &s.kind {
            SingleKind::Composite { parts, .. } => {
                chain(parts, out);
                continue;
            }
            &SingleKind::Registry { code, inverse } if code >= PROJ_CODES => {
                StepKind::EnsembleIdentity {
                    method: registry().ops[&code].method,
                    inverse,
                }
            }
            &SingleKind::Registry { code, inverse } => StepKind::Epsg { code, inverse },
            &SingleKind::Projection { crs, inverse } => StepKind::Projection { crs, inverse },
            SingleKind::SameDatum => StepKind::SameDatum,
            SingleKind::EnsembleMember => StepKind::EnsembleMember,
            SingleKind::VerticalConversion => StepKind::VerticalConversion,
            SingleKind::BallparkVertical => StepKind::BallparkVertical,
        };
        out.push(OperationStep {
            name: s.name.clone(),
            kind,
            source: s.src.into(),
            target: s.dst.into(),
            accuracy: (s.accuracy >= 0.0).then_some(s.accuracy),
        });
    }
}

impl OperationInfo {
    fn of(c: &Candidate, steps: &[Exec]) -> OperationInfo {
        fn transformations(steps: &[candidate::Single], out: &mut Vec<(u32, bool)>) {
            for s in steps {
                match &s.kind {
                    SingleKind::Registry { code, inverse } if *code < PROJ_CODES => {
                        out.push((*code, *inverse));
                    }
                    SingleKind::Composite { parts, .. } => transformations(parts, out),
                    _ => {}
                }
            }
        }
        let mut codes = Vec::new();
        transformations(&c.steps, &mut codes);
        let needs_epoch = codes.iter().any(|(code, _)| {
            registry()
                .ops
                .get(code)
                .is_some_and(|op| operation::needs_epoch(op.method))
        });
        let mut grids: Vec<String> = Vec::new();
        for step in steps {
            for f in step.grid_files() {
                if !grids.contains(&f.name) {
                    grids.push(f.name.clone());
                }
            }
        }
        OperationInfo {
            name: c.name.clone(),
            code: c.registry.filter(|&(code, _)| code < PROJ_CODES),
            steps: codes,
            accuracy: (c.accuracy >= 0.0).then_some(c.accuracy),
            area: c
                .extent
                .as_ref()
                .map(|e| [e.west, e.south, e.east, e.north]),
            area_name: c.extent.as_ref().and_then(|e| e.name.map(str::to_owned)),
            grids,
            time_dependent: c.requires_time || needs_epoch,
            needs_epoch,
            only_at_epoch: None,
            scope: c
                .registry
                .and_then(|(code, _)| registry().ops.get(&code))
                .and_then(|op| op.scope.map(str::to_owned)),
            chain: {
                let mut out = Vec::new();
                chain(&c.steps, &mut out);
                out
            },
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
    /// Used for every coordinate, whatever its area ([`Prepared::everywhere`]).
    everywhere: bool,
    /// Through a time-specific Helmert: only for a coordinate at this epoch.
    only_at: Option<f64>,
}

/// How near a coordinate's epoch must be to a time-specific Helmert's: the
/// same thousandth of a year.
const SAME_EPOCH_YEARS: f64 = 5e-4;

/// Options of the search for operations between datums, as PROJ's
/// operation factory context takes them from `proj_create_crs_to_crs`
/// (pyproj's `area_of_interest` and `accuracy`). The default is none.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    /// West, south, east, north in degrees (west > east across the
    /// antimeridian): it replaces the CRSs' areas of use as the area
    /// operations must intersect and are ranked by.
    pub area_of_interest: Option<[f64; 4]>,
    /// In metres: only operations whose accuracy is known and no worse.
    /// 0 sets no limit, as in PROJ.
    pub accuracy: Option<f64>,
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
    /// How many of `list` go through a time-specific Helmert
    /// ([`Prepared::only_at`]): counted once, so that a coordinate's choice
    /// doesn't look for them where there are none.
    at_epoch: usize,
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

/// The routes through EPSG's time-specific Helmerts, which PROJ's choice
/// never meets: for each one joining the two CRSs' datums, the change into
/// its source CRS on the one datum, the Helmert, the change out of its
/// target CRS on the other, with the one epoch it holds at.
fn time_specific_routes(
    src: CrsRef,
    dst: CrsRef,
    grids: &Grids,
    selection: &Selection,
    crs: &mut CrsCache,
) -> Vec<(Candidate, f64)> {
    let (Some(ds), Some(dt)) = (crs.datum(src), crs.datum(dst)) else {
        return Vec::new();
    };
    let touched = operation::time_specific_datums();
    if ds == dt || !touched.contains(&ds) || !touched.contains(&dt) {
        return Vec::new();
    }
    let r = registry();
    let mut codes: Vec<u32> = r
        .ops
        .values()
        .filter(|op| !op.deprecated && operation::time_specific_epoch(op).is_some())
        .map(|op| op.code)
        .collect();
    codes.sort_unstable();
    let mut out = Vec::new();
    for code in codes {
        let op = &r.ops[&code];
        let (Some(a), Some(b)) = (op.src, op.dst) else {
            continue;
        };
        let (a, b) = (CrsRef::Epsg(a), CrsRef::Epsg(b));
        let inverse = match (crs.datum(a), crs.datum(b)) {
            (Some(x), Some(y)) if x == ds && y == dt => false,
            (Some(x), Some(y)) if x == dt && y == ds => true,
            _ => continue,
        };
        let (from, to) = if inverse { (b, a) } else { (a, b) };
        let Some(helmert) = candidate::registry_candidate(code, inverse, crs) else {
            continue;
        };
        // On one datum, each end is a conversion (or nothing).
        let mut parts = Vec::new();
        for (x, y) in [(src, from), (to, dst)] {
            if x != y {
                let (found, _) = search::create_operations(x, y, grids.clone(), selection);
                let Some(first) = found.into_iter().next() else {
                    parts.clear();
                    break;
                };
                parts.push(first);
            }
        }
        let route = match parts.len() {
            0 if src == from && dst == to => vec![helmert],
            0 => continue,
            1 if src == from => vec![helmert, parts.remove(0)],
            1 => vec![parts.remove(0), helmert],
            _ => vec![parts.remove(0), helmert, parts.remove(0)],
        };
        if let Some(c) = candidate::concatenate(route, true, crs)
            && filter::is_forcible(&c)
            && let Some(epoch) = operation::time_specific_epoch(op)
        {
            out.push((c, epoch));
        }
    }
    out
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
        selection: &Selection,
    ) -> Result<Candidates, Error> {
        let (sorted, mut crs) =
            search::create_operations(src_ref, dst_ref, grids.clone(), selection);
        let sorted = sorted.into_iter().filter(filter::is_exportable).collect();
        let at_epoch = time_specific_routes(src_ref, dst_ref, grids, selection, &mut crs);
        Candidates::prepare_with(sorted, at_epoch, src, dst, None)
    }

    /// `sorted`, best first, made ready to run: each one's area converted
    /// into the source CRS. Those that can't be set up are left out.
    fn prepare(
        sorted: Vec<Candidate>,
        src: &Crs,
        dst: &Crs,
        motion: Option<&Motion>,
    ) -> Result<Candidates, Error> {
        Candidates::prepare_with(sorted, Vec::new(), src, dst, motion)
    }

    /// [`Candidates::prepare`], with the routes through time-specific
    /// Helmerts after the others, each with its epoch.
    fn prepare_with(
        sorted: Vec<Candidate>,
        at_epoch: Vec<(Candidate, f64)>,
        src: &Crs,
        dst: &Crs,
        motion: Option<&Motion>,
    ) -> Result<Candidates, Error> {
        let mut builder = CrsBuilder::default();
        let src_geographic = matches!(src.kind, Kind::Geographic { .. });
        let src_geocentric = matches!(src.kind, Kind::Geocentric { .. });
        let mut list = Vec::new();
        let mut operations = 0;
        let mut first: Option<(Arc<Candidate>, Vec<Exec>, Arc<OperationInfo>)> = None;
        let mut ordinary_operations = 0;
        let all = sorted
            .into_iter()
            .map(|c| (c, None))
            .chain(at_epoch.into_iter().map(|(c, e)| (c, Some(e))));
        for (c, only_at) in all {
            let Ok(steps) = executable(&c, src, dst, &mut builder, motion) else {
                continue;
            };
            let index = operations;
            operations += 1;
            if only_at.is_none() {
                ordinary_operations += 1;
            }
            let mut info = OperationInfo::of(&c, &steps);
            info.only_at_epoch = only_at;
            let info = Arc::new(info);
            let c = Arc::new(c);
            if first.is_none() && only_at.is_none() {
                first = Some((c.clone(), steps.clone(), info.clone()));
            }
            let extent = c.extent.clone().unwrap_or_else(crate::extent::world);
            let area_name = extent.name.unwrap_or_default();
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
                    everywhere: false,
                    only_at,
                });
            }
        }
        // A single operation whose area doesn't convert into the source CRS
        // still serves, as PROJ returns it directly.
        let ordinary = list.iter().filter(|p| p.only_at.is_none()).count();
        if ordinary == 0
            && ordinary_operations == 1
            && let Some((c, steps, info)) = first
        {
            list.insert(0, Prepared::everywhere(c, steps, info));
        }
        if list.is_empty() {
            return Err(Error::NoTransformation {
                src: src.label(),
                dst: dst.label(),
            });
        }
        // PROJ's `proj_create_crs_to_crs`: a single operation, or a single
        // prepared one, is returned as the transformation itself. Of the
        // others, those through a time-specific Helmert don't count.
        let ordinary = list.iter().filter(|p| p.only_at.is_none()).count();
        let single = ordinary > 0 && (ordinary_operations == 1 || ordinary == 1);
        Ok(Candidates {
            grid_files: grid_files(&list),
            at_epoch: list.len() - ordinary,
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
        grid_files(
            self.list
                .iter()
                .filter(|p| meets(&p.area) || in_box(&p.bbox)),
        )
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
        if !filter::is_forcible(&c) {
            return Err(Error::Unsupported {
                crs: format!("EPSG operation {code}"),
                what: "its method".into(),
            });
        }
        Candidates::one(c, src, dst, None)
    }

    /// These candidates with the coordinates moved from one coordinate
    /// epoch to another by a point motion operation (`motion`), before the
    /// operations or after them. The choice among them stays as it is: by
    /// their own areas and accuracies.
    pub(crate) fn moved(
        &self,
        src: &Crs,
        dst: &Crs,
        src_ref: CrsRef,
        dst_ref: CrsRef,
        motion: &Motion,
    ) -> Result<Candidates, Error> {
        let mut cache = CrsCache::default();
        let mut wrapped = Vec::new();
        let mut last = None;
        for p in &self.list {
            // Between two coordinate epochs, no time-specific Helmert.
            if last == Some(p.index) || p.only_at.is_some() {
                continue;
            }
            last = Some(p.index);
            let c = motion
                .wrap(Some(&p.candidate), src_ref, dst_ref, &mut cache)
                .ok_or_else(|| Error::NoTransformation {
                    src: src.label(),
                    dst: dst.label(),
                })?;
            wrapped.push(c);
        }
        match wrapped.as_slice() {
            // One operation for every coordinate stays that.
            [c] if self.list[0].everywhere => Candidates::one(c.clone(), src, dst, Some(motion)),
            _ => Candidates::prepare(wrapped, src, dst, Some(motion)),
        }
    }

    /// Between CRSs on one datum: the point motion operation alone, for
    /// every coordinate.
    pub(crate) fn motion_only(
        src: &Crs,
        dst: &Crs,
        src_ref: CrsRef,
        dst_ref: CrsRef,
        motion: &Motion,
    ) -> Result<Candidates, Error> {
        let c = motion
            .wrap(None, src_ref, dst_ref, &mut CrsCache::default())
            .ok_or_else(|| Error::NoTransformation {
                src: src.label(),
                dst: dst.label(),
            })?;
        Candidates::one(c, src, dst, Some(motion))
    }

    /// One candidate, run for every coordinate whatever its area.
    fn one(
        c: Candidate,
        src: &Crs,
        dst: &Crs,
        motion: Option<&Motion>,
    ) -> Result<Candidates, Error> {
        let steps = executable(&c, src, dst, &mut CrsBuilder::default(), motion)?;
        let info = Arc::new(OperationInfo::of(&c, &steps));
        let list = vec![Prepared::everywhere(Arc::new(c), steps, info)];
        Ok(Candidates {
            grid_files: grid_files(&list),
            list,
            src_geographic: matches!(src.kind, Kind::Geographic { .. }),
            src_geocentric: matches!(src.kind, Kind::Geocentric { .. }),
            single: true,
            at_epoch: 0,
        })
    }

    /// Candidate `index` only (see [`Candidates::info`]), for every
    /// coordinate: one member of PROJ's `TransformerGroup`.
    pub(crate) fn only(
        &self,
        index: usize,
        src: &Crs,
        dst: &Crs,
    ) -> Option<Result<Candidates, Error>> {
        let p = self.list.iter().find(|p| p.index == index)?;
        Some(Candidates::one((*p.candidate).clone(), src, dst, None))
    }

    /// The first candidate the other way, for every coordinate: the
    /// inverse of [`Candidates::only`]. `src` and `dst` are the inverse's.
    pub(crate) fn inverse_of_first(&self, src: &Crs, dst: &Crs) -> Result<Candidates, Error> {
        let inverse = self.list[0].candidate.inverse(&mut CrsCache::default());
        Candidates::one(inverse, src, dst, None)
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

    /// Whether a prepared candidate's area, converted into the source
    /// CRS, contains a point of [`Candidates::choice_point`].
    fn contains(&self, alt: &Prepared, [x, y]: [f64; 2]) -> bool {
        let [minx, miny, maxx, maxy] = alt.bbox;
        let mut inside = x >= minx && y >= miny && x <= maxx && y <= maxy;
        if !inside && self.src_geographic && y >= miny && y <= maxy {
            let lon = normalize_longitude(x);
            inside = lon >= minx && lon <= maxx;
        }
        inside
    }

    /// `pj_get_suggested_operation` for a coordinate in the source CRS's
    /// internal form, telling `rec` why each candidate whose area contains
    /// it isn't the one. The routes through a time-specific Helmert aren't
    /// among them: [`Candidates::choose_at_epoch`] chooses those.
    fn suggest<R: Recorder>(
        &self,
        point: [f64; 2],
        excluded: &[usize],
        rec: &mut R,
    ) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_accuracy = f64::MAX;
        for (i, alt) in self.list.iter().enumerate() {
            if alt.only_at.is_some() || excluded.contains(&i) || !self.contains(alt, point) {
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
            if R::DETAILED
                && let Some(b) = best
            {
                let cur = &self.list[b];
                if replace {
                    // The one displaced is passed over for this one.
                    let reason = if alt.accuracy < best_accuracy {
                        PassReason::LessAccurate
                    } else {
                        PassReason::LargerArea
                    };
                    rec.passed(b, i, reason);
                } else {
                    rec.passed(i, b, pass_reason(alt, cur, best_accuracy));
                }
            }
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
    /// candidate used. `epoch` is the coordinate's, a decimal year, for
    /// the time-dependent operations; the choice looks at it only for the
    /// routes through a time-specific Helmert
    /// ([`Candidates::choose_at_epoch`]).
    pub(crate) fn run_with(
        &self,
        src: &Crs,
        dst: &Crs,
        internal: [f64; 3],
        epoch: Option<f64>,
    ) -> Result<([f64; 3], usize), CoordinateError> {
        self.choose(src, dst, internal, epoch, &mut ())
    }

    /// [`Candidates::run_with`], telling `rec` how the choice is made and,
    /// for a tracing recorder, the steps run.
    pub(crate) fn choose<R: Recorder>(
        &self,
        src: &Crs,
        dst: &Crs,
        internal: [f64; 3],
        epoch: Option<f64>,
        rec: &mut R,
    ) -> Result<([f64; 3], usize), CoordinateError> {
        if self.at_epoch > 0
            && let Some(found) = self.choose_at_epoch(src, dst, internal, epoch, rec)?
        {
            return Ok(found);
        }
        if self.at_epoch == self.list.len() {
            // Only routes through a time-specific Helmert: a coordinate
            // must be at the epoch of one.
            let Some(epoch) = epoch else {
                return Err(CoordinateError::MissingEpoch);
            };
            let Some(first) = self.list.iter().position(|p| p.holds_at(epoch)) else {
                return Err(CoordinateError::EpochOutOfRange);
            };
            // None that holds at the epoch converted the coordinate in its
            // area. The first does, whatever its area: the only operation
            // there is, as PROJ uses a single operation for every
            // coordinate; or, of several, as PROJ falls back.
            rec.fallback(first);
            let result = run(
                &self.list[first].steps,
                src,
                dst,
                internal,
                Some(epoch),
                rec,
            );
            rec.tried(first, result.err());
            return result.map(|out| (out, first));
        }
        if self.single {
            if R::DETAILED
                && let Ok(point) = self.choice_point(src, internal)
            {
                rec.inside(0, self.contains(&self.list[0], point));
            }
            let result = run(&self.list[0].steps, src, dst, internal, epoch, rec);
            rec.tried(0, result.err());
            return result.map(|out| (out, 0));
        }
        let point = self.choice_point(src, internal)?;
        if R::DETAILED {
            for (i, alt) in self.list.iter().enumerate() {
                rec.inside(i, self.contains(alt, point));
            }
        }
        // The routes through a time-specific Helmert had their turn:
        // `suggest` leaves them out.
        let mut excluded = Vec::new();
        let mut last_error = CoordinateError::OutsideDomain;
        for _ in 0..3 {
            rec.round();
            let Some(best) = self.suggest(point, &excluded, rec) else {
                break;
            };
            let result = run(&self.list[best].steps, src, dst, internal, epoch, rec);
            rec.tried(best, result.err());
            match result {
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
        if let Some(i) = self
            .list
            .iter()
            .position(|p| !p.has_grids && p.only_at.is_none())
        {
            rec.fallback(i);
            let result = run(&self.list[i].steps, src, dst, internal, epoch, rec);
            rec.tried(i, result.err());
            return result.map(|out| (out, i));
        }
        Err(last_error)
    }

    /// For a coordinate at a time-specific Helmert's epoch, inside the area
    /// of a route through it, the most accurate such route, if no other
    /// operation there is more accurate; `None` to go on with the others
    /// (or, where there are only such routes, with the first that holds at
    /// the epoch, whatever its area).
    fn choose_at_epoch<R: Recorder>(
        &self,
        src: &Crs,
        dst: &Crs,
        internal: [f64; 3],
        epoch: Option<f64>,
        rec: &mut R,
    ) -> Result<Option<([f64; 3], usize)>, CoordinateError> {
        let holds = |p: &Prepared| epoch.is_some_and(|epoch| p.holds_at(epoch));
        if R::DETAILED {
            for (i, p) in self.list.iter().enumerate() {
                if p.only_at.is_some() && !holds(p) {
                    rec.not_at_epoch(i);
                }
            }
            // Every candidate's area: the choice may end here, before the
            // others' are looked at.
            if let Ok(point) = self.choice_point(src, internal) {
                for (i, p) in self.list.iter().enumerate() {
                    rec.inside(i, self.contains(p, point));
                }
            }
        }
        let Some(epoch) = epoch else {
            return Ok(None);
        };
        let point = self.choice_point(src, internal)?;
        let at = |p: &Prepared| holds(p) && self.contains(p, point);
        let known = |a: f64| if a >= 0.0 { a } else { f64::MAX };
        let Some(best) = (0..self.list.len())
            .filter(|&i| at(&self.list[i]))
            .min_by(|&a, &b| known(self.list[a].accuracy).total_cmp(&known(self.list[b].accuracy)))
        else {
            return Ok(None);
        };
        // Why `other` wins over `passed`, by accuracy alone.
        let less_accurate = |passed: &Prepared, other: &Prepared| {
            if known(passed.accuracy) <= known(other.accuracy) {
                None
            } else if passed.accuracy < 0.0 {
                Some(PassReason::UnknownAccuracy)
            } else {
                Some(PassReason::LessAccurate)
            }
        };
        if R::DETAILED {
            // The other routes that hold at the epoch here.
            for i in (0..self.list.len()).filter(|&i| i != best && at(&self.list[i])) {
                let reason = less_accurate(&self.list[i], &self.list[best]);
                rec.passed(i, best, reason.unwrap_or(PassReason::Earlier));
            }
        }
        let other = self.suggest(point, &[], rec);
        if let Some(o) = other
            && known(self.list[o].accuracy) < known(self.list[best].accuracy)
        {
            if let Some(reason) = less_accurate(&self.list[best], &self.list[o]) {
                rec.passed(best, o, reason);
            }
            return Ok(None);
        }
        let result = run(&self.list[best].steps, src, dst, internal, Some(epoch), rec);
        rec.tried(best, result.err());
        if result.is_ok()
            && let Some(o) = other
        {
            let reason = less_accurate(&self.list[o], &self.list[best]);
            rec.passed(o, best, reason.unwrap_or(PassReason::AtEpoch));
        }
        Ok(result.ok().map(|out| (out, best)))
    }

    /// One operation, and it goes through a time-specific Helmert.
    fn only_route(&self) -> bool {
        self.at_epoch == self.list.len()
            && self.list.first().map(|p| p.index) == self.list.last().map(|p| p.index)
    }

    /// How the operation for one coordinate is chosen, by operation of
    /// [`Candidates::info`].
    pub(crate) fn explain(
        &self,
        src: &Crs,
        dst: &Crs,
        internal: [f64; 3],
        epoch: Option<f64>,
    ) -> Explanation {
        let mut rec = Explain::new(self.list.len());
        let result = self.choose(src, dst, internal, epoch, &mut rec);
        let used = result.as_ref().ok().map(|&(_, i)| i);
        let operations = self.list.last().map_or(0, |p| p.index + 1);
        let mut candidates: Vec<Option<Explained>> = vec![None; operations];
        for (i, p) in self.list.iter().enumerate() {
            let outcome = if used == Some(i) {
                // A route through a time-specific Helmert is chosen for
                // the coordinate's epoch, beside the only other operation;
                // alone, it is the only one for a coordinate at its epoch.
                if (self.single && p.only_at.is_none()) || self.only_route() {
                    Outcome::Only
                } else if rec.fallback == Some(i) {
                    Outcome::Fallback
                } else {
                    Outcome::Chosen
                }
            } else if let Some(Some(error)) = rec.tried[i] {
                Outcome::Failed(error)
            } else if rec.not_at_epoch[i] {
                Outcome::NotAtEpoch
            } else if rec.inside[i] == Some(false) {
                Outcome::Outside
            } else if let Some((by, reason)) = rec.passed[i] {
                // Passed over for one that failed in the last of the three
                // rounds: the choice stopped before this one.
                if rec.rounds == 3 && matches!(rec.tried[by], Some(Some(_))) {
                    Outcome::NotReached
                } else {
                    Outcome::Passed {
                        by: self.list[by].index,
                        reason,
                    }
                }
            } else {
                Outcome::NotReached
            };
            // An operation used whatever its area has no area to test.
            let explained = Explained {
                inside: if p.everywhere { None } else { rec.inside[i] },
                outcome,
            };
            let slot = &mut candidates[p.index];
            *slot = Some(match slot.take() {
                // An operation across the antimeridian is prepared twice:
                // the part that says more speaks for it.
                Some(other) => {
                    let inside = match (other.inside, explained.inside) {
                        (Some(a), Some(b)) => Some(a || b),
                        (a, b) => a.or(b),
                    };
                    let outcome = if explained.outcome.weight() > other.outcome.weight() {
                        explained.outcome
                    } else {
                        other.outcome
                    };
                    Explained { inside, outcome }
                }
                None => explained,
            });
        }
        Explanation {
            used: used.map(|i| self.list[i].index),
            error: result.err(),
            candidates: candidates
                .into_iter()
                .map(|c| {
                    c.unwrap_or(Explained {
                        inside: None,
                        outcome: Outcome::NotReached,
                    })
                })
                .collect(),
        }
    }

    pub(crate) fn run(
        &self,
        src: &Crs,
        dst: &Crs,
        internal: [f64; 3],
        epoch: Option<f64>,
    ) -> Result<[f64; 3], CoordinateError> {
        self.run_with(src, dst, internal, epoch).map(|(out, _)| out)
    }

    /// Name of the candidate a coordinate goes through.
    pub(crate) fn used(
        &self,
        src: &Crs,
        dst: &Crs,
        internal: [f64; 3],
        epoch: Option<f64>,
    ) -> Option<&str> {
        self.run_with(src, dst, internal, epoch)
            .ok()
            .map(|(_, i)| self.list[i].name.as_str())
    }

    /// Index (see [`Candidates::info`]) of the candidate a coordinate goes
    /// through.
    pub(crate) fn used_index(
        &self,
        src: &Crs,
        dst: &Crs,
        internal: [f64; 3],
        epoch: Option<f64>,
    ) -> Option<usize> {
        self.run_with(src, dst, internal, epoch)
            .ok()
            .map(|(_, i)| self.list[i].index)
    }
}

impl Prepared {
    /// A route through a time-specific Helmert that holds at `epoch`.
    fn holds_at(&self, epoch: f64) -> bool {
        self.only_at
            .is_some_and(|t| (epoch - t).abs() <= SAME_EPOCH_YEARS)
    }

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
            everywhere: true,
            only_at: None,
        }
    }
}

/// Why `alt`, whose area contains the point, doesn't displace `cur`, the
/// best so far, of accuracy `best_accuracy`: the rule of
/// [`Candidates::suggest`] read backwards.
fn pass_reason(alt: &Prepared, cur: &Prepared, best_accuracy: f64) -> PassReason {
    if alt.accuracy == best_accuracy {
        if cur.priority {
            PassReason::Priority
        } else if alt.unknown_area_name && !cur.unknown_area_name {
            PassReason::UnknownAreaName
        } else if alt.pseudo_area >= cur.pseudo_area {
            PassReason::LargerArea
        } else if alt.offshore {
            PassReason::Offshore
        } else {
            PassReason::Earlier
        }
    } else if alt.accuracy >= 0.0 && alt.accuracy < best_accuracy {
        // More accurate, and still not chosen: its area is offshore.
        PassReason::Offshore
    } else if alt.accuracy < 0.0 {
        PassReason::UnknownAccuracy
    } else if best_accuracy < 0.0 {
        // The best so far has unknown accuracy, which PROJ's comparison
        // doesn't displace.
        PassReason::Earlier
    } else {
        PassReason::LessAccurate
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

fn grid_files<'a>(list: impl IntoIterator<Item = &'a Prepared>) -> Vec<Arc<GridFile>> {
    let mut out: Vec<Arc<GridFile>> = Vec::new();
    for p in list {
        for step in &p.steps {
            for f in step.grid_files() {
                if !out.iter().any(|g| Arc::ptr_eq(g, &f)) {
                    out.push(f);
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

/// The runnable steps of a candidate from `src` to `dst`. `motion` gives
/// the epochs of the point motion operation among them, if there is one.
fn executable(
    c: &Candidate,
    src: &Crs,
    dst: &Crs,
    builder: &mut CrsBuilder,
    motion: Option<&Motion>,
) -> Result<Vec<Exec>, Error> {
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
                if operation::is_point_motion(op) {
                    let unsupported = |what: String| Error::Unsupported {
                        crs: format!("EPSG operation {code}"),
                        what,
                    };
                    let epochs = motion
                        .filter(|m| m.operation == *code)
                        .ok_or_else(|| unsupported("a point motion without epochs".into()))?;
                    let point_motion =
                        PointMotion::new(op, &from).map_err(|e| unsupported(format!("{e:?}")))?;
                    steps.push(Exec::Move {
                        code: *code,
                        motion: Arc::new(point_motion),
                        crs: Box::new(from),
                        from: epochs.from,
                        to: epochs.to,
                    });
                    continue;
                }
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
                    code: *code,
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
    // A last conversion into the target CRS, as the registry has it, is the
    // one `run` ends with anyway: kept, every coordinate would be projected
    // into it, back to geographic and into it again.
    if let Some(Exec::Convert(to)) = steps.last()
        && to.equivalent(dst, false)
    {
        steps.pop();
    }
    drop_identities(&mut steps, src);
    Ok(steps)
}

/// Leaves out the steps that change nothing from a geographic CRS on:
/// conversions between geographic CRSs, and operations that are
/// identities between two (a Helmert of zeros between near-equal
/// ellipsoids). Internally both CRSs hold the same longitude, latitude and
/// height, and the next conversion out of a geographic CRS checks them
/// as the one left out did.
fn drop_identities(steps: &mut Vec<Exec>, src: &Crs) {
    let geographic = |c: &Crs| matches!(c.kind, Kind::Geographic { .. });
    // Whether the coordinates are in a geographic CRS before each step, as
    // `run` moves them; `run`'s horizontal stack, for a restore.
    let mut on_geographic = geographic(src);
    let mut saved = Vec::new();
    steps.retain(|step| {
        let keep = match step {
            Exec::Convert(to) => !(on_geographic && geographic(to)),
            Exec::Apply {
                method, from, to, ..
            } => !(on_geographic && method.is_identity() && geographic(from) && geographic(to)),
            Exec::Move { .. } | Exec::SaveHorizontal | Exec::RestoreHorizontal => true,
        };
        match step {
            Exec::Convert(to) if !is_vertical(to) => on_geographic = geographic(to),
            Exec::Apply {
                from, to, inverse, ..
            } => {
                let output = if *inverse { from } else { to };
                if !is_vertical(output) {
                    on_geographic = geographic(output);
                }
            }
            Exec::Move { crs, .. } => on_geographic = geographic(crs),
            Exec::SaveHorizontal => saved.push(on_geographic),
            Exec::RestoreHorizontal => {
                if let Some(g) = saved.pop() {
                    on_geographic = g;
                }
            }
            Exec::Convert(_) => {}
        }
        keep
    });
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
            Exec::Move { .. } | Exec::SaveHorizontal | Exec::RestoreHorizontal => return false,
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

/// Run a candidate's steps. `epoch` is the coordinate's, a decimal year
/// (PROJ's `t`): a time-dependent operation applies its rates up to it, and
/// stays at its reference epoch without one. A point motion step changes
/// it for the steps after (PROJ's `set +v_4`).
fn run<R: Recorder>(
    steps: &[Exec],
    src: &Crs,
    dst: &Crs,
    internal: [f64; 3],
    epoch: Option<f64>,
    rec: &mut R,
) -> Result<[f64; 3], CoordinateError> {
    if R::TRACE {
        rec.start_run();
    }
    // All of them left out (`drop_identities`): what the loop would end
    // with, without setting it up for every coordinate.
    if steps.is_empty() {
        let c = convert(src, dst, internal)?;
        if R::TRACE {
            rec.step(Event::Convert, dst, c, None);
        }
        return Ok(c);
    }
    let mut cur = src;
    let mut c = internal;
    let mut epoch = epoch;
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
            Exec::SaveHorizontal => {
                horizontal.push(([c[0], c[1]], cur));
                if R::TRACE {
                    rec.step(Event::Save, cur, c, Some(ellipsoidal));
                }
            }
            Exec::RestoreHorizontal => {
                if let Some((xy, crs)) = horizontal.pop() {
                    // The height went on through the CRSs since: carry it
                    // back to the saved CRS's frame first.
                    let h = convert(cur, crs, c)?[2];
                    c = [xy[0], xy[1], h];
                    cur = crs;
                    if R::TRACE {
                        rec.step(Event::Restore, cur, c, Some(ellipsoidal));
                    }
                }
            }
            Exec::Convert(to) => {
                if !is_vertical(to) {
                    c = convert(cur, to, c)?;
                    cur = to;
                    if R::TRACE {
                        rec.step(Event::Convert, cur, c, Some(ellipsoidal));
                    }
                }
            }
            Exec::Move {
                code,
                motion,
                crs,
                from,
                to,
            } => {
                c = convert(cur, crs, c)?;
                if R::TRACE {
                    rec.step(Event::Convert, crs, c, Some(ellipsoidal));
                }
                // Whatever the height is, it rises and falls with the
                // ground (see `docs/reprojection.md`, coordinate epochs).
                c = motion.moved(c, to - from)?;
                if !c.iter().all(|v| v.is_finite()) {
                    return Err(CoordinateError::OutsideDomain);
                }
                cur = crs;
                epoch = Some(*to);
                if R::TRACE {
                    let event = Event::Apply {
                        code: *code,
                        inverse: false,
                        keeps_height: false,
                    };
                    rec.step(event, cur, c, Some(ellipsoidal));
                }
            }
            Exec::Apply {
                code,
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
                    if R::TRACE {
                        rec.step(Event::Convert, input, c, Some(ellipsoidal));
                    }
                }
                if keeps_height {
                    saved.get_or_insert(c[2]);
                }
                // The height goes through (`forward_3d`: a geog2D-domain
                // Helmert between 3D CRSs carries it); the one saved is
                // put back below.
                c = match (epoch, *inverse) {
                    (None, false) => method.forward_3d(c)?,
                    (None, true) => method.inverse_3d(c)?,
                    (Some(t), false) => method.forward_3d_at(c, t)?,
                    (Some(t), true) => method.inverse_3d_at(c, t)?,
                };
                if keeps_height
                    && !next_keeps_height(steps, i, ellipsoidal)
                    && let Some(h) = saved.take()
                {
                    c[2] = h;
                }
                if !c.iter().all(|v| v.is_finite()) {
                    return Err(CoordinateError::OutsideDomain);
                }
                if is_vertical(output) {
                    ellipsoidal = false;
                } else if is_vertical(input) {
                    ellipsoidal = true;
                }
                if R::TRACE {
                    // Into a vertical CRS, the horizontal position stays
                    // in the operation's interpolation CRS.
                    let frame = if !is_vertical(output) {
                        output
                    } else if !is_vertical(input) {
                        input
                    } else {
                        cur
                    };
                    let event = Event::Apply {
                        code: *code,
                        inverse: *inverse,
                        keeps_height,
                    };
                    rec.step(event, frame, c, Some(ellipsoidal));
                }
                if !is_vertical(output) {
                    cur = output;
                }
            }
        }
    }
    let c = convert(cur, dst, c)?;
    if R::TRACE {
        rec.step(Event::Convert, dst, c, None);
    }
    Ok(c)
}
