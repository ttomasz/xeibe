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
fn a_time_specific_route_is_marked_and_not_looked_at() {
    // One operation, and after it the route through "WGS 84 to ETRS89
    // (2)", which holds at 2014.81 only: no epoch is set.
    let s = Session::new("4326", "2180", &Options::default()).unwrap();
    let info = s.info();
    let operations = info["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 2, "{info:#}");
    assert!(operations[0]["only_at_epoch"].is_null());
    assert_eq!(operations[1]["only_at_epoch"], 2014.81);
    // Warsaw, and the German North Sea, inside the route's area.
    for (point, inside) in [((52.2, 21.0, None), false), ((54.5, 6.5, None), true)] {
        let details = s.details(point);
        assert_eq!(details["op"], 0, "{details:#}");
        let candidates = &details["candidates"];
        assert_eq!(candidates[0]["outcome"]["kind"], "only");
        assert_eq!(candidates[1]["outcome"]["kind"], "not_at_epoch");
        assert_eq!(candidates[1]["inside"], inside);
    }
    // No operation covers a point there but the route: that is no
    // operation that failed.
    let s = Session::new("4326", "4258", &Options::default()).unwrap();
    let info = s.info();
    let route = info["operations"]
        .as_array()
        .unwrap()
        .iter()
        .position(|o| !o["only_at_epoch"].is_null())
        .unwrap();
    let details = s.details((54.5, 6.5, None));
    assert_eq!(
        details["candidates"][route]["outcome"]["kind"],
        "not_at_epoch"
    );
    assert!(
        !details["warnings"].to_string().contains("fallback_failed"),
        "{details:#}"
    );
}

#[test]
fn an_epoch_for_the_points_reaches_the_operations_that_need_one() {
    let at = |epoch: f64| Options {
        epoch: Some(epoch),
        ..Options::default()
    };
    // "WGS 84 to ETRS89 (2)" at 2014.81, in the German North Sea.
    let s = Session::new("4326", "2180", &at(2014.81)).unwrap();
    assert_eq!(s.info()["epoch"], 2014.81);
    let sea = s.details((54.5, 6.5, None));
    assert_eq!(sea["op"], 1, "{sea:#}");
    assert_eq!(sea["candidates"][1]["outcome"]["kind"], "chosen");
    assert_eq!(sea["candidates"][0]["outcome"]["kind"], "passed");
    assert_eq!(sea["candidates"][0]["outcome"]["by"], 1);
    // The trace and the round trip run at the epoch too.
    let steps = sea["trace"]["steps"].to_string();
    assert!(steps.contains("WGS 84 to ETRS89 (2)"), "{steps}");
    assert!(sea["round_trip"]["distance"].as_f64().unwrap() < 1e-6);
    let warsaw = s.details((52.2, 21.0, None));
    assert_eq!(warsaw["op"], 0);
    assert_eq!(warsaw["candidates"][1]["outcome"]["kind"], "outside");
    // At another epoch the route is as without one.
    let s = Session::new("4326", "2180", &at(2020.0)).unwrap();
    let sea = s.details((54.5, 6.5, None));
    assert_eq!(sea["op"], 0);
    assert_eq!(sea["candidates"][1]["outcome"]["kind"], "not_at_epoch");

    // Between ITRF2000 and SIRGAS 2000 there is only a route at 2000.4:
    // without an epoch a point fails, and the page can say what is missing.
    let s = Session::new("7909", "4989", &Options::default()).unwrap();
    assert!(s.info()["epoch"].is_null());
    let points = page::parse_points("[[-33.45, -70.67, 520], [52.23, 21.01, 100]]").unwrap();
    let out = s.convert(&points);
    assert_eq!(out[0]["missing_epoch"], true, "{out}");
    assert_eq!(
        s.details((-33.45, -70.67, Some(520.0)))["missing_epoch"],
        true
    );
    // With it, both convert: the only operation, used whatever its area.
    let s = Session::new("7909", "4989", &at(2000.4)).unwrap();
    let out = s.convert(&points);
    assert_eq!(out[0]["op"], 0, "{out}");
    assert_eq!(out[0]["warnings"], serde_json::json!([]));
    assert!(
        out[1]["warnings"].to_string().contains("only_area"),
        "{out}"
    );
    // And at another epoch they fail, for another reason.
    let s = Session::new("7909", "4989", &at(2020.0)).unwrap();
    let out = s.convert(&points);
    assert_eq!(out[0]["missing_epoch"], false, "{out}");
    assert!(out[0]["error"].as_str().unwrap().contains("epoch"));
}

#[test]
fn a_point_can_have_an_epoch_of_its_own() {
    // The fourth value: one point at the route's epoch, one at another,
    // one without.
    let points =
        page::parse_points("[[54.5, 6.5, 0, 2014.81], [54.5, 6.5, 0, 2020], [54.5, 6.5, 0], null]")
            .unwrap();
    assert_eq!(points[0], Some(((54.5, 6.5, Some(0.0)), Some(2014.81))));
    assert_eq!(points[2], Some(((54.5, 6.5, Some(0.0)), None)));
    let s = Session::new("4326", "2180", &Options::default()).unwrap();
    let out = s.convert(&points);
    assert_eq!(out[0]["op"], 1, "{out}");
    assert_eq!(out[1]["op"], 0);
    assert_eq!(out[2]["op"], 0);
    assert_eq!(out[1]["out"], out[2]["out"]);
    assert_ne!(out[0]["out"], out[1]["out"]);
    assert!(out[3].is_null());
    // Its details are at its epoch, and say which.
    let own = s.details_at((54.5, 6.5, Some(0.0)), Some(2014.81));
    assert_eq!(own["epoch"], 2014.81);
    assert_eq!(own["op"], 1);
    assert_eq!(own["candidates"][1]["outcome"]["kind"], "chosen");
    assert_eq!(own["out"], out[0]["out"]);
    let none = s.details((54.5, 6.5, Some(0.0)));
    assert!(none["epoch"].is_null());
    assert_eq!(none["candidates"][1]["outcome"]["kind"], "not_at_epoch");
    // The page's epoch for every point is for those without their own.
    let all = Options {
        epoch: Some(2014.81),
        ..Options::default()
    };
    let s = Session::new("4326", "2180", &all).unwrap();
    let out = s.convert(&points);
    assert_eq!(out[1]["op"], 0, "{out}");
    assert_eq!(out[2]["op"], 1);
    // The bounds and the grids don't mind the fourth value.
    assert!(page::lonlat_bounds(4326, false, &points).is_some());
    assert!(s.grid_requests(&points).is_empty());
    // An epoch is a decimal year.
    for bad in ["[[54.5, 6.5, 0, -1]]", "[[1, 2, 3, 4, 5]]", "[[1]]"] {
        assert!(page::parse_points(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_epoch_is_a_decimal_year() {
    assert_eq!(
        Options::from_json(r#"{"epoch": 2014.81}"#).unwrap().epoch,
        Some(2014.81)
    );
    assert_eq!(
        Options::from_json(r#"{"epoch": null}"#).unwrap().epoch,
        None
    );
    assert_eq!(Options::from_json("{}").unwrap().epoch, None);
    for bad in [
        r#"{"epoch": "2014"}"#,
        r#"{"epoch": -1}"#,
        r#"{"epoch": 0}"#,
    ] {
        assert!(Options::from_json(bad).is_err(), "{bad}");
    }
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

#[test]
fn the_details_trace_the_point_step_by_step() {
    let s = Session::new("4230", "2180", &Options::default()).unwrap();
    // Warsaw, from ED50 to PL-1992.
    let details = s.details((52.23, 21.01, None));
    let steps = details["trace"]["steps"].as_array().unwrap();
    assert_eq!(steps[0]["kind"], "start", "{details:#}");
    assert_eq!(steps[0]["crs"]["label"], "EPSG:4230");
    assert_eq!(steps[0]["coords"], serde_json::json!([52.23, 21.01]));
    assert_eq!(steps[0]["axes"][0]["direction"], "north");
    assert!(
        steps.iter().any(|s| s["kind"] == "operation"
            && s["detail"]["code"].is_u64()
            && s["detail"]["method"]["name"].is_string()),
        "{details:#}"
    );
    let last = steps.last().unwrap();
    assert_eq!(last["crs"]["label"], "EPSG:2180");
    assert_eq!(last["coords"], details["out"], "{details:#}");
    assert!(details["trace"]["error"].is_null());
}

#[test]
fn the_trace_is_in_the_pages_axis_order() {
    let options = Options {
        east_first: true,
        ..Options::default()
    };
    let s = Session::new("4258", "2180", &options).unwrap();
    let details = s.details((21.01, 52.23, None));
    let steps = details["trace"]["steps"].as_array().unwrap();
    assert_eq!(steps[0]["axes"][0]["direction"], "east", "{details:#}");
    let last = steps.last().unwrap();
    // PL-1992 is northing first: easting first now, as the result is.
    assert_eq!(last["axes"][0]["direction"], "east", "{details:#}");
    assert_eq!(last["coords"], details["out"]);
}

#[test]
fn a_point_that_fails_has_the_steps_up_to_the_failure() {
    // ED50 to ETRS89 in Barcelona: the Catalan grid isn't loaded.
    let s = Session::new("4230", "4258", &Options::default()).unwrap();
    let details = s.details((41.3874, 2.1686, None));
    assert!(details["error"].is_string(), "{details:#}");
    let trace = &details["trace"];
    assert!(trace["error"].is_string(), "{details:#}");
    assert_eq!(trace["steps"][0]["kind"], "start");
}
