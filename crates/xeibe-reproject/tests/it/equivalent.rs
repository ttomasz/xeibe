//! `Crs::equivalent`: CRSs that define the same coordinates, whatever their
//! names and identifiers, as PROJ's `proj_is_equivalent_to` judges them.

use serde_json::Value;
use xeibe_reproject::Crs;

fn epsg(code: u32) -> Crs {
    Crs::from_epsg(code).unwrap()
}

/// EPSG's PROJJSON for `code`, changed by `edit`.
fn edited(code: u32, edit: impl FnOnce(&mut Value)) -> Crs {
    let mut value: Value = serde_json::from_str(xeibe_crs::projjson(code).unwrap()).unwrap();
    edit(&mut value);
    Crs::from_projjson(&value.to_string()).unwrap()
}

/// The identifiers of CRSs and datums left out, and the CRSs renamed,
/// recursively. Parameters keep theirs: the reader names them by id.
fn anonymous(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.ends_with("CRS"))
            {
                map.remove("id");
                map.insert("name".into(), Value::String("unnamed".into()));
            }
            for key in ["datum", "datum_ensemble"] {
                if let Some(Value::Object(datum)) = map.get_mut(key) {
                    datum.remove("id");
                }
            }
            map.values_mut().for_each(anonymous);
        }
        Value::Array(items) => items.iter_mut().for_each(anonymous),
        _ => {}
    }
}

#[test]
fn names_and_identifiers_do_not_matter() {
    for code in [4326, 2180, 32633, 4978, 5773, 9707, 27700] {
        let crs = epsg(code);
        assert!(crs.equivalent(&crs, false), "EPSG:{code}");
        let bare = edited(code, anonymous);
        assert_eq!(bare.code(), None);
        assert!(crs.equivalent(&bare, false), "EPSG:{code} without ids");
        assert!(
            bare.equivalent(&crs, false),
            "EPSG:{code} without ids, reversed"
        );
    }
}

#[test]
fn axis_order_matters_unless_ignored() {
    let crs84 = edited(4326, |v| {
        v["coordinate_system"]["axis"]
            .as_array_mut()
            .unwrap()
            .reverse();
    });
    assert!(!epsg(4326).equivalent(&crs84, false));
    assert!(epsg(4326).equivalent(&crs84, true));
    // Different units are never the same axes.
    let grads = edited(4326, |v| {
        for axis in v["coordinate_system"]["axis"].as_array_mut().unwrap() {
            axis["unit"] = serde_json::json!({
                "type": "AngularUnit", "name": "grad", "conversion_factor": 0.015707963267949
            });
        }
    });
    assert!(!epsg(4326).equivalent(&grads, true));
}

#[test]
fn different_definitions_are_not_equivalent() {
    // Another datum, ellipsoid or kind of CRS.
    assert!(!epsg(4326).equivalent(&epsg(4258), false));
    assert!(!epsg(4326).equivalent(&epsg(4978), false));
    assert!(!epsg(4326).equivalent(&epsg(4979), false));
    // Another zone: one parameter differs.
    assert!(!epsg(32633).equivalent(&epsg(32634), false));
    // Another projection of the same datum.
    assert!(!epsg(3857).equivalent(&epsg(32633), false));
    // Another vertical datum.
    assert!(!epsg(5773).equivalent(&epsg(3855), false));
}

#[test]
fn numbers_agree_to_a_relative_1e_10() {
    let nudged = |by: f64| {
        edited(32633, |v| {
            let fe = &mut v["conversion"]["parameters"][3]["value"];
            *fe = Value::from(fe.as_f64().unwrap() * (1.0 + by));
        })
    };
    assert!(epsg(32633).equivalent(&nudged(1e-12), false));
    assert!(!epsg(32633).equivalent(&nudged(1e-6), false));
}

#[test]
fn compound_crss_compare_their_parts() {
    let built = Crs::compound(&epsg(4326), &epsg(5773)).unwrap();
    assert!(epsg(9707).equivalent(&built, false));
    let other = Crs::compound(&epsg(4326), &epsg(3855)).unwrap();
    assert!(!epsg(9707).equivalent(&other, false));
}
