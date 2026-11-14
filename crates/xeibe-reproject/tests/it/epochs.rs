//! Coordinate epochs (`docs/reprojection.md`, "Coordinate epochs"): a
//! time-dependent operation applies its rates up to the epoch of the
//! coordinates, which comes with each coordinate
//! ([`Transform::convert_at`]) or with the transform
//! ([`Transform::with_epochs`]).
//!
//! `epochs.jsonl` is PROJ's (`scripts/gen_transformation_vectors.py
//! --epochs`): each time-dependent Helmert transformation forced, forwards
//! and back, and the automatic choice between CRSs they join, every point
//! at an epoch of its own or at none. PROJ's results with an epoch on the
//! CRS (`cs2cs --s_epoch`, `--t_epoch`) are written out here.

use std::path::{Path, PathBuf};

use serde_json::Value;
use xeibe_reproject::{AxisOrder, CoordinateError, Crs, Error, Grids, Transform};

/// The committed vectors.
fn committed() -> PathBuf {
    xeibe_testkit::data_dir().join("reprojection/epochs.jsonl")
}

/// Every time-dependent operation's: `--epochs --all`.
fn all() -> PathBuf {
    match std::env::var_os("XEIBE_TRANSFORMATION_VECTORS") {
        Some(dir) => PathBuf::from(dir).join("epochs.jsonl"),
        None => {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/reprojection/epochs.jsonl")
        }
    }
}

fn records(path: &Path, kind: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; run scripts/gen_transformation_vectors.py --epochs",
            path.display()
        )
    });
    text.lines()
        .skip(1)
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|r| r["kind"] == kind)
        .collect()
}

fn numbers(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

/// A point's epoch: NaN for one without (PROJ's `HUGE_VAL`).
fn epoch(point: &Value) -> f64 {
    point["t"].as_f64().unwrap_or(f64::NAN)
}

fn convert_at(t: &Transform, p: &[f64], epoch: f64) -> Vec<f64> {
    let (x, y, z) = t
        .convert_at(p[0], p[1], p.get(2).copied(), epoch)
        .unwrap_or_else(|e| panic!("{p:?} at {epoch}: {e}"));
    [Some(x), Some(y), z].into_iter().flatten().collect()
}

/// Within `metres`, or as many degrees on the ellipsoid as that is.
fn assert_close(crs: &Crs, got: &[f64], want: &[f64], metres: f64, context: &str) {
    assert_eq!(got.len(), want.len(), "{context}");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let tolerance = if crs.kind_name() == "geographic" && i < 2 {
            metres * 1e-5
        } else {
            metres
        };
        assert!(
            (g - w).abs() <= tolerance,
            "{context}: got {got:?}, PROJ {want:?}"
        );
    }
}

/// Each operation forced, forwards and back. Returns how many points the
/// epoch moved.
fn check_operations(path: &Path) -> usize {
    let records = records(path, "operation");
    assert!(!records.is_empty());
    let mut moved = 0;
    for r in &records {
        let op = r["op"].as_u64().unwrap() as u32;
        let src = Crs::from_epsg(r["src"].as_u64().unwrap() as u32).unwrap();
        let dst = Crs::from_epsg(r["dst"].as_u64().unwrap() as u32).unwrap();
        let forward = Transform::via(&src, &dst, op, AxisOrder::Authority).unwrap();
        let back = Transform::via(&dst, &src, op, AxisOrder::Authority).unwrap();
        for point in r["points"].as_array().unwrap() {
            let (input, t) = (numbers(&point["in"]), epoch(point));
            let context = format!("EPSG:{op} at {t}");
            if point["out"].is_null() || point["back"].is_null() {
                continue;
            }
            let want = numbers(&point["out"]);
            let got = convert_at(&forward, &input, t);
            assert_close(&dst, &got, &want, 1e-6, &context);
            let returned = convert_at(&back, &want, t);
            assert_close(&src, &returned, &numbers(&point["back"]), 1e-6, &context);
            // The epoch matters: at the reference epoch the result is
            // another, by more than the tolerance, for most points.
            let at_reference = convert_at(&forward, &input, f64::NAN);
            if got
                .iter()
                .zip(&at_reference)
                .any(|(a, b)| (a - b).abs() > 1e-3)
            {
                moved += 1;
            }
        }
    }
    moved
}

#[test]
fn forced_operations_follow_the_epoch_of_each_coordinate() {
    let moved = check_operations(&committed());
    assert!(moved > 20, "{moved} points moved with their epoch");
}

/// `scripts/gen_transformation_vectors.py --epochs --all` first.
#[test]
#[ignore = "needs target/reprojection/epochs.jsonl"]
fn every_time_dependent_operation_matches_proj() {
    let moved = check_operations(&all());
    println!("{moved} points moved with their epoch");
}

/// A route's tolerance, the other selection vectors' (`transformations.rs`):
/// a millimetre. Across GRS 1980 and WGS 84 xeibe keeps the 0.1 mm between
/// the ellipsoids where PROJ's pipeline drops it (`docs/reprojection.md`,
/// deviations); an epoch moves a coordinate by centimetres.
const ROUTE_M: f64 = 1e-3;

/// The automatic choice between two CRSs. With `same_operation`, PROJ's
/// operation for each point is xeibe's; without, a point that takes
/// another one (a tie broken differently, `transformations.rs`) is left
/// out. Returns how many were.
fn check_pairs(path: &Path, same_operation: bool) -> usize {
    let records = records(path, "pair");
    assert!(!records.is_empty());
    let mut other = 0;
    for r in &records {
        let (s, d) = (
            r["src"].as_u64().unwrap() as u32,
            r["dst"].as_u64().unwrap() as u32,
        );
        let (src, dst) = (Crs::from_epsg(s).unwrap(), Crs::from_epsg(d).unwrap());
        let t = Transform::with_grids(&src, &dst, AxisOrder::Authority, Grids::None).unwrap();
        for point in r["points"].as_array().unwrap() {
            let (input, at) = (numbers(&point["in"]), epoch(point));
            let context = format!("EPSG:{s} to EPSG:{d} at {at}");
            if point["out"].is_null() {
                continue;
            }
            let want = numbers(&point["out"]);
            // The epoch has no say in which operation a coordinate takes.
            let name = t.operation_for(input[0], input[1], input.get(2).copied());
            if same_operation {
                assert_eq!(name, point["op"].as_str(), "{context}");
            } else if name != point["op"].as_str() {
                other += 1;
                continue;
            }
            assert_close(&dst, &convert_at(&t, &input, at), &want, ROUTE_M, &context);
            // The same epoch on the transform, for the CRS that takes one.
            if at.is_finite() {
                let with = if src.is_dynamic() {
                    t.with_epochs(Some(at), None)
                } else {
                    t.with_epochs(None, Some(at))
                }
                .unwrap_or_else(|e| panic!("{context}: {e}"));
                let (x, y, z) = with
                    .convert(input[0], input[1], input.get(2).copied())
                    .unwrap();
                let got: Vec<f64> = [Some(x), Some(y), z].into_iter().flatten().collect();
                assert_close(&dst, &got, &want, ROUTE_M, &context);
            }
        }
    }
    other
}

#[test]
fn chosen_operations_follow_the_epoch_of_each_coordinate() {
    check_pairs(&committed(), true);
}

/// `scripts/gen_transformation_vectors.py --epochs --all` first.
#[test]
#[ignore = "needs target/reprojection/epochs.jsonl"]
fn every_time_dependent_route_matches_proj() {
    let other = check_pairs(&all(), false);
    println!("{other} points through another operation than PROJ's");
}

/// ITRF2014 to GDA2020, latitude first, with `ITRF2014 to GDA2020 (1)`:
/// rates only, from 2020.
fn itrf2014_to_gda2020() -> Transform {
    Transform::from_epsg(7912, 7843, AxisOrder::Authority).unwrap()
}

const POINT: (f64, f64, f64) = (-25.0, 135.0, 0.0);

/// PROJ 9.8.1's `cs2cs --s_epoch 2030 EPSG:7912 EPSG:7843` for [`POINT`],
/// and what it gives without an epoch for the point at 2025.
const AT_2030: [f64; 3] = [-25.000_005_301, 134.999_996_938, 0.001_507_531];
const AT_2025: [f64; 3] = [-25.000_002_651, 134.999_998_469, 0.000_753_755];

fn assert_point(got: (f64, f64, Option<f64>), want: [f64; 3]) {
    let got = [got.0, got.1, got.2.unwrap()];
    assert!(
        (got[0] - want[0]).abs() < 1e-9
            && (got[1] - want[1]).abs() < 1e-9
            && (got[2] - want[2]).abs() < 1e-8,
        "{got:?}, PROJ {want:?}"
    );
}

#[test]
fn a_transform_epoch_is_every_coordinates_epoch() {
    let plain = itrf2014_to_gda2020();
    let (lat, lon, h) = POINT;
    // Without an epoch: at the reference epoch, where the rates do nothing.
    assert_point(plain.convert(lat, lon, Some(h)).unwrap(), [lat, lon, h]);
    assert_eq!((plain.source_epoch(), plain.epoch()), (None, None));
    let at_2030 = plain.with_epochs(Some(2030.0), None).unwrap();
    assert_eq!(at_2030.source_epoch(), Some(2030.0));
    assert_eq!(at_2030.target_epoch(), None);
    assert_eq!(at_2030.epoch(), Some(2030.0));
    assert_point(at_2030.convert(lat, lon, Some(h)).unwrap(), AT_2030);
    // The buffers and the diagnostics run at it too.
    let (mut x, mut y, mut z) = ([lat], [lon], [h]);
    at_2030.convert_xyz(&mut x, &mut y, &mut z).unwrap();
    assert_point((x[0], y[0], Some(z[0])), AT_2030);
    let trace = at_2030.trace_point(lat, lon, Some(h)).unwrap();
    let [x, y, z] = trace.steps.last().unwrap().coords;
    assert_point((x, y, Some(z)), AT_2030);
}

#[test]
fn a_target_epoch_works_like_a_source_epoch() {
    // PROJ 9.8.1's `cs2cs --t_epoch 2030 EPSG:7843 EPSG:7912`.
    let back = Transform::from_epsg(7843, 7912, AxisOrder::Authority)
        .unwrap()
        .with_epochs(None, Some(2030.0))
        .unwrap();
    assert_eq!(back.epoch(), Some(2030.0));
    let (lat, lon, h) = POINT;
    assert_point(
        back.convert(lat, lon, Some(h)).unwrap(),
        [-24.999_994_699, 135.000_003_062, -0.001_507_461],
    );
}

#[test]
fn the_transforms_epoch_replaces_a_coordinates() {
    let plain = itrf2014_to_gda2020();
    let at_2030 = plain.with_epochs(Some(2030.0), None).unwrap();
    let (lat, lon, h) = POINT;
    // On its own the coordinate's epoch counts.
    assert_point(
        plain.convert_at(lat, lon, Some(h), 2025.0).unwrap(),
        AT_2025,
    );
    assert!(!plain.overrides_epoch(2025.0));
    // PROJ's `cs2cs --s_epoch 2030` for the point at 2025: 2030's result.
    assert_point(
        at_2030.convert_at(lat, lon, Some(h), 2025.0).unwrap(),
        AT_2030,
    );
    assert!(at_2030.overrides_epoch(2025.0));
    // Nothing to override: the same epoch, or a coordinate without one.
    assert!(!at_2030.overrides_epoch(2030.0));
    assert!(!at_2030.overrides_epoch(f64::NAN));
    assert!(!at_2030.overrides_epoch(f64::INFINITY));
}

#[test]
fn an_epoch_that_is_not_finite_is_none() {
    let plain = itrf2014_to_gda2020();
    let (lat, lon, h) = POINT;
    let without = plain.convert(lat, lon, Some(h)).unwrap();
    for none in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(plain.convert_at(lat, lon, Some(h), none).unwrap(), without);
    }
}

#[test]
fn buffers_take_an_epoch_per_coordinate() {
    let plain = itrf2014_to_gda2020();
    let (lat, lon, h) = POINT;
    let epochs = [2025.0, f64::NAN, 2030.0];
    let (mut x, mut y, mut z) = ([lat; 3], [lon; 3], [h; 3]);
    plain.convert_xyzt(&mut x, &mut y, &mut z, &epochs).unwrap();
    assert_point((x[0], y[0], Some(z[0])), AT_2025);
    assert_point((x[1], y[1], Some(z[1])), [lat, lon, h]);
    assert_point((x[2], y[2], Some(z[2])), AT_2030);
    // Without heights, between the geographic 2D CRSs.
    let flat = Transform::from_epsg(9000, 7844, AxisOrder::Authority).unwrap();
    let (mut x, mut y) = ([lat; 3], [lon; 3]);
    flat.convert_xyt(&mut x, &mut y, &epochs).unwrap();
    for (i, t) in epochs.iter().enumerate() {
        let (want_x, want_y, _) = flat.convert_at(lat, lon, None, *t).unwrap();
        assert_eq!((x[i], y[i]), (want_x, want_y));
    }
    assert!((x[0] - AT_2025[0]).abs() < 1e-9 && (y[0] - AT_2025[1]).abs() < 1e-9);
}

#[test]
fn the_inverse_takes_the_epochs_with_their_crss() {
    let at_2030 = itrf2014_to_gda2020()
        .with_epochs(Some(2030.0), None)
        .unwrap();
    let back = at_2030.inverse().unwrap();
    assert_eq!(
        (back.source_epoch(), back.target_epoch()),
        (None, Some(2030.0))
    );
    let (lat, lon, h) = POINT;
    let there = at_2030.convert(lat, lon, Some(h)).unwrap();
    // Back where it started, as far as PROJ's reverse Helmert goes.
    let (lat2, lon2, h2) = back.convert(there.0, there.1, there.2).unwrap();
    assert!((lat2 - lat).abs() < 1e-11 && (lon2 - lon).abs() < 1e-11);
    assert!((h2.unwrap() - h).abs() < 1e-6);
    // One operation of the choice keeps the epoch too.
    let only = at_2030.with_operation_index(0).unwrap().unwrap();
    assert_eq!(only.epoch(), Some(2030.0));
    assert_point(only.convert(lat, lon, Some(h)).unwrap(), AT_2030);
    assert_eq!(only.inverse().unwrap().target_epoch(), Some(2030.0));
}

#[test]
fn only_a_dynamic_crs_takes_an_epoch() {
    // ITRF and WGS 84 realizations, and WGS 84 itself, as in PROJ.
    for dynamic in [7912, 7789, 9000, 4979, 4326, 32633] {
        assert!(Crs::from_epsg(dynamic).unwrap().is_dynamic(), "{dynamic}");
    }
    // Plate-fixed and national CRSs.
    for fixed in [7843, 4258, 4937, 2180, 4269] {
        assert!(!Crs::from_epsg(fixed).unwrap().is_dynamic(), "{fixed}");
    }
    let plain = itrf2014_to_gda2020();
    // PROJ: "Coordinate epoch should not be provided for a static CRS".
    let error = plain.with_epochs(None, Some(2030.0)).unwrap_err();
    assert!(
        matches!(&error, Error::Epoch { crs, .. } if crs == "EPSG:7843"),
        "{error}"
    );
    assert!(error.to_string().contains("static CRS"), "{error}");
    let static_source = Transform::from_epsg(4258, 2180, AxisOrder::EastFirst).unwrap();
    assert!(matches!(
        static_source.with_epochs(Some(2020.0), None),
        Err(Error::Epoch { .. })
    ));
    // Not a year.
    for bad in [f64::NAN, f64::INFINITY, 0.0, -2020.0] {
        assert!(
            matches!(plain.with_epochs(Some(bad), None), Err(Error::Epoch { .. })),
            "{bad}"
        );
    }
    // No epochs is no change.
    assert_eq!(plain.with_epochs(None, None).unwrap().epoch(), None);
}

#[test]
fn two_epochs_without_a_point_motion_operation_are_refused() {
    // ITRF2014 to ITRF2008: both dynamic, and EPSG has no point motion
    // operation for either. PROJ leaves the coordinates at the source's
    // epoch.
    let t = Transform::from_epsg(7912, 7911, AxisOrder::Authority).unwrap();
    let same = t.with_epochs(Some(2030.0), Some(2030.0)).unwrap();
    assert_eq!(
        (same.source_epoch(), same.target_epoch()),
        (Some(2030.0), Some(2030.0))
    );
    let error = t.with_epochs(Some(2030.0), Some(2025.0)).unwrap_err();
    assert!(matches!(error, Error::Epoch { .. }), "{error}");
    assert!(error.to_string().contains("point motion"), "{error}");
    // PROJ's rounding: within a millionth of a year of a thousandth.
    let rounded = t.with_epochs(Some(2030.0000004), Some(2030.0)).unwrap();
    assert_eq!(rounded.source_epoch(), Some(2030.0));
    assert_eq!(
        t.with_epochs(Some(2030.0004), None).unwrap().epoch(),
        Some(2030.0004)
    );
}

#[test]
fn an_epoch_changes_nothing_without_a_time_dependent_operation() {
    // WGS 84 to ETRS89 / PL-1992: a Helmert of zeros and a projection.
    let plain = Transform::from_epsg(4326, 2180, AxisOrder::EastFirst).unwrap();
    assert!(plain.operation_info().iter().all(|o| !o.time_dependent));
    let at = plain.with_epochs(Some(2024.5), None).unwrap();
    assert_eq!(
        at.convert(21.0, 52.2, None).unwrap(),
        plain.convert(21.0, 52.2, None).unwrap()
    );
    assert_eq!(
        plain.convert_at(21.0, 52.2, None, 2024.5).unwrap(),
        plain.convert(21.0, 52.2, None).unwrap()
    );
    // On one datum: a conversion, with nothing that moves.
    let conversion = Transform::from_epsg(4326, 32633, AxisOrder::EastFirst)
        .unwrap()
        .with_epochs(Some(2024.5), Some(2024.5))
        .unwrap();
    let (x, y, _) = conversion.convert(15.0, 52.0, None).unwrap();
    let (px, py, _) = Transform::from_epsg(4326, 32633, AxisOrder::EastFirst)
        .unwrap()
        .convert(15.0, 52.0, None)
        .unwrap();
    assert_eq!((x, y), (px, py));
}

// Deformation models and point motion operations: what needs an epoch and
// a grid. `epochs-grids.jsonl` is PROJ's on the grids of
// `grids/cropped` (`scripts/gen_grid_fixtures.py`): PROJ-data's files cut
// down to a window, under their own names. The grid store is the process's
// and goes by name, so the tests on the whole files (`--ignored`) run in a
// process of their own: `cargo test -- --ignored`, not `--include-ignored`.

/// The tolerance of what follows, run with PROJ's steps on PROJ's grids: a
/// micrometre (the full sets agree to 2e-8 m and 2e-13 degrees).
const GRID_M: f64 = 1e-6;

/// ETRF2014 to NKG_ETRF14 (1), velocities from the coordinate's epoch to
/// 2000.0, is PROJ's `deformation` backwards, whose iteration steps away
/// from the solution: 2e-5 m off at 2030 (8e-5 m at 2060), where xeibe's
/// and the first estimate agree to 6e-8 m (`docs/reprojection.md`, level
/// 3's deviations). Routes through it are compared to a tenth of a
/// millimetre, as are the other routes of a transform that has one.
const TO_NKG_ETRF14: u32 = 10809;
const REVERSE_VELOCITY_M: f64 = 1e-4;

/// The tolerance for a transform's routes.
fn grid_tolerance(t: &Transform) -> f64 {
    let through = |o: &xeibe_reproject::OperationInfo| {
        o.code == Some((TO_NKG_ETRF14, false))
            || o.code == Some((TO_NKG_ETRF14, true))
            || o.steps.iter().any(|&(code, _)| code == TO_NKG_ETRF14)
    };
    if t.operation_info().iter().any(through) {
        REVERSE_VELOCITY_M
    } else {
        GRID_M
    }
}

/// The cropped grids, and PROJ's vectors on them.
fn cropped() -> PathBuf {
    xeibe_testkit::data_dir().join("reprojection/grids/cropped")
}

fn committed_grids() -> PathBuf {
    xeibe_testkit::data_dir().join("reprojection/epochs-grids.jsonl")
}

/// `--all --epochs --grids target/proj-grids`, and the grids there (or in
/// `XEIBE_GRID_DIR`).
fn all_grids() -> (PathBuf, PathBuf) {
    let vectors = all().with_file_name("epochs-grids.jsonl");
    let grids = match std::env::var_os("XEIBE_GRID_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/proj-grids"),
    };
    (vectors, grids)
}

fn code(v: &Value) -> u32 {
    v.as_u64().unwrap() as u32
}

/// A transform's grids, whole.
fn loaded(t: Transform, dir: &Path) -> Transform {
    t.load_grids_from_dir(dir, None)
        .unwrap_or_else(|e| panic!("{:?}: {e}", t.operations()));
    t
}

/// Each operation that needs a coordinate epoch and a grid (deformation
/// models, velocities to a target epoch) forced, forwards and back. A
/// point without an epoch has no result in PROJ, and is an error here. Returns how many
/// points were compared.
fn check_model_operations(path: &Path, dir: &Path) -> usize {
    let records = records(path, "operation");
    assert!(!records.is_empty());
    let mut compared = 0;
    for r in &records {
        let op = code(&r["op"]);
        let src = Crs::from_epsg(code(&r["src"])).unwrap();
        let dst = Crs::from_epsg(code(&r["dst"])).unwrap();
        let via =
            |a: &Crs, b: &Crs| loaded(Transform::via(a, b, op, AxisOrder::Authority).unwrap(), dir);
        let (forward, back) = (via(&src, &dst), via(&dst, &src));
        let tolerance = grid_tolerance(&forward);
        for point in r["points"].as_array().unwrap() {
            let (input, t) = (numbers(&point["in"]), epoch(point));
            let context = format!("EPSG:{op} at {t}");
            let got = forward.convert_at(input[0], input[1], input.get(2).copied(), t);
            if point["out"].is_null() || point["back"].is_null() {
                assert!(got.is_err(), "{context}: {got:?}, PROJ has no result");
                if t.is_nan() {
                    assert_eq!(got, Err(CoordinateError::MissingEpoch), "{context}");
                }
                continue;
            }
            let want = numbers(&point["out"]);
            assert_close(
                &dst,
                &convert_at(&forward, &input, t),
                &want,
                tolerance,
                &context,
            );
            let returned = convert_at(&back, &want, t);
            assert_close(
                &src,
                &returned,
                &numbers(&point["back"]),
                tolerance,
                &context,
            );
            compared += 1;
        }
    }
    compared
}

#[test]
fn a_deformation_model_matches_proj() {
    let compared = check_model_operations(&committed_grids(), &cropped());
    assert!(compared >= 20, "{compared} points");
}

/// `scripts/gen_transformation_vectors.py --all --epochs --grids
/// target/proj-grids` first; the grids are read from there too.
#[test]
#[ignore = "needs target/reprojection/epochs-grids.jsonl and target/proj-grids"]
fn a_whole_deformation_model_matches_proj() {
    let (vectors, grids) = all_grids();
    let compared = check_model_operations(&vectors, &grids);
    println!("{compared} points");
}

/// The automatic choice through an operation that needs a coordinate
/// epoch, each coordinate at its epoch; then the same with the epoch on
/// the transform. Without an epoch PROJ goes another way, if there is one.
fn check_model_pairs(path: &Path, dir: &Path) -> usize {
    let records = records(path, "pair");
    assert!(!records.is_empty());
    let mut compared = 0;
    for r in &records {
        let (s, d) = (code(&r["src"]), code(&r["dst"]));
        let (src, dst) = (Crs::from_epsg(s).unwrap(), Crs::from_epsg(d).unwrap());
        let t = Transform::with_grids(&src, &dst, AxisOrder::Authority, Grids::ProjData).unwrap();
        let t = loaded(t, dir);
        for point in r["points"].as_array().unwrap() {
            let (input, at) = (numbers(&point["in"]), epoch(point));
            let context = format!("EPSG:{s} to EPSG:{d} at {at}");
            let z = input.get(2).copied();
            if point["out"].is_null() {
                let got = t.convert_at(input[0], input[1], z, at);
                assert!(got.is_err(), "{context}: {got:?}, PROJ has no result");
                continue;
            }
            // A 2D target has no height: `convert` hands back the one
            // given, where PROJ's pipeline lets the model's shift through.
            let n = if dst.dimension() == 3 { input.len() } else { 2 };
            let want = &numbers(&point["out"])[..n];
            let tolerance = grid_tolerance(&t);
            assert_close(
                &dst,
                &convert_at(&t, &input, at)[..n],
                want,
                tolerance,
                &context,
            );
            // Without an epoch, where PROJ goes another way than through
            // the operations that need one: the same way.
            if at.is_nan() {
                assert_eq!(
                    t.operation_for(input[0], input[1], z),
                    point["op"].as_str(),
                    "{context}"
                );
                compared += 1;
                continue;
            }
            let with = if src.is_dynamic() {
                t.with_epochs(Some(at), None)
            } else {
                t.with_epochs(None, Some(at))
            }
            .unwrap_or_else(|e| panic!("{context}: {e}"));
            assert_eq!(
                with.operation_for(input[0], input[1], z),
                point["op"].as_str(),
                "{context}"
            );
            // An operation that needs the epoch isn't taken without one.
            let info = with.operation_info();
            if let Some(i) = with.operation_index_for(input[0], input[1], z)
                && info[i].needs_epoch
            {
                assert_ne!(
                    t.operation_for(input[0], input[1], z),
                    point["op"].as_str(),
                    "{context}"
                );
            }
            let (x, y, z) = with.convert(input[0], input[1], z).unwrap();
            let got: Vec<f64> = [Some(x), Some(y), z].into_iter().flatten().collect();
            assert_close(&dst, &got[..n], want, tolerance, &context);
            compared += 1;
        }
    }
    compared
}

#[test]
fn routes_through_a_deformation_model_match_proj() {
    let compared = check_model_pairs(&committed_grids(), &cropped());
    assert!(compared >= 100, "{compared} points");
}

/// As [`a_whole_deformation_model_matches_proj`].
#[test]
#[ignore = "needs target/reprojection/epochs-grids.jsonl and target/proj-grids"]
fn routes_through_a_whole_deformation_model_match_proj() {
    let (vectors, grids) = all_grids();
    let compared = check_model_pairs(&vectors, &grids);
    println!("{compared} points");
}

/// Between two coordinate epochs, through a point motion operation: PROJ's
/// `EPSG:8254@2010` to `EPSG:8254@2002`.
fn check_motion(path: &Path, dir: &Path) -> usize {
    let records = records(path, "motion");
    assert!(!records.is_empty());
    let mut compared = 0;
    for r in &records {
        let (s, d) = (code(&r["src"]), code(&r["dst"]));
        let (from, to) = (r["from"].as_f64().unwrap(), r["to"].as_f64().unwrap());
        let (src, dst) = (Crs::from_epsg(s).unwrap(), Crs::from_epsg(d).unwrap());
        let context = format!("EPSG:{s}@{from} to EPSG:{d}@{to}");
        let t = Transform::with_grids(&src, &dst, AxisOrder::Authority, Grids::ProjData)
            .unwrap()
            .with_epochs(Some(from), Some(to))
            .unwrap_or_else(|e| panic!("{context}: {e}"));
        let t = loaded(t, dir);
        assert!(
            t.operation_info().iter().all(|o| o.time_dependent),
            "{context}"
        );
        for point in r["points"].as_array().unwrap() {
            let input = numbers(&point["in"]);
            let z = input.get(2).copied();
            if point["out"].is_null() {
                continue;
            }
            assert_eq!(
                t.operation_for(input[0], input[1], z),
                point["op"].as_str(),
                "{context}"
            );
            let (x, y, z) = t
                .convert(input[0], input[1], z)
                .unwrap_or_else(|e| panic!("{context}, {input:?}: {e}"));
            let got: Vec<f64> = [Some(x), Some(y), z].into_iter().flatten().collect();
            assert_close(&dst, &got, &numbers(&point["out"]), GRID_M, &context);
            compared += 1;
        }
    }
    compared
}

#[test]
fn a_change_of_epoch_matches_proj() {
    let compared = check_motion(&committed_grids(), &cropped());
    assert!(compared >= 200, "{compared} points");
}

/// As [`a_whole_deformation_model_matches_proj`].
#[test]
#[ignore = "needs target/reprojection/epochs-grids.jsonl and target/proj-grids"]
fn every_change_of_epoch_matches_proj() {
    let (vectors, grids) = all_grids();
    let compared = check_motion(&vectors, &grids);
    println!("{compared} points");
}

/// ITRF96 to NZGD2000, latitude first, through New Zealand's deformation
/// model, with its (cropped) grids.
fn itrf96_to_nzgd2000() -> Transform {
    loaded(
        Transform::from_epsg(7907, 4959, AxisOrder::Authority).unwrap(),
        &cropped(),
    )
}

/// North of Kaikoura, inside the cropped grids.
const KAIKOURA: (f64, f64, f64) = (-42.63, 173.01, 10.0);

/// PROJ 9.8.1 on the whole model, for [`KAIKOURA`] at 2011.3 (between the
/// second and third Christchurch earthquakes' patches) and at 2020.
const KAIKOURA_2011: [f64; 3] = [
    -42.630_004_788_706_56,
    173.010_016_070_549_37,
    9.648_730_786_288_402,
];
const KAIKOURA_2020: [f64; 3] = [-42.630_003_992_318_97, 173.010_007_238_755_86, 10.0];

/// Within a hundredth of a millimetre.
fn assert_exact(got: (f64, f64, Option<f64>), want: [f64; 3]) {
    let got = [got.0, got.1, got.2.unwrap()];
    assert!(
        (got[0] - want[0]).abs() < 1e-10
            && (got[1] - want[1]).abs() < 1e-10
            && (got[2] - want[2]).abs() < 1e-5,
        "{got:?}, PROJ {want:?}"
    );
}

#[test]
fn a_deformation_model_needs_an_epoch() {
    let t = itrf96_to_nzgd2000();
    let (lat, lon, h) = KAIKOURA;
    let info = t.operation_info();
    assert_eq!(info.len(), 1);
    assert_eq!(info[0].name, "ITRF96 to NZGD2000 (7)");
    assert_eq!(info[0].steps, [(9090, false)]);
    assert_eq!(info[0].grids, ["nz_linz_nzgd2000-20180701.json"]);
    assert!(info[0].time_dependent && info[0].needs_epoch);
    // A coordinate at an epoch: the cropped grids give what PROJ reads
    // from the whole ones.
    assert_exact(
        t.convert_at(lat, lon, Some(h), 2011.3).unwrap(),
        KAIKOURA_2011,
    );
    assert_exact(
        t.convert_at(lat, lon, Some(h), 2020.0).unwrap(),
        KAIKOURA_2020,
    );
    // Without one there is no result, as in PROJ, where a time-dependent
    // Helmert runs at its reference epoch.
    assert_eq!(
        t.convert(lat, lon, Some(h)),
        Err(CoordinateError::MissingEpoch)
    );
    assert_eq!(t.operation_for(lat, lon, Some(h)), None);
    let explained = t.explain_point(lat, lon, Some(h)).unwrap();
    assert_eq!(explained.error, Some(CoordinateError::MissingEpoch));
    // In buffers, the coordinate without an epoch is the one that fails.
    let (mut x, mut y, mut z) = ([lat; 2], [lon; 2], [h; 2]);
    let error = t
        .convert_xyzt(&mut x, &mut y, &mut z, &[2020.0, f64::NAN])
        .unwrap_err();
    assert!(
        matches!(
            error,
            Error::Coordinate {
                index: 1,
                reason: CoordinateError::MissingEpoch,
                ..
            }
        ),
        "{error}"
    );
    // The model covers 1900 to 2050.
    for outside in [1899.9, 2050.1] {
        assert_eq!(
            t.convert_at(lat, lon, Some(h), outside),
            Err(CoordinateError::EpochOutOfRange),
            "{outside}"
        );
    }
    // Outside the model (the cropped one ends a few kilometres away).
    assert_eq!(
        t.convert_at(-41.0, 174.0, Some(h), 2020.0),
        Err(CoordinateError::OutsideGrid)
    );
}

#[test]
fn a_deformation_model_takes_the_transforms_epoch() {
    let t = itrf96_to_nzgd2000();
    let (lat, lon, h) = KAIKOURA;
    // ITRF96 is the dynamic CRS, NZGD2000 the static one.
    let at_2020 = t.with_epochs(Some(2020.0), None).unwrap();
    assert_exact(at_2020.convert(lat, lon, Some(h)).unwrap(), KAIKOURA_2020);
    assert_eq!(
        at_2020.operation_for(lat, lon, Some(h)),
        Some("ITRF96 to NZGD2000 (7)")
    );
    let trace = at_2020.trace_point(lat, lon, Some(h)).unwrap();
    let [x, y, z] = trace.steps.last().unwrap().coords;
    assert_exact((x, y, Some(z)), KAIKOURA_2020);
    assert!(matches!(
        t.with_epochs(None, Some(2020.0)),
        Err(Error::Epoch { .. })
    ));
    // Back, with the epoch on the target: where it started, as closely as
    // the model's iteration ends (PROJ's: a tenth of a nanoradian).
    let back = at_2020.inverse().unwrap();
    assert_eq!(
        (back.source_epoch(), back.target_epoch()),
        (None, Some(2020.0))
    );
    let (lat2, lon2, h2) = back
        .convert(KAIKOURA_2020[0], KAIKOURA_2020[1], Some(KAIKOURA_2020[2]))
        .unwrap();
    assert!(
        (lat2 - lat).abs() < 1e-9 && (lon2 - lon).abs() < 1e-9,
        "{lat2} {lon2}"
    );
    assert!((h2.unwrap() - h).abs() < 1e-6);
}

/// NAD83(CSRS)v7 from one coordinate epoch to another, latitude first,
/// with the (cropped) velocity grid.
fn csrs_v7(from: f64, to: f64) -> Transform {
    let t = Transform::from_epsg(8254, 8254, AxisOrder::Authority)
        .unwrap()
        .with_epochs(Some(from), Some(to))
        .unwrap();
    loaded(t, &cropped())
}

/// In Manitoba, inside the cropped grid.
const BRANDON: (f64, f64, f64) = (50.0, -100.0, 10.0);

/// PROJ 9.8.1's `EPSG:8254@2010` to `EPSG:8254@2002` for [`BRANDON`], on
/// the whole grid: eight years of the plate's motion and of the land's
/// uplift, backwards.
const BRANDON_2002: [f64; 3] = [
    50.000_000_072_840_71,
    -100.000_000_271_951_85,
    10.013_936_479_575_932,
];

#[test]
fn two_epochs_move_the_coordinates_in_time() {
    let (lat, lon, h) = BRANDON;
    // On one CRS and without epochs there is nothing to do.
    let plain = Transform::from_epsg(8254, 8254, AxisOrder::Authority).unwrap();
    assert!(plain.operations().is_empty());
    assert_eq!(
        plain.convert(lat, lon, Some(h)).unwrap(),
        (lat, lon, Some(h))
    );
    // NAD83(CSRS)v7 is a static CRS that takes an epoch: EPSG has a point
    // motion operation for it. One epoch, or the same one twice, moves
    // nothing.
    assert!(!Crs::from_epsg(8254).unwrap().is_dynamic());
    for (from, to) in [
        (Some(2010.0), None),
        (None, Some(2002.0)),
        (Some(2010.0), Some(2010.0)),
    ] {
        let t = plain.with_epochs(from, to).unwrap();
        assert!(t.operations().is_empty());
        assert_eq!(t.convert(lat, lon, Some(h)).unwrap(), (lat, lon, Some(h)));
    }
    let t = csrs_v7(2010.0, 2002.0);
    assert_eq!(
        (t.source_epoch(), t.target_epoch()),
        (Some(2010.0), Some(2002.0))
    );
    assert_eq!(
        t.operations(),
        ["Canada velocity grid v7 from epoch 2010 to epoch 2002"]
    );
    let info = t.operation_info();
    assert_eq!(info[0].steps, [(9483, false)]);
    assert_eq!(info[0].grids, ["ca_nrc_NAD83v70VG.tif"]);
    assert_eq!(info[0].accuracy, Some(0.01));
    // It depends on the transform's two epochs, and needs none more.
    assert!(info[0].time_dependent && !info[0].needs_epoch);
    assert_eq!(t.grids(), ["ca_nrc_NAD83v70VG.tif"]);
    assert_exact(t.convert(lat, lon, Some(h)).unwrap(), BRANDON_2002);
    // A coordinate's own epoch is replaced, as by any epoch of a transform.
    assert!(t.overrides_epoch(2015.0));
    assert_exact(
        t.convert_at(lat, lon, Some(h), 2015.0).unwrap(),
        BRANDON_2002,
    );
    // The diagnostics run the same steps.
    assert_eq!(t.operation_for(lat, lon, Some(h)), Some(t.operations()[0]));
    let trace = t.trace_point(lat, lon, Some(h)).unwrap();
    let [x, y, z] = trace.steps.last().unwrap().coords;
    assert_exact((x, y, Some(z)), BRANDON_2002);
    // Outside the velocity grid there is no result.
    assert_eq!(
        t.convert(60.0, -120.0, Some(h)),
        Err(CoordinateError::OutsideGrid)
    );
}

#[test]
fn a_change_of_epoch_goes_back_with_the_epochs_exchanged() {
    let (lat, lon, h) = BRANDON;
    let t = csrs_v7(2010.0, 2002.0);
    let back = t.inverse().unwrap();
    assert_eq!(
        (back.source_epoch(), back.target_epoch()),
        (Some(2002.0), Some(2010.0))
    );
    assert_eq!(
        back.operations(),
        ["Canada velocity grid v7 from epoch 2002 to epoch 2010"]
    );
    // The velocity is read where the coordinate is, so the way back ends
    // where the way out started only as far as the velocity is the same
    // 2 cm away: far below a micrometre.
    let (lat2, lon2, h2) = back
        .convert(BRANDON_2002[0], BRANDON_2002[1], Some(BRANDON_2002[2]))
        .unwrap();
    assert!(
        (lat2 - lat).abs() < 1e-11 && (lon2 - lon).abs() < 1e-11,
        "{lat2} {lon2}"
    );
    assert!((h2.unwrap() - h).abs() < 1e-6);
    // With a transformation before the motion: ITRF2014 at 2010 to
    // NAD83(CSRS)v7 at 2002, and one operation of the choice alone.
    let through = Transform::from_epsg(7912, 8254, AxisOrder::Authority)
        .unwrap()
        .with_epochs(Some(2010.0), Some(2002.0))
        .unwrap();
    let through = loaded(through, &cropped());
    // PROJ 9.8.1, on the whole grid.
    let want = [
        49.999_992_534_258_915,
        -99.999_986_268_927_4,
        10.700_481_265_783_31,
    ];
    assert_exact(through.convert(lat, lon, Some(h)).unwrap(), want);
    let only = through.with_operation_index(0).unwrap().unwrap();
    assert_eq!(only.operations(), through.operations());
    assert_eq!(
        (only.source_epoch(), only.target_epoch()),
        (Some(2010.0), Some(2002.0))
    );
    assert_exact(only.convert(lat, lon, Some(h)).unwrap(), want);
    let home = through.inverse().unwrap();
    assert_eq!(
        home.operations()[0],
        "Canada velocity grid v7 from epoch 2002 to epoch 2010 \
         + Conversion from NAD83(CSRS)v7 (geog3D) to NAD83(CSRS)v7 (geocentric) \
         + Inverse of ITRF2014 to NAD83(CSRS)v7 (1) \
         + Conversion from ITRF2014 (geocentric) to ITRF2014 (geog3D)"
    );
    let (lat2, lon2, h2) = home.convert(want[0], want[1], Some(want[2])).unwrap();
    assert!(
        (lat2 - lat).abs() < 1e-10 && (lon2 - lon).abs() < 1e-10,
        "{lat2} {lon2}"
    );
    assert!((h2.unwrap() - h).abs() < 1e-5);
    // Without epochs again: the plain transformation.
    let plain = through.with_epochs(None, None).unwrap();
    assert_eq!(
        plain.operations(),
        Transform::from_epsg(7912, 8254, AxisOrder::Authority)
            .unwrap()
            .operations()
    );
}

#[test]
fn a_change_of_epoch_is_made_where_proj_leaves_it_out() {
    // PROJ puts the point motion operation in only when it looks for the
    // CRSs of each datum: between the two geocentric CRSs, which one EPSG
    // transformation joins, `EPSG:7789@2010` to `EPSG:8253@2002` is the
    // Helmert alone, at 2010. And it crashes from a projected CRS. Here
    // both are the route between the geographic CRSs, which PROJ makes.
    let (lat, lon, h) = BRANDON;
    let geographic = Transform::from_epsg(7912, 8254, AxisOrder::Authority)
        .unwrap()
        .with_epochs(Some(2010.0), Some(2002.0))
        .unwrap();
    let geographic = loaded(geographic, &cropped());
    let want = geographic.convert(lat, lon, Some(h)).unwrap();
    // ITRF2014 and NAD83(CSRS)v7, geocentric.
    let to_xyz = Transform::from_epsg(7912, 7789, AxisOrder::Authority).unwrap();
    let from_xyz = Transform::from_epsg(8253, 8254, AxisOrder::Authority).unwrap();
    let geocentric = Transform::from_epsg(7789, 8253, AxisOrder::Authority)
        .unwrap()
        .with_epochs(Some(2010.0), Some(2002.0))
        .unwrap();
    let geocentric = loaded(geocentric, &cropped());
    assert_eq!(
        geocentric.operations(),
        ["ITRF2014 to NAD83(CSRS)v7 (1) \
          + Conversion from NAD83(CSRS)v7 (geocentric) to NAD83(CSRS)v7 (geog3D) \
          + Canada velocity grid v7 from epoch 2010 to epoch 2002 \
          + Conversion from NAD83(CSRS)v7 (geog3D) to NAD83(CSRS)v7 (geocentric)"]
    );
    let (x, y, z) = to_xyz.convert(lat, lon, Some(h)).unwrap();
    let (x, y, z) = geocentric.convert(x, y, z).unwrap();
    let got = from_xyz.convert(x, y, z).unwrap();
    assert!(
        (got.0 - want.0).abs() < 1e-10
            && (got.1 - want.1).abs() < 1e-10
            && (got.2.unwrap() - want.2.unwrap()).abs() < 1e-5,
        "{got:?}, {want:?}"
    );
    // From NAD83(CSRS)v7 / UTM zone 14N at 2010 to the geographic CRS at
    // 2002: the projection undone, then the motion.
    let utm = Transform::from_epsg(8254, 22714, AxisOrder::Authority).unwrap();
    let (e, n, _) = utm.convert(lat, lon, None).unwrap();
    let projected = Transform::from_epsg(22714, 8255, AxisOrder::Authority)
        .unwrap()
        .with_epochs(Some(2010.0), Some(2002.0))
        .unwrap();
    let projected = loaded(projected, &cropped());
    let flat = Transform::from_epsg(8255, 8255, AxisOrder::Authority)
        .unwrap()
        .with_epochs(Some(2010.0), Some(2002.0))
        .unwrap();
    let flat = loaded(flat, &cropped());
    let got = projected.convert(e, n, None).unwrap();
    let want = flat.convert(lat, lon, None).unwrap();
    assert!(
        (got.0 - want.0).abs() < 1e-10 && (got.1 - want.1).abs() < 1e-10,
        "{got:?}, {want:?}"
    );
}

#[test]
fn a_change_of_epoch_that_cannot_be_made_is_an_error() {
    let v7 = Crs::from_epsg(8254).unwrap();
    // The grid isn't to be had: with no grids at all.
    let without = Transform::with_grids(&v7, &v7, AxisOrder::Authority, Grids::None).unwrap();
    let error = without.with_epochs(Some(2010.0), Some(2002.0)).unwrap_err();
    assert!(matches!(error, Error::Epoch { .. }), "{error}");
    assert!(
        error
            .to_string()
            .contains("Canada velocity grid v7 (EPSG:9483)")
            && error.to_string().contains("not available"),
        "{error}"
    );
    // One epoch needs no grid.
    assert!(without.with_epochs(Some(2010.0), None).is_ok());
    // From or to a Bound CRS.
    let bound =
        Crs::from_user_input("+proj=longlat +datum=WGS84 +towgs84=0,0,0 +type=crs").unwrap();
    let error = Transform::new(&v7, &bound, AxisOrder::Authority)
        .unwrap()
        .with_epochs(Some(2010.0), Some(2002.0))
        .unwrap_err();
    assert!(error.to_string().contains("bound CRS"), "{error}");
    // EPSG's other point motion operations: Canada's latest, whose grid
    // PROJ-data doesn't have, and Indonesia's and New Caledonia's, by grids
    // of geocentric velocities that no one publishes in a form PROJ reads.
    for (crs, operation, why) in [
        (
            10413,
            "Canada velocity grid v8 (EPSG:10707)",
            "NAD83v80VG.gvb is not available",
        ),
        (
            9468,
            "Indonesian Deformation Model 2020 (EPSG:9375)",
            "does not implement",
        ),
        (
            10308,
            "New Caledonia velocity model 2015 (EPSG:10323)",
            "does not implement",
        ),
    ] {
        let error = Transform::from_epsg(crs, crs, AxisOrder::Authority)
            .unwrap()
            .with_epochs(Some(2012.0), Some(2020.5))
            .unwrap_err()
            .to_string();
        assert!(error.contains(operation) && error.contains(why), "{error}");
    }
    // A static CRS without a point motion operation takes no epoch at all:
    // NAD83(CSRS), the ensemble of the realizations.
    let csrs = Transform::from_epsg(4617, 4617, AxisOrder::Authority).unwrap();
    assert!(matches!(
        csrs.with_epochs(Some(2010.0), Some(2002.0)),
        Err(Error::Epoch { .. })
    ));
}
