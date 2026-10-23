//! The generated EPSG tables (`scripts/gen_crs_tables.py`).
//!
//! These assert properties of the *data*, not of the generator: that the codes
//! real GML files use are present, that the axis orders match what
//! `docs/geometry.md` records, and that the normalisations the EPSG terms
//! permit were actually applied.

use xeibe_crs::{CrsKind, FirstAxis, alias, crs_remarks, get, operation_remarks, projjson, usages};

#[test]
fn knows_which_crss_are_northing_or_latitude_first() {
    for code in [4326, 4258, 2180, 2176, 3301, 31256] {
        assert_eq!(
            get(code).unwrap().first_axis,
            FirstAxis::NorthOrLat,
            "EPSG:{code}"
        );
    }
    for code in [25832, 3812, 2056, 3857, 32633] {
        assert_eq!(
            get(code).unwrap().first_axis,
            FirstAxis::EastOrLon,
            "EPSG:{code}"
        );
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
    assert!(
        projjson(27582).is_some(),
        "deprecated CRSs still get PROJJSON"
    );
}

#[test]
fn areas_of_use_are_epsg_bounds_and_reorder_to_axis_order() {
    let etrs89 = get(4258).unwrap();
    let [south, west, north, east] = etrs89.area_wgs84.expect("an area of use");
    assert!(south > 20.0 && north < 90.0, "latitude: {south}..{north}");
    assert!(west > -40.0 && east < 50.0, "longitude: {west}..{east}");

    // EPSG:4258 is latitude-first, so the axis-order view keeps that order.
    let [min_first, min_second, max_first, max_second] = etrs89
        .area_in_axis_order()
        .expect("geographic CRSs have one");
    assert_eq!(
        [min_first, min_second, max_first, max_second],
        [south, west, north, east]
    );

    let inside =
        |a: f32, b: f32| a >= min_first && a <= max_first && b >= min_second && b <= max_second;
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
    let (_, metadata) = version
        .split_once('+')
        .expect("build metadata naming the EPSG version");
    let expected = format!(
        "epsg-{}",
        xeibe_crs::EPSG_VERSION.replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', "-")
    );
    assert_eq!(
        metadata, expected,
        "crate version and table.rs disagree; rerun the generator"
    );
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
        assert!(
            doc["type"].as_str().unwrap().ends_with("CRS"),
            "EPSG:{code}"
        );
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
    assert_eq!(
        longitude["unit"], "degree",
        "relabelled, not left as sexagesimal"
    );
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
        assert!(
            record.code > previous,
            "CRS must be sorted by code for binary search"
        );
        previous = record.code;
        assert!(!record.name.is_empty(), "EPSG:{}", record.code);
        assert!(
            record.dimension >= 1 && record.dimension <= 4,
            "EPSG:{}",
            record.code
        );
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

#[test]
fn aliases_name_one_crs_and_ignore_case() {
    // "Poland alternative identifier" in EPSG's alias table.
    assert_eq!(alias("PL-1992"), Some(2180));
    assert_eq!(alias("pl-evrf2007-nh"), Some(9651));
    assert_eq!(alias(" PL-KRON86 "), Some(9650));
    assert_eq!(alias("ETRS89-LAEA"), Some(3035));
    // EPSG gives this one to EPSG:9700, 9701 and 9702: no guess.
    assert_eq!(alias("PL-ETRF2000"), None);
    // ISO Geodetic Register codes are numbers, which read as EPSG codes.
    assert_eq!(alias("727"), None);
    assert_eq!(alias("PL-KRON86-NH"), None, "not EPSG's spelling");
}

#[test]
fn every_alias_names_a_crs_in_the_table() {
    for (alias_name, code) in xeibe_crs::ALIASES {
        assert!(get(*code).is_some(), "{alias_name} → {code}");
        assert_eq!(alias(alias_name), Some(*code), "{alias_name}");
    }
    assert!(
        xeibe_crs::ALIASES.windows(2).all(|w| w[0].0 < w[1].0),
        "sorted and unique"
    );
}

#[test]
fn methods_and_parameters_by_name() {
    use xeibe_crs::{method_code, parameter_code};
    assert_eq!(method_code("Transverse Mercator"), Some(9807));
    assert_eq!(method_code("lambert conic conformal (2SP)"), Some(9802));
    assert_eq!(parameter_code("False easting"), Some(8806));
    assert_eq!(parameter_code(" Latitude of natural origin "), Some(8801));
    // EPSG's aliases count too.
    assert_eq!(parameter_code("Central Meridian"), Some(8802));
    assert_eq!(method_code("not a method"), None);
    // Every conversion in the PROJJSON names its method and parameters in a
    // way the lookup gives back.
    for code in [2180, 3035, 3857, 5514, 27700, 32633] {
        let doc: serde_json::Value =
            serde_json::from_str(xeibe_crs::projjson(code).unwrap()).unwrap();
        let conv = &doc["conversion"];
        let id = |v: &serde_json::Value| v["id"]["code"].as_u64().map(|c| c as u32);
        assert_eq!(
            method_code(conv["method"]["name"].as_str().unwrap()),
            id(&conv["method"])
        );
        for p in conv["parameters"].as_array().unwrap() {
            assert_eq!(parameter_code(p["name"].as_str().unwrap()), id(p), "{code}");
        }
    }
}

#[test]
fn datums_and_units_by_name() {
    let d = xeibe_crs::datum(6258).unwrap();
    assert_eq!(
        d.name,
        "European Terrestrial Reference System 1989 ensemble"
    );
    assert_eq!(d.kind, xeibe_crs::DatumKind::Geodetic);
    assert_eq!(
        xeibe_crs::datum(5100).unwrap().kind,
        xeibe_crs::DatumKind::Vertical
    );
    assert!(xeibe_crs::datum(6747).unwrap().deprecated, "Greenland 1996");
    assert!(xeibe_crs::DATUMS.windows(2).all(|w| w[0].code < w[1].code));
    assert!(
        xeibe_crs::DATUM_ALIASES
            .iter()
            .any(|&(code, alias)| code == 6258 && alias == "ETRS89")
    );
    assert_eq!(xeibe_crs::unit_code("metre"), Some(9001));
    assert_eq!(xeibe_crs::unit_code("US survey foot"), Some(9003));
    assert_eq!(xeibe_crs::unit_code("grad"), Some(9105));
    assert_eq!(xeibe_crs::unit_code("Metre"), None, "exact names");
}

#[test]
fn operations_hold_the_registry() {
    let doc: serde_json::Value = serde_json::from_str(xeibe_crs::operations_json()).unwrap();
    assert_eq!(doc["epsg_version"], xeibe_crs::EPSG_VERSION);
    let ops = doc["operations"].as_array().unwrap();
    assert!(ops.len() > 3000, "{}", ops.len());
    // ETRS89 to WGS 84 (1): a null geocentric translation, good to 1 m.
    let op = ops.iter().find(|o| o["code"] == 1149).unwrap();
    assert_eq!(op["method"], 9603);
    assert_eq!(op["accuracy"], 1.0);
    assert_eq!(
        (op["src"].as_u64(), op["dst"].as_u64()),
        (Some(4258), Some(4326))
    );
}

#[test]
fn projjson_carries_areas_of_use_and_scopes_as_proj_writes_them() {
    // One usage: top-level scope, area and bbox.
    let doc: serde_json::Value = serde_json::from_str(projjson(2180).unwrap()).unwrap();
    assert_eq!(
        doc["scope"],
        "Topographic mapping (medium and small scale)."
    );
    assert!(doc["area"].as_str().unwrap().starts_with("Poland"));
    assert_eq!(doc["bbox"]["west_longitude"], 14.14);
    assert_eq!(doc["bbox"]["north_latitude"], 55.93);
    // Components and base CRSs have none, as in PROJ's output.
    let compound: serde_json::Value = serde_json::from_str(projjson(9707).unwrap()).unwrap();
    assert!(compound["scope"].is_string());
    assert!(compound["components"][0]["scope"].is_null());
    assert!(doc["base_crs"]["scope"].is_null());
    // Several usages: an array.
    let several = (2000..40000u32)
        .filter_map(projjson)
        .map(|t| serde_json::from_str::<serde_json::Value>(t).unwrap())
        .find(|d| d["usages"].is_array())
        .expect("a CRS with several usages");
    assert!(several["usages"].as_array().unwrap().len() > 1);
    assert!(several["scope"].is_null());
}

#[test]
fn remarks_are_epsg_text_by_code() {
    let crs = crs_remarks(2180).expect("EPSG:2180 has remarks");
    assert!(!crs.is_empty());
    // ETRS89 to WGS 84 (1).
    let op = operation_remarks(1149).expect("EPSG:1149 has remarks");
    assert!(op.contains("ETRS89"), "{op}");
    assert_eq!(crs_remarks(98765), None);
    assert_eq!(operation_remarks(98765), None);
}

#[test]
fn usages_give_area_scope_and_box() {
    let [poland] = usages(2180) else {
        panic!("EPSG:2180 has one usage")
    };
    assert_eq!(poland.area, Some("Poland - onshore and offshore."));
    assert_eq!(
        poland.scope,
        Some("Topographic mapping (medium and small scale).")
    );
    assert_eq!(poland.bbox, Some([49.0, 14.14, 55.93, 24.15]));
    assert!(usages(98765).is_empty());
    // What projjson writes is what usages gives.
    let doc: serde_json::Value = serde_json::from_str(projjson(2180).unwrap()).unwrap();
    assert_eq!(doc["area"], "Poland - onshore and offshore.");
    assert_eq!(doc["bbox"]["south_latitude"], 49.0);
}

#[test]
fn deprecated_crss_name_their_replacements() {
    // EPSG:4140 NAD83(CSRS98) was replaced by NAD83(CSRS), 4617.
    assert_eq!(xeibe_crs::replacements(4140).collect::<Vec<_>>(), [4617]);
    assert_eq!(xeibe_crs::replacements(4326).count(), 0);
    // A CRS replaced by several.
    assert!(xeibe_crs::replacements(26591).count() >= 1);
}

#[test]
#[allow(clippy::approx_constant)] // EPSG's pi
fn units_carry_their_factors() {
    let ftus = xeibe_crs::unit(9003).unwrap();
    assert_eq!(ftus.name, "US survey foot");
    assert_eq!(ftus.kind, xeibe_crs::UnitKind::Length);
    assert_eq!(ftus.factor, Some(12.0 / 39.37));
    // EPSG's degree is its 15-digit pi over 180, as in PROJ's database.
    assert_eq!(
        xeibe_crs::unit(9102).unwrap().factor,
        Some(3.14159265358979 / 180.0)
    );
    // Sexagesimal DMS has no factor.
    assert_eq!(xeibe_crs::unit(9110).unwrap().factor, None);
    assert!(
        xeibe_crs::UNITS_OF_MEASURE
            .windows(2)
            .all(|w| w[0].code < w[1].code)
    );
}
