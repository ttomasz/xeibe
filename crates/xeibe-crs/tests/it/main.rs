//! The generated EPSG tables (`scripts/gen_crs_tables.py`).
//!
//! These assert properties of the *data*, not of the generator: that the codes
//! real GML files use are present, that the axis orders match what
//! `docs/geometry.md` records, and that the normalisations the EPSG terms
//! permit were actually applied.

use xeibe_crs::{CrsKind, FirstAxis, get, projjson};

#[test]
fn knows_which_crss_are_northing_or_latitude_first() {
    for code in [4326, 4258, 2180, 2176, 3301, 31256] {
        assert_eq!(get(code).unwrap().first_axis, FirstAxis::NorthOrLat, "EPSG:{code}");
    }
    for code in [25832, 3812, 2056, 3857, 32633] {
        assert_eq!(get(code).unwrap().first_axis, FirstAxis::EastOrLon, "EPSG:{code}");
    }
}

#[test]
fn axes_that_are_not_east_or_north_are_marked() {
    // Krovak is south-west; swapping alone does not normalize it.
    assert_eq!(get(2065).unwrap().first_axis, FirstAxis::Other);
}

#[test]
fn kinds_dimensions_and_units_are_recorded() {
    let wgs84 = get(4326).unwrap();
    assert_eq!(wgs84.kind, CrsKind::Geographic2d);
    assert!(wgs84.kind.is_geographic());
    assert_eq!(wgs84.dimension, 2);

    // The dimension rule uses this when srsDimension is missing.
    assert_eq!(get(4979).unwrap().dimension, 3);
    assert_eq!(get(4979).unwrap().kind, CrsKind::Geographic3d);

    let pl1992 = get(2180).unwrap();
    assert_eq!(pl1992.kind, CrsKind::Projected);
    assert_eq!(pl1992.linear_unit_m, Some(1.0), "metres");
    assert_eq!(pl1992.angular_unit_rad, None);
}

#[test]
fn a_compound_crs_reports_its_combined_dimension() {
    // Horizontal 2D + vertical 1D. Only the first two are ever swapped.
    let compound = get(5628).unwrap();
    assert_eq!(compound.kind, CrsKind::Compound);
    assert_eq!(compound.dimension, 3);
}

#[test]
fn deprecated_codes_are_kept_because_real_data_uses_them() {
    // EPSG:27582 is deprecated but appears in the corpus (mapsref.brgm.fr WFS).
    // EPSG publishes no WKT for deprecated CRSs, so this also proves the table
    // is built from the relational dataset rather than the WKT release.
    let ntf = get(27582).expect("EPSG:27582");
    assert!(ntf.deprecated);
    assert_eq!(ntf.kind, CrsKind::Projected);
    assert!(projjson(27582).is_some(), "deprecated CRSs still get PROJJSON");
}

#[test]
fn areas_of_use_are_epsg_bounds_and_reorder_to_axis_order() {
    let etrs89 = get(4258).unwrap();
    let [south, west, north, east] = etrs89.area_wgs84.expect("an area of use");
    assert!(south > 20.0 && north < 90.0, "latitude: {south}..{north}");
    assert!(west > -40.0 && east < 50.0, "longitude: {west}..{east}");

    // EPSG:4258 is latitude-first, so the axis-order view keeps that order.
    let [min_first, min_second, max_first, max_second] =
        etrs89.area_in_axis_order().expect("geographic CRSs have one");
    assert_eq!([min_first, min_second, max_first, max_second], [south, west, north, east]);

    let inside = |a: f32, b: f32| a >= min_first && a <= max_first && b >= min_second && b <= max_second;
    assert!(inside(52.23, 21.01), "Warsaw, latitude first");
    assert!(!inside(21.01, 52.23), "the swapped reading falls outside");

    // A projected CRS abstains rather than comparing metres against degrees.
    assert_eq!(get(2180).unwrap().area_in_axis_order(), None);
}

#[test]
fn the_crate_version_carries_the_epsg_version_as_build_metadata() {
    // `0.1.0+epsg-13.103`: the semver core is ours and means API compatibility,
    // the build metadata is the data's provenance. EPSG's own numbering cannot
    // be used as major.minor -- `13.005` is not a legal semver minor, `12.059a`
    // has a letter in it, and v13's parallel streams let the minor go
    // backwards. See `update_crate_version` in scripts/gen_crs_tables.py.
    let version = env!("CARGO_PKG_VERSION");
    let (_, metadata) = version.split_once('+').expect("build metadata naming the EPSG version");
    let expected = format!("epsg-{}", xeibe_crs::EPSG_VERSION.replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', "-"));
    assert_eq!(metadata, expected, "crate version and table.rs disagree; rerun the generator");
}

#[test]
fn an_unknown_code_is_absent_rather_than_guessed() {
    assert_eq!(get(98765), None);
    assert_eq!(projjson(98765), None);
}

#[test]
fn projjson_is_parseable_and_identifies_its_own_code() {
    for code in [2180u32, 4326, 25832, 27700, 5514] {
        let text = projjson(code).unwrap_or_else(|| panic!("no PROJJSON for EPSG:{code}"));
        let doc: serde_json::Value = serde_json::from_str(text).expect("valid JSON");
        assert_eq!(doc["id"]["authority"], "EPSG", "EPSG:{code}");
        assert_eq!(doc["id"]["code"], code, "EPSG:{code}");
        assert!(doc["type"].as_str().unwrap().ends_with("CRS"), "EPSG:{code}");
    }
}

#[test]
fn sexagesimal_angles_were_decoded_and_relabelled() {
    // EPSG stores this as -58.3 in unit of measure 9110, meaning -58°30'.
    // Emitting it verbatim would be a silent 0.2° error, so this is the one
    // conversion most worth pinning down. See `docs/geometry.md`.
    let doc: serde_json::Value = serde_json::from_str(projjson(2009).unwrap()).unwrap();
    let params = doc["conversion"]["parameters"].as_array().unwrap();
    let longitude = params
        .iter()
        .find(|p| p["name"] == "Longitude of natural origin")
        .expect("a longitude of natural origin");
    assert_eq!(longitude["value"].as_f64().unwrap(), -58.5);
    assert_eq!(longitude["unit"], "degree", "relabelled, not left as sexagesimal");
}

#[test]
fn sphere_ellipsoids_use_the_radius_form() {
    // PROJJSON spells a sphere as a radius, not as equal semi-axes.
    let doc: serde_json::Value = serde_json::from_str(projjson(3408).unwrap()).unwrap();
    let ellipsoid = &doc["base_crs"]["datum"]["ellipsoid"];
    assert_eq!(ellipsoid["radius"].as_f64().unwrap(), 6371228.0);
    assert!(ellipsoid["semi_major_axis"].is_null());
}

#[test]
fn every_record_has_a_sane_shape() {
    const { assert!(CRS_LEN > 8000, "expected the whole EPSG dataset") };
    let mut previous = 0;
    for record in xeibe_crs::CRS {
        assert!(record.code > previous, "CRS must be sorted by code for binary search");
        previous = record.code;
        assert!(!record.name.is_empty(), "EPSG:{}", record.code);
        assert!(record.dimension >= 1 && record.dimension <= 4, "EPSG:{}", record.code);
        if let Some([south, west, north, east]) = record.area_wgs84 {
            assert!((-90.0..=90.0).contains(&south), "EPSG:{}", record.code);
            assert!((-90.0..=90.0).contains(&north), "EPSG:{}", record.code);
            assert!(south <= north, "EPSG:{}", record.code);
            // West may exceed east: an area of use can cross the antimeridian.
            assert!((-180.0..=180.0).contains(&west), "EPSG:{}", record.code);
            assert!((-180.0..=180.0).contains(&east), "EPSG:{}", record.code);
        }
    }
}

const CRS_LEN: usize = xeibe_crs::CRS.len();
