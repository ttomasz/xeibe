//! Loading grids (`docs/reprojection.md`, level 3): what a transform asks
//! for, and the errors when a grid is missing. PROJ's results with the
//! committed grids are checked in `transformations.rs`.

use std::path::PathBuf;

use xeibe_reproject::{AxisOrder, CoordinateError, Crs, GridRequest, Grids, Transform};

fn grids_dir() -> PathBuf {
    xeibe_testkit::data_dir().join("reprojection/grids")
}

/// St. George Island, Alaska, latitude first.
const ST_GEORGE: (f64, f64) = (56.6, -169.55);

/// NADCON from St. George Island to NAD83 (EPSG:1457).
fn st_george() -> Transform {
    let (a, b) = (Crs::from_epsg(4138).unwrap(), Crs::from_epsg(4269).unwrap());
    Transform::via(&a, &b, 1457, AxisOrder::Authority).unwrap()
}

/// OSTN15 (EPSG:7709): a grid no test loads, since it isn't committed.
fn ostn15() -> Transform {
    let (a, b) = (Crs::from_epsg(4277).unwrap(), Crs::from_epsg(4258).unwrap());
    Transform::via(&a, &b, 7709, AxisOrder::EastFirst).unwrap()
}

#[test]
fn grids_are_named_by_proj_data() {
    let t = ostn15();
    assert_eq!(t.grids(), ["uk_os_OSTN15_NTv2_OSGBtoETRS.tif"]);
    let requests = t.grid_requests(None);
    // The start of the file first, for its directories.
    assert_eq!(
        requests,
        [GridRequest {
            file: "uk_os_OSTN15_NTv2_OSGBtoETRS.tif".into(),
            offset: 0,
            length: Some(16 * 1024),
        }]
    );
    assert_eq!(
        requests[0].url(),
        "https://cdn.proj.org/uk_os_OSTN15_NTv2_OSGBtoETRS.tif"
    );
}

#[test]
fn a_grid_that_is_not_loaded_is_an_error() {
    assert_eq!(
        ostn15().convert(-1.5, 52.5, None),
        Err(CoordinateError::GridNotLoaded)
    );
}

#[test]
fn loaded_grids_convert_and_ask_for_nothing_more() {
    let t = st_george();
    let (lat, lon) = ST_GEORGE;
    let bounds = [lat - 0.01, lon - 0.01, lat + 0.01, lon + 0.01];
    t.load_grids_from_dir(grids_dir(), Some(bounds)).unwrap();
    assert!(t.grid_requests(Some(bounds)).is_empty());
    let (y, x, _) = t.convert(lat, lon, None).unwrap();
    // NADCON moves St. George Island by about 200 m.
    assert!((y - lat).abs() < 0.01 && (x - lon).abs() < 0.01 && (x - lon).abs() > 1e-4);
}

/// Grid data stays in the process after the transform that loaded it is
/// gone: a new transform asks for nothing more.
#[test]
fn loaded_grids_outlive_their_transform() {
    let (lat, lon) = ST_GEORGE;
    let bounds = [lat - 0.01, lon - 0.01, lat + 0.01, lon + 0.01];
    st_george()
        .load_grids_from_dir(grids_dir(), Some(bounds))
        .unwrap();
    let t = st_george();
    assert!(t.grid_requests(Some(bounds)).is_empty());
    assert!(t.convert(lat, lon, None).is_ok());
}

#[test]
fn without_grids_only_parametric_operations_are_candidates() {
    let (a, b) = (Crs::from_epsg(4267).unwrap(), Crs::from_epsg(4269).unwrap());
    let with = Transform::new(&a, &b, AxisOrder::EastFirst).unwrap();
    assert!(!with.grids().is_empty());
    match Transform::with_grids(&a, &b, AxisOrder::EastFirst, Grids::None) {
        Ok(without) => assert!(without.grids().is_empty()),
        Err(e) => panic!("NAD27 to NAD83 has Helmert alternatives: {e}"),
    }
}

/// NADCON5's reverse is iterated, as EPSG describes it: it takes PROJ's
/// forward results back to their inputs. (PROJ, like NOAA's NCAT, keeps
/// the first estimate, about 5 cm off here.)
#[test]
fn the_nadcon5_reverse_inverts_the_forward() {
    let text = std::fs::read_to_string(
        xeibe_testkit::data_dir().join("reprojection/operations-grids.jsonl"),
    )
    .unwrap();
    let record: serde_json::Value = text
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|r| r["op"] == 8546)
        .expect("St. George Island to NAD83 (2), NADCON5");
    let (a, b) = (Crs::from_epsg(4138).unwrap(), Crs::from_epsg(4269).unwrap());
    let back = Transform::via(&b, &a, 8546, AxisOrder::Authority).unwrap();
    back.load_grids_from_dir(grids_dir(), None).unwrap();
    let mut checked = 0;
    for (input, out) in record["in"]
        .as_array()
        .unwrap()
        .iter()
        .zip(record["out"].as_array().unwrap())
    {
        let Some(out) = out.as_array() else { continue };
        let (lat, lon) = (out[0].as_f64().unwrap(), out[1].as_f64().unwrap());
        let (y, x, _) = back.convert(lat, lon, None).unwrap();
        let (dy, dx) = (
            y - input[0].as_f64().unwrap(),
            x - input[1].as_f64().unwrap(),
        );
        assert!(
            dy.abs() < 1e-10 && dx.abs() < 1e-10,
            "{out:?}: off by {dy:e}, {dx:e}"
        );
        checked += 1;
    }
    assert!(checked > 5);
}
