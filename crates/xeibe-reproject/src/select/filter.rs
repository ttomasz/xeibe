//! PROJ's `FilterResults` and `SortFunction`: which candidates survive and
//! in which order.
//!
//! Ported from PROJ 9.8.1 `src/iso19111/operation/coordinateoperationfactory.cpp`
//! (MIT, see PROJ-NOTICE.md), for the context `proj_create_crs_to_crs` uses
//! with ballpark operations disallowed: areas of interest from the smaller of
//! the two CRSs' areas, partial intersection, grids discarded when missing.
//! The source warns that "the order of the comparisons is extremely
//! important"; it is kept, up to the last tie-breaks. Where PROJ compares
//! names, xeibe uses EPSG's data (`docs/reprojection.md`, level 2,
//! deviations): no emulation where EPSG has the operation it emulates,
//! fewer operations run backwards, a longitude rotation that agrees with the
//! prime meridians, then the newer EPSG variant.

use std::cmp::Ordering;

use super::candidate::{
    BALLPARK_GEOCENTRIC_TRANSLATION, BALLPARK_GEOGRAPHIC_OFFSET, Candidate,
    NULL_GEOCENTRIC_TRANSLATION, NULL_GEOGRAPHIC_OFFSET, SingleKind,
};
use super::crs::{CrsCache, CrsKind};
use crate::extent::Extent;
use crate::operation::param::LONGITUDE_OFFSET;
use crate::registry::{ParamValue, registry};

/// The areas the filter works with: the two CRSs' areas of use and the area
/// of interest derived from them.
#[derive(Debug, Clone)]
pub(crate) struct Areas {
    pub(crate) extent1: Option<Extent>,
    pub(crate) extent2: Option<Extent>,
    pub(crate) interest: Option<Extent>,
}

impl Areas {
    /// `SourceTargetCRSExtentUse::SMALLEST`, PROJ's default.
    pub(crate) fn new(extent1: Option<Extent>, extent2: Option<Extent>) -> Areas {
        let interest = match (&extent1, &extent2) {
            (Some(a), Some(b)) => Some(if a.pseudo_area() < b.pseudo_area() {
                a.clone()
            } else {
                b.clone()
            }),
            (Some(a), None) => Some(a.clone()),
            (None, b) => b.clone(),
        };
        Areas {
            extent1,
            extent2,
            interest,
        }
    }
}

pub(crate) fn is_null_transformation(name: &str) -> bool {
    if name.contains(" + ") {
        return false;
    }
    [
        BALLPARK_GEOCENTRIC_TRANSLATION,
        BALLPARK_GEOGRAPHIC_OFFSET,
        NULL_GEOGRAPHIC_OFFSET,
        NULL_GEOCENTRIC_TRANSLATION,
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

/// Whether xeibe can run every step: PROJ's "exportable to a PROJ string".
pub(crate) fn is_exportable(c: &Candidate) -> bool {
    c.leaves().iter().all(|s| match s.kind {
        SingleKind::Registry { code, .. } => {
            let op = &registry().ops[&code];
            op.table == crate::registry::Table::Conversion || crate::operation::is_exportable(op)
        }
        _ => true,
    })
}

/// `FilterResults::filterOut` with no desired accuracy and ballparks
/// disallowed. `strict` is PROJ's `forceStrictContainmentTest`.
pub(crate) fn filter_out(list: &[Candidate], areas: &Areas, strict: bool) -> Vec<Candidate> {
    let mut res = Vec::new();
    let mut has_non_ballpark_op_with_extent = false;
    let mut has_non_ballpark_without_extent = false;
    for op in list {
        if op.has_ballpark() {
            continue;
        }
        if let Some(interest) = &areas.interest {
            let Some(extent) = &op.extent else {
                has_non_ballpark_without_extent = true;
                continue;
            };
            has_non_ballpark_op_with_extent = true;
            if strict && !extent.contains(interest) {
                continue;
            }
            if !strict && !extent.intersects(interest) {
                continue;
            }
        }
        res.push(op.clone());
    }
    // Ballparks never get in, so "only ballparks" means none.
    if res.is_empty() && (!has_non_ballpark_op_with_extent || has_non_ballpark_without_extent) {
        res = list
            .iter()
            .filter(|op| !op.has_ballpark())
            .cloned()
            .collect();
    }
    res
}

/// Whether the area of interest is contained by a candidate that uses no
/// grid (part of `filterOut`, used later by `removeSyntheticNullTransforms`).
fn has_op_containing_interest_without_grid(list: &[Candidate], areas: &Areas) -> bool {
    let Some(interest) = &areas.interest else {
        return false;
    };
    list.iter()
        .any(|op| op.extent.as_ref().is_some_and(|e| e.contains(interest)) && !op.has_grids())
}

/// Sort keys, computed once per candidate.
struct Keys {
    area: f64,
    accuracy: f64,
    exportable: bool,
    has_grids: bool,
    null_transformation: bool,
    step_count: usize,
    proj_step_count: usize,
    /// Registry operations that emulate another one (an NTv2 version of a
    /// geocentric grid method, by EPSG's remarks).
    emulations: usize,
    /// Registry operations run against their published direction.
    inverted: usize,
    /// Longitude rotations that disagree with their CRSs' prime meridians.
    off_meridian: usize,
    /// The registry operations, in order: (EPSG variant, code, inverse).
    registry: Vec<(u32, u32, bool)>,
    /// PROJ's duplicate test: the pipeline and the area.
    duplicate: String,
}

fn area_for_sort(op: &Candidate, areas: &Areas) -> f64 {
    let Some(extent) = &op.extent else {
        return 0.0;
    };
    let pa = |e: Option<Extent>| e.map_or(0.0, |e| e.pseudo_area());
    if let Some(interest) = &areas.interest {
        pa(extent.intersection(interest))
    } else {
        match (&areas.extent1, &areas.extent2) {
            (Some(e1), Some(e2)) => {
                let x = extent.intersection(e1);
                let y = extent.intersection(e2);
                let both = match (&x, &y) {
                    (Some(x), Some(y)) => pa(x.intersection(y)),
                    _ => 0.0,
                };
                pa(x) + pa(y) - both
            }
            (Some(e), None) | (None, Some(e)) => pa(extent.intersection(e)),
            (None, None) => extent.pseudo_area(),
        }
    }
}

/// What PROJ's `removeDuplicateOps` compares: the pipeline and the area.
fn duplicate_key(op: &Candidate, crs: &mut CrsCache) -> String {
    let mut key = format!("{:?}", pipeline_tokens(op, crs));
    if let Some(e) = &op.extent {
        key += &format!("-{}-{}-{}-{}", e.west, e.south, e.east, e.north);
    }
    key
}

fn keys(op: &Candidate, areas: &Areas, crs: &mut CrsCache) -> Keys {
    let registry: Vec<(u32, u32, bool)> = op
        .leaves()
        .into_iter()
        .filter_map(|s| match s.kind {
            SingleKind::Registry { code, inverse } => {
                Some((registry().ops[&code].variant, code, inverse))
            }
            _ => None,
        })
        .collect();
    Keys {
        area: area_for_sort(op, areas),
        accuracy: op.accuracy,
        exportable: is_exportable(op),
        has_grids: op.has_grids(),
        null_transformation: is_null_transformation(&op.name),
        step_count: op.steps.len(),
        // PROJ leaves pipelines with `xyzgridshift` out of this comparison:
        // they have more steps but are more precise.
        proj_step_count: if registry.iter().any(|(_, code, _)| {
            crate::operation::grid::XYZGRIDSHIFT_METHODS
                .contains(&crate::registry::registry().ops[code].method)
        }) {
            0
        } else {
            pipeline_tokens(op, crs).len() + 1
        },
        emulations: registry
            .iter()
            .filter(|(_, code, _)| !crate::registry::registry().ops[code].emulates.is_empty())
            .count(),
        inverted: registry.iter().filter(|(_, _, inverse)| *inverse).count(),
        duplicate: duplicate_key(op, crs),
        off_meridian: registry
            .iter()
            .filter(|(_, code, _)| off_meridian(*code))
            .count(),
        registry,
    }
}

/// A longitude rotation whose angle isn't the difference between its CRSs'
/// prime meridians. EPSG has two from NTF (Paris) to NTF: IGN's value for
/// the Paris meridian, which is the one EPSG defines it with, and the Royal
/// Geographic Society's, 1.6 m away (EPSG: "OGP prefers value from IGN
/// Paris").
fn off_meridian(code: u32) -> bool {
    let op = &registry().ops[&code];
    let Some(angle) = op.param(LONGITUDE_OFFSET) else {
        return false;
    };
    let meridian = |c: Option<u32>| {
        crate::crs::Crs::from_epsg(c?)
            .ok()
            .map(|crs| crs.datum().map_or(0.0, |d| d.prime_meridian))
    };
    match (meridian(op.src), meridian(op.dst)) {
        (Some(src), Some(dst)) => (angle - (src - dst)).abs() > 1e-12,
        _ => false,
    }
}

/// `SortFunction::compare`: whether `a` goes before `b`.
fn before(a: &Candidate, ka: &Keys, b: &Candidate, kb: &Keys) -> bool {
    if ka.exportable != kb.exportable {
        return ka.exportable;
    }
    // Ballpark and ballpark-vertical operations never get here.
    if ka.null_transformation != kb.null_transformation {
        return !ka.null_transformation;
    }
    // Grids are discarded when missing, so all remaining ones are available
    // and known.
    let (acc_a, acc_b) = (ka.accuracy, kb.accuracy);
    if acc_a >= 0.0 && acc_b < 0.0 {
        return true;
    }
    if acc_b >= 0.0 && acc_a < 0.0 {
        return false;
    }
    if acc_a < 0.0 && acc_b < 0.0 && ka.has_grids != kb.has_grids {
        return ka.has_grids;
    }
    if ka.area > 0.0 {
        if ka.area > kb.area {
            return true;
        }
        if ka.area < kb.area {
            return false;
        }
    } else if kb.area > 0.0 {
        return false;
    }
    if acc_a >= 0.0 && acc_a < acc_b {
        return true;
    }
    if acc_b >= 0.0 && acc_b < acc_a {
        return false;
    }
    if acc_a >= 0.0 && acc_a == acc_b && ka.has_grids != kb.has_grids {
        return !ka.has_grids;
    }
    // PROJ's IOGP 373-07-7 rule for ETRS89 realizations sits here; as
    // written its conditions contradict each other and it never applies.
    if ka.step_count != kb.step_count {
        return ka.step_count < kb.step_count;
    }
    if ka.proj_step_count != 0
        && kb.proj_step_count != 0
        && ka.proj_step_count != kb.proj_step_count
    {
        return ka.proj_step_count < kb.proj_step_count;
    }
    // Two operations with the same pipeline and area differ by name only,
    // and nothing in EPSG's data tells them apart: PROJ's order, the
    // shorter name first (its duplicate removal then keeps that one).
    if ka.duplicate == kb.duplicate && a.name.len() != b.name.len() {
        return a.name.len() < b.name.len();
    }
    // From here PROJ compares names: the shorter first, then the reverse
    // alphabetical order, with a preference for one of the NTF operations.
    // xeibe prefers the operation EPSG defines over its emulation by
    // another method, whichever direction either is published in.
    if ka.emulations != kb.emulations {
        return ka.emulations < kb.emulations;
    }
    if ka.inverted != kb.inverted {
        return ka.inverted < kb.inverted;
    }
    if ka.off_meridian != kb.off_meridian {
        return ka.off_meridian < kb.off_meridian;
    }
    // The newer variants first, then the EPSG codes, so the order is total.
    let variants = |k: &Keys| k.registry.iter().map(|r| r.0).collect::<Vec<_>>();
    match variants(kb).cmp(&variants(ka)) {
        Ordering::Equal => {}
        o => return o == Ordering::Less,
    }
    match ka.registry.cmp(&kb.registry) {
        Ordering::Equal => a.name < b.name,
        o => o == Ordering::Less,
    }
}

/// `filterAndSort`: filter, sort, then PROJ's clean-ups.
pub(crate) fn filter_and_sort(
    list: &[Candidate],
    areas: &Areas,
    crs: &mut CrsCache,
) -> Vec<Candidate> {
    let mut res = filter_out(list, areas, false);
    let contains_without_grid = has_op_containing_interest_without_grid(&res, areas);
    let keyed: Vec<Keys> = res.iter().map(|op| keys(op, areas, crs)).collect();
    let mut order: Vec<usize> = (0..res.len()).collect();
    // The comparison orders strictly (the codes, then the names, last).
    order.sort_by(|&i, &j| {
        if before(&res[i], &keyed[i], &res[j], &keyed[j]) {
            Ordering::Less
        } else if before(&res[j], &keyed[j], &res[i], &keyed[i]) {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });
    res = order.into_iter().map(|i| res[i].clone()).collect();
    remove_synthetic_null_transforms(&mut res, contains_without_grid);
    remove_uninteresting_ops(&mut res);
    remove_duplicate_ops(&mut res, crs);
    remove_synthetic_null_transforms(&mut res, contains_without_grid);
    res
}

fn remove_synthetic_null_transforms(res: &mut Vec<Candidate>, contains_without_grid: bool) {
    if contains_without_grid
        && res.len() > 1
        && res
            .last()
            .is_some_and(|op| is_null_transformation(&op.name))
    {
        res.pop();
    }
}

fn transformation_step_count(op: &Candidate) -> usize {
    if op.steps.len() == 1 {
        return 1;
    }
    op.steps.iter().filter(|s| !s.is_conversion).count()
}

fn remove_uninteresting_ops(res: &mut Vec<Candidate>) {
    let mut kept: Vec<Candidate> = Vec::new();
    let mut last: Option<(f64, Option<Extent>, usize, Candidate)> = None;
    for op in res.drain(..) {
        let cur_accuracy = op.accuracy;
        let cur_extent = op.extent.clone();
        let cur_steps = if op.identified {
            1
        } else {
            transformation_step_count(&op)
        };
        if let Some((last_accuracy, last_extent, last_steps, last_op)) = &last {
            if *last_op == op {
                continue;
            }
            let same_extent = match (&cur_extent, last_extent) {
                (None, None) => true,
                (Some(c), Some(l)) => c.contains(l) && l.contains(c),
                _ => false,
            };
            if ((cur_accuracy >= *last_accuracy && *last_accuracy >= 0.0)
                || (cur_accuracy < 0.0 && *last_accuracy >= 0.0))
                && same_extent
                && cur_steps > *last_steps
            {
                continue;
            }
        }
        kept.push(op.clone());
        last = Some((cur_accuracy, cur_extent, cur_steps, op));
    }
    *res = kept;
}

/// `removeDuplicateOps`: candidates with the same pipeline and area.
fn remove_duplicate_ops(res: &mut Vec<Candidate>, crs: &mut CrsCache) {
    if res.len() <= 1 {
        return;
    }
    let mut seen = std::collections::HashSet::new();
    res.retain(|op| seen.insert(duplicate_key(op, crs)));
}

/// A model of the PROJ pipeline a candidate exports to, for PROJ's
/// duplicate test and step count: the steps with their parameters, after
/// PROJ's removal of neighbouring steps that undo each other. The steps PROJ
/// adds at both ends for axis order and units are the same for every
/// candidate between two CRSs and are left out.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    Push,
    Pop,
    /// Geographic to geocentric on an ellipsoid (by EPSG code).
    Cart(u32),
    InvCart(u32),
    /// Any other step: an identity for comparison plus its direction.
    Step(String, bool),
}

impl Token {
    fn inverse(&self) -> Token {
        match self {
            Token::Push => Token::Pop,
            Token::Pop => Token::Push,
            Token::Cart(e) => Token::InvCart(*e),
            Token::InvCart(e) => Token::Cart(*e),
            Token::Step(s, inv) => Token::Step(s.clone(), !inv),
        }
    }
}

fn ellipsoid_of(crs: &mut CrsCache, c: super::crs::CrsRef) -> u32 {
    crs.datum(c)
        .and_then(|d| registry().datums.get(&d))
        .map_or(0, |d| d.ellipsoid)
}

pub(crate) fn pipeline_tokens(op: &Candidate, crs: &mut CrsCache) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    for s in &op.steps {
        step_tokens(s, crs, &mut tokens);
    }
    // Remove neighbours that undo each other, repeatedly.
    loop {
        let mut changed = false;
        let mut i = 0;
        while i + 1 < tokens.len() {
            if tokens[i].inverse() == tokens[i + 1] {
                tokens.drain(i..i + 2);
                changed = true;
            } else {
                i += 1;
            }
        }
        if !changed {
            break;
        }
    }
    tokens
}

/// The pipeline steps PROJ writes for one step.
fn step_tokens(s: &super::candidate::Single, crs: &mut CrsCache, tokens: &mut Vec<Token>) {
    {
        let mut t = match &s.kind {
            SingleKind::Composite {
                parts,
                save_horizontal,
            } => {
                if *save_horizontal {
                    tokens.push(Token::Step("push v_1 v_2".into(), false));
                }
                for p in parts {
                    step_tokens(p, crs, tokens);
                }
                if *save_horizontal {
                    tokens.push(Token::Step("pop v_1 v_2".into(), false));
                }
                return;
            }
            // Heights are metres up throughout; PROJ's unit conversions and
            // axis swaps for them aren't counted.
            SingleKind::VerticalConversion | SingleKind::BallparkVertical => vec![],
            SingleKind::SameDatum | SingleKind::EnsembleMember => {
                let (a, b) = (crs.kind(s.src), crs.kind(s.dst));
                match (a, b) {
                    (CrsKind::Geocentric, CrsKind::Geocentric) => vec![],
                    (_, CrsKind::Geocentric) => vec![Token::Cart(ellipsoid_of(crs, s.src))],
                    (CrsKind::Geocentric, _) => vec![Token::InvCart(ellipsoid_of(crs, s.dst))],
                    _ => vec![],
                }
            }
            SingleKind::Projection { crs: c, inverse } => {
                vec![Token::Step(format!("proj {c}"), *inverse)]
            }
            SingleKind::Registry { code, inverse } => {
                let forward = registry_tokens(*code, crs);
                if *inverse {
                    forward.iter().rev().map(Token::inverse).collect()
                } else {
                    forward
                }
            }
        };
        tokens.append(&mut t);
    }
}

fn registry_tokens(code: u32, crs: &mut CrsCache) -> Vec<Token> {
    let op = &registry().ops[&code];
    // A grid step is PROJ-data's file, inverted when the file goes the
    // other way: two EPSG operations on one file can cancel.
    if op.table == crate::registry::Table::Grid
        && let Some(entry) = op
            .grid_name()
            .and_then(|n| crate::grid::lookup(n, &op.name))
    {
        // What PROJ runs: a geoid model is `vgridshift` backwards whichever
        // EPSG method names it, an offset grid `vgridshift` with its sign.
        let step = if crate::operation::is_geoid_grid(op.method) {
            "vgridshift".to_owned()
        } else if let Some(sign) = crate::operation::offset_grid_sign(op.method) {
            format!("vgridshift {sign}")
        } else {
            op.method.to_string()
        };
        let inverse = entry.inverse != crate::operation::is_geoid_grid(op.method);
        return vec![Token::Step(format!("{step} {}", entry.file), inverse)];
    }
    let src = super::crs::CrsRef::Epsg(op.src.unwrap_or(0));
    let dst = super::crs::CrsRef::Epsg(op.dst.unwrap_or(0));
    let params: Vec<String> = op
        .params
        .iter()
        .map(|(c, v)| match v {
            ParamValue::Number(n) => format!("{c}={n:e}"),
            ParamValue::File(f) => format!("{c}={f}"),
            ParamValue::Missing => format!("{c}=?"),
        })
        .collect();
    let math = Token::Step(format!("{} {}", op.method, params.join(" ")), false);
    let zero = op.params.iter().all(|(_, v)| *v == ParamValue::Number(0.0));
    let (es, ed) = (ellipsoid_of(crs, src), ellipsoid_of(crs, dst));
    match op.method {
        // Geographic 2D domain: height kept aside.
        9603 | 9606 | 9607 | 9636 | 1063 | 1133 | 1054 | 1057 => {
            if zero && es == ed {
                vec![]
            } else if zero {
                vec![Token::Push, Token::Cart(es), Token::InvCart(ed), Token::Pop]
            } else {
                vec![
                    Token::Push,
                    Token::Cart(es),
                    math,
                    Token::InvCart(ed),
                    Token::Pop,
                ]
            }
        }
        // Geographic 3D domain.
        1035 | 1037 | 1038 | 1039 | 1062 | 1140 | 1055 | 1058 => {
            if zero && es == ed {
                vec![]
            } else if zero {
                vec![Token::Cart(es), Token::InvCart(ed)]
            } else {
                vec![Token::Cart(es), math, Token::InvCart(ed)]
            }
        }
        _ => {
            if zero && matches!(op.method, 1031..=1033) {
                vec![]
            } else {
                vec![math]
            }
        }
    }
}
