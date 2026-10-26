use xeibe_wasm_reproject::page::{self, Options, Session};

#[test]
fn codes_are_numbers_with_or_without_the_prefix() {
    assert_eq!(page::parse_code("2180"), Ok(2180));
    assert_eq!(page::parse_code(" epsg:2180 "), Ok(2180));
    assert_eq!(page::parse_code("EPSG: 4326"), Ok(4326));
    for bad in [
        "",
        "EPSG:",
        "urn:ogc:def:crs:EPSG::2180",
        "2180.5",
        "PL-1992",
    ] {
        assert!(page::parse_code(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_card_has_the_axes_datum_and_projection() {
    let card = page::crs_card("EPSG:2180");
    println!("{card:#}");
    assert_eq!(card["name"], "ETRS89 / PL-1992");
    assert_eq!(card["kind"], "projected");
    assert_eq!(card["first_axis_north"], true);
    assert_eq!(card["axes"][0]["direction"], "north");
    assert_eq!(card["datum"]["ensemble"], true);
    assert_eq!(card["datum"]["ellipsoid"]["name"], "GRS 1980");
    assert_eq!(card["conversion"]["method"]["code"], 9807);
    assert!(card["error"].is_null());
}

#[test]
fn a_card_says_why_a_code_cant_be_used() {
    assert!(
        page::crs_card("1")
            .get("error")
            .is_some_and(|e| e.is_string())
    );
    assert!(page::crs_card("abc")["error"].is_string());
    // A vertical CRS alone has no position.
    assert!(page::crs_card("9651")["error"].is_string());
}

#[test]
fn one_datum_is_a_conversion() {
    let s = Session::new("4258", "2180", &Options::default()).unwrap();
    assert_eq!(s.info()["conversion"], true);
    let points = page::parse_points("[[52.2, 21.0], null, [91, 21]]").unwrap();
    let out = s.convert(&points);
    println!("{out:#}");
    let xy = out[0]["out"].as_array().unwrap();
    // PL-1992 is northing first.
    assert!((xy[0].as_f64().unwrap() - 483_432.2).abs() < 0.1, "{xy:?}");
    assert!((xy[1].as_f64().unwrap() - 636_640.5).abs() < 0.1, "{xy:?}");
    assert!(out[1].is_null());
    assert!(
        out[2]["error"]
            .as_str()
            .unwrap()
            .starts_with("out of range"),
        "{out}"
    );
}

#[test]
fn between_datums_the_candidates_are_listed_and_compared() {
    let s = Session::new("4326", "2180", &Options::default()).unwrap();
    let info = s.info();
    println!("{:#}", info["operations"][0]);
    let operations = info["operations"].as_array().unwrap();
    assert!(!operations.is_empty());
    let points = page::parse_points("[[52.2, 21.0]]").unwrap();
    let out = s.convert(&points);
    assert!(out[0]["op"].is_u64(), "{out}");
    let details = s.details((52.2, 21.0, None));
    println!("{details:#}");
    let candidates = details["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), operations.len());
    let chosen = details["op"].as_u64().unwrap() as usize;
    assert_eq!(candidates[chosen]["distance"].as_f64(), Some(0.0));
    assert!(details["round_trip"]["distance"].as_f64().unwrap() < 1e-6);
}

#[test]
fn a_point_outside_the_areas_is_warned_about() {
    let s = Session::new("4258", "2180", &Options::default()).unwrap();
    // Lisbon in PL-1992.
    let out = s.convert(&page::parse_points("[[38.7, -9.1]]").unwrap());
    let warnings = out[0]["warnings"].to_string();
    assert!(warnings.contains("target_area"), "{out}");
}

#[test]
fn east_first_swaps_a_latitude_first_crs() {
    let options = Options {
        east_first: true,
        ..Options::default()
    };
    let s = Session::new("4258", "2180", &options).unwrap();
    let out = s.convert(&page::parse_points("[[21.0, 52.2]]").unwrap());
    let xy = out[0]["out"].as_array().unwrap();
    // Easting first now.
    assert!(
        (xy[0].as_f64().unwrap() - 637_000.0).abs() < 1000.0,
        "{xy:?}"
    );
}

#[test]
fn a_forced_operation_is_used_for_every_point() {
    let all = Session::new("4326", "2180", &Options::default()).unwrap();
    let n = all.info()["operations"].as_array().unwrap().len();
    let options = Options {
        operation: Some(n - 1),
        ..Options::default()
    };
    let s = Session::new("4326", "2180", &options).unwrap();
    assert_eq!(s.info()["forced"], n - 1);
    let out = s.convert(&page::parse_points("[[52.2, 21.0]]").unwrap());
    assert_eq!(out[0]["op"], n - 1);
}

#[test]
fn the_points_bounds_are_in_degrees() {
    let points = page::parse_points("[[486000, 637000], null, [500000, 700000]]").unwrap();
    let [west, south, east, north] = page::lonlat_bounds(2180, false, &points).unwrap();
    assert!(west < east && south < north);
    assert!(
        (20.0..22.5).contains(&west) && (52.0..53.0).contains(&south),
        "{west} {south}"
    );
}

#[test]
fn a_point_no_operation_covers_is_a_fallback() {
    let s = Session::new("4230", "4258", &Options::default()).unwrap();
    // Berlin: no ED50 to ETRS89 for onshore Germany.
    let out = s.convert(&page::parse_points("[[52.52, 13.405]]").unwrap());
    assert_eq!(
        out[0]["warnings"],
        serde_json::json!(["fallback_none"]),
        "{out}"
    );
    let details = s.details((52.52, 13.405, None));
    let first = &details["candidates"][0];
    assert_eq!(first["outcome"]["kind"], "fallback", "{details:#}");
    assert_eq!(first["inside"], false);
}

#[test]
fn the_candidates_say_why_they_were_passed_over() {
    let s = Session::new("4230", "4258", &Options::default()).unwrap();
    // Paris: France's operation is chosen; each other one is outside its
    // area or passed over.
    let details = s.details((48.8566, 2.3522, None));
    let candidates = details["candidates"].as_array().unwrap();
    let chosen = details["op"].as_u64().unwrap() as usize;
    assert_eq!(
        candidates[chosen]["outcome"]["kind"], "chosen",
        "{details:#}"
    );
    for c in candidates {
        let kind = c["outcome"]["kind"].as_str().unwrap();
        assert!(["chosen", "passed", "outside"].contains(&kind), "{c}");
        assert_eq!(kind == "outside", c["inside"] == false, "{c}");
    }
}

#[test]
fn the_only_operation_outside_its_area_and_null() {
    let s = Session::new("4326", "2180", &Options::default()).unwrap();
    assert_eq!(s.info()["operations"][0]["null"], true);
    // Cairo.
    let out = s.convert(&page::parse_points("[[30.0444, 31.2357]]").unwrap());
    assert!(
        out[0]["warnings"].to_string().contains("only_area"),
        "{out}"
    );
    let s = Session::new("4230", "4258", &Options::default()).unwrap();
    assert_eq!(s.info()["operations"][0]["null"], false);
}

#[test]
fn a_forced_operation_outside_its_area() {
    let options = Options {
        operation: Some(1),
        ..Options::default()
    };
    let s = Session::new("4230", "4258", &options).unwrap();
    // Berlin, through France's operation.
    let out = s.convert(&page::parse_points("[[52.52, 13.405]]").unwrap());
    assert_eq!(
        out[0]["warnings"],
        serde_json::json!(["forced_area"]),
        "{out}"
    );
    let details = s.details((52.52, 13.405, None));
    assert!(details["candidates"][1]["outcome"].is_null());
}
