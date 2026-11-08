//! [`Transform::trace_point`]: each step one coordinate goes through, with
//! the coordinates after it (`docs/web-reproject.md`, piece 9.2). It runs
//! the same steps as `convert`, so its last coordinates must be
//! `convert`'s on every point of the selection vectors; and the cases
//! below say what the steps are.

use std::path::Path;

use serde_json::Value;
use xeibe_reproject::{
    AxisOrder, CoordinateError, Crs, Grids, Selection, StepKind, Trace, TraceStep, TracedHeight,
    Transform,
};

fn transform(src: u32, dst: u32, grids: Grids) -> Transform {
    let (a, b) = (Crs::from_epsg(src).unwrap(), Crs::from_epsg(dst).unwrap());
    Transform::with_grids(&a, &b, AxisOrder::Authority, grids).unwrap()
}

fn grids_dir() -> std::path::PathBuf {
    xeibe_testkit::data_dir().join("reprojection/grids")
}

/// What must hold of any trace, whatever the point: it starts with the
/// point as given in the source CRS, ends in the target CRS with
/// `convert`'s result, and names the operation `convert` uses.
fn check(t: &Transform, trace: &Trace, (x, y, z): (f64, f64, Option<f64>)) -> Result<(), String> {
    let first = trace.steps.first().ok_or("no steps")?;
    if first.step != TraceStep::Start || !first.crs.equivalent(t.source(), false) {
        return Err(format!(
            "starts with {:?} in {}",
            first.step,
            first.crs.name()
        ));
    }
    if first.coords[..2] != [x, y] {
        return Err(format!("starts at {:?}", first.coords));
    }
    let expected_op = if t.operations().is_empty() {
        None
    } else {
        t.operation_index_for(x, y, z)
    };
    if trace.used != expected_op {
        return Err(format!(
            "used {:?}, operation_index_for {expected_op:?}",
            trace.used
        ));
    }
    match t.convert(x, y, z) {
        Ok((cx, cy, cz)) => {
            if let Some(e) = trace.error {
                return Err(format!("error {e:?}, convert gives ({cx}, {cy})"));
            }
            let last = trace.steps.last().unwrap();
            if !last.crs.equivalent(t.target(), false) {
                return Err(format!("ends in {}", last.crs.name()));
            }
            let want_z = (t.target().dimension() == 3).then_some(cz).flatten();
            if last.coords[..2] != [cx, cy] || want_z.is_some_and(|z| z != last.coords[2]) {
                return Err(format!(
                    "ends at {:?}, convert gives ({cx}, {cy}, {cz:?})",
                    last.coords
                ));
            }
        }
        Err(e) => {
            if trace.error != Some(e) {
                return Err(format!("error {:?}, convert's {e:?}", trace.error));
            }
        }
    }
    Ok(())
}

fn kinds(trace: &Trace) -> Vec<&TraceStep> {
    trace.steps.iter().map(|s| &s.step).collect()
}

#[test]
fn a_conversion_is_a_projection() {
    let t = transform(4258, 2180, Grids::None);
    let p = (52.23, 21.01, None);
    let trace = t.trace_point(p.0, p.1, p.2).unwrap();
    check(&t, &trace, p).unwrap();
    assert_eq!(kinds(&trace), [&TraceStep::Start, &TraceStep::Convert]);
    assert_eq!(trace.used, None);
    assert_eq!(trace.steps[1].crs.code(), Some(2180));
    assert_eq!(trace.steps[0].height, TracedHeight::None);
}

#[test]
fn a_helmert_transformation_is_one_step() {
    let t = transform(4230, 4258, Grids::None);
    // Barcelona.
    let p = (41.3874, 2.1686, None);
    let trace = t.trace_point(p.0, p.1, p.2).unwrap();
    check(&t, &trace, p).unwrap();
    let used = &t.operation_info()[trace.used.unwrap()];
    let ops: Vec<_> = trace
        .steps
        .iter()
        .filter_map(|s| match &s.step {
            TraceStep::Operation { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    let codes: Vec<_> = ops
        .iter()
        .filter_map(|k| match *k {
            StepKind::Epsg { code, inverse } => Some((code, inverse)),
            _ => None,
        })
        .collect();
    assert_eq!(codes, used.steps, "{trace:#?}");
    // The shift is the difference across the step: some metres.
    let i = trace
        .steps
        .iter()
        .position(|s| matches!(s.step, TraceStep::Operation { .. }))
        .unwrap();
    let (before, after) = (&trace.steps[i - 1], &trace.steps[i]);
    let d = (after.coords[0] - before.coords[0]).abs() + (after.coords[1] - before.coords[1]).abs();
    assert!(d > 1e-5 && d < 1e-2, "{d}");
}

#[test]
fn a_projected_source_is_unprojected_first() {
    // PL-1992 to ED50: the inverse projection, then the transformations.
    let t = transform(2180, 4230, Grids::None);
    let p = (500_000.0, 500_000.0, None);
    let trace = t.trace_point(p.0, p.1, p.2).unwrap();
    check(&t, &trace, p).unwrap();
    assert_eq!(trace.steps[1].step, TraceStep::Convert, "{trace:#?}");
    let geographic = &trace.steps[1];
    assert!(geographic.coords[0].abs() <= 90.0 && geographic.coords[1].abs() <= 180.0);
}

#[test]
fn a_grid_step_carries_its_shift() {
    let (a, b) = (Crs::from_epsg(4138).unwrap(), Crs::from_epsg(4269).unwrap());
    let t = Transform::via(&a, &b, 1457, AxisOrder::Authority).unwrap();
    let p = (56.6, -169.55, None);
    t.load_grids_from_dir(grids_dir(), Some([56.5, -169.7, 56.7, -169.4]))
        .unwrap();
    let trace = t.trace_point(p.0, p.1, p.2).unwrap();
    check(&t, &trace, p).unwrap();
    let grid = trace
        .steps
        .iter()
        .position(|s| {
            matches!(
                s.step,
                TraceStep::Operation {
                    kind: StepKind::Epsg { code: 1457, .. },
                    ..
                }
            )
        })
        .unwrap_or_else(|| panic!("{trace:#?}"));
    let (before, after) = (&trace.steps[grid - 1], &trace.steps[grid]);
    assert_ne!(before.coords, after.coords);
}

#[test]
fn a_grid_not_loaded_ends_the_steps_with_the_error() {
    let (a, b) = (Crs::from_epsg(4138).unwrap(), Crs::from_epsg(4269).unwrap());
    let t = Transform::via(&a, &b, 1457, AxisOrder::Authority).unwrap();
    xeibe_reproject::clear_grids();
    let p = (56.6, -169.55, None);
    let trace = t.trace_point(p.0, p.1, p.2).unwrap();
    if trace.error.is_none() {
        // Another test loaded the grid into the process meanwhile.
        return;
    }
    assert_eq!(trace.error, Some(CoordinateError::GridNotLoaded));
    assert_eq!(trace.used, None);
    assert_eq!(trace.steps[0].step, TraceStep::Start);
    assert!(
        !trace
            .steps
            .iter()
            .any(|s| matches!(s.step, TraceStep::Operation { .. })),
        "{trace:#?}"
    );
}

#[test]
fn a_geoid_model_keeps_the_horizontal_position() {
    // RGAF09 + IGN 2008 LD height (compound) to RGAF09 (geographic 3D),
    // through the geoid model's grid.
    let t = transform(9536, 5488, Grids::ProjData);
    let p = (16.28, -61.10333333333334, Some(477.911));
    // In the source's axis order, latitude first.
    t.load_grids_from_dir(grids_dir(), Some([16.2, -61.2, 16.4, -61.0]))
        .unwrap();
    let trace = t.trace_point(p.0, p.1, p.2).unwrap();
    check(&t, &trace, p).unwrap();
    assert_eq!(trace.steps[0].height, TracedHeight::GravityRelated);
    let last = trace.steps.last().unwrap();
    assert_eq!(last.height, TracedHeight::Ellipsoidal, "{trace:#?}");
    // The geoid height: some 43 m here.
    assert!(
        (trace.steps[0].coords[2] - last.coords[2] - 43.6).abs() < 1.0,
        "{trace:#?}"
    );
}

#[test]
fn the_position_is_kept_around_a_geoid_model_on_another_datum() {
    // ETRS89 + EGM2008 height to ETRS89 (geographic 3D): EGM2008's model
    // is on WGS 84, so the position goes there for it and is kept
    // meanwhile. Its grid isn't committed: the steps end where it's read.
    let src = Crs::compound(
        &Crs::from_epsg(4258).unwrap(),
        &Crs::from_epsg(3855).unwrap(),
    )
    .unwrap();
    let dst = Crs::from_epsg(4937).unwrap();
    let t = Transform::with_grids(&src, &dst, AxisOrder::Authority, Grids::ProjData).unwrap();
    let p = (52.23, 21.01, Some(100.0));
    let trace = t.trace_point(p.0, p.1, p.2).unwrap();
    check(&t, &trace, p).unwrap();
    assert_eq!(trace.error, Some(CoordinateError::GridNotLoaded));
    let kinds = kinds(&trace);
    assert!(kinds.contains(&&TraceStep::SaveHorizontal), "{kinds:?}");
}

#[test]
fn a_bound_crs_goes_through_its_hub() {
    // ED50 with a TOWGS84 to WGS 84.
    let src = Crs::from_wkt(
        r#"GEOGCS["ED50",DATUM["European_Datum_1950",SPHEROID["International 1924",6378388,297],
           TOWGS84[-87,-98,-121,0,0,0,0]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#,
    )
    .unwrap();
    let dst = Crs::from_epsg(4326).unwrap();
    let t = Transform::new(&src, &dst, AxisOrder::EastFirst).unwrap();
    let p = (2.1686, 41.3874, None);
    let trace = t.trace_point(p.0, p.1, p.2).unwrap();
    check(&t, &trace, p).unwrap();
    assert!(
        trace
            .steps
            .iter()
            .any(|s| matches!(s.step, TraceStep::Bound { inverse: false, .. })),
        "{trace:#?}"
    );
}

#[test]
fn a_point_that_cant_be_placed_has_no_trace() {
    let t = transform(4230, 4258, Grids::None);
    assert!(t.trace_point(f64::NAN, 1.0, None).is_none());
}

/// Every point of the committed selection vectors, with the committed
/// grids for the `-grids` ones: the trace ends where `convert` does.
#[test]
fn traces_end_with_convert_on_the_selection_vectors() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/reprojection");
    let mut points = 0;
    let mut failures = Vec::new();
    for (file, grids) in [
        ("selection.jsonl", Grids::None),
        ("selection-options.jsonl", Grids::None),
        ("selection-vertical.jsonl", Grids::None),
        ("selection-grids.jsonl", Grids::ProjData),
        ("selection-vertical-grids.jsonl", Grids::ProjData),
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
            let Ok(t) = Transform::with_selection(
                &src,
                &dst,
                AxisOrder::Authority,
                grids.clone(),
                selection,
            ) else {
                continue;
            };
            let inputs: Vec<Vec<f64>> = r["points"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    p["in"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_f64().unwrap())
                        .collect()
                })
                .collect();
            if grids == Grids::ProjData && !t.grids().is_empty() {
                // Whatever of them is committed; the rest fail alike.
                let _ = t.load_grids_from_dir(grids_dir(), None);
            }
            for v in inputs {
                let point = (v[0], v[1], v.get(2).copied());
                let Some(trace) = t.trace_point(point.0, point.1, point.2) else {
                    failures.push(format!("{} -> {}: {point:?}: no trace", r["src"], r["dst"]));
                    continue;
                };
                points += 1;
                if let Err(why) = check(&t, &trace, point) {
                    failures.push(format!("{} -> {}: {point:?}: {why}", r["src"], r["dst"]));
                }
            }
        }
    }
    assert!(points > 1000, "only {points} points traced");
    assert!(
        failures.is_empty(),
        "{} of {points}:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}
