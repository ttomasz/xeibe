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
use xeibe_reproject::{AxisOrder, Crs, Error, Grids, Transform};

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
fn two_different_epochs_are_refused() {
    // ITRF2014 to ITRF2008: both dynamic.
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
