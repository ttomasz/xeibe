//! The operations a transform chooses among (`Transform::operation_info`),
//! forcing one of them (`with_operation_index`), and choosing among the
//! grids of a local copy of PROJ-data (`Grids::Local`).

use std::path::PathBuf;

use xeibe_reproject::{AxisOrder, Crs, Error, Grids, Transform};

fn grids_dir() -> PathBuf {
    xeibe_testkit::data_dir().join("reprojection/grids")
}

fn transform(src: u32, dst: u32, grids: Grids) -> Result<Transform, Error> {
    let (a, b) = (Crs::from_epsg(src)?, Crs::from_epsg(dst)?);
    Transform::with_grids(&a, &b, AxisOrder::Authority, grids)
}

/// PROJ's results for EPSG operation `op` on the committed grids:
/// (input, output) pairs, latitude first.
fn vectors(op: u64) -> Vec<([f64; 2], [f64; 2])> {
    let path = xeibe_testkit::data_dir().join("reprojection/operations-grids.jsonl");
    let text = std::fs::read_to_string(path).unwrap();
    let record: serde_json::Value = text
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|r| r["op"] == op)
        .unwrap();
    let pair = |v: &serde_json::Value| [v[0].as_f64().unwrap(), v[1].as_f64().unwrap()];
    record["in"]
        .as_array()
        .unwrap()
        .iter()
        .zip(record["out"].as_array().unwrap())
        .filter(|(_, out)| !out.is_null())
        .map(|(i, o)| (pair(i), pair(o)))
        .collect()
}

#[test]
fn operations_are_described_best_first() {
    let t = transform(4326, 2180, Grids::ProjData).unwrap();
    let info = t.operation_info();
    let names: Vec<&str> = info.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, t.operations());
    let best = &info[0];
    assert_eq!(best.name, "Inverse of ETRS89 to WGS 84 (1) + Poland CS92");
    // ETRS89 to WGS 84 (1), inverted; the projection isn't a transformation.
    assert_eq!(best.steps, [(1149, true)]);
    assert_eq!(best.code, None);
    assert_eq!(best.accuracy, Some(1.0));
    assert!(best.grids.is_empty() && !best.time_dependent);
    // A route the search put together has no EPSG scope of its own.
    assert_eq!(best.scope, None);
    let [w, s, e, n] = best.area.unwrap();
    assert!(
        w < 14.2 && s < 49.1 && e > 24.0 && n > 54.8,
        "{:?}",
        best.area
    );
}

#[test]
fn one_datum_has_no_operations() {
    let t = transform(4326, 3857, Grids::ProjData).unwrap();
    assert!(t.operation_info().is_empty());
    assert_eq!(t.operation_index_for(52.0, 21.0, None), None);
    assert!(t.with_operation_index(0).is_none());
}

#[test]
fn operations_name_their_grids() {
    let t = transform(4138, 4269, Grids::ProjData).unwrap();
    let info = t.operation_info();
    let by_code = |code: u32| info.iter().find(|o| o.code == Some((code, false))).unwrap();
    assert_eq!(by_code(1457).grids, ["us_noaa_stgeorge.tif"]);
    assert!(
        by_code(1457)
            .scope
            .as_deref()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(
        by_code(8546).grids,
        ["us_noaa_nadcon5_sg1952_nad83_1986_stgeorge.tif"]
    );
}

#[test]
fn one_operation_can_be_forced_both_ways() {
    let t = transform(4138, 4269, Grids::ProjData).unwrap();
    let index = t
        .operation_info()
        .iter()
        .position(|o| o.code == Some((1457, false)))
        .unwrap();
    let only = t.with_operation_index(index).unwrap().unwrap();
    assert_eq!(only.operations(), ["St. George Island to NAD83 (1)"]);
    only.load_grids_from_dir(grids_dir(), None).unwrap();
    let back = only.inverse().unwrap();
    assert_eq!(
        back.operations(),
        ["Inverse of St. George Island to NAD83 (1)"]
    );
    for (input, want) in vectors(1457) {
        let (y, x, _) = only.convert(input[0], input[1], None).unwrap();
        assert!((y - want[0]).abs() < 1e-11 && (x - want[1]).abs() < 1e-11);
        assert_eq!(only.operation_index_for(input[0], input[1], None), Some(0));
        let (y0, x0, _) = back.convert(y, x, None).unwrap();
        assert!((y0 - input[0]).abs() < 1e-9 && (x0 - input[1]).abs() < 1e-9);
    }
    assert!(t.with_operation_index(t.operation_info().len()).is_none());
}

#[test]
fn the_index_for_a_coordinate_names_the_operation_it_goes_through() {
    let t = transform(4138, 4269, Grids::ProjData).unwrap();
    t.load_grids_from_dir(grids_dir(), None).unwrap();
    let info = t.operation_info();
    let (input, _) = vectors(8546)[0];
    let index = t.operation_index_for(input[0], input[1], None).unwrap();
    assert_eq!(
        Some(info[index].name.as_str()),
        t.operation_for(input[0], input[1], None)
    );
}

#[test]
fn local_grids_limit_the_choice_to_their_operations() {
    // Only NADCON's file is local: NADCON5's operation drops out.
    let t = transform(4138, 4269, Grids::local(["us_noaa_stgeorge.tif"])).unwrap();
    assert_eq!(t.operations(), ["St. George Island to NAD83 (1)"]);
    t.load_grids_from_dir(grids_dir(), None).unwrap();
    let (input, want) = vectors(1457)[0];
    let (y, x, _) = t.convert(input[0], input[1], None).unwrap();
    assert!((y - want[0]).abs() < 1e-11 && (x - want[1]).abs() < 1e-11);
    // Both files local: both operations, as with the CDN.
    let both = Grids::local([
        "us_noaa_stgeorge.tif",
        "us_noaa_nadcon5_sg1952_nad83_1986_stgeorge.tif",
    ]);
    assert_eq!(
        transform(4138, 4269, both).unwrap().operations(),
        transform(4138, 4269, Grids::ProjData).unwrap().operations()
    );
}

#[test]
fn no_local_grids_is_no_grids() {
    let none = transform(4138, 4269, Grids::None).unwrap_err();
    let empty = transform(4138, 4269, Grids::local(Vec::<String>::new())).unwrap_err();
    assert_eq!(none, empty);
    assert!(matches!(none, Error::NoTransformation { .. }));
}
