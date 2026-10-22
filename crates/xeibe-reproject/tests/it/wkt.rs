//! Reading WKT (`docs/crs-wkt.md`): EPSG's own WKT2 export, read and
//! compared with the PROJJSON `xeibe-crs` generates from the same EPSG
//! release, and PROJ's WKT2, read and compared with the EPSG CRS
//! (`scripts/gen_wkt_vectors.py`).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use xeibe_reproject::{AxisOrder, Crs, Error, Transform, wkt};

/// The records of a JSONL file whose first line is a header.
fn records(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Keys EPSG's WKT export leaves out (usages, remarks), and keys only the
/// WKT has: geoid models, which the generator doesn't emit. Ids that only the WKT has (conversions, coordinate systems) and
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

/// Why the CRS read from `text` isn't the EPSG one, for codes
/// `Crs::from_epsg` takes.
fn converts(code: u32, text: &str, source: &str) -> Option<String> {
    let epsg = Crs::from_epsg(code).ok()?;
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
    let towgs84 = r#"GEOGCS["x",DATUM["y",SPHEROID["GRS 1980",6378137,298.257222101],
        TOWGS84[0,0,0,0,0,0,0]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    assert!(matches!(
        wkt::to_projjson(towgs84),
        Err(Error::Unsupported { what, .. }) if what.contains("TOWGS84")
    ));
    let missing = r#"GEOGCRS["x",CS[ellipsoidal,2],AXIS["lat",north],AXIS["lon",east]]"#;
    assert!(matches!(wkt::to_projjson(missing), Err(Error::Wkt { .. })));
}

// --- writing ---------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Token {
    Open(String),
    Close,
    Text(String),
    Number(f64),
    Word(String),
}

/// WKT as tokens, without commas and white space.
fn tokens(s: &str) -> Vec<Token> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            let mut t = String::new();
            i += 1;
            loop {
                if b[i] == b'"' {
                    if b.get(i + 1) == Some(&b'"') {
                        t.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                let ch = s[i..].chars().next().unwrap();
                t.push(ch);
                i += ch.len_utf8();
            }
            out.push(Token::Text(t));
        } else if c == b']' {
            out.push(Token::Close);
            i += 1;
        } else if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'+' | b'.') {
            let start = i;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'-' | b'+' | b'.' | b'_'))
            {
                i += 1;
            }
            let word = &s[start..i];
            if b.get(i) == Some(&b'[') {
                out.push(Token::Open(word.to_owned()));
                i += 1;
            } else if let Ok(v) = word.parse() {
                out.push(Token::Number(v));
            } else {
                out.push(Token::Word(word.to_owned()));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Where two WKT texts first differ: the keywords enclosing it and the two
/// tokens. Numbers within 1e-11 relative are the same.
fn first_difference(ours: &str, theirs: &str) -> Option<(String, String)> {
    let (a, b) = (tokens(ours), tokens(theirs));
    let mut path: Vec<&str> = Vec::new();
    for i in 0..a.len().max(b.len()) {
        let same = match (a.get(i), b.get(i)) {
            (Some(Token::Number(x)), Some(Token::Number(y))) => {
                (x - y).abs() <= 1e-11 * x.abs().max(y.abs())
            }
            (x, y) => x == y,
        };
        if !same {
            return Some((path.join("/"), format!("{:?} != {:?}", a.get(i), b.get(i))));
        }
        match &a[i] {
            Token::Open(k) => path.push(k),
            Token::Close => {
                path.pop();
            }
            _ => {}
        }
    }
    None
}

/// PROJ writes the remarks its database has; `xeibe-crs`'s PROJJSON has
/// none. The last node before the final bracket.
fn without_remark(wkt: &str) -> &str {
    match wkt.rfind(",REMARK[") {
        Some(at) if wkt.ends_with("\"]]") => &wkt[..at],
        _ => wkt.strip_suffix(']').unwrap_or(wkt),
    }
}

/// Where our WKT differs from PROJ's because PROJ's database differs from
/// EPSG's dataset, not because of how either writes WKT:
///
/// - PROJ stores EPSG's derived vertical CRSs (depths, feet) as plain ones
///   with their base's datum, so it writes `VDATUM` where EPSG's
///   definition, and ours, has `BASEVERTCRS` and a conversion;
/// - PROJ fixes Modified Krovak's evaluation point and C1 to C10 in code
///   and leaves them out, where EPSG gives them as parameters;
/// - PROJ makes a vertical datum with a frame reference epoch dynamic.
///   EPSG types RH2000 (EPSG:5208) as a plain vertical datum with an epoch
///   of 2000, and its own WKT has no `DYNAMIC`; we follow EPSG.
fn known_writing(version: wkt::Version, code: u32, at: &str, what: &str) -> bool {
    if version == wkt::Version::Wkt1Gdal {
        return KNOWN_WKT1_WRITING
            .iter()
            .any(|(codes, _)| codes.contains(&code));
    }
    (at.ends_with("VERTCRS") && what.starts_with(r#"Some(Open("BASEVERTCRS"))"#))
        || (at.ends_with("PROJCRS/CONVERSION") && what.starts_with(r#"Some(Open("PARAMETER"))"#))
        || (at.ends_with("VERTCRS") && what == r#"Some(Open("VDATUM")) != Some(Open("DYNAMIC"))"#)
}

/// Our WKT2 2019 for each EPSG CRS in `wkt.jsonl` against PROJ's, and read
/// back by our reader as a CRS equivalent to the EPSG one.
/// Where our WKT1_GDAL knowingly differs from PROJ's.
const KNOWN_WKT1_WRITING: &[(&[u32], &str)] = &[
    // PROJ writes this deprecated CRS through a path of its own, with
    // TOWGS84 and without the "(deprecated)" it gives other deprecated
    // CRSs.
    (&[3785], "Popular Visualisation CRS / Mercator"),
];

fn check_writing(path: &Path, version: wkt::Version) -> (usize, usize, String) {
    let key = match version {
        wkt::Version::Wkt1Gdal => "WKT1_GDAL",
        _ => "WKT2_2019",
    };
    let mut same = 0;
    let mut written = 0;
    let mut differing: BTreeMap<String, Vec<(u32, String)>> = BTreeMap::new();
    for r in records(path) {
        let code = r["code"].as_u64().unwrap() as u32;
        let Some(json) = xeibe_crs::projjson(code) else {
            continue;
        };
        let ours = wkt::from_projjson(json, version, false);
        let Some(theirs) = r["wkt"][key].as_str() else {
            // PROJ can't write it either.
            if let Ok(ours) = ours {
                differing
                    .entry("written where PROJ can't".into())
                    .or_default()
                    .push((code, ours));
            }
            continue;
        };
        let ours = match ours {
            Ok(w) => w,
            Err(e) => {
                differing
                    .entry(format!("error: {e}"))
                    .or_default()
                    .push((code, String::new()));
                continue;
            }
        };
        written += 1;
        // Read back, the CRS is the EPSG one; in WKT1 only when PROJ's own
        // WKT1 reads back so (it can't say northing, easting).
        let expected = match version {
            wkt::Version::Wkt1Gdal => r["read"][key]["equals"].as_bool().unwrap_or(false),
            _ => true,
        };
        if let Ok(epsg) = Crs::from_epsg(code)
            && Crs::from_wkt(&ours).is_ok_and(|c| c.equivalent(&epsg, false)) != expected
            && !KNOWN_WKT1.iter().any(|(codes, _)| codes.contains(&code))
        {
            differing
                .entry("read back: not the EPSG CRS".into())
                .or_default()
                .push((code, ours.clone()));
        }
        match first_difference(without_remark(&ours), without_remark(theirs)) {
            None => same += 1,
            Some((at, what)) if known_writing(version, code, &at, &what) => same += 1,
            Some((at, what)) => differing.entry(at).or_default().push((code, what)),
        }
    }
    let mut report = String::new();
    for (at, ds) in &differing {
        let codes: Vec<u32> = ds.iter().take(6).map(|d| d.0).collect();
        report += &format!("{} at {at}: {} {codes:?}\n", ds.len(), ds[0].1);
    }
    (written, same, report)
}

#[test]
fn epsg_crss_write_as_proj_writes_them() {
    let (written, same, report) = check_writing(
        &xeibe_testkit::data_dir().join("crs-wkt/wkt.jsonl"),
        wkt::Version::Wkt2_2019,
    );
    assert!(written > 80, "{written}");
    assert_eq!(same, written, "{report}");
}

#[test]
#[ignore = "needs target/crs-wkt/wkt.jsonl (scripts/gen_wkt_vectors.py --all)"]
fn every_epsg_crs_writes_as_proj_writes_it() {
    let (written, same, report) = check_writing(
        &xeibe_testkit::data_dir().join("../../target/crs-wkt/wkt.jsonl"),
        wkt::Version::Wkt2_2019,
    );
    println!("{written} written, {same} as PROJ writes them (or a listed difference)\n{report}");
    assert_eq!(same, written, "{report}");
}

#[test]
fn epsg_crss_write_wkt1_as_proj_writes_them() {
    let (written, same, report) = check_writing(
        &xeibe_testkit::data_dir().join("crs-wkt/wkt.jsonl"),
        wkt::Version::Wkt1Gdal,
    );
    assert!(written > 80, "{written}");
    assert!(report.is_empty(), "{report}");
    assert_eq!(same, written);
}

#[test]
#[ignore = "needs target/crs-wkt/wkt.jsonl (scripts/gen_wkt_vectors.py --all)"]
fn every_epsg_crs_writes_wkt1_as_proj_writes_it() {
    let (written, same, report) = check_writing(
        &xeibe_testkit::data_dir().join("../../target/crs-wkt/wkt.jsonl"),
        wkt::Version::Wkt1Gdal,
    );
    println!("{written} written, {same} as PROJ writes them (or a listed difference)\n{report}");
    assert!(report.is_empty(), "{report}");
}

#[test]
fn pretty_wkt_is_indented_as_proj_indents_it() {
    let text = wkt::from_projjson(
        xeibe_crs::projjson(9651).unwrap(),
        wkt::Version::Wkt2_2019,
        true,
    )
    .unwrap();
    // PROJ 9.8.1's `CRS(9651).to_wkt(pretty=True)`.
    let proj = r#"VERTCRS["PL-EVRF2007 height",
    VDATUM["European Vertical Reference Frame 2007 Poland"],
    CS[vertical,1],
        AXIS["gravity-related height (H)",up,
            LENGTHUNIT["metre",1]],
    USAGE[
        SCOPE["Geodesy, engineering survey, topographic mapping."],
        AREA["Poland - onshore."],
        BBOX[49,14.14,54.89,24.15]],
    ID["EPSG",9651]]"#;
    assert_eq!(text, proj);
    // Axes one level deeper than their CS, units in grads.
    let text = wkt::from_projjson(
        xeibe_crs::projjson(4807).unwrap(),
        wkt::Version::Wkt2_2019,
        true,
    )
    .unwrap();
    let proj = r#"GEOGCRS["NTF (Paris)",
    DATUM["Nouvelle Triangulation Francaise (Paris)",
        ELLIPSOID["Clarke 1880 (IGN)",6378249.2,293.466021293627,
            LENGTHUNIT["metre",1]]],
    PRIMEM["Paris",2.5969213,
        ANGLEUNIT["grad",0.0157079632679489]],
    CS[ellipsoidal,2],
        AXIS["geodetic latitude (Lat)",north,
            ORDER[1],
            ANGLEUNIT["grad",0.0157079632679489]],
        AXIS["geodetic longitude (Lon)",east,
            ORDER[2],
            ANGLEUNIT["grad",0.0157079632679489]],
    USAGE[
        SCOPE["Geodesy."],
        AREA["France - onshore - mainland and Corsica."],
        BBOX[41.31,-4.87,51.14,9.63]],
    ID["EPSG",4807]]"#;
    assert_eq!(text, proj);
}

// --- WKT1 ------------------------------------------------------------------

/// The axis directions of a PROJJSON CRS, a compound CRS's in turn.
fn directions(v: &Value) -> Vec<String> {
    match v["components"].as_array() {
        Some(list) => list.iter().flat_map(directions).collect(),
        None => v["coordinate_system"]["axis"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| a["direction"].as_str().unwrap_or("").to_owned())
            .collect(),
    }
}

/// Where our reading of PROJ's WKT1 knowingly differs from PROJ's:
const KNOWN_WKT1: &[(&[u32], &str)] = &[
    // PROJ compares axes along meridians by direction only. UPS North and
    // South and RSPS2000 have northing first in EPSG; WKT1 without AXIS
    // gives easting first, which PROJ calls equal and we don't.
    (
        &[5482, 32661, 32761],
        "PROJ ignores the order of polar axes",
    ),
    // Deprecated method 9823 written as Equirectangular reads back as 1029.
    (&[3786], "a deprecated method under another's WKT1 name"),
    // Axes along meridians that amount to east and north (EPSG's) against
    // WKT1's east and north: equal here, not in PROJ.
    (&[3408, 3409, 27702], "polar axes that are east and north"),
    // "degree minute second hemisphere" with a degree's factor.
    (&[4226], "a unit equal by factor, not by name"),
    // GDAL writes M'poraloko as M_poraloko; the AUTHORITY ids agree.
    (&[4266, 26632, 26692], "a datum equal by id, not by name"),
    // An unspecified axis direction, which `Crs` doesn't take.
    (&[5817], "an unspecified axis"),
    // TOWGS84: a bound CRS, step 6 of docs/crs-wkt.md.
    (&[3785], "TOWGS84"),
];

/// Reads PROJ's WKT1 for each EPSG CRS in `wkt.jsonl` and compares with
/// PROJ's own reading of it: the same axes, and equivalent to the EPSG CRS
/// exactly when PROJ finds it equal.
fn check_wkt1(path: &Path, version: &str) -> (usize, String) {
    let mut read = 0;
    let mut differing: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for r in records(path) {
        let code = r["code"].as_u64().unwrap() as u32;
        let Some(text) = r["wkt"][version].as_str() else {
            continue;
        };
        let proj = &r["read"][version];
        read += 1;
        if KNOWN_WKT1.iter().any(|(codes, _)| codes.contains(&code)) {
            continue;
        }
        let ours: Value = match wkt::to_projjson(text) {
            Ok(j) => serde_json::from_str(&j).unwrap(),
            Err(e) => {
                differing
                    .entry(format!("error: {e}"))
                    .or_default()
                    .push(code);
                continue;
            }
        };
        let theirs: Vec<String> = proj["axes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| a.as_str().unwrap().to_owned())
            .collect();
        if directions(&ours) != theirs {
            differing
                .entry(format!("axes {:?} != PROJ's {theirs:?}", directions(&ours)))
                .or_default()
                .push(code);
        }
        if let Ok(epsg) = Crs::from_epsg(code) {
            let equal = match Crs::from_wkt(text) {
                Ok(c) => c.equivalent(&epsg, false),
                Err(e) => {
                    differing.entry(format!("Crs: {e}")).or_default().push(code);
                    continue;
                }
            };
            if Some(equal) != proj["equals"].as_bool() {
                differing
                    .entry(format!(
                        "equivalent {equal}, PROJ's equals {}",
                        proj["equals"]
                    ))
                    .or_default()
                    .push(code);
            }
        }
    }
    let mut report = String::new();
    for (what, codes) in &differing {
        report += &format!(
            "{} x {what}: {:?}\n",
            codes.len(),
            &codes[..codes.len().min(8)]
        );
    }
    (read, report)
}

#[test]
fn proj_wkt1_gdal_reads_as_proj_reads_it() {
    let (read, report) = check_wkt1(
        &xeibe_testkit::data_dir().join("crs-wkt/wkt.jsonl"),
        "WKT1_GDAL",
    );
    assert!(read > 80, "{read}");
    assert!(report.is_empty(), "{report}");
}

#[test]
fn wkt1_reading() {
    // No AXIS and no AUTHORITY: WKT1's longitude, latitude.
    let lonlat = r#"GEOGCS["x",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],
        PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    let json: Value = serde_json::from_str(&wkt::to_projjson(lonlat).unwrap()).unwrap();
    assert_eq!(directions(&json), ["east", "north"]);
    // GDAL's WGS_1984 is EPSG's datum ensemble.
    assert_eq!(
        json["datum"]["id"],
        json!({"authority": "EPSG", "code": 6326})
    );
    // An AUTHORITY: the EPSG CRS's axes, latitude first.
    let epsg = lonlat.replace(
        r#"0.0174532925199433]]"#,
        r#"0.0174532925199433],AUTHORITY["EPSG","4326"]]"#,
    );
    let json: Value = serde_json::from_str(&wkt::to_projjson(&epsg).unwrap()).unwrap();
    assert_eq!(directions(&json), ["north", "east"]);
    assert!(
        Crs::from_wkt(&epsg)
            .unwrap()
            .equivalent(&Crs::from_epsg(4326).unwrap(), false)
    );
    // Paris written in degrees under a grad unit, as GDAL writes it.
    let paris = r#"GEOGCS["NTF (Paris)",DATUM["Nouvelle_Triangulation_Francaise_Paris",
        SPHEROID["Clarke 1880 (IGN)",6378249.2,293.466021293627]],PRIMEM["Paris",2.33722917],
        UNIT["grad",0.0157079632679489],AUTHORITY["EPSG","4807"]]"#;
    assert!(
        Crs::from_wkt(paris)
            .unwrap()
            .equivalent(&Crs::from_epsg(4807).unwrap(), false)
    );
}

#[test]
#[ignore = "needs target/crs-wkt/wkt.jsonl (scripts/gen_wkt_vectors.py --all)"]
fn every_proj_wkt1_gdal_reads_as_proj_reads_it() {
    let (read, report) = check_wkt1(
        &xeibe_testkit::data_dir().join("../../target/crs-wkt/wkt.jsonl"),
        "WKT1_GDAL",
    );
    println!("{read} WKT1_GDAL strings\n{report}");
    assert!(report.is_empty(), "{report}");
}

/// pyproj's `to_epsg()`: the best match at 70 or more.
fn to_epsg(matches: &[(u32, u8)]) -> Option<u32> {
    matches.first().filter(|m| m.1 >= 70).map(|m| m.0)
}

fn deprecated(code: u32) -> bool {
    xeibe_crs::get(code).is_some_and(|r| r.deprecated)
}

/// How a `to_epsg()` result relates to the CRS the WKT was written for.
fn outcome(code: u32, found: Option<u32>) -> &'static str {
    match found {
        Some(c) if c == code => "itself",
        Some(_) => "another code",
        None => "nothing",
    }
}

/// Where `to_epsg()` knowingly differs from PROJ's, other than where it
/// finds the CRS the WKT was written for, which is right by construction
/// (deprecated CRSs, which PROJ's search leaves out; WKT1 compound CRSs,
/// which PROJ doesn't find; a CRS by its own name before the UTM CRS PROJ
/// builds for any UTM zone on WGS 84).
const KNOWN_IDENTIFY: &[(&[u32], &str)] = &[
    // Deprecated CRSs on a deprecated datum with the name of its
    // replacement, where a live CRS has their name too (RRAF 1991,
    // IGD05): the live one, which PROJ finds at 25 only.
    (
        &[2989, 4640, 4972, 4973, 6978, 6979, 6980, 6985, 6986, 6987],
        "a live CRS of the same name and definition",
    ),
    // Lambert Azimuthal Equal Area (Spherical) on WGS 84, as WKT1 can't
    // say: read as the ellipsoidal method, as PROJ reads it, which is
    // EPSG:6931's and the others'. PROJ finds them at 25.
    (&[3973, 3974, 3975], "a spherical method in WKT1"),
    // As read (KNOWN_WKT1): "degree minute second hemisphere" with a
    // degree's factor, and 9842 written as Equirectangular.
    (&[4235], "a unit equal by factor, not by name"),
    (&[32663], "a deprecated method under another's WKT1 name"),
    // Not read yet (KNOWN_WKT1): TOWGS84, step 6 of docs/crs-wkt.md, and
    // an engineering CRS with an unspecified axis.
    (&[3785], "TOWGS84"),
    (&[5817], "an unspecified axis"),
];

/// Identifies PROJ's WKT for each EPSG CRS in `identify.jsonl`, with its
/// ids (the string as `wkt.jsonl` has it) and without, and compares
/// `to_epsg()` with PROJ's. Returns the number identified, a report of
/// the differences (by version, ids, PROJ's outcome and ours), and how
/// many CRSs were left out for a projection method `Crs` doesn't build.
fn check_identify(dir: &Path) -> (usize, String, usize) {
    let mut written: BTreeMap<(u32, String), String> = BTreeMap::new();
    for r in records(&dir.join("wkt.jsonl")) {
        let code = r["code"].as_u64().unwrap() as u32;
        for (version, text) in r["wkt"].as_object().unwrap() {
            if let Some(text) = text.as_str() {
                written.insert((code, version.clone()), text.to_owned());
            }
        }
    }
    let mut checked = 0;
    let mut unsupported = std::collections::BTreeSet::new();
    let mut differing: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for r in records(&dir.join("identify.jsonl")) {
        let code = r["code"].as_u64().unwrap() as u32;
        let version = r["version"].as_str().unwrap();
        if version == "WKT1_ESRI" {
            continue; // step 5 of docs/crs-wkt.md
        }
        if xeibe_crs::get(code).is_none() {
            continue; // EPSG's worked examples, which xeibe-crs leaves out
        }
        let known = KNOWN_IDENTIFY
            .iter()
            .any(|(codes, _)| codes.contains(&code));
        let stripped = r["wkt"].as_str().unwrap();
        let kept = &written[&(code, version.to_owned())];
        for (ids, text, proj) in [
            ("stripped", stripped, &r["stripped"]),
            ("kept", kept.as_str(), &r["kept"]),
        ] {
            let Some(theirs) = proj.get("to_epsg") else {
                continue;
            };
            let ours = match Crs::from_wkt(text) {
                Ok(crs) => to_epsg(&crs.identify()),
                Err(Error::Unsupported { what, .. })
                    if what.starts_with("the projection method") =>
                {
                    unsupported.insert(code);
                    continue;
                }
                Err(_) if known => continue,
                Err(e) => {
                    differing
                        .entry(format!("{version} {ids}: {e}"))
                        .or_default()
                        .push(code);
                    continue;
                }
            };
            checked += 1;
            let theirs = theirs.as_u64().map(|c| c as u32);
            if ours != theirs && ours != Some(code) && !known {
                differing
                    .entry(format!(
                        "{version} {ids}{}: PROJ {}, ours {}",
                        if deprecated(code) { " deprecated" } else { "" },
                        outcome(code, theirs),
                        outcome(code, ours)
                    ))
                    .or_default()
                    .push(code);
            }
        }
    }
    let mut report = String::new();
    for (what, codes) in &differing {
        report += &format!(
            "{} x {what}: {:?}\n",
            codes.len(),
            &codes[..codes.len().min(12)]
        );
    }
    (checked, report, unsupported.len())
}

#[test]
fn epsg_crss_are_identified_as_proj_identifies_them() {
    let (checked, report, _) = check_identify(&xeibe_testkit::data_dir().join("crs-wkt"));
    assert!(checked > 500, "{checked}");
    assert!(report.is_empty(), "{report}");
}

#[test]
#[ignore = "needs target/crs-wkt/ (scripts/gen_wkt_vectors.py --all)"]
fn every_epsg_crs_is_identified_as_proj_identifies_it() {
    let (checked, report, unsupported) =
        check_identify(&xeibe_testkit::data_dir().join("../../target/crs-wkt"));
    println!(
        "{checked} identifications, {unsupported} CRSs with a method Crs doesn't build\n{report}"
    );
    assert!(report.is_empty(), "{report}");
}

/// EPSG:2180 as GDAL writes it, with `name` and `datum`, and `extra`
/// (an `AUTHORITY`) at the end.
fn poland(name: &str, datum: &str, extra: &str) -> String {
    format!(
        r#"PROJCS["{name}",GEOGCS["ETRS89",DATUM["{datum}",
            SPHEROID["GRS 1980",6378137,298.257222101]],PRIMEM["Greenwich",0],
            UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],
            PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",19],
            PARAMETER["scale_factor",0.9993],PARAMETER["false_easting",500000],
            PARAMETER["false_northing",-5300000],UNIT["metre",1]{extra}]"#
    )
}

#[test]
fn identification_rules() {
    let etrs = "European_Terrestrial_Reference_System_1989";
    let identify = |text: &str| Crs::from_wkt(text).unwrap().identify();
    // No AXIS: easting first, against EPSG's northing first, and still
    // EPSG:2180, with its AUTHORITY or by its name (PROJ: 25 and 100).
    let authority = r#",AUTHORITY["EPSG","2180"]"#;
    assert_eq!(
        identify(&poland("ETRS89 / PL-1992", etrs, authority)),
        [(2180, 100)]
    );
    assert_eq!(
        identify(&poland("ETRS89 / PL-1992", etrs, ""))[0],
        (2180, 100)
    );
    // Through an alias, and under another name.
    assert_eq!(
        identify(&poland("ETRS89 / Poland CS92", etrs, ""))[0],
        (2180, 90)
    );
    assert_eq!(identify(&poland("my_poland", etrs, ""))[0], (2180, 70));
    // A datum EPSG doesn't name: the same ellipsoid, at 60, or 70 for an
    // "unknown" datum; pyproj's to_epsg() gives nothing for the first.
    assert_eq!(
        identify(&poland("my_poland", "my_datum", ""))[0],
        (2180, 60)
    );
    assert_eq!(identify(&poland("my_poland", "unknown", ""))[0], (2180, 70));
    // An AUTHORITY that doesn't fit the definition.
    assert_eq!(
        identify(&poland("my_poland", etrs, r#",AUTHORITY["EPSG","2177"]"#))
            .iter()
            .find(|m| m.0 == 2177),
        Some(&(2177, 25))
    );
    // A deprecated CRS by its own name before the live CRS it equals:
    // EPSG:3454 was deprecated for having EPSG:3455's parameters.
    let json = xeibe_crs::projjson(3454).unwrap();
    let text = wkt::from_projjson(json, wkt::Version::Wkt2_2019, false).unwrap();
    let without_id = text.replace(r#",ID["EPSG",3454]"#, "");
    assert_eq!(identify(&without_id)[..2], [(3454, 100), (3455, 70)]);
    // No engineering CRS is identified, as in PROJ.
    let json = xeibe_crs::projjson(5800).unwrap();
    let text = wkt::from_projjson(json, wkt::Version::Wkt2_2019, false).unwrap();
    assert_eq!(identify(&text), []);
}

#[test]
fn wkt_without_ids_transforms_between_datums() {
    // The datum transformation is looked up by the EPSG CRS the WKT is.
    let text = poland(
        "my_poland",
        "European_Terrestrial_Reference_System_1989",
        "",
    );
    let from_wkt = Crs::from_wkt(&text).unwrap();
    assert_eq!(from_wkt.code(), None);
    let pulkovo = Crs::from_epsg(4179).unwrap();
    let ours = Transform::new(&from_wkt, &pulkovo, AxisOrder::EastFirst).unwrap();
    let epsg = Transform::from_epsg(2180, 4179, AxisOrder::EastFirst).unwrap();
    assert_eq!(ours.operations(), epsg.operations());
    let (x, y) = (636_640.467, 483_432.201);
    assert_eq!(
        ours.convert(x, y, None).unwrap(),
        epsg.convert(x, y, None).unwrap()
    );
}
