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
    assert_eq!(t.operation_info().len(), 1);
    // Cairo, outside ETRS89's Europe.
    let p = (30.0444, 31.2357, None);
    let e = t.explain_point(p.0, p.1, p.2).unwrap();
    check(&t, &e, p).unwrap();
    assert_eq!(e.candidates[0].outcome, Outcome::Only);
    assert_eq!(e.candidates[0].inside, Some(false));
    let warsaw = t.explain_point(52.23, 21.01, None).unwrap();
    assert_eq!(warsaw.candidates[0].inside, Some(true));
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
