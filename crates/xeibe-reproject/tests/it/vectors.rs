//! Differential test against PROJ (`docs/reprojection.md`, level 1,
//! "Validation" 3): every point of every vector, forward and inverse.
//!
//! Tolerances are fixed: 1 mm for projected and geocentric coordinates,
//! 1e-9 of the angular unit (about 0.1 mm) for geographic ones.

// `!(err <= tolerance)` also fails on NaN.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use std::collections::BTreeMap;
use std::path::Path;

use xeibe_reproject::{AxisOrder, Crs, Error, Transform};

use crate::support::{Vector, load};

/// Methods that must convert every CRS in the vectors. A method that
/// silently became unsupported would otherwise only show up as skipped.
const IMPLEMENTED: &[&str] = &[
    "Transverse Mercator",
    "Lambert Conic Conformal (1SP)",
    "Lambert Conic Conformal (2SP)",
    "Mercator (variant A)",
    "Mercator (variant B)",
    "Popular Visualisation Pseudo Mercator",
    "Mercator (1SP) (Spherical)",
    "Transverse Mercator (South Orientated)",
    "Transverse Mercator 3D",
    "Lambert Conic Conformal (2SP Belgium)",
    "Lambert Conic Conformal (2SP Michigan)",
    "Lambert Conic Conformal (1SP variant B)",
    "Hotine Oblique Mercator (variant A)",
    "Hotine Oblique Mercator (variant B)",
    "Laborde Oblique Mercator",
    "Krovak",
    "Krovak (North Orientated)",
    "Krovak Modified",
    "Krovak Modified (North Orientated)",
    "New Zealand Map Grid",
    "Cassini-Soldner",
    "Hyperbolic Cassini-Soldner",
    "Albers Equal Area",
    "American Polyconic",
    "Equal Earth",
    "Equidistant Cylindrical",
    "Equidistant Cylindrical (Spherical)",
    "Lambert Cylindrical Equal Area",
    "Lambert Cylindrical Equal Area (Spherical)",
    "Colombia Urban",
    "Oblique Stereographic",
    "Polar Stereographic (variant A)",
    "Polar Stereographic (variant B)",
    "Lambert Azimuthal Equal Area",
    "Lambert Azimuthal Equal Area (Spherical)",
    "Azimuthal Equidistant",
    "Modified Azimuthal Equidistant",
    "Guam Projection",
    "Local Orthographic",
];

const LINEAR_TOLERANCE_M: f64 = 1e-3;
const ANGULAR_TOLERANCE: f64 = 1e-9;

/// Ellipsoids PROJ swaps for a built-in one with a rounded semi-major axis.
/// EPSG gives Bessel Namibia as a = 6377397.155 German legal metres, which is
/// 6377483.86528 m; PROJ's `bess_nam` has a = 6377483.865, 0.28 mm shorter.
/// Points then differ by up to 0.15 mm: within the linear tolerance, but
/// beyond the angular one, which gets the matching slack.
const PROJ_ROUNDED_ELLIPSOIDS: &[&str] = &["Bessel Namibia (GLM)"];
const ROUNDED_ELLIPSOID_TOLERANCE: f64 = 2e-9;

#[derive(Default)]
struct Stats {
    crss: usize,
    points: usize,
    max_linear_m: f64,
    max_angular: f64,
}

#[derive(Default)]
struct Report {
    by_method: BTreeMap<String, Stats>,
    unsupported: BTreeMap<String, Vec<u32>>,
    failures: Vec<String>,
}

impl Report {
    fn fail(&mut self, message: String) {
        self.failures.push(message);
    }

    fn print(&self) {
        eprintln!(
            "{:<50} {:>5} {:>6} {:>12} {:>12}",
            "method", "CRSs", "points", "max m", "max angle"
        );
        for (method, s) in &self.by_method {
            eprintln!(
                "{method:<50} {:>5} {:>6} {:>12.3e} {:>12.3e}",
                s.crss, s.points, s.max_linear_m, s.max_angular
            );
        }
        for (method, codes) in &self.unsupported {
            eprintln!("unsupported: {method} ({} CRSs)", codes.len());
        }
    }
}

fn check(vectors: &[Vector]) -> Report {
    let mut report = Report::default();
    for v in vectors {
        let method = if v.kind == "geocentric" {
            "geocentric".to_owned()
        } else {
            v.method.clone()
        };
        let dst = match Crs::from_epsg(v.code) {
            Ok(crs) => crs,
            Err(Error::Unsupported { .. }) => {
                report.unsupported.entry(method).or_default().push(v.code);
                continue;
            }
            Err(e) => {
                report.fail(format!("EPSG:{}: {e}", v.code));
                continue;
            }
        };
        let src = match v.source() {
            Ok(crs) => crs,
            Err(e) => {
                report.fail(format!("EPSG:{} source: {e}", v.code));
                continue;
            }
        };
        let forward = match Transform::new(&src, &dst, AxisOrder::Authority) {
            Ok(t) => t,
            Err(e) => {
                report.fail(format!("EPSG:{}: {e}", v.code));
                continue;
            }
        };
        let inverse = forward.inverse().unwrap();
        let record = xeibe_crs::get(v.code);
        let unit_m = record.and_then(|r| r.linear_unit_m).unwrap_or(1.0);
        // A full turn in the source's angular unit, for longitudes on the
        // antimeridian (-180 and 180 are the same).
        let turn = match src.code().and_then(xeibe_crs::get) {
            Some(r) => r
                .angular_unit_rad
                .map_or(360.0, |u| std::f64::consts::TAU / u),
            None => 360.0,
        };
        let angular_tolerance = match xeibe_crs::projjson(v.code) {
            Some(doc) if PROJ_ROUNDED_ELLIPSOIDS.iter().any(|e| doc.contains(e)) => {
                ROUNDED_ELLIPSOID_TOLERANCE
            }
            _ => ANGULAR_TOLERANCE,
        };
        let stats = report.by_method.entry(method.clone()).or_default();
        stats.crss += 1;
        let mut failures = Vec::new();
        for (i, (geog, expected)) in v.geog.iter().zip(&v.proj).enumerate() {
            stats.points += 1;
            let z = geog.get(2).copied();
            let got = forward.convert(geog[0], geog[1], z);
            let Some(expected) = expected else {
                if let Ok(got) = got {
                    failures.push(format!(
                        "EPSG:{} point {i} {geog:?}: PROJ fails, we give {got:?}",
                        v.code
                    ));
                }
                continue;
            };
            let got = match got {
                Ok(got) => got,
                Err(e) => {
                    failures.push(format!(
                        "EPSG:{} point {i} {geog:?}: {e}, PROJ gives {expected:?}",
                        v.code
                    ));
                    continue;
                }
            };
            let mut err = ((got.0 - expected[0]).abs()).max((got.1 - expected[1]).abs());
            if let (Some(gz), Some(ez)) = (got.2, expected.get(2)) {
                err = err.max((gz - ez).abs());
            }
            let err_m = err * unit_m;
            stats.max_linear_m = stats.max_linear_m.max(err_m);
            if !(err_m <= LINEAR_TOLERANCE_M) {
                failures.push(format!(
                    "EPSG:{} point {i} {geog:?}: forward {got:?}, PROJ {expected:?} ({err_m:.3e} m)",
                    v.code
                ));
            }
            // Back from PROJ's projected point to the geographic one.
            let ez = expected.get(2).copied();
            match inverse.convert(expected[0], expected[1], ez) {
                Ok(back) => {
                    let diff = |a: f64, b: f64| {
                        let d = (a - b).abs() % turn;
                        d.min(turn - d)
                    };
                    let mut err = diff(back.0, geog[0]).max(diff(back.1, geog[1]));
                    stats.max_angular = stats.max_angular.max(err);
                    if let (Some(bz), Some(gz)) = (back.2, z) {
                        // Height comes back in metres: hold it to the linear tolerance.
                        if (bz - gz).abs() > LINEAR_TOLERANCE_M {
                            err = f64::INFINITY;
                        }
                    }
                    if !(err <= angular_tolerance) {
                        failures.push(format!(
                            "EPSG:{} point {i}: inverse of {expected:?} is {back:?}, want {geog:?} ({err:.3e})",
                            v.code
                        ));
                    }
                }
                Err(e) => failures.push(format!(
                    "EPSG:{} point {i}: inverse of {expected:?}: {e}",
                    v.code
                )),
            }
        }
        report.failures.extend(failures);
    }
    report
}

fn assert_clean(report: &Report) {
    report.print();
    let mut problems = report.failures.clone();
    for method in IMPLEMENTED {
        if let Some(codes) = report.unsupported.get(*method) {
            problems.push(format!(
                "{method} is implemented but unsupported for {codes:?}"
            ));
        }
    }
    if !problems.is_empty() {
        let shown: Vec<_> = problems.iter().take(40).collect();
        panic!(
            "{} failures:\n{}",
            problems.len(),
            shown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// The committed subset in `tests/data/reprojection/vectors.jsonl`.
#[test]
fn matches_proj_on_committed_vectors() {
    let vectors = load(&xeibe_testkit::data_dir().join("reprojection/vectors.jsonl"));
    assert!(vectors.len() > 100);
    let report = check(&vectors);
    for method in IMPLEMENTED {
        assert!(
            report.by_method.contains_key(*method),
            "no vectors for {method}"
        );
    }
    assert_clean(&report);
}

/// Every projected and geocentric CRS. Generate the vectors first:
/// `scripts/gen_reprojection_vectors.py --all`. `XEIBE_REPROJECTION_VECTORS`
/// overrides where they are read from.
#[test]
#[ignore = "needs target/reprojection/all.jsonl"]
fn matches_proj_on_every_crs() {
    let path = match std::env::var_os("XEIBE_REPROJECTION_VECTORS") {
        Some(path) => path.into(),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/reprojection/all.jsonl"),
    };
    let vectors = load(&path);
    let report = check(&vectors);
    assert_clean(&report);
}
