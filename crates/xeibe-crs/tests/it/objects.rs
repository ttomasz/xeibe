//! EPSG's objects on their own (`data/objects.bin`), against PROJ's
//! (`scripts/gen_object_vectors.py`, `docs/pyproj-phase3.md`, "The objects
//! table").

use std::path::{Path, PathBuf};

use serde_json::Value;
use xeibe_crs::{CrsKind, ObjectKind, object, objects, objects_by_name};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data/crs-objects")
        .join(name)
}

fn all(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/crs-objects")
        .join(name)
}

/// The records of a vectors file, after its header line.
fn load(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}; run scripts/gen_object_vectors.py", path.display()));
    text.lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn kind(name: &str) -> ObjectKind {
    match name {
        "ellipsoid" => ObjectKind::Ellipsoid,
        "prime_meridian" => ObjectKind::PrimeMeridian,
        "datum" => ObjectKind::Datum,
        "conversion" => ObjectKind::Conversion,
        other => panic!("unknown kind {other}"),
    }
}

/// `value` without the ids of the objects inside it: PROJ writes only the
/// root's, where ours keep each object's, as a CRS's PROJJSON does.
fn without_nested_ids(value: &Value, root: bool) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| root || k.as_str() != "id")
                .filter(|(k, _)| k.as_str() != "$schema")
                .map(|(k, v)| (k.clone(), without_nested_ids(v, k == "members")))
                .collect(),
        ),
        // The members of an ensemble keep their ids, as PROJ writes them.
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| without_nested_ids(v, root)).collect())
        }
        other => other.clone(),
    }
}

/// Equal JSON, numbers to 1e-12 relative.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w))
        }
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            (x - y).abs() <= 1e-12 * x.abs().max(1.0)
        }
        _ => a == b,
    }
}

/// Objects whose PROJJSON differs from PROJ's, why given: (kind, code).
const DEVIATIONS: &[(&str, u32)] = &[
    // Rikets hojdsystem 2000: EPSG's type is a static vertical datum, and
    // its own WKT is a VDATUM without an epoch, but the record has a frame
    // reference epoch (2000), from which PROJ makes it a dynamic one.
    ("datum", 5208),
];

/// PROJ's database holds at most this many parameters of a conversion, and
/// drops the rest (Krovak Modified's evaluation point and C1 to C10).
const PROJ_MAX_PARAMETERS: usize = 7;

fn check_objects(records: &[Value]) {
    let mut failures = Vec::new();
    for r in records {
        let (k, code) = (
            r["kind"].as_str().unwrap(),
            r["code"].as_u64().unwrap() as u32,
        );
        if DEVIATIONS.contains(&(k, code)) {
            continue;
        }
        let Some(ours) = object(kind(k), code) else {
            // EPSG's worked examples are left out, as their CRSs are.
            if !r["projjson"]["name"]
                .as_str()
                .unwrap()
                .starts_with("EPSG example")
            {
                failures.push(format!("{k} {code}: missing"));
            }
            continue;
        };
        let ours: Value = serde_json::from_str(ours.projjson).unwrap();
        let mut ours = without_nested_ids(&ours, true);
        let proj = without_nested_ids(&r["projjson"], true);
        if let Some(Value::Array(params)) = ours.get_mut("parameters") {
            params.truncate(PROJ_MAX_PARAMETERS);
        }
        if !same(&ours, &proj) {
            failures.push(format!("{k} {code}:\n  ours {ours}\n  PROJ {proj}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        records.len(),
        failures[..failures.len().min(10)].join("\n")
    );
}

/// What pyproj's `from_name` finds for a class among the objects with
/// that name: a datum is looked for among ensembles, then geodetic datums,
/// then vertical ones, never engineering ones.
fn pyproj_choice(kind: ObjectKind, name: &str) -> Option<u32> {
    let found = objects_by_name(kind, name);
    if kind != ObjectKind::Datum {
        return found.first().map(|o| o.code);
    }
    let type_of = |o: &&xeibe_crs::EpsgObject| {
        let doc: Value = serde_json::from_str(o.projjson).unwrap();
        doc["type"].as_str().unwrap().to_owned()
    };
    let rank = |t: &str| match t {
        "DatumEnsemble" => Some(0),
        "GeodeticReferenceFrame" | "DynamicGeodeticReferenceFrame" => Some(1),
        "VerticalReferenceFrame" | "DynamicVerticalReferenceFrame" => Some(2),
        _ => None,
    };
    found
        .iter()
        .filter_map(|o| rank(&type_of(o)).map(|r| (r, o.code)))
        .min_by_key(|(r, _)| *r)
        .map(|(_, code)| code)
}

fn check_names(records: &[Value]) {
    let mut failures = Vec::new();
    let mut compared = 0;
    for r in records {
        let (k, name) = (
            kind(r["kind"].as_str().unwrap()),
            r["name"].as_str().unwrap(),
        );
        // Esri's names of EPSG objects: another authority's, which stay out.
        if r["source"].as_str().is_some_and(|s| s.starts_with("ESRI")) {
            continue;
        }
        // pyproj made it without PROJ's database (an ellipsoid from its
        // `+ellps` table), or found another authority's: not ours to find.
        let want = match &r["found"] {
            Value::Null => None,
            Value::Array(found) if found[0] == "EPSG" => Some(found[1].as_u64().unwrap() as u32),
            _ => continue,
        };
        // The names of EPSG's worked examples, which are left out.
        let of = r["of"].as_u64().unwrap() as u32;
        if want == Some(of) && object(k, of).is_none() {
            continue;
        }
        compared += 1;
        let got = pyproj_choice(k, name);
        if got != want {
            failures.push(format!("{k:?} {name:?}: ours {got:?}, PROJ {want:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {compared} differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}

/// PROJ's `query_geodetic_crs_from_datum` from the CRS table: the current
/// geodetic CRSs on the datum, of the type asked for, by code.
fn check_query(records: &[Value]) {
    let mut failures = Vec::new();
    for r in records {
        if r["crs_auth_name"] != "EPSG" {
            continue;
        }
        let datum = r["datum"].as_u64().unwrap() as u32;
        let kinds: &[CrsKind] = match r["pj_type"].as_str() {
            None => &[
                CrsKind::Geographic2d,
                CrsKind::Geographic3d,
                CrsKind::Geocentric,
            ],
            Some("GEOGRAPHIC_2D_CRS") => &[CrsKind::Geographic2d],
            Some("GEOGRAPHIC_3D_CRS") => &[CrsKind::Geographic3d],
            Some("GEOCENTRIC_CRS") => &[CrsKind::Geocentric],
            Some(other) => panic!("pj_type {other}"),
        };
        let ours: Vec<u32> = xeibe_crs::CRS
            .iter()
            .filter(|c| c.datum == Some(datum) && !c.deprecated && kinds.contains(&c.kind))
            .map(|c| c.code)
            .collect();
        let proj: Vec<u32> = r["crs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c[1].as_u64().unwrap() as u32)
            .collect();
        if ours != proj {
            failures.push(format!(
                "datum {datum} {}: ours {ours:?}, PROJ {proj:?}",
                r["pj_type"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} differ:\n{}",
        failures.len(),
        failures[..failures.len().min(10)].join("\n")
    );
}

#[test]
fn objects_by_code() {
    let wgs84 = object(ObjectKind::Ellipsoid, 7030).unwrap();
    assert_eq!(wgs84.name(), "WGS 84");
    assert!(!wgs84.deprecated);
    let etrs89 = object(ObjectKind::Datum, 6258).unwrap();
    assert!(etrs89.names.contains(&"ETRS89"), "{:?}", etrs89.names);
    assert!(etrs89.projjson.contains("\"type\":\"DatumEnsemble\""));
    assert_eq!(
        object(ObjectKind::Conversion, 16031).unwrap().name(),
        "UTM zone 31N"
    );
    assert_eq!(
        object(ObjectKind::PrimeMeridian, 8903).unwrap().name(),
        "Paris"
    );
    // Kinds have codes of their own.
    assert!(object(ObjectKind::Ellipsoid, 6258).is_none());
    assert_eq!(objects(ObjectKind::PrimeMeridian).count(), 15);
}

#[test]
fn objects_by_name_as_proj_finds_them() {
    let codes = |kind, name| {
        objects_by_name(kind, name)
            .iter()
            .map(|o| o.code)
            .collect::<Vec<_>>()
    };
    // EPSG's name, an alias, PROJ's own alias, any ASCII case.
    assert_eq!(
        codes(ObjectKind::Datum, "World Geodetic System 1984 ensemble"),
        [6326]
    );
    assert_eq!(codes(ObjectKind::Datum, "wgs 84"), [6326]);
    assert_eq!(codes(ObjectKind::Datum, "WGS84"), [6326]);
    assert_eq!(codes(ObjectKind::Ellipsoid, "airy 1830"), [7001]);
    assert!(
        codes(ObjectKind::Ellipsoid, "intl").is_empty(),
        "PROJ's +ellps names aren't EPSG's"
    );
    assert!(codes(ObjectKind::Datum, "W").is_empty());
}

#[test]
fn objects_match_proj() {
    check_objects(&load(&data("objects.jsonl")));
}

#[test]
fn names_match_proj() {
    check_names(&load(&data("names.jsonl")));
}

#[test]
fn query_geodetic_crs_from_datum_matches_proj() {
    check_query(&load(&data("query.jsonl")));
}

/// `scripts/gen_object_vectors.py --all` first.
#[test]
#[ignore = "needs target/crs-objects/"]
fn every_object_matches_proj() {
    check_objects(&load(&all("objects.jsonl")));
    check_names(&load(&all("names.jsonl")));
    check_query(&load(&all("query.jsonl")));
}
