//! IOGP's GIGS test dataset (series 5100, conversions, and 5200,
//! transformations): `scripts/gen_gigs_vectors.py` writes the points to
//! `target/gigs/gigs.jsonl` from GIGS's files, which stay out of git (their
//! licence isn't open), with PROJ's result for each. Run with `--ignored`,
//! after the generator and `scripts/fetch_proj_grids.py`.
//!
//! A point passes when xeibe's result is within GIGS's tolerance of GIGS's,
//! or, where it isn't, within a micrometre (1e-11°) of PROJ's: a difference
//! between GIGS and PROJ, which `docs/reprojection.md` lists. GIGS's round
//! trip (its "Round Trip calculation point"s) goes back and forth 1000 times
//! and must stay within its round-trip tolerance of the first result, or
//! end where PROJ's does (a 2D transformation drifts, in PROJ too). A
//! test file in [`GAPS`] may fail to set up, for the reason given; any other
//! error fails the test.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use xeibe_reproject::{AxisOrder, Crs, Transform};

/// Test files xeibe can't run, and why.
const GAPS: &[(&str, &str)] = &[
    (
        "GIGS_tfm_5203_PosVec_output_part2.txt",
        "OSGB36 geographic 3D, which EPSG doesn't have: xeibe forces an operation only from or \
         to a CRS EPSG has on the datum with the same dimension",
    ),
    (
        "GIGS_tfm_5204_CoordFrame_output_part2.txt",
        "Belge 1972 geographic 3D, as 5203",
    ),
    (
        "GIGS_tfm_5211_3trnslt_Geocen_output.txt",
        "OSGB36 geocentric, as 5203",
    ),
    (
        "GIGS_tfm_5212_3trnslt_Geog3D_output_EPSGconcat.txt",
        "OSGB36 geographic 3D, as 5203",
    ),
];

fn vectors() -> PathBuf {
    match std::env::var_os("XEIBE_GIGS") {
        Some(path) => PathBuf::from(path),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/gigs/gigs.jsonl"),
    }
}

fn grids() -> PathBuf {
    match std::env::var_os("XEIBE_GRID_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/proj-grids"),
    }
}

fn crs(spec: &Value) -> Crs {
    if let Some(code) = spec.get("epsg") {
        return Crs::from_epsg(code.as_u64().unwrap() as u32).unwrap();
    }
    if let Some(pair) = spec.get("compound") {
        let code = |i: usize| Crs::from_epsg(pair[i].as_u64().unwrap() as u32).unwrap();
        return Crs::compound(&code(0), &code(1)).unwrap();
    }
    Crs::from_projjson_value(&spec["projjson"]).unwrap()
}

fn numbers(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

fn convert(t: &Transform, p: &[f64]) -> Option<Vec<f64>> {
    let (x, y, z) = t.convert(p[0], p[1], p.get(2).copied()).ok()?;
    let mut out = vec![x, y];
    if p.len() > 2 {
        out.push(z?);
    }
    Some(out)
}

#[derive(Default)]
struct Tally {
    points: usize,
    within_gigs: usize,
    as_proj: usize,
    gaps: usize,
    round_trips: usize,
    round_trips_within: usize,
    round_trips_as_proj: usize,
    worst: f64,
}

#[test]
#[ignore = "needs scripts/gen_gigs_vectors.py's target/gigs and PROJ-data's grids"]
fn gigs_conversions_and_transformations() {
    let path = vectors();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run scripts/gen_gigs_vectors.py", path.display()));
    let gaps: BTreeMap<&str, &str> = GAPS.iter().copied().collect();
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut transforms: BTreeMap<String, Option<Transform>> = BTreeMap::new();
    for line in text.lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let file = r["file"].as_str().unwrap();
        let tally = tallies
            .entry(r["test"].as_str().unwrap().into())
            .or_default();
        tally.points += 1;
        let key = format!("{file} {} {} {}", r["src"], r["dst"], r["op"]);
        let t = transforms.entry(key).or_insert_with(|| {
            let (src, dst) = (crs(&r["src"]), crs(&r["dst"]));
            let t = match r["op"].as_u64() {
                Some(op) => Transform::via(&src, &dst, op as u32, AxisOrder::Authority),
                None => Transform::new(&src, &dst, AxisOrder::Authority),
            }
            .ok()?;
            if !t.grids().is_empty() {
                t.load_grids_from_dir(grids(), None).ok()?;
            }
            Some(t)
        });
        let context = format!("{file} {}", r["point"].as_str().unwrap());
        let Some(t) = t else {
            if gaps.contains_key(file) {
                tally.gaps += 1;
            } else {
                failures.push(format!("{context}: no transform"));
            }
            continue;
        };
        if gaps.contains_key(file) {
            failures.push(format!("{context}: runs now; take it out of GAPS"));
        }
        let input = numbers(&r["in"]);
        let want = numbers(&r["out"]);
        let tolerance = numbers(&r["tolerance"]);
        let Some(got) = convert(t, &input) else {
            if r["proj"].is_null() {
                tally.as_proj += 1;
            } else {
                failures.push(format!("{context}: no result, PROJ has one"));
            }
            continue;
        };
        let within = |a: &[f64], b: &[f64], tol: &[f64]| {
            a.iter()
                .zip(b)
                .zip(tol)
                .all(|((x, y), t)| (x - y).abs() <= *t)
        };
        let off: f64 = got
            .iter()
            .zip(&want)
            .zip(&tolerance)
            .map(|((g, w), t)| (g - w).abs() / t)
            .fold(0.0, f64::max);
        tally.worst = tally.worst.max(off);
        if within(&got, &want, &tolerance) {
            tally.within_gigs += 1;
        } else if let Some(proj) = r["proj"].as_array().map(|_| numbers(&r["proj"])) {
            // Micrometres and 1e-11°: what two implementations of the same
            // formulas leave.
            let close: Vec<f64> = tolerance
                .iter()
                .map(|t| if *t < 1e-3 { 1e-11 } else { 1e-6 })
                .collect();
            if within(&got, &proj, &close) {
                tally.as_proj += 1;
            } else {
                failures.push(format!(
                    "{context}: {got:?}, GIGS {want:?} (tolerance {tolerance:?}), PROJ {proj:?}"
                ));
            }
        } else {
            failures.push(format!("{context}: {got:?}, GIGS {want:?}, PROJ has none"));
        }
        if r["round_trip"].as_bool().unwrap() {
            tally.round_trips += 1;
            let trip = numbers(&r["round_trip_tolerance"]);
            let back_t = t.inverse().unwrap();
            let mut p = got.clone();
            for _ in 0..1000 {
                let Some(back) = convert(&back_t, &p) else {
                    failures.push(format!("{context}: round trip failed"));
                    break;
                };
                p = convert(t, &back).unwrap();
            }
            // A transformation between geographic 2D CRSs drops the height
            // it gives, so back and forth isn't the identity: it drifts, in
            // PROJ as here (by 1e-4° in 1000 trips at 80°N for a 7-parameter
            // Helmert). Where it drifts beyond GIGS's tolerance, it must
            // drift to where PROJ's does.
            let proj_trip = r["proj_round_trip"]
                .as_array()
                .map(|_| numbers(&r["proj_round_trip"]));
            if within(&p, &got, &trip) {
                tally.round_trips_within += 1;
            } else if let Some(proj_trip) = proj_trip
                && within(&p, &proj_trip, &[1e-9, 1e-9, 1e-5])
            {
                tally.round_trips_as_proj += 1;
            } else {
                failures.push(format!(
                    "{context}: round trip drifted to {p:?} from {got:?}, PROJ to {:?}",
                    r["proj_round_trip"]
                ));
            }
        }
    }
    for (test, t) in &tallies {
        println!(
            "GIGS {test}: {} points, {} within GIGS's tolerance (worst {:.2} of it), {} as PROJ, \
             {} not run; {} round trips, {} within GIGS's tolerance, {} as PROJ's",
            t.points,
            t.within_gigs,
            t.worst,
            t.as_proj,
            t.gaps,
            t.round_trips,
            t.round_trips_within,
            t.round_trips_as_proj
        );
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
