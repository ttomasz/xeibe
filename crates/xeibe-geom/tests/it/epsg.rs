//! The built-in CRS table (`docs/geometry.md`, "CRS axis order table").
//!
//! The expected axis orders are the ones GDAL prints for the same codes in the
//! sample reference output (`AXIS[...]` in `tests/data/samples/**/*.gdal.txt`).

use xeibe_geom::epsg::{CrsInfo, CrsTable, FirstAxis};

#[track_caller]
fn first_axis(code: &str) -> FirstAxis {
    CrsTable::builtin()
        .get("EPSG", code)
        .unwrap_or_else(|| panic!("EPSG:{code} is missing from the built-in table"))
        .first_axis
}

#[test]
fn knows_which_crss_are_northing_or_latitude_first() {
    for code in ["4326", "4258", "2180", "2176", "3301", "31256"] {
        assert_eq!(first_axis(code), FirstAxis::NorthOrLat, "EPSG:{code}");
    }
    for code in ["25832", "3812", "2056", "3857", "32633"] {
        assert_eq!(first_axis(code), FirstAxis::EastOrLon, "EPSG:{code}");
    }
}

#[test]
fn axes_that_are_not_east_or_north_are_marked() {
    // Krovak (South-West); swapping alone does not normalize these.
    assert_eq!(first_axis("2065"), FirstAxis::Other);
}

#[test]
fn geographic_crss_and_dimensions_are_recorded() {
    let table = CrsTable::builtin();
    let wgs84 = table.get("EPSG", "4326").expect("EPSG:4326");
    assert!(wgs84.geographic);
    assert_eq!(wgs84.dimension, 2);

    // A 3D geographic CRS: the dimension rule uses this when `srsDimension` is
    // missing (`docs/geometry.md`, "Dimension").
    let wgs84_3d = table.get("EPSG", "4979").expect("EPSG:4979");
    assert_eq!(wgs84_3d.dimension, 3);

    let projected = table.get("EPSG", "2180").expect("EPSG:2180");
    assert!(!projected.geographic);
    assert_eq!(projected.linear_unit_m, Some(1.0), "metres");
}

#[test]
fn areas_of_use_are_in_the_authority_axis_order() {
    // The range check compares coordinates as written against this.
    let table = CrsTable::builtin();
    let etrs89 = table.get("EPSG", "4258").expect("EPSG:4258");
    let area = etrs89.area_of_use.expect("an area of use");
    let [min_lat, min_lon, max_lat, max_lon] = area;
    assert!(min_lat > 20.0 && max_lat < 90.0, "latitude first: {area:?}");
    assert!(min_lon > -40.0 && max_lon < 50.0, "then longitude: {area:?}");
    // Warsaw is inside, and the swapped reading is not.
    let inside = |lat: f64, lon: f64| lat >= min_lat && lat <= max_lat && lon >= min_lon && lon <= max_lon;
    assert!(inside(52.23, 21.01));
    assert!(!inside(21.01, 52.23), "the swapped reading falls outside");
}

#[test]
fn an_unknown_code_is_absent_rather_than_guessed() {
    assert_eq!(CrsTable::builtin().get("EPSG", "98765"), None);
    assert_eq!(CrsTable::builtin().get("NOSUCH", "1"), None);
}

#[test]
fn user_entries_extend_and_override_the_built_in_table() {
    let entry = CrsInfo {
        first_axis: FirstAxis::EastOrLon,
        dimension: 2,
        geographic: false,
        area_of_use: None,
        linear_unit_m: Some(1.0),
    };
    let table = CrsTable::builtin().with_user_entries(vec![
        ("EPSG".into(), "2180".into(), entry.clone()),
        ("XX".into(), "1".into(), entry.clone()),
    ]);
    assert_eq!(
        table.get("EPSG", "2180").map(|info| info.first_axis),
        Some(FirstAxis::EastOrLon),
        "a user entry wins over the built-in one"
    );
    assert_eq!(table.get("XX", "1"), Some(entry));
    // Untouched codes still come from the built-in table.
    assert_eq!(
        table.get("EPSG", "4326").map(|info| info.first_axis),
        Some(FirstAxis::NorthOrLat)
    );
}
