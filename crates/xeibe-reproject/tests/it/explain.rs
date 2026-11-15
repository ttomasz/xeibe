//! [`Transform::explain_point`]: how the operation for one coordinate is
//! chosen. It runs the same choice as `convert`, so it must agree with it
//! on every point of the selection vectors; and the cases below say what
//! it says.

use std::path::Path;

use serde_json::Value;
use xeibe_reproject::{
    AxisOrder, CoordinateError, Crs, Explanation, Grids, Outcome, PassReason, Selection, Transform,
};

fn transform(src: u32, dst: u32, grids: Grids) -> Transform {
    let (a, b) = (Crs::from_epsg(src).unwrap(), Crs::from_epsg(dst).unwrap());
    Transform::with_grids(&a, &b, AxisOrder::Authority, grids).unwrap()
}

fn index_of(t: &Transform, name: &str) -> usize {
    t.operation_info()
        .iter()
        .position(|o| o.name == name)
        .unwrap_or_else(|| panic!("{name} isn't among {:?}", t.operations()))
}

/// What must hold of any explanation, whatever the point.
fn check(t: &Transform, e: &Explanation, (x, y, z): (f64, f64, Option<f64>)) -> Result<(), String> {
    let n = t.operation_info().len();
    if e.candidates.len() != n {
        return Err(format!("{} explained, {n} operations", e.candidates.len()));
    }
    if e.used != t.operation_index_for(x, y, z) {
        return Err(format!(
            "used {:?}, operation_index_for {:?}",
            e.used,
            t.operation_index_for(x, y, z)
        ));
    }
    if e.used.is_some() != t.convert(x, y, z).is_ok() || e.used.is_some() == e.error.is_some() {
        return Err(format!("used {:?}, error {:?}", e.used, e.error));
    }
    for (i, c) in e.candidates.iter().enumerate() {
        let used = matches!(
            c.outcome,
            Outcome::Chosen | Outcome::Fallback | Outcome::Only
        );
        if used != (e.used == Some(i)) {
            return Err(format!("operation {i}: {:?}, used {:?}", c.outcome, e.used));
        }
        match c.outcome {
            Outcome::Chosen | Outcome::Passed { .. } if c.inside != Some(true) => {
                return Err(format!(
                    "operation {i}: {:?} but inside {:?}",
                    c.outcome, c.inside
                ));
            }
            Outcome::Outside if c.inside != Some(false) => {
                return Err(format!("operation {i}: outside but inside {:?}", c.inside));
            }
            Outcome::Passed { by, .. } if by >= n || by == i => {
                return Err(format!("operation {i}: passed over for {by}"));
            }
            _ => {}
        }
    }
    Ok(())
}

#[test]
fn a_point_outside_every_area_falls_back_to_the_first_without_grids() {
    let t = transform(4230, 4258, Grids::None);
    // Berlin: EPSG has no ED50 to ETRS89 for onshore Germany.
    let p = (52.52, 13.405, None);
    let e = t.explain_point(p.0, p.1, p.2).unwrap();
    check(&t, &e, p).unwrap();
    assert_eq!(e.used, Some(0));
    assert_eq!(e.candidates[0].outcome, Outcome::Fallback);
    assert_eq!(e.candidates[0].inside, Some(false));
    assert!(
        e.candidates.iter().all(|c| c.inside == Some(false)),
        "{e:#?}"
    );
}

#[test]
fn a_point_inside_several_areas_goes_through_the_most_accurate() {
    let t = transform(4230, 4258, Grids::None);
    // Barcelona: inside Spain's (except the northwest), 1.5 m, and the
    // box of France's, 2 m.
    let p = (41.3874, 2.1686, None);
    let e = t.explain_point(p.0, p.1, p.2).unwrap();
    check(&t, &e, p).unwrap();
    let used = e.used.unwrap();
    assert_eq!(e.candidates[used].outcome, Outcome::Chosen);
    let info = t.operation_info();
    let best = info[used].accuracy.unwrap();
    let passed: Vec<_> = e
        .candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match c.outcome {
            Outcome::Passed { reason, .. } => Some((i, reason)),
            _ => None,
        })
        .collect();
    assert!(!passed.is_empty(), "{e:#?}");
    for (i, reason) in passed {
        let accuracy = info[i].accuracy;
        match reason {
            PassReason::LessAccurate => assert!(accuracy.unwrap() > best, "{i}: {accuracy:?}"),
            PassReason::LargerArea => assert_eq!(accuracy, Some(best)),
            _ => {}
        }
    }
}

#[test]
fn a_grid_not_loaded_is_the_error_of_the_operation_tried() {
    let t = transform(4230, 4258, Grids::ProjData);
    // Barcelona: inside the Catalan grid's area, 5 cm, the most accurate.
    let catalonia = index_of(&t, "ED50 to ETRS89 (14)");
    let p = (41.3874, 2.1686, None);
    let e = t.explain_point(p.0, p.1, p.2).unwrap();
    check(&t, &e, p).unwrap();
    assert_eq!(e.used, None);
    assert_eq!(e.error, Some(CoordinateError::GridNotLoaded));
    assert_eq!(
        e.candidates[catalonia].outcome,
        Outcome::Failed(CoordinateError::GridNotLoaded)
    );
}

#[test]
fn the_only_operation_is_used_whatever_its_area() {
    let t = transform(4326, 2180, Grids::None);
    // One, and after it the route through "WGS 84 to ETRS89 (2)" for
    // coordinates at its epoch (2014.81) only.
    let info = t.operation_info();
    assert_eq!(info.len(), 2);
    assert_eq!(info[0].only_at_epoch, None);
    assert_eq!(info[1].only_at_epoch, Some(2014.81));
    // Cairo, outside ETRS89's Europe.
    let p = (30.0444, 31.2357, None);
    let e = t.explain_point(p.0, p.1, p.2).unwrap();
    check(&t, &e, p).unwrap();
    assert_eq!(e.candidates[0].outcome, Outcome::Only);
    assert_eq!(e.candidates[0].inside, Some(false));
    assert_eq!(e.candidates[1].outcome, Outcome::NotAtEpoch);
    let warsaw = t.explain_point(52.23, 21.01, None).unwrap();
    assert_eq!(warsaw.candidates[0].inside, Some(true));
    assert_eq!(warsaw.candidates[1].outcome, Outcome::NotAtEpoch);
    assert_eq!(warsaw.candidates[1].inside, Some(false));
}

/// "WGS 84 to ETRS89 (2)" holds at 2014.81, in the German and Dutch North
/// Sea, to 0.1 m; the null "ETRS89 to WGS 84 (1)" (1 m) is for every other
/// coordinate.
#[test]
fn a_time_specific_route_is_for_a_coordinate_at_its_epoch() {
    let (a, b) = (Crs::from_epsg(4937).unwrap(), Crs::from_epsg(4979).unwrap());
    let t = Transform::new(&a, &b, AxisOrder::EastFirst).unwrap();
    let info = t.operation_info();
    let route = info
        .iter()
        .position(|o| o.only_at_epoch == Some(2014.81))
        .unwrap();
    let null = index_of(&t, "ETRS89 to WGS 84 (1)");
    let sea = (6.5, 54.5, Some(30.0));
    let warsaw = (21.01, 52.23, Some(100.0));

    // Without an epoch, and at another one: not looked at, though its
    // area contains the point.
    for t in [t.clone(), t.with_epochs(None, Some(2020.0)).unwrap()] {
        let e = t.explain_point(sea.0, sea.1, sea.2).unwrap();
        check(&t, &e, sea).unwrap();
        assert_eq!(e.used, Some(null));
        assert_eq!(e.candidates[route].outcome, Outcome::NotAtEpoch);
        assert_eq!(e.candidates[route].inside, Some(true));
        assert!(
            e.candidates
                .iter()
                .all(|c| c.outcome != Outcome::NotReached),
            "{e:?}"
        );
    }

    let at = t.with_epochs(None, Some(2014.81)).unwrap();
    // At its epoch, in its area: chosen, and the others say why not they.
    let e = at.explain_point(sea.0, sea.1, sea.2).unwrap();
    check(&at, &e, sea).unwrap();
    assert_eq!(e.used, Some(route));
    assert_eq!(e.candidates[route].outcome, Outcome::Chosen);
    assert_eq!(
        e.candidates[null].outcome,
        Outcome::Passed {
            by: route,
            reason: PassReason::LessAccurate
        }
    );
    assert!(
        e.candidates
            .iter()
            .all(|c| c.outcome != Outcome::NotReached),
        "{e:?}"
    );
    // At its epoch, outside its area.
    let e = at.explain_point(warsaw.0, warsaw.1, warsaw.2).unwrap();
    check(&at, &e, warsaw).unwrap();
    assert_eq!(e.used, Some(null));
    assert_eq!(e.candidates[route].outcome, Outcome::Outside);

    // A coordinate's own epoch does as the transform's, for the
    // explanation, the operation and the trace as for the conversion.
    for (p, epoch) in [(sea, 2014.81), (warsaw, 2014.81), (sea, 2020.0)] {
        let fixed = t.with_epochs(None, Some(epoch)).unwrap();
        let (x, y, z) = p;
        assert_eq!(t.convert_at(x, y, z, epoch), fixed.convert(x, y, z));
        assert_eq!(
            t.explain_point_at(x, y, z, epoch),
            fixed.explain_point(x, y, z)
        );
        assert_eq!(
            t.operation_index_for_at(x, y, z, epoch),
            fixed.operation_index_for(x, y, z)
        );
        assert_eq!(
            t.operation_for_at(x, y, z, epoch),
            fixed.operation_for(x, y, z)
        );
        let (own, of_transform) = (
            t.trace_point_at(x, y, z, epoch).unwrap(),
            fixed.trace_point(x, y, z).unwrap(),
        );
        assert_eq!(own.used, of_transform.used);
        assert_eq!(format!("{own:?}"), format!("{of_transform:?}"));
    }
    // No epoch (PROJ's HUGE_VAL) is the coordinate as `explain_point`
    // takes it.
    assert_eq!(
        t.explain_point_at(sea.0, sea.1, sea.2, f64::INFINITY),
        t.explain_point(sea.0, sea.1, sea.2)
    );
    assert_eq!(
        t.trace_point_at(sea.0, sea.1, sea.2, 2014.81).unwrap().used,
        Some(route)
    );
}

/// Beside the only other operation, a time-specific route that is chosen
/// isn't "the only one"; and where there are only such routes, each says
/// why a coordinate can't be converted.
#[test]
fn a_time_specific_route_beside_one_operation_or_none() {
    let t = transform(4326, 2180, Grids::None)
        .with_epochs(Some(2014.81), None)
        .unwrap();
    let sea = (54.5, 6.5, None);
    let e = t.explain_point(sea.0, sea.1, sea.2).unwrap();
    check(&t, &e, sea).unwrap();
    assert_eq!(e.used, Some(1));
    assert_eq!(e.candidates[1].outcome, Outcome::Chosen);
    assert_eq!(
        e.candidates[0].outcome,
        Outcome::Passed {
            by: 1,
            reason: PassReason::LessAccurate
        }
    );
    let warsaw = (52.23, 21.01, None);
    let e = t.explain_point(warsaw.0, warsaw.1, warsaw.2).unwrap();
    check(&t, &e, warsaw).unwrap();
    assert_eq!(e.candidates[0].outcome, Outcome::Only);
    assert_eq!(e.candidates[1].outcome, Outcome::Outside);

    // ITRF2008 to PZ-90.11: EPSG has the one transformation, at 2010.0.
    let (a, b) = (Crs::from_epsg(5332).unwrap(), Crs::from_epsg(7679).unwrap());
    let pz = Transform::new(&a, &b, AxisOrder::Authority).unwrap();
    assert_eq!(pz.operation_info()[0].only_at_epoch, Some(2010.0));
    let p = (2_845_455.975_3, 2_160_954.307_3, Some(5_265_993.265_6));
    let e = pz.explain_point(p.0, p.1, p.2).unwrap();
    assert_eq!(e.used, None);
    assert_eq!(e.error, Some(CoordinateError::MissingEpoch));
    assert!(
        e.candidates
            .iter()
            .all(|c| c.outcome == Outcome::NotAtEpoch),
        "{e:?}"
    );
}

#[test]
fn a_forced_operation_has_no_area_to_test() {
    let all = transform(4230, 4258, Grids::None);
    let forced = all.with_operation_index(3).unwrap().unwrap();
    let e = forced.explain_point(52.52, 13.405, None).unwrap();
    assert_eq!(e.used, Some(0));
    assert_eq!(e.candidates.len(), 1);
    assert_eq!(e.candidates[0].outcome, Outcome::Only);
    assert_eq!(e.candidates[0].inside, None);
}

#[test]
fn a_conversion_has_nothing_to_explain() {
    let t = transform(4258, 2180, Grids::None);
    assert_eq!(t.explain_point(52.23, 21.01, None), None);
}

/// Every point of the committed selection vectors (without grids): the
/// explanation agrees with the conversion.
#[test]
fn explanations_agree_with_the_choice_on_the_selection_vectors() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/reprojection");
    let mut points = 0;
    let mut failures = Vec::new();
    for file in [
        "selection.jsonl",
        "selection-options.jsonl",
        "selection-vertical.jsonl",
    ] {
        let text = std::fs::read_to_string(dir.join(file)).unwrap();
        for line in text.lines().skip(1) {
            let r: Value = serde_json::from_str(line).unwrap();
            let crs = |v: &Value| match v.as_array() {
                Some(parts) => Crs::compound(
                    &Crs::from_epsg(parts[0].as_u64().unwrap() as u32).unwrap(),
                    &Crs::from_epsg(parts[1].as_u64().unwrap() as u32).unwrap(),
                ),
                None => Crs::from_epsg(v.as_u64().unwrap() as u32),
            };
            let (Ok(src), Ok(dst)) = (crs(&r["src"]), crs(&r["dst"])) else {
                continue;
            };
            let selection = Selection {
                area_of_interest: r["area_of_interest"].as_array().map(|a| {
                    let v: Vec<f64> = a.iter().map(|x| x.as_f64().unwrap()).collect();
                    [v[0], v[1], v[2], v[3]]
                }),
                accuracy: r["accuracy"].as_f64(),
            };
            let Ok(t) =
                Transform::with_selection(&src, &dst, AxisOrder::Authority, Grids::None, selection)
            else {
                continue;
            };
            for p in r["points"].as_array().unwrap() {
                let v: Vec<f64> = p["in"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_f64().unwrap())
                    .collect();
                let point = (v[0], v[1], v.get(2).copied());
                let Some(e) = t.explain_point(point.0, point.1, point.2) else {
                    if !t.operations().is_empty()
                        && t.operation_index_for(point.0, point.1, point.2).is_some()
                    {
                        failures.push(format!(
                            "{} -> {}: {point:?}: no explanation",
                            r["src"], r["dst"]
                        ));
                    }
                    continue;
                };
                points += 1;
                if let Err(why) = check(&t, &e, point) {
                    failures.push(format!("{} -> {}: {point:?}: {why}", r["src"], r["dst"]));
                }
            }
        }
    }
    assert!(points > 1000, "only {points} points explained");
    assert!(
        failures.is_empty(),
        "{} of {points}:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}

/// "ITRF2000 to SIRGAS 2000 (1)" holds at 2000.4, in Latin America, and
/// is all EPSG has between the two: a coordinate at that epoch goes
/// through it whatever its area, as through any only operation.
#[test]
fn the_only_time_specific_route_is_used_whatever_its_area() {
    let (a, b) = (Crs::from_epsg(7909).unwrap(), Crs::from_epsg(4989).unwrap());
    let t = Transform::new(&a, &b, AxisOrder::EastFirst).unwrap();
    let info = t.operation_info();
    assert_eq!(info.len(), 1, "{:?}", t.operations());
    assert_eq!(info[0].only_at_epoch, Some(2000.4));
    let santiago = (-70.67, -33.45, Some(520.0));
    let warsaw = (21.01, 52.23, Some(100.0));

    let at = t.with_epochs(Some(2000.4), None).unwrap();
    for (p, inside) in [(santiago, true), (warsaw, false)] {
        let e = at.explain_point(p.0, p.1, p.2).unwrap();
        check(&at, &e, p).unwrap();
        assert_eq!(e.used, Some(0), "{e:?}");
        assert_eq!(e.candidates[0].outcome, Outcome::Only);
        assert_eq!(e.candidates[0].inside, Some(inside));
        // The coordinate's own epoch does as the transform's.
        assert_eq!(
            t.convert_at(p.0, p.1, p.2, 2000.4),
            at.convert(p.0, p.1, p.2)
        );
    }
    // Not at its epoch: nothing converts the coordinate, wherever it is.
    for p in [santiago, warsaw] {
        assert_eq!(t.convert(p.0, p.1, p.2), Err(CoordinateError::MissingEpoch));
        assert_eq!(
            t.convert_at(p.0, p.1, p.2, 2020.0),
            Err(CoordinateError::EpochOutOfRange)
        );
        let e = t
            .with_epochs(Some(2020.0), None)
            .unwrap()
            .explain_point(p.0, p.1, p.2)
            .unwrap();
        assert_eq!(e.error, Some(CoordinateError::EpochOutOfRange));
        assert_eq!(e.candidates[0].outcome, Outcome::NotAtEpoch);
    }
}
