//! Datum transformations against PROJ (`docs/reprojection.md`, level 2,
//! "Validation"), from `scripts/gen_transformation_vectors.py`.
//!
//! * `operations.jsonl`: one EPSG transformation forced with
//!   [`Transform::via`], to test each method's math.
//! * `selection.jsonl`: the automatic choice between two CRSs, per point:
//!   the same operation as PROJ (by name), and the same coordinates. Where
//!   xeibe passes coordinates unchanged between a datum ensemble and one of
//!   its members, which PROJ doesn't do, only the coordinates are compared,
//!   except for the pairs in [`ENSEMBLE_DEVIATIONS`]. Where candidates tie
//!   and PROJ decides by name, xeibe may pick another one
//!   ([`TIE_BREAK_DEVIATIONS`]).

// `!(err <= tolerance)` also fails on NaN.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use xeibe_reproject::{AxisOrder, Crs, Error, Transform};

/// Linear tolerance in metres, and the angular one in degrees (about 1 mm).
const LINEAR_M: f64 = 1e-3;
const ANGULAR_DEG: f64 = 1e-8;

/// The name of xeibe's identity between an ensemble and its member.
const ENSEMBLE_MEMBER: &str = "Ensemble member identity";

/// Pairs where that identity knowingly differs from PROJ.
const ENSEMBLE_DEVIATIONS: &[(u32, u32)] = &[
    // With no EPSG transformation between a national ETRS89 realization and
    // ETRS89, PROJ goes to an older national datum and back with two
    // separately fitted Helmerts; EPSG states the ensemble's members agree
    // to 0.1 m.
    (4619, 4258),  // ETRS89-SWE [SWEREF 99], via RT90: up to 0.62 m
    (4765, 4258),  // ETRS89-SVN [D96], via MGI 1901: up to 0.81 m
    (8685, 4258),  // ETRS89-SRB [STRS00], via MGI 1901: up to 0.03 m
    (9140, 4258),  // ETRS89-XKX [KOSOVAREF01], via MGI 1901: up to 2.2 km
    (10328, 4258), // ETRS89-BIH [BH_ETRS89], via MGI 1901: up to 2.0 m
    // Through a national realization, where EPSG's operation to it (often
    // its replacement for the one to ETRS89) plus the identity states a
    // better accuracy than PROJ's direct route.
    (4124, 4258), // RT90 via SWEREF 99: 0.5 m -> 0.2 m, moves up to 0.30 m
    (3021, 4258), // RT90 2.5 gon V, the same
    (4123, 4258), // KKJ via EUREF-FIN: 1.5 m -> 0.6 m, up to 0.07 m
    (2393, 4258), // KKJ / Finland Uniform, the same
    (4121, 4258), // GGRS87 via HTRS07: 2.0 m (via WGS 84) -> 1.1 m, up to 1.08 m
    (3906, 4258), // MGI 1901 via D96: 1.0 m -> 0.4 m, up to 0.18 m
    // Pulkovo 1942(83) in Bulgaria: PROJ falls back to an East German
    // operation outside its area; xeibe uses the Bulgarian one, up to 1.03 m.
    (4178, 4258),
];

/// Ties PROJ breaks by name (the shorter first) and xeibe by EPSG's data
/// (the newer variant first): (xeibe's choice, PROJ's).
const TIE_BREAK_DEVIATIONS: &[(&str, &str)] = &[
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

/// Largest difference between two coordinates, in metres for linear CRSs
/// and degrees (scaled to the angular tolerance) for geographic ones.
fn within(dst: u32, got: (f64, f64, Option<f64>), want: &[f64]) -> (bool, f64) {
    let record = xeibe_crs::get(dst);
    let angular = record.is_some_and(|r| r.kind.is_geographic());
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

fn check_operations(records: &[Value]) -> (BTreeMap<u64, OperationStats>, Vec<String>) {
    let mut stats: BTreeMap<u64, OperationStats> = BTreeMap::new();
    let mut failures = Vec::new();
    for r in records {
        let (op, src, dst) = (code(&r["op"]), code(&r["src"]), code(&r["dst"]));
        let s = stats.entry(r["method"].as_u64().unwrap()).or_default();
        let t = match (Crs::from_epsg(src), Crs::from_epsg(dst)) {
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
    /// Of those, PROJ transforms them otherwise (`ENSEMBLE_DEVIATIONS`).
    ensemble_deviations: usize,
    /// Pairs PROJ has no transformation for, and xeibe only this identity.
    ensemble_only_ours: usize,
    /// Ties broken otherwise than PROJ (`TIE_BREAK_DEVIATIONS`).
    tie_break_deviations: usize,
}

fn check_selection(records: &[Value]) -> (SelectionStats, Vec<String>) {
    let mut s = SelectionStats::default();
    let mut failures = Vec::new();
    for r in records {
        let (src, dst) = (code(&r["src"]), code(&r["dst"]));
        s.pairs += 1;
        let points = r["points"].as_array().unwrap();
        let proj_has_any = points.iter().any(|p| !p["out"].is_null());
        let t = Transform::from_epsg(src, dst, AxisOrder::Authority);
        let t = match t {
            Ok(t) => t,
            Err(e) => {
                if proj_has_any {
                    failures.push(format!("EPSG:{src} -> EPSG:{dst}: {e}; PROJ transforms"));
                } else {
                    s.no_transform_both += 1;
                }
                continue;
            }
        };
        if !proj_has_any {
            if t.operations().iter().all(|n| n.contains(ENSEMBLE_MEMBER)) {
                s.ensemble_only_ours += 1;
            } else {
                failures.push(format!(
                    "EPSG:{src} -> EPSG:{dst}: PROJ has no transformation, we have {:?}",
                    t.operations()
                ));
            }
        }
        for p in points {
            let input = floats(&p["in"]).unwrap();
            s.points += 1;
            let ours = t
                .operation_for(input[0], input[1], input.get(2).copied())
                .map(str::to_owned);
            let theirs = p["op"].as_str().map(str::to_owned);
            let conversion = t.operations().is_empty();
            if conversion {
                s.same_datum_points += 1;
            }
            let ensemble = ours.as_deref().is_some_and(|n| n.contains(ENSEMBLE_MEMBER));
            if ensemble {
                s.ensemble_points += 1;
                if theirs.is_none() {
                    continue;
                }
                if ENSEMBLE_DEVIATIONS.contains(&(src, dst)) {
                    s.ensemble_deviations += 1;
                    continue;
                }
            } else if let (Some(o), Some(t)) = (&ours, &theirs)
                && TIE_BREAK_DEVIATIONS.contains(&(o.as_str(), t.as_str()))
            {
                s.tie_break_deviations += 1;
                continue;
            } else if ours != theirs && !(conversion && theirs.is_some()) {
                s.different_operation += 1;
                failures.push(format!(
                    "EPSG:{src} -> EPSG:{dst} at {input:?}:\n    ours:  {ours:?}\n    PROJ:  {theirs:?}"
                ));
                continue;
            }
            if !ensemble {
                s.same_operation += 1;
            }
            if let (Ok(got), Some(want)) = (convert(&t, &input), floats(&p["out"])) {
                let (ok, err) = within(dst, got, &want);
                if !ok {
                    s.coordinate_failures += 1;
                    failures.push(format!(
                        "EPSG:{src} -> EPSG:{dst} at {input:?} via {ours:?}: {got:?}, PROJ {want:?} ({err:.3e})"
                    ));
                }
            }
        }
    }
    (s, failures)
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
         no transformation either side {}; ensemble-member identity {} (PROJ differs knowingly {}), pairs only xeibe transforms {}; \
         ties broken otherwise {}",
        s.pairs,
        s.points,
        s.same_operation,
        s.same_datum_points,
        s.different_operation,
        s.coordinate_failures,
        s.no_transform_both,
        s.ensemble_points,
        s.ensemble_deviations,
        s.ensemble_only_ours,
        s.tie_break_deviations
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
    let (stats, failures) = check_operations(&load(&data("operations.jsonl")));
    report_operations(&stats);
    fail_on(&failures);
}

#[test]
fn selection_matches_proj() {
    let (stats, failures) = check_selection(&load(&data("selection.jsonl")));
    report_selection(&stats);
    fail_on(&failures);
}

/// `scripts/gen_transformation_vectors.py --all` first.
#[test]
#[ignore = "needs target/reprojection/operations.jsonl"]
fn every_operation_matches_proj() {
    let (stats, failures) = check_operations(&load(&all("operations.jsonl")));
    report_operations(&stats);
    fail_on(&failures);
}

/// `scripts/gen_transformation_vectors.py --all` first.
#[test]
#[ignore = "needs target/reprojection/selection.jsonl"]
fn every_selection_matches_proj() {
    let (stats, failures) = check_selection(&load(&all("selection.jsonl")));
    report_selection(&stats);
    fail_on(&failures);
}
