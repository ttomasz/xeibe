//! Datum transformations against PROJ (`docs/reprojection.md`, levels 2
//! and 3, "Validation"), from `scripts/gen_transformation_vectors.py`:
//! PROJ without grids against [`Grids::None`], and PROJ with every
//! PROJ-data grid (`*-grids.jsonl`) against [`Grids::ProjData`], with the
//! grids loaded from a local copy of PROJ-data.
//!
//! * `operations.jsonl`: one EPSG transformation forced with
//!   [`Transform::via`], to test each method's math.
//! * `selection.jsonl`: the automatic choice between two CRSs, per point:
//!   the same operation as PROJ (by name), and the same coordinates. PROJ
//!   runs on EPSG's operations plus its own null operations between a datum
//!   ensemble and each member, which xeibe has too. Where xeibe passes
//!   coordinates unchanged between CRSs of an ensemble and a member without
//!   PROJ's operation (its own identity), and where PROJ counts a member as
//!   the ensemble by an alias ([`DATUM_ALIAS_PAIRS`]), only the coordinates
//!   are compared. Routes through an ensemble that knowingly differ are in
//!   [`ENSEMBLE_DEVIATIONS`]. Where candidates tie and PROJ decides by name,
//!   xeibe may pick another one ([`TIE_BREAK_DEVIATIONS`]).
//! * `selection-vertical*.jsonl`: the same between CRSs with heights:
//!   compound CRSs to geographic 3D CRSs and to each other.

// `!(err <= tolerance)` also fails on NaN.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use xeibe_reproject::{AxisOrder, Crs, Error, Grids, Transform};

/// Linear tolerance in metres, and the angular one in degrees (about 1 mm).
const LINEAR_M: f64 = 1e-3;
const ANGULAR_DEG: f64 = 1e-8;

/// The name of xeibe's identity between an ensemble and its member.
const ENSEMBLE_MEMBER: &str = "Ensemble member identity";

/// Pairs where xeibe goes through a datum ensemble (PROJ's null operation
/// between it and a member, or xeibe's identity) and knowingly differs from
/// PROJ: routes through a national realization, where EPSG's operation to
/// it (often its replacement for the one to the ensemble) plus the null
/// operation states a better accuracy than PROJ's direct one. PROJ looks
/// for routes through other CRSs only when there is no direct operation.
const ENSEMBLE_DEVIATIONS: &[(u32, u32)] = &[
    // Pulkovo 1942(83) in Bulgaria: PROJ falls back to an East German
    // operation outside its area; xeibe uses the Bulgarian one, up to 1.03 m.
    (4178, 4258),
    // MGI 1901 via D96: 1.0 m -> 0.4 m, up to 0.18 m.
    (3906, 4258),
    // With grids. HD72 to ETRF2000 (2) by grid, 0.015 m, against PROJ's
    // 0.4 m Helmert (0.1-0.3 m apart).
    (4237, 4258),
    // GDA94 to GDA2020 (2) by grid and on to WGS 84 (G1762): 2.25 m against
    // the null GDA94 to WGS 84 (1), 3 m; 1.5 m apart. Projected CRSs on
    // GDA94 alike.
    (4283, 4326),
    (3111, 4326),
    (10452, 4326),
    (10462, 4326),
    (28351, 4326),
];

/// Pairs where PROJ counts a national realization of ETRS89 as ETRS89
/// itself, by an EPSG alias of its datum (EUREF-FIN and EUREF89 were once
/// named "European Terrestrial Reference System 1989"), and needs no step
/// between them; xeibe goes through PROJ's null operation between the
/// ensemble and the member. The same coordinates: judged by them.
const DATUM_ALIAS_PAIRS: &[(u32, u32)] = &[
    (3067, 4258), // ETRS89-FIN [EUREF-FIN] / TM35FIN
    // ETRS89-NOR [EUREF89] with SVD2006, NN2000:2018 and NN54 heights.
    (20001, 4937),
    (4937, 20001),
    (20001, 4979),
    (5942, 4937),
    (4937, 5942),
    (5942, 4979),
    (6144, 4937),
    (4937, 6144),
    (6144, 4979),
];

/// Ties PROJ breaks by name (the shorter first, then the reverse
/// alphabetical order) and xeibe by EPSG's data (`docs/reprojection.md`,
/// deviations): (xeibe's choice, PROJ's).
const TIE_BREAK_DEVIATIONS: &[(&str, &str)] = &[
    // Through either datum ensemble, which have no publication date to
    // tell them apart: xeibe by the CRS codes, PROJ by name. The same
    // coordinates.
    (
        "Monte Mario to ETRS89 (1) + Inverse of ED50 to ETRS89 (10)",
        "Monte Mario to WGS 84 (4) + Inverse of ED50 to WGS 84 (17)",
    ),
    // (12) replaces (1), says EPSG; 0.25 m apart.
    (
        "MGI 1901 to ETRS89-SVN [D96] (12)",
        "MGI 1901 to ETRS89-SVN [D96] (1)",
    ),
    // EPSG on each Coordinate Frame version: "OGP recommends this
    // alternative", the Molodensky-Badekas one; 2 mm apart.
    (
        "Bogota 1975 to MAGNA-SIRGAS (13)",
        "Bogota 1975 to MAGNA-SIRGAS (5)",
    ),
    (
        "Bogota 1975 to MAGNA-SIRGAS (14)",
        "Bogota 1975 to MAGNA-SIRGAS (6)",
    ),
    (
        "Bogota 1975 to MAGNA-SIRGAS (16)",
        "Bogota 1975 to MAGNA-SIRGAS (8)",
    ),
    // Equivalent by EPSG (two methods, the same parameters); 1 mm apart.
    (
        "LUREF to ETRS89-LUX [2025] (10)",
        "LUREF to ETRS89-LUX [2025] (9)",
    ),
    // The operation EPSG publishes in this direction rather than the
    // inverse of the other one; the same coordinates.
    (
        "ETRS89-SVK [SKTRF09] to S-JTSK [JTSK03] (1) + Inverse of ETRS89 to S-JTSK [JTSK03] (1)",
        "Inverse of S-JTSK [JTSK03] to ETRS89-SVK [SKTRF09] (1) + S-JTSK [JTSK03] to ETRS89 (1)",
    ),
];

/// With grids: ties PROJ breaks by name and xeibe by EPSG's data, where
/// the two choices differ by name only or by EPSG's stated preference:
/// (xeibe's choice, PROJ's).
const GRID_TIE_BREAK_DEVIATIONS: &[(&str, &str)] = &[
    // EPSG: the second step of (2) "is an emulation (using the NTv2 method)
    // of the geocentric interpolation" in (1). Both 1 m.
    (
        "NTF (Paris) to ETRS89-FRA [RGF93 v1] (1)",
        "NTF (Paris) to ETRS89-FRA [RGF93 v1] (2)",
    ),
    // Both routes run a grid backwards and then forwards (EPSG: "Parameter
    // file is from ..."): the identity either way, named differently.
    (
        "Inverse of NAD27 to NAD83(CSRS)v2 (1) + NAD27 to WGS 84 (34)",
        "Inverse of NAD83 to NAD83(CSRS)v2 (1) + NAD83 to WGS 84 (6)",
    ),
    (
        "Inverse of MTM zone 6 + Inverse of NAD27 to NAD83(CSRS)v2 (1) + NAD27 to WGS 84 (34)",
        "Inverse of MTM zone 6 + Inverse of NAD83 to NAD83(CSRS)v2 (1) + NAD83 to WGS 84 (6)",
    ),
    (
        "Inverse of UTM zone 13N + Inverse of NAD27 to NAD83(CSRS)v2 (2) + NAD27 to WGS 84 (32)",
        "Inverse of UTM zone 13N + Inverse of NAD83 to NAD83(CSRS)v2 (2) + NAD83 to WGS 84 (7)",
    ),
    (
        "Inverse of UTM zone 14N + Inverse of NAD27 to NAD83(CSRS)v2 (2) + NAD27 to WGS 84 (32)",
        "Inverse of UTM zone 14N + Inverse of NAD83 to NAD83(CSRS)v2 (2) + NAD83 to WGS 84 (7)",
    ),
];

/// With heights (`selection-vertical*.jsonl`): pairs where xeibe's choice
/// differs from PROJ's on purpose.
const VERTICAL_PAIR_DEVIATIONS: &[(u32, u32)] = &[
    // NTF + NGF-IGN69 height to WGS 84 3D: tied routes through RGF93 v2 and
    // RAF18 (PROJ, the shorter name) or RGF93 v2b and RAF20 (xeibe: EPSG
    // says RAF18 is "Replaced by RAF18b model"); heights up to 0.041 m
    // apart.
    (5707, 4979),
    (7400, 4979),
    (7421, 4979),
    (7422, 4979),
    // CD Norway depth at one inland point, where the newest grid has no
    // value: PROJ counts ETRS89 and ETRS89-NOR [EUREF89] as one datum (by an
    // EPSG alias), so its next candidate is the older grid; xeibe's own
    // candidates through the ensemble identity and NGO 1948 use up the three
    // attempts on the newest grid, and the point fails
    // (`docs/reprojection.md`, level 2).
    (9883, 4937),
    (4937, 9883),
    (9883, 4979),
    // BES2020 Saba and Sint Eustatius with their heights: EPSG's
    // concatenation of a Helmert and a zero geoid height, which PROJ fails
    // to build ("Mismatched units between step 1 and 2"); xeibe's result is
    // PROJ's Helmert.
    (10644, 10638),
    (10638, 10644),
    (10742, 10738),
    (10738, 10742),
];

/// With grids: pairs where xeibe's candidates differ from PROJ's on
/// purpose (or, for the last, for a reason not yet found).
const GRID_PAIR_DEVIATIONS: &[(u32, u32)] = &[
    // A superseded operation whose replacement uses a grid PROJ-data
    // doesn't publish stays usable. PROJ keeps it when it searches direct
    // operations but drops it when it searches through an intermediate
    // datum, and then has no route.
    (4122, 8240),
    (4269, 8252),
    (4269, 8255),
    // EPSG supersedes NTv1 (3) by NTv2 (4), which PROJ skips (and raises
    // NTv1's accuracy instead). 500 km off the Pacific coast only NTv1's grid
    // reaches; xeibe has no operation there.
    (4267, 4269),
    // Not yet explained: PROJ also composes the steps of EPSG's NAD27 to
    // ITRF2014 (2), whose own area is offshore, over CONUS.
    (4267, 7789),
];

fn load(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; run scripts/gen_transformation_vectors.py",
            path.display()
        )
    });
    let mut lines = text.lines();
    let header: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!(header["generator"], "scripts/gen_transformation_vectors.py");
    lines.map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn floats(v: &Value) -> Option<Vec<f64>> {
    v.as_array()
        .map(|a| a.iter().map(|x| x.as_f64().unwrap()).collect())
}

fn code(v: &Value) -> u32 {
    v.as_u64().unwrap() as u32
}

/// A CRS's EPSG code, or 0 for `[horizontal, vertical]`.
fn epsg(v: &Value) -> u32 {
    v.as_u64().map_or(0, |c| c as u32)
}

/// "EPSG:4326", or "EPSG:4167+5770" for a compound CRS made up of two.
fn label(v: &Value) -> String {
    match v.as_array() {
        Some(parts) => format!("EPSG:{}+{}", code(&parts[0]), code(&parts[1])),
        None => format!("EPSG:{}", code(v)),
    }
}

/// An EPSG code, or `[horizontal, vertical]` for their compound CRS.
fn crs_of(v: &Value) -> Result<Crs, Error> {
    match v.as_array() {
        Some(parts) => Crs::compound(
            &Crs::from_epsg(code(&parts[0]))?,
            &Crs::from_epsg(code(&parts[1]))?,
        ),
        None => Crs::from_epsg(code(v)),
    }
}

/// The code of the CRS, or of a compound CRS's horizontal part.
fn horizontal_code(v: &Value) -> u32 {
    v.as_array()
        .map_or_else(|| code(v), |parts| code(&parts[0]))
}

/// Largest difference between two coordinates, in metres for linear CRSs
/// and degrees (scaled to the angular tolerance) for geographic ones.
fn within(dst: u32, got: (f64, f64, Option<f64>), want: &[f64]) -> (bool, f64) {
    let record = xeibe_crs::get(dst);
    // A compound CRS is angular when its horizontal part is geographic.
    let angular = record.is_some_and(|r| {
        r.kind.is_geographic()
            || (r.kind == xeibe_crs::CrsKind::Compound && r.angular_unit_rad.is_some())
    });
    let dx = (got.0 - want[0]).abs().max((got.1 - want[1]).abs());
    let dz = match (got.2, want.get(2)) {
        (Some(g), Some(w)) => (g - w).abs(),
        _ => 0.0,
    };
    if angular {
        let deg = record
            .and_then(|r| r.angular_unit_rad)
            .unwrap_or(1.0f64.to_radians())
            .to_degrees();
        let err = dx * deg;
        (
            err <= ANGULAR_DEG && dz <= LINEAR_M,
            err.max(dz * ANGULAR_DEG / LINEAR_M),
        )
    } else {
        let unit = record.and_then(|r| r.linear_unit_m).unwrap_or(1.0);
        let err = dx.max(dz) * unit;
        (err <= LINEAR_M, err)
    }
}

/// A local copy of PROJ-data, for the `--ignored` tests.
fn grid_dir() -> PathBuf {
    match std::env::var_os("XEIBE_GRID_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/proj-grids"),
    }
}

/// The grids PROJ had: none, or all of PROJ-data in a directory.
#[derive(Clone, Copy)]
enum World<'a> {
    NoGrids,
    Grids(&'a Path),
}

impl World<'_> {
    fn grids(self) -> Grids {
        match self {
            World::NoGrids => Grids::None,
            World::Grids(_) => Grids::ProjData,
        }
    }
}

/// Load what `t` needs around `points` (in its source CRS and axis order).
fn load_grids(t: &Transform, world: World, points: &[Vec<f64>]) -> Result<(), Error> {
    let World::Grids(dir) = world else {
        return Ok(());
    };
    if t.grids().is_empty() || points.is_empty() {
        return Ok(());
    }
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for p in points {
        b = [
            b[0].min(p[0]),
            b[1].min(p[1]),
            b[2].max(p[0]),
            b[3].max(p[1]),
        ];
    }
    t.load_grids_from_dir(dir, Some(b))
}

fn convert(
    t: &Transform,
    p: &[f64],
) -> Result<(f64, f64, Option<f64>), xeibe_reproject::CoordinateError> {
    t.convert(p[0], p[1], p.get(2).copied())
}

#[derive(Default)]
struct OperationStats {
    ops: usize,
    points: usize,
    max: f64,
    unsupported: usize,
}

fn check_operations(
    records: &[Value],
    world: World,
) -> (BTreeMap<u64, OperationStats>, Vec<String>) {
    let mut stats: BTreeMap<u64, OperationStats> = BTreeMap::new();
    let mut failures = Vec::new();
    for r in records {
        let op = code(&r["op"]);
        // The horizontal CRS judges the coordinates' units.
        let dst = horizontal_code(&r["dst"]);
        let s = stats.entry(r["method"].as_u64().unwrap()).or_default();
        let t = match (crs_of(&r["src"]), crs_of(&r["dst"])) {
            (Ok(a), Ok(b)) => Transform::via(&a, &b, op, AxisOrder::Authority),
            (Err(e), _) | (_, Err(e)) => Err(e),
        };
        let t = match t {
            Ok(t) => t,
            Err(Error::Unsupported { .. }) => {
                s.unsupported += 1;
                continue;
            }
            Err(e) => {
                failures.push(format!("operation {op}: {e}"));
                continue;
            }
        };
        s.ops += 1;
        let inputs: Vec<Vec<f64>> = r["in"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(floats)
            .collect();
        if let Err(e) = load_grids(&t, world, &inputs) {
            failures.push(format!("operation {op}: {e}"));
            continue;
        }
        for (p, want) in r["in"]
            .as_array()
            .unwrap()
            .iter()
            .zip(r["out"].as_array().unwrap())
        {
            let p = floats(p).unwrap();
            s.points += 1;
            match (convert(&t, &p), floats(want)) {
                (Ok(got), Some(want)) => {
                    let (ok, err) = within(dst, got, &want);
                    s.max = s.max.max(err);
                    if !ok {
                        failures.push(format!(
                            "operation {op} at {p:?}: {got:?}, PROJ {want:?} ({err:.3e})"
                        ));
                    }
                }
                (Err(e), Some(want)) => {
                    failures.push(format!("operation {op} at {p:?}: {e}, PROJ {want:?}"))
                }
                (Ok(got), None) => failures.push(format!(
                    "operation {op} at {p:?}: PROJ fails, we give {got:?}"
                )),
                (Err(_), None) => {}
            }
        }
    }
    (stats, failures)
}

#[derive(Default)]
struct SelectionStats {
    pairs: usize,
    points: usize,
    same_operation: usize,
    different_operation: usize,
    coordinate_failures: usize,
    no_transform_both: usize,
    /// Pairs on one datum: a conversion for xeibe, which PROJ runs as a
    /// conversion or null operation of its own naming. Judged by coordinates.
    same_datum_points: usize,
    /// Through xeibe's ensemble-member identity: judged by coordinates.
    ensemble_points: usize,
    /// Points of `ENSEMBLE_DEVIATIONS` pairs where xeibe and PROJ differ.
    ensemble_deviations: usize,
    /// Points of `DATUM_ALIAS_PAIRS` pairs where xeibe and PROJ differ by
    /// name: judged by coordinates.
    alias_points: usize,
    /// Pairs PROJ has no transformation for, and xeibe only this identity.
    ensemble_only_ours: usize,
    /// Ties broken otherwise than PROJ (`TIE_BREAK_DEVIATIONS`).
    tie_break_deviations: usize,
    /// Points of `GRID_PAIR_DEVIATIONS` pairs where xeibe and PROJ differ.
    pair_deviations: usize,
    /// Listed pairs PROJ has no transformation for at all.
    listed_pairs_proj_none: usize,
    /// Points whose route differs from PROJ's only in EPSG variants: ties
    /// broken by the newer variant.
    variant_ties: usize,
    /// Points where PROJ's reverse of a grid is an estimate (NADCON5's,
    /// which EPSG iterates): xeibe's result goes back to the input through
    /// the forward operation, PROJ's doesn't.
    approximate_inverses: usize,
}

fn check_selection(records: &[Value], world: World) -> (SelectionStats, Vec<String>) {
    let mut s = SelectionStats::default();
    let mut failures = Vec::new();
    for r in records {
        let (src_v, dst_v) = (&r["src"], &r["dst"]);
        // EPSG codes for the deviation lists; none for a compound CRS made
        // up of two.
        let (src, dst) = (epsg(src_v), epsg(dst_v));
        let (sl, dl) = (label(src_v), label(dst_v));
        s.pairs += 1;
        let points = r["points"].as_array().unwrap();
        let proj_has_any = points.iter().any(|p| !p["out"].is_null());
        let t = match (crs_of(src_v), crs_of(dst_v)) {
            (Ok(a), Ok(b)) => Transform::with_grids(&a, &b, AxisOrder::Authority, world.grids()),
            (Err(e), _) | (_, Err(e)) => Err(e),
        };
        let t = match t {
            Ok(t) => t,
            Err(e) => {
                if proj_has_any {
                    failures.push(format!("{sl} -> {dl}: {e}; PROJ transforms"));
                } else {
                    s.no_transform_both += 1;
                }
                continue;
            }
        };
        // Pairs PROJ transforms none of the points of: fine where xeibe
        // only has the ensemble identity; otherwise judged point by point
        // (with grids, PROJ may have operations whose grids don't cover the
        // sampled points).
        let only_identity = t.operations().iter().all(|n| n.contains(ENSEMBLE_MEMBER));
        if !proj_has_any && only_identity {
            s.ensemble_only_ours += 1;
        } else if points.is_empty() {
            // PROJ has no transformation at all, so no points to judge by.
            if matches!(world, World::Grids(_))
                && (GRID_PAIR_DEVIATIONS.contains(&(src, dst))
                    || VERTICAL_PAIR_DEVIATIONS.contains(&(src, dst)))
            {
                s.listed_pairs_proj_none += 1;
            } else {
                failures.push(format!("{sl} -> {dl}: {:?}; PROJ has none", t.operations()));
            }
            continue;
        }
        let inputs: Vec<Vec<f64>> = points.iter().filter_map(|p| floats(&p["in"])).collect();
        if let Err(e) = load_grids(&t, world, &inputs) {
            failures.push(format!("{sl} -> {dl}: {e}"));
            continue;
        }
        for p in points {
            let input = floats(&p["in"]).unwrap();
            s.points += 1;
            let ours = t
                .operation_for(input[0], input[1], input.get(2).copied())
                .map(str::to_owned);
            // PROJ names an operation whose grid PROJ-data stores the other
            // way round "Inverse of Inverse of …": the operation itself.
            let theirs = p["op"]
                .as_str()
                .map(|n| n.replace("Inverse of Inverse of ", ""));
            let conversion = t.operations().is_empty();
            if conversion {
                s.same_datum_points += 1;
            }
            let ensemble = ours.as_deref().is_some_and(|n| n.contains(ENSEMBLE_MEMBER));
            let alias = ours != theirs && DATUM_ALIAS_PAIRS.contains(&(src, dst));
            if ensemble {
                s.ensemble_points += 1;
                if theirs.is_none() {
                    continue;
                }
                if ENSEMBLE_DEVIATIONS.contains(&(src, dst)) {
                    s.ensemble_deviations += 1;
                    continue;
                }
            } else if ours != theirs && ENSEMBLE_DEVIATIONS.contains(&(src, dst)) {
                s.ensemble_deviations += 1;
                continue;
            } else if alias {
                s.alias_points += 1;
            } else if let (Some(o), Some(t)) = (&ours, &theirs)
                && (TIE_BREAK_DEVIATIONS.contains(&(o.as_str(), t.as_str()))
                    || (matches!(world, World::Grids(_))
                        && GRID_TIE_BREAK_DEVIATIONS.contains(&(o.as_str(), t.as_str()))))
            {
                s.tie_break_deviations += 1;
                continue;
            } else if ours != theirs
                && matches!(world, World::Grids(_))
                && (GRID_PAIR_DEVIATIONS.contains(&(src, dst))
                    || VERTICAL_PAIR_DEVIATIONS.contains(&(src, dst)))
            {
                s.pair_deviations += 1;
                continue;
            } else if let (Some(o), Some(t)) = (&ours, &theirs)
                && o.contains(" + ")
                && other_variants(o, t)
            {
                // The same route but for EPSG variants: a tie PROJ breaks
                // by name length and xeibe by the newer variant (level 2),
                // met again in each concatenation with heights.
                s.variant_ties += 1;
                continue;
            } else if ours != theirs && !(conversion && theirs.is_some()) {
                s.different_operation += 1;
                let apart = match (convert(&t, &input), floats(&p["out"])) {
                    (Ok(got), Some(want)) => {
                        format!("{:.3e}", within(horizontal_code(dst_v), got, &want).1)
                    }
                    _ => "-".into(),
                };
                failures.push(format!(
                    "{sl} -> {dl} at {input:?} (apart {apart}):\n    ours:  {ours:?}\n    PROJ:  {theirs:?}"
                ));
                continue;
            }
            if !ensemble && !alias {
                s.same_operation += 1;
            }
            if let (Ok(got), Some(want)) = (convert(&t, &input), floats(&p["out"])) {
                let (ok, err) = within(horizontal_code(dst_v), got, &want);
                if !ok && reverses_better(src_v, dst_v, world, &input, got, &want) {
                    s.approximate_inverses += 1;
                } else if !ok {
                    s.coordinate_failures += 1;
                    failures.push(format!(
                        "{sl} -> {dl} at {input:?} via {ours:?}: {got:?}, PROJ {want:?} ({err:.3e})"
                    ));
                }
            }
        }
    }
    (s, failures)
}

/// Two operation names that differ only in their EPSG variants: "(47)"
/// against "(1)".
fn other_variants(a: &str, b: &str) -> bool {
    let strip = |s: &str| {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '(' && chars.peek().is_some_and(char::is_ascii_digit) {
                while chars.next().is_some_and(|c| c != ')') {}
                out.push_str("(n)");
            } else {
                out.push(c);
            }
        }
        out
    };
    a != b && strip(a) == strip(b)
}

/// Whether `got` converts back to `input` through the reverse transform
/// and PROJ's `want` doesn't.
fn reverses_better(
    src: &Value,
    dst: &Value,
    world: World,
    input: &[f64],
    got: (f64, f64, Option<f64>),
    want: &[f64],
) -> bool {
    let (Ok(a), Ok(b)) = (crs_of(src), crs_of(dst)) else {
        return false;
    };
    let Ok(back) = Transform::with_grids(&b, &a, AxisOrder::Authority, world.grids()) else {
        return false;
    };
    let ours = vec![got.0, got.1];
    if load_grids(&back, world, &[ours.clone(), want.to_vec()]).is_err() {
        return false;
    }
    let returns =
        |p: &[f64]| convert(&back, p).is_ok_and(|r| within(horizontal_code(src), r, input).0);
    returns(&ours) && !returns(want)
}

fn report_operations(stats: &BTreeMap<u64, OperationStats>) {
    eprintln!(
        "{:>7} {:>5} {:>7} {:>12} {:>11}",
        "method", "ops", "points", "max err", "unsupported"
    );
    for (m, s) in stats {
        eprintln!(
            "{m:>7} {:>5} {:>7} {:>12.3e} {:>11}",
            s.ops, s.points, s.max, s.unsupported
        );
    }
}

fn report_selection(s: &SelectionStats) {
    eprintln!(
        "pairs {}, points {}: same operation {} (of which on one datum {}), different {}, coordinate failures {}, \
         no transformation either side {}; ensemble-member identity {}, through an ensemble knowingly unlike PROJ {}, \
         PROJ's datum alias {}, pairs only xeibe transforms {}; \
         ties broken otherwise {} (by variant in a route {}); listed pair deviations {} (and {} pairs PROJ can't transform); PROJ's inverse approximate {}",
        s.pairs,
        s.points,
        s.same_operation,
        s.same_datum_points,
        s.different_operation,
        s.coordinate_failures,
        s.no_transform_both,
        s.ensemble_points,
        s.ensemble_deviations,
        s.alias_points,
        s.ensemble_only_ours,
        s.tie_break_deviations,
        s.variant_ties,
        s.pair_deviations,
        s.listed_pairs_proj_none,
        s.approximate_inverses
    );
}

fn fail_on(failures: &[String]) {
    // All of them, for analysis: XEIBE_FAILURES_FILE=path.
    if let Some(path) = std::env::var_os("XEIBE_FAILURES_FILE") {
        std::fs::write(path, failures.join("\n")).unwrap();
    }
    if !failures.is_empty() {
        let shown: Vec<_> = failures.iter().take(30).map(String::as_str).collect();
        panic!("{} failures:\n{}", failures.len(), shown.join("\n"));
    }
}

fn data(name: &str) -> PathBuf {
    xeibe_testkit::data_dir().join("reprojection").join(name)
}

fn all(name: &str) -> PathBuf {
    match std::env::var_os("XEIBE_TRANSFORMATION_VECTORS") {
        Some(dir) => PathBuf::from(dir).join(name),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/reprojection")
            .join(name),
    }
}

#[test]
fn operations_match_proj() {
    let (stats, failures) = check_operations(&load(&data("operations.jsonl")), World::NoGrids);
    report_operations(&stats);
    fail_on(&failures);
}

#[test]
fn selection_matches_proj() {
    let (stats, failures) = check_selection(&load(&data("selection.jsonl")), World::NoGrids);
    report_selection(&stats);
    fail_on(&failures);
}

/// The committed grids (`scripts/gen_grid_fixtures.py`) and PROJ's results
/// with them.
fn fixture_grids() -> PathBuf {
    data("grids")
}

#[test]
fn grid_operations_match_proj() {
    let dir = fixture_grids();
    let (stats, failures) =
        check_operations(&load(&data("operations-grids.jsonl")), World::Grids(&dir));
    report_operations(&stats);
    fail_on(&failures);
}

#[test]
fn grid_selection_matches_proj() {
    let dir = fixture_grids();
    let (stats, failures) =
        check_selection(&load(&data("selection-grids.jsonl")), World::Grids(&dir));
    report_selection(&stats);
    fail_on(&failures);
}

#[test]
fn vertical_selection_matches_proj() {
    let (stats, failures) =
        check_selection(&load(&data("selection-vertical.jsonl")), World::NoGrids);
    report_selection(&stats);
    fail_on(&failures);
}

#[test]
fn vertical_grid_selection_matches_proj() {
    let dir = fixture_grids();
    let (stats, failures) = check_selection(
        &load(&data("selection-vertical-grids.jsonl")),
        World::Grids(&dir),
    );
    report_selection(&stats);
    fail_on(&failures);
}

/// `scripts/gen_transformation_vectors.py --all` first.
#[test]
#[ignore = "needs target/reprojection/operations.jsonl"]
fn every_operation_matches_proj() {
    let (stats, failures) = check_operations(&load(&all("operations.jsonl")), World::NoGrids);
    report_operations(&stats);
    fail_on(&failures);
}

/// `scripts/gen_transformation_vectors.py --all` first.
#[test]
#[ignore = "needs target/reprojection/selection.jsonl"]
fn every_selection_matches_proj() {
    let (stats, failures) = check_selection(&load(&all("selection.jsonl")), World::NoGrids);
    report_selection(&stats);
    fail_on(&failures);
}

/// `scripts/gen_transformation_vectors.py --all --grids target/proj-grids`
/// first; the grids are read from there too (or `XEIBE_GRID_DIR`).
#[test]
#[ignore = "needs target/reprojection/operations-grids.jsonl and target/proj-grids"]
fn every_grid_operation_matches_proj() {
    let dir = grid_dir();
    let (stats, failures) =
        check_operations(&load(&all("operations-grids.jsonl")), World::Grids(&dir));
    report_operations(&stats);
    fail_on(&failures);
}

/// As [`every_grid_operation_matches_proj`].
#[test]
#[ignore = "needs target/reprojection/selection-grids.jsonl and target/proj-grids"]
fn every_grid_selection_matches_proj() {
    let dir = grid_dir();
    let (stats, failures) =
        check_selection(&load(&all("selection-grids.jsonl")), World::Grids(&dir));
    report_selection(&stats);
    fail_on(&failures);
}

/// `scripts/gen_transformation_vectors.py --all --vertical --grids
/// target/proj-grids` first.
#[test]
#[ignore = "needs target/reprojection/selection-vertical-grids.jsonl and target/proj-grids"]
fn every_vertical_selection_matches_proj() {
    let dir = grid_dir();
    let (stats, failures) = check_selection(
        &load(&all("selection-vertical-grids.jsonl")),
        World::Grids(&dir),
    );
    report_selection(&stats);
    fail_on(&failures);
}
