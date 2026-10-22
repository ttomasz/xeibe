//! Reading WKT (`docs/crs-wkt.md`): EPSG's own WKT2 export, read and
//! compared with the PROJJSON `xeibe-crs` generates from the same EPSG
//! release, and PROJ's WKT2, read and compared with the EPSG CRS
//! (`scripts/gen_wkt_vectors.py`).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use xeibe_reproject::{Crs, Error, wkt};

/// The records of a JSONL file whose first line is a header.
fn records(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Keys EPSG's WKT export leaves out (usages, remarks), and keys only the
/// WKT has: anchor epochs and geoid models, which the generator doesn't
/// emit. Ids that only the WKT has (conversions, coordinate systems) and
/// the coordinate system of a base CRS, which WKT doesn't carry and the
/// reader fills in, are skipped in `normalise`.
const IGNORED: &[&str] = &[
    "$schema",
    "usages",
    "scope",
    "area",
    "bbox",
    "remarks",
    "vertical_extent",
    "temporal_extent",
    "anchor",
    "anchor_epoch",
    "geoid_model",
    "geoid_models",
    "deformation_models",
];

/// A unit as (name, factor), whichever way it is spelled.
fn unit(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            let factor = match s.as_str() {
                "metre" | "unity" => 1.0,
                "degree" => 0.0174532925199433,
                _ => f64::NAN,
            };
            json!({"name": s, "factor": factor})
        }
        Value::Object(o) => json!({"name": o["name"], "factor": o["conversion_factor"]}),
        other => other.clone(),
    }
}

/// `{value, unit}` or a bare number in the default unit, as a value in SI
/// units.
fn si(v: &Value, default_factor: f64) -> f64 {
    match v {
        Value::Object(o) => {
            o["value"].as_f64().unwrap() * unit(&o["unit"])["factor"].as_f64().unwrap()
        }
        other => other.as_f64().unwrap() * default_factor,
    }
}

/// Both sides in one shape: SI measures, units as (name, factor), an
/// ellipsoid's shape as (a, 1/f), absent and empty parameter lists alike.
fn normalise(v: &Value, key: &str) -> Value {
    match v {
        Value::Object(o) => {
            if key == "unit" {
                return unit(v);
            }
            if key == "ellipsoid" {
                let a = o.get("semi_major_axis").or(o.get("radius")).unwrap();
                let a = si(a, 1.0);
                let rf = match (o.get("inverse_flattening"), o.get("semi_minor_axis")) {
                    (Some(rf), _) => rf.as_f64().unwrap(),
                    (None, Some(b)) => a / (a - si(b, 1.0)),
                    (None, None) => 0.0,
                };
                return json!({"name": o["name"], "a": a, "rf": rf, "id": o.get("id")});
            }
            let mut out = Map::new();
            for (k, v) in o {
                if IGNORED.contains(&k.as_str()) || v.is_null() {
                    continue;
                }
                if (k == "id" && matches!(key, "coordinate_system" | "conversion"))
                    || (k == "coordinate_system" && key == "base_crs")
                {
                    continue;
                }
                let v = match (key, k.as_str()) {
                    // EPSG's export rounds prime meridians: Paris is
                    // 0.040792344 radian, 2.5969213 grad in the database.
                    ("prime_meridian", "longitude") => {
                        json!((si(v, 0.0174532925199433) * 1e9).round() / 1e9)
                    }
                    ("meridian", "longitude") => json!(si(v, 0.0174532925199433)),
                    ("datum_ensemble", "accuracy") => {
                        json!(v.as_str().unwrap().parse::<f64>().unwrap())
                    }
                    _ => normalise(v, k),
                };
                out.insert(k.clone(), v);
            }
            if key == "conversion" && !out.contains_key("parameters") {
                out.insert("parameters".into(), json!([]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| normalise(x, key)).collect()),
        other => other.clone(),
    }
}

/// Where `a` and `b` differ, as `path: a != b`, numbers within 1e-11
/// relative.
fn diff(a: &Value, b: &Value, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                let p = format!("{path}.{k}");
                match (x.get(k), y.get(k)) {
                    (Some(u), Some(v)) => diff(u, v, &p, out),
                    (u, v) => out.push(format!("{p}: {u:?} != {v:?}")),
                }
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                diff(u, v, &format!("{path}[{i}]"), out);
            }
        }
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            if (x - y).abs() > 1e-11 * x.abs().max(y.abs()) {
                out.push(format!("{path}: {x} != {y}"));
            }
        }
        _ if a == b => {}
        _ => out.push(format!("{path}: {a} != {b}")),
    }
}

/// The differences between our reading of `text` and `xeibe-crs`'s
/// PROJJSON for `code`.
fn compare(code: u32, text: &str) -> Result<Vec<String>, Error> {
    let ours: Value = serde_json::from_str(&wkt::to_projjson(text)?).unwrap();
    let theirs: Value = serde_json::from_str(xeibe_crs::projjson(code).unwrap()).unwrap();
    let mut out = Vec::new();
    diff(&normalise(&ours, ""), &normalise(&theirs, ""), "", &mut out);
    Ok(out)
}

/// A difference's path with array indices dropped, to group them.
fn category(d: &str) -> String {
    let path = d.split(':').next().unwrap_or(d);
    let mut out = String::new();
    let mut skip = false;
    for c in path.chars() {
        match c {
            '[' => skip = true,
            ']' => skip = false,
            _ if !skip => out.push(c),
            _ => {}
        }
    }
    out
}

/// Known: PROJ's WKT2 2015 writes the GR96 ensemble (EPSG:1421) under the
/// name of the datum it superseded, "Greenland 1996" (EPSG:6747). Telling
/// them apart needs EPSG's datum names, which come with WKT1
/// (`docs/crs-wkt.md`, decision 4).
fn known(source: &str, text: &str) -> bool {
    source == "PROJ's WKT2_2015" && text.contains(r#"DATUM["Greenland 1996""#)
}

/// Why the CRS read from `text` isn't the EPSG one, for codes
/// `Crs::from_epsg` takes.
fn converts(code: u32, text: &str, source: &str) -> Option<String> {
    let epsg = Crs::from_epsg(code).ok()?;
    if known(source, text) {
        return None;
    }
    match Crs::from_wkt(text) {
        Ok(crs) if crs.equivalent(&epsg, false) => None,
        Ok(_) => Some(format!("{source}: not equivalent")),
        Err(e) => Some(format!("{source}: {e}")),
    }
}

/// Reads every record of `epsg.jsonl` (EPSG's own WKT) and of `wkt.jsonl`
/// (PROJ's WKT2 2019 and 2015), and returns what went wrong, grouped:
///
/// - EPSG's WKT must read as the PROJJSON `xeibe-crs` has for the code;
/// - any WKT2 for a code `Crs::from_epsg` takes must give a CRS
///   `equivalent` to it.
fn check(dir: &Path) -> (usize, String) {
    let mut failed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut differing: BTreeMap<String, Vec<(u32, String)>> = BTreeMap::new();
    let mut read = 0;
    for r in records(&dir.join("epsg.jsonl")) {
        let code = r["code"].as_u64().unwrap() as u32;
        let text = r["wkt"].as_str().unwrap();
        if xeibe_crs::projjson(code).is_none() {
            continue;
        }
        read += 1;
        match compare(code, text) {
            Err(e) => failed
                .entry(format!("EPSG's WKT: {e}"))
                .or_default()
                .push(code.to_string()),
            Ok(d) => {
                for d in d {
                    differing.entry(category(&d)).or_default().push((code, d));
                }
            }
        }
        if let Some(e) = converts(code, text, "EPSG's WKT") {
            failed.entry(e).or_default().push(code.to_string());
        }
    }
    for r in records(&dir.join("wkt.jsonl")) {
        let code = r["code"].as_u64().unwrap() as u32;
        for version in ["WKT2_2019", "WKT2_2015"] {
            if let Some(text) = r["wkt"][version].as_str() {
                read += 1;
                if let Some(e) = converts(code, text, &format!("PROJ's {version}")) {
                    failed.entry(e).or_default().push(code.to_string());
                }
            }
        }
    }
    let mut report = String::new();
    for (e, codes) in &failed {
        report += &format!(
            "{} x {e}: {:?}\n",
            codes.len(),
            &codes[..codes.len().min(8)]
        );
    }
    for (c, ds) in &differing {
        report += &format!("{} differ at {c}: {}\n", ds.len(), ds[0].1);
    }
    (read, report)
}

#[test]
fn epsg_and_proj_wkt2_read_as_the_epsg_crs() {
    let (read, report) = check(&xeibe_testkit::data_dir().join("crs-wkt"));
    assert!(read > 250, "{read}");
    assert!(report.is_empty(), "{report}");
}

#[test]
#[ignore = "needs target/crs-wkt/ (scripts/gen_wkt_vectors.py --all)"]
fn every_epsg_and_proj_wkt2_reads_as_the_epsg_crs() {
    let (read, report) = check(&xeibe_testkit::data_dir().join("../../target/crs-wkt"));
    println!("{read} WKT strings\n{report}");
    assert!(report.is_empty(), "{report}");
}

#[test]
fn wkt_crs_converts_like_the_epsg_one() {
    let wkt = r#"PROJCRS["ETRS89 / UTM zone 32N",
        BASEGEOGCRS["ETRS89",
            DATUM["European Terrestrial Reference System 1989",
                ELLIPSOID["GRS 1980",6378137,298.257222101,LENGTHUNIT["metre",1]],
                ID["EPSG",6258]],
            PRIMEM["Greenwich",0,ANGLEUNIT["degree",0.0174532925199433]],
            ID["EPSG",4258]],
        CONVERSION["UTM zone 32N",
            METHOD["Transverse Mercator"],
            PARAMETER["Latitude of natural origin",0],
            PARAMETER["Longitude of natural origin",9],
            PARAMETER["Scale factor at natural origin",0.9996],
            PARAMETER["False easting",500000],
            PARAMETER["False northing",0]],
        CS[Cartesian,2],
            AXIS["(E)",east,ORDER[1]],
            AXIS["(N)",north,ORDER[2]],
            LENGTHUNIT["metre",1],
        USAGE[SCOPE["Engineering survey."],AREA["Europe 6°E to 12°E."],BBOX[38.76,6,84.33,12]],
        ID["EPSG",25832]]"#;
    let json: Value = serde_json::from_str(&wkt::to_projjson(wkt).unwrap()).unwrap();
    // Parameters without units take the CRS's (metre) and the base's
    // (degree), and one usage is written flat.
    assert_eq!(json["conversion"]["parameters"][1]["unit"], "degree");
    assert_eq!(json["conversion"]["parameters"][2]["unit"], "unity");
    assert_eq!(json["conversion"]["parameters"][3]["unit"], "metre");
    assert_eq!(json["coordinate_system"]["axis"][0]["abbreviation"], "E");
    assert_eq!(json["area"], "Europe 6°E to 12°E.");
    assert_eq!(json["bbox"]["east_longitude"], 12);
    // Method and parameters without ids are found by their EPSG names.
    let from_wkt = Crs::from_wkt(wkt).unwrap();
    assert!(from_wkt.equivalent(&Crs::from_epsg(25832).unwrap(), false));
}

#[test]
fn wkt_2015_spellings() {
    // GEODCRS with an ellipsoidal CS is how WKT2 2015 writes a geographic
    // CRS; round brackets, lower-case keywords and SPHEROID are allowed.
    let wkt = r#"geodcrs("NTF (Paris)",
        datum("Nouvelle Triangulation Francaise (Paris)",
            spheroid("Clarke 1880 (IGN)",6378249.2,293.466021293627)),
        primem("Paris",2.5969213,angleunit("grad",0.015707963267949)),
        cs(ellipsoidal,2),
            axis("latitude (Lat)",NORTH),axis("longitude (Lon)",EAST),
            angleunit("grad",0.015707963267949),
        scope("Geodesy."),area("France."),bbox(41.31,-4.87,51.14,9.63),
        id("EPSG","4807"))"#;
    let json: Value = serde_json::from_str(&wkt::to_projjson(wkt).unwrap()).unwrap();
    assert_eq!(json["type"], "GeographicCRS");
    assert_eq!(json["id"], json!({"authority": "EPSG", "code": 4807}));
    assert_eq!(
        json["datum"]["prime_meridian"]["longitude"]["value"],
        2.5969213
    );
    assert_eq!(json["coordinate_system"]["axis"][0]["direction"], "north");
    assert_eq!(json["coordinate_system"]["axis"][1]["unit"]["name"], "grad");
    assert_eq!(json["scope"], "Geodesy.");
    assert!(
        Crs::from_wkt(wkt)
            .unwrap()
            .equivalent(&Crs::from_epsg(4807).unwrap(), false)
    );
}

#[test]
fn what_is_not_read_is_named() {
    let unknown = r#"GEOGCRS["x",DATUM["y",ELLIPSOID["z",6378137,298.257223563]],
        CS[ellipsoidal,2],AXIS["lat",north],AXIS["lon",east],
        ANGLEUNIT["degree",0.0174532925199433],FROBNICATE[1]]"#;
    let e = wkt::to_projjson(unknown).unwrap_err();
    assert!(e.to_string().contains("FROBNICATE in GEOGCRS"), "{e}");
    let wkt1 = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],
        PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    assert!(matches!(
        wkt::to_projjson(wkt1),
        Err(Error::Unsupported { what, .. }) if what == "WKT1"
    ));
    let missing = r#"GEOGCRS["x",CS[ellipsoidal,2],AXIS["lat",north],AXIS["lon",east]]"#;
    assert!(matches!(wkt::to_projjson(missing), Err(Error::Wkt { .. })));
}
