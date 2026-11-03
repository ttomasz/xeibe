//! Parts of CRSs on their own compared as CRSs compare them
//! (`equivalent_parts`), against PROJ's `proj_is_equivalent_to` over
//! EPSG's ellipsoids, prime meridians and datums (pyproj's `==`).

use std::collections::HashMap;

use serde_json::{Value, json};
use xeibe_crs::{EpsgObject, ObjectKind, objects};
use xeibe_reproject::{equivalent_parts, wkt};

fn parts(kind: ObjectKind) -> Vec<(&'static EpsgObject, Value)> {
    objects(kind)
        .map(|o| (o, serde_json::from_str(o.projjson).unwrap()))
        .collect()
}

/// The pairs of EPSG's objects that compare equivalent, by code.
fn equivalent_pairs(group: &[(&EpsgObject, Value)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for (i, (x, a)) in group.iter().enumerate() {
        for (y, b) in &group[i + 1..] {
            if equivalent_parts(a, b).unwrap() == Some(true) {
                out.push((x.code, y.code));
            }
        }
    }
    out
}

#[test]
fn each_part_is_its_own_wkt() {
    for kind in [
        ObjectKind::Ellipsoid,
        ObjectKind::PrimeMeridian,
        ObjectKind::Datum,
    ] {
        for (o, v) in parts(kind) {
            let text =
                wkt::from_projjson_value(&v, wkt::Version::Wkt2_2019, false, wkt::AxisRule::Auto)
                    .unwrap();
            let back: Value =
                serde_json::from_str(&wkt::object_to_projjson(&text).unwrap()).unwrap();
            assert_eq!(
                equivalent_parts(&v, &back).unwrap(),
                Some(true),
                "{kind:?} {}: {text}",
                o.code
            );
        }
    }
}

/// PROJ's pairs: the inverse flattenings compared where both ellipsoids
/// have one, else the semi-minor axes, so that GRS 1980 and WGS 84
/// (semi-minor axes 1.6e-11 apart) stay apart.
#[test]
fn ellipsoids_as_proj_compares_them() {
    assert_eq!(
        equivalent_pairs(&parts(ObjectKind::Ellipsoid)),
        [
            (1024, 7019), // CGCS2000, GRS 1980
            (7003, 7050), // Australian National Spheroid, GRS 1967 Modified
            (7006, 7046), // Bessel Namibia, in metres and German legal metres
            (7013, 7034), // Clarke 1880 (Arc), Clarke 1880: semi-minor axes 0.2 mm apart
            (7030, 7031), // WGS 84, GEM 10C
            (7047, 7052), // two authalic spheres of one radius
        ]
    );
    assert!(equivalent_pairs(&parts(ObjectKind::PrimeMeridian)).is_empty());
}

/// No two of EPSG's datums are equivalent. PROJ's are in six pairs, where
/// one datum's alias is the other's name (`Mhast` is an alias of both
/// `Mhast (onshore)` and `Mhast (offshore)`, which PROJ keeps apart):
/// equivalence by name is transitive in ours. Only datums of one type
/// and semi-major axis are compared: no others can be equivalent.
#[test]
fn no_two_datums_are_equivalent() {
    let mut groups: HashMap<(String, u64), Vec<(&EpsgObject, Value)>> = HashMap::new();
    for (o, v) in parts(ObjectKind::Datum) {
        let a = v["ellipsoid"]["semi_major_axis"].as_f64().unwrap_or(0.0);
        let key = (v["type"].as_str().unwrap().to_owned(), a.round() as u64);
        groups.entry(key).or_default().push((o, v));
    }
    let pairs: Vec<_> = groups.values().flat_map(|g| equivalent_pairs(g)).collect();
    assert!(pairs.is_empty(), "{pairs:?}");
}

#[test]
fn parts_written_another_way() {
    let feet =
        json!({"type": "LinearUnit", "name": "Clarke's foot", "conversion_factor": 0.3047972654});
    let metres = |v: f64| v * 0.3047972654;
    // Clarke 1858 in feet by its semi-axes, and in metres by its inverse
    // flattening, as WKT writes it.
    let a = json!({"type": "Ellipsoid", "name": "Clarke 1858",
        "semi_major_axis": {"value": 20926348.0, "unit": feet},
        "semi_minor_axis": {"value": 20855233.0, "unit": feet}});
    let b = json!({"type": "Ellipsoid", "name": "another name",
        "semi_major_axis": metres(20926348.0),
        "inverse_flattening": 20926348.0 / (20926348.0 - 20855233.0)});
    assert_eq!(equivalent_parts(&a, &b).unwrap(), Some(true));
    // Paris in grads and in degrees.
    let grad =
        json!({"type": "AngularUnit", "name": "grad", "conversion_factor": 0.015707963267949});
    let paris = json!({"type": "PrimeMeridian", "name": "Paris",
        "longitude": {"value": 2.5969213, "unit": grad}});
    let degrees = json!({"type": "PrimeMeridian", "name": "Paris", "longitude": 2.33722917});
    assert_eq!(equivalent_parts(&paris, &degrees).unwrap(), Some(true));
    // An ensemble and a datum, of different kinds, and parts not compared.
    let ensemble: Value =
        serde_json::from_str(xeibe_crs::object(ObjectKind::Datum, 6326).unwrap().projjson).unwrap();
    let mut frame = ensemble.clone();
    frame["type"] = json!("GeodeticReferenceFrame");
    frame.as_object_mut().unwrap().remove("members");
    assert_eq!(equivalent_parts(&ensemble, &frame).unwrap(), Some(false));
    assert_eq!(equivalent_parts(&a, &paris).unwrap(), Some(false));
    let cs = json!({"type": "CoordinateSystem", "subtype": "Cartesian", "axis": []});
    assert_eq!(equivalent_parts(&cs, &cs).unwrap(), None);
}
