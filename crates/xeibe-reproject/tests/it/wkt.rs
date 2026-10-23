//! Reading WKT (`docs/crs-wkt.md`): EPSG's own WKT2 export, read and
//! compared with the PROJJSON `xeibe-crs` generates from the same EPSG
//! release, and PROJ's WKT2, read and compared with the EPSG CRS
//! (`scripts/gen_wkt_vectors.py`).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use xeibe_reproject::{AxisOrder, Crs, Error, Grids, Transform, wkt};

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
    // TOWGS84 takes 3 or 7 values.
    let towgs84 = r#"GEOGCS["x",DATUM["y",SPHEROID["GRS 1980",6378137,298.257222101],
        TOWGS84[0,0,0,0]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    let e = wkt::to_projjson(towgs84).unwrap_err();
    assert!(e.to_string().contains("3 or 7 values"), "{e}");
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
    first_token_difference(&tokens(ours), &tokens(theirs))
}

fn first_token_difference(a: &[Token], b: &[Token]) -> Option<(String, String)> {
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
    // A datum or conversion written on its own is at the top: no path.
    (at.ends_with("VERTCRS") && what.starts_with(r#"Some(Open("BASEVERTCRS"))"#))
        || (at.ends_with("CONVERSION") && what.starts_with(r#"Some(Open("PARAMETER"))"#))
        || ((at.ends_with("VERTCRS") || at.is_empty())
            && what == r#"Some(Open("VDATUM")) != Some(Open("DYNAMIC"))"#)
}

/// Where our WKT1_GDAL knowingly differs from PROJ's.
const KNOWN_WKT1_WRITING: &[(&[u32], &str)] = &[
    // PROJ writes this deprecated CRS through a path of its own, with
    // TOWGS84 and without the "(deprecated)" it gives other deprecated
    // CRSs.
    (&[3785], "Popular Visualisation CRS / Mercator"),
];

/// Our WKT for each EPSG CRS in `wkt.jsonl` against PROJ's, and read back
/// by our reader as a CRS equivalent to the EPSG one.
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

/// `tokens` without the `ID` and `AUTHORITY` nodes: PROJ looks up the ids
/// of a CRS's parts in its database when it writes one on its own, where
/// the PROJJSON has none.
fn without_ids(tokens: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::new();
    let mut skipping = 0;
    for t in tokens {
        match &t {
            Token::Open(k) if skipping > 0 || k == "ID" || k == "AUTHORITY" => skipping += 1,
            Token::Close if skipping > 0 => skipping -= 1,
            _ if skipping > 0 => {}
            _ => out.push(t),
        }
    }
    out
}

/// A CRS's part as pyproj hands it to the writer: its PROJJSON with a type.
fn part(v: &Value, kind: &str) -> Value {
    let mut out = Map::new();
    out.insert("type".into(), json!(kind));
    for (k, x) in v.as_object().into_iter().flatten() {
        out.insert(k.clone(), x.clone());
    }
    Value::Object(out)
}

/// The parts of an EPSG CRS that pyproj's `CRS.datum`, `.ellipsoid`, …
/// return, by the names `parts.jsonl` uses.
fn parts_of(crs: &Value) -> Vec<(&'static str, Value)> {
    let holder = match crs["type"].as_str() {
        Some("ProjectedCRS") => &crs["base_crs"],
        Some("DerivedProjectedCRS") => &crs["base_crs"]["base_crs"],
        Some("DerivedVerticalCRS") => &crs["base_crs"],
        _ => crs,
    };
    let mut out = Vec::new();
    let datum = if holder["datum"].is_object() {
        holder["datum"].clone()
    } else {
        part(&holder["datum_ensemble"], "DatumEnsemble")
    };
    if datum["ellipsoid"].is_object() {
        out.push(("ellipsoid", part(&datum["ellipsoid"], "Ellipsoid")));
        let pm = match &datum["prime_meridian"] {
            Value::Null => json!({"name": "Greenwich", "longitude": 0}),
            pm => pm.clone(),
        };
        out.push(("prime_meridian", part(&pm, "PrimeMeridian")));
    }
    out.push(("datum", datum));
    out.push((
        "coordinate_system",
        part(&crs["coordinate_system"], "CoordinateSystem"),
    ));
    if crs["conversion"].is_object() && crs["type"] != "DerivedVerticalCRS" {
        out.push((
            "coordinate_operation",
            part(&crs["conversion"], "Conversion"),
        ));
    }
    out
}

/// Our WKT for each CRS in `parts.jsonl` against PROJ's: with each
/// `output_axis_rule`, and for each of its parts on its own (without ids).
fn check_parts(path: &Path) -> (usize, usize, String) {
    let mut written = 0;
    let mut same = 0;
    let mut differing: BTreeMap<String, Vec<(u32, String)>> = BTreeMap::new();
    let mut compare =
        |code: u32, at: &str, ours: Result<String, Error>, theirs: &Value, ids: bool| {
            let (ours, theirs) = match (ours, theirs.as_str()) {
                (Err(_), None) => return,
                (Ok(o), None) => {
                    differing
                        .entry(format!("{at}: written where PROJ can't"))
                        .or_default()
                        .push((code, o));
                    return;
                }
                (Err(e), Some(_)) => {
                    differing
                        .entry(format!("{at}: error: {e}"))
                        .or_default()
                        .push((code, String::new()));
                    return;
                }
                (Ok(o), Some(t)) => (o, t.to_owned()),
            };
            written += 1;
            let difference = if ids {
                first_difference(without_remark(&ours), without_remark(&theirs))
            } else {
                first_token_difference(&without_ids(tokens(&ours)), &without_ids(tokens(&theirs)))
            };
            match difference {
                None => same += 1,
                Some((path, what))
                    if known_writing(
                        if at.starts_with("WKT1") {
                            wkt::Version::Wkt1Gdal
                        } else {
                            wkt::Version::Wkt2_2019
                        },
                        code,
                        &path,
                        &what,
                    ) =>
                {
                    same += 1
                }
                Some((path, what)) => differing
                    .entry(format!("{at} {path}"))
                    .or_default()
                    .push((code, what)),
            }
        };
    for r in records(path) {
        let code = r["code"].as_u64().unwrap() as u32;
        let Some(json) = xeibe_crs::projjson(code) else {
            continue;
        };
        for (key, version, rule) in [
            (
                "WKT2_2019 false",
                wkt::Version::Wkt2_2019,
                wkt::AxisRule::Never,
            ),
            (
                "WKT1_GDAL true",
                wkt::Version::Wkt1Gdal,
                wkt::AxisRule::Always,
            ),
            (
                "WKT1_GDAL false",
                wkt::Version::Wkt1Gdal,
                wkt::AxisRule::Never,
            ),
        ] {
            let ours = wkt::from_projjson_with(json, version, false, rule);
            compare(code, key, ours, &r["axes"][key], true);
        }
        let crs: Value = serde_json::from_str(json).unwrap();
        for (name, value) in parts_of(&crs) {
            let Some(theirs) = r["parts"].get(name) else {
                continue;
            };
            for (key, version) in [
                ("WKT2_2019", wkt::Version::Wkt2_2019),
                ("WKT1_GDAL", wkt::Version::Wkt1Gdal),
            ] {
                let ours = wkt::from_projjson(&value.to_string(), version, false);
                compare(code, &format!("{key} {name}"), ours, &theirs[key], false);
            }
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
fn axis_rules_and_parts_write_as_proj_writes_them() {
    let (written, same, report) =
        check_parts(&xeibe_testkit::data_dir().join("crs-wkt/parts.jsonl"));
    assert!(written > 500, "{written}");
    assert!(report.is_empty(), "{report}");
    assert_eq!(same, written);
}

#[test]
#[ignore = "needs target/crs-wkt/parts.jsonl (scripts/gen_wkt_vectors.py --all)"]
fn every_axis_rule_and_part_writes_as_proj_writes_it() {
    let (written, same, report) =
        check_parts(&xeibe_testkit::data_dir().join("../../target/crs-wkt/parts.jsonl"));
    println!("{written} written, {same} as PROJ writes them (or a listed difference)\n{report}");
    assert!(report.is_empty(), "{report}");
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
    (
        &[3786, 32663],
        "a deprecated method under another's WKT1 name",
    ),
    // Axes along meridians that amount to east and north (EPSG's) against
    // WKT1's east and north: equal here, not in PROJ.
    (&[3408, 3409, 27702], "polar axes that are east and north"),
    // "degree minute second hemisphere" with a degree's factor.
    (&[4226], "a unit equal by factor, not by name"),
    // GDAL writes M'poraloko as M_poraloko; the AUTHORITY ids agree.
    (&[4266, 26632, 26692], "a datum equal by id, not by name"),
    // An unspecified axis direction, which `Crs` doesn't take.
    (&[5817], "an unspecified axis"),
    // Popular Visualisation CRS / Mercator, which PROJ writes through a
    // path of its own (KNOWN_WKT1_WRITING) and finds its ESRI form equal
    // to; we read Web Mercator by GDAL's extension, as PROJ does.
    (&[3785], "Popular Visualisation CRS / Mercator"),
];

/// Where our reading of PROJ's WKT1_ESRI knowingly differs from PROJ's,
/// beyond `KNOWN_WKT1`:
const KNOWN_WKT1_ESRI: &[(&[u32], &str)] = &[
    // PROJ writes and reads names from its own copy of Esri's names
    // (`esri.sql`), which Esri's current data, in xeibe-esri, no longer has
    // ("SIRGAS-Chile_realization_1_epoch_2002").
    (
        &[5361, 5362, 9149, 9150, 9154, 9155, 20042, 20048, 20049],
        "Esri names only PROJ's copy has",
    ),
    // Written on D_ETRS_1989, the ETRS89 ensemble, where EPSG has a member
    // of it (EUREF89): equal to PROJ through Esri's names, not to
    // Crs::equivalent, which keeps an ensemble and its members apart.
    (
        &[
            5971, 5972, 5973, 5974, 5975, 5976, 6171, 6172, 6173, 6174, 6175, 6176, 11020, 11021,
            11022, 11023, 11024, 11025, 11026, 11027,
        ],
        "the ETRS89 ensemble for one of its members",
    ),
    // Reads as the EPSG CRS in every field; PROJ doesn't find its own ESRI
    // form of it equal.
    (&[3410], "equal here, not in PROJ"),
    // A VERTCS on a geodetic DATUM: ellipsoidal heights, not read.
    (&[9895], "a VERTCS on a geodetic DATUM"),
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
        let known = KNOWN_WKT1
            .iter()
            .chain(if version == "WKT1_ESRI" {
                KNOWN_WKT1_ESRI
            } else {
                &[]
            })
            .any(|(codes, _)| codes.contains(&code));
        if known {
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
    // A Transverse Mercator with UTM's parameters is named for its zone,
    // as PROJ names it.
    let utm = |central_meridian: f64, false_northing: f64| {
        let text = format!(
            r#"PROJCS["p",{lonlat},PROJECTION["Transverse_Mercator"],
            PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",{central_meridian}],
            PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],
            PARAMETER["false_northing",{false_northing}],UNIT["metre",1]]"#
        );
        let json: Value = serde_json::from_str(&wkt::to_projjson(&text).unwrap()).unwrap();
        json["conversion"].clone()
    };
    assert_eq!(utm(15.0, 0.0)["name"], "UTM zone 33N");
    assert_eq!(utm(15.0, 0.0)["id"]["code"], 16033);
    assert_eq!(utm(-177.0, 10_000_000.0)["name"], "UTM zone 1S");
    assert_eq!(utm(16.0, 0.0)["name"], "unnamed");
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
        &[
            2989, 4383, 4640, 4972, 4973, 6978, 6979, 6980, 6985, 6986, 6987, 7804,
        ],
        "a live CRS of the same name and definition",
    ),
    // Lambert Azimuthal Equal Area (Spherical) on WGS 84, as WKT1 can't
    // say: read as the ellipsoidal method, as PROJ reads it, which is
    // EPSG:6931's and the others'. PROJ finds them at 25.
    (&[3973, 3974, 3975], "a spherical method in WKT1"),
    // As read (KNOWN_WKT1): "degree minute second hemisphere" with a
    // degree's factor, and 9842 written as Equirectangular.
    (&[4235], "a unit equal by factor, not by name"),
    (
        &[32662, 32663],
        "a deprecated method under another's WKT1 name",
    ),
    // EPSG:3786 and 4088 have one name and definition and are both
    // deprecated: the lower code comes first.
    (&[4088], "two deprecated CRSs alike"),
    // As in KNOWN_WKT1_ESRI: names from PROJ's copy of Esri's, which
    // xeibe-esri doesn't have, so the datum or CRS isn't found by name.
    (
        &[
            4969, 5359, 5360, 5361, 5362, 9147, 9148, 9149, 9150, 9152, 9153, 9154, 9155, 9183,
            9184, 20040, 20041, 20042, 20048, 20049,
        ],
        "Esri names only PROJ's copy has",
    ),
    (&[9895], "a VERTCS on a geodetic DATUM"),
    // Not read (KNOWN_WKT1): an engineering CRS with an unspecified axis.
    (&[5817], "an unspecified axis"),
    // Popular Visualisation CRS / Mercator, deprecated: its WKT1 is Web
    // Mercator by its PROJ4 extension, which is EPSG:3857's definition,
    // and that is what we find. PROJ finds nothing (itself, from ESRI's).
    (&[3785], "Web Mercator, which is EPSG:3857"),
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
        if xeibe_crs::get(code).is_none() {
            continue; // EPSG's worked examples, which xeibe-crs leaves out
        }
        if version == "WKT1_ESRI" && !cfg!(feature = "esri") {
            continue; // Esri's names are behind the feature
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

/// Where reading Esri's WKT for an EPSG CRS knowingly differs from PROJ's
/// reading, other than where it identifies the CRS Esri wrote it for.
#[cfg(feature = "esri")]
const KNOWN_ESRI: &[(&[u32], &str)] = &[
    // Not read; PROJ reads them as CRSs it can't identify (25).
    (&[9895], "a 3D PROJCS (LINUNIT)"),
    (
        &[9354],
        "Polar_Stereographic_Variant_B, which PROJ has no mapping for",
    ),
    // EPSG:3786 and 4088 have one name and definition and are both
    // deprecated since Esri's EPSG 12.049: the lower code comes first.
    (&[4088], "two deprecated CRSs alike"),
    // Esri's definition is another EPSG CRS's, which we find and PROJ
    // doesn't: Deir ez Zor as Oblique Stereographic (EPSG:22780), Plate
    // Carree on the ellipsoid (EPSG:4087).
    (&[22700, 32662], "another EPSG CRS's definition"),
];

/// Reads Esri's WKT for each EPSG CRS in `esri.jsonl`
/// (`scripts/gen_wkt_vectors.py`) and compares it with PROJ's reading: the
/// same axes, and the same `to_epsg()` unless ours is the code Esri wrote
/// it for. Returns the number read, a report of the differences by reason,
/// and how many have a method `Crs` doesn't build.
#[cfg(feature = "esri")]
fn check_esri(path: &Path) -> (usize, String, usize) {
    let mut read = 0;
    let mut unsupported = 0;
    let mut differing: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for r in records(path) {
        let code = r["code"].as_u64().unwrap() as u32;
        if xeibe_crs::get(code).is_none()
            || KNOWN_ESRI.iter().any(|(codes, _)| codes.contains(&code))
        {
            continue;
        }
        let kind = r["kind"].as_str().unwrap();
        let proj = &r["proj"];
        let text = r["wkt"].as_str().unwrap();
        let json: Value = match wkt::to_projjson(text) {
            Ok(j) => serde_json::from_str(&j).unwrap(),
            Err(e) => {
                let e = e.to_string();
                let what = e.split(": ").last().unwrap_or(&e).to_owned();
                differing
                    .entry(format!("error: {what}"))
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
        if proj.get("axes").is_some() && directions(&json) != theirs {
            differing
                .entry(format!(
                    "{kind} axes {:?} != PROJ's {theirs:?}",
                    directions(&json)
                ))
                .or_default()
                .push(code);
        }
        let crs = match Crs::from_wkt(text) {
            Ok(crs) => crs,
            Err(Error::Unsupported { what, .. }) if what.starts_with("the projection method") => {
                unsupported += 1;
                continue;
            }
            Err(e) => {
                differing.entry(format!("Crs: {e}")).or_default().push(code);
                continue;
            }
        };
        read += 1;
        let ours = to_epsg(&crs.identify());
        let theirs = proj["to_epsg"].as_u64().map(|c| c as u32);
        if ours != theirs && ours != Some(code) {
            differing
                .entry(format!(
                    "{kind}: PROJ {}, ours {}",
                    outcome(code, theirs),
                    outcome(code, ours)
                ))
                .or_default()
                .push(code);
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
    (read, report, unsupported)
}

#[cfg(feature = "esri")]
#[test]
fn esri_wkt_reads_as_the_epsg_crs() {
    let (read, report, _) = check_esri(&xeibe_testkit::data_dir().join("crs-wkt/esri.jsonl"));
    assert!(read > 60, "{read}");
    assert!(report.is_empty(), "{report}");
}

#[cfg(feature = "esri")]
#[test]
#[ignore = "needs target/crs-wkt/esri.jsonl (scripts/gen_esri_tables.py --all)"]
fn every_esri_wkt_reads_as_the_epsg_crs() {
    let (read, report, unsupported) =
        check_esri(&xeibe_testkit::data_dir().join("../../target/crs-wkt/esri.jsonl"));
    println!("{read} ESRI WKT strings, {unsupported} with a method Crs doesn't build\n{report}");
    assert!(report.is_empty(), "{report}");
}

#[cfg(feature = "esri")]
#[test]
fn proj_wkt1_esri_reads_as_proj_reads_it() {
    let (read, report) = check_wkt1(
        &xeibe_testkit::data_dir().join("crs-wkt/wkt.jsonl"),
        "WKT1_ESRI",
    );
    assert!(read > 80, "{read}");
    assert!(report.is_empty(), "{report}");
}

#[cfg(feature = "esri")]
#[test]
#[ignore = "needs target/crs-wkt/wkt.jsonl (scripts/gen_wkt_vectors.py --all)"]
fn every_proj_wkt1_esri_reads_as_proj_reads_it() {
    let (read, report) = check_wkt1(
        &xeibe_testkit::data_dir().join("../../target/crs-wkt/wkt.jsonl"),
        "WKT1_ESRI",
    );
    println!("{read} WKT1_ESRI strings\n{report}");
    assert!(report.is_empty(), "{report}");
}

#[cfg(feature = "esri")]
#[test]
fn esri_wkt_reading() {
    let identify = |text: &str| Crs::from_wkt(text).unwrap().identify();
    let gcs_wgs84 = r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",
        SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],
        UNIT["Degree",0.0174532925199433]]"#;
    // Web Mercator as ArcGIS writes it: Esri's names give EPSG's.
    let web = format!(
        r#"PROJCS["WGS_1984_Web_Mercator_Auxiliary_Sphere",{gcs_wgs84},
        PROJECTION["Mercator_Auxiliary_Sphere"],PARAMETER["False_Easting",0.0],
        PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",0.0],
        PARAMETER["Standard_Parallel_1",0.0],PARAMETER["Auxiliary_Sphere_Type",0.0],
        UNIT["Meter",1.0]]"#
    );
    assert_eq!(
        Crs::from_wkt(&web).unwrap().name(),
        "WGS 84 / Pseudo-Mercator"
    );
    assert_eq!(identify(&web), [(3857, 100)]);
    // EPSG:2180 as Esri names it, on ETRF2000-PL, a member of the ETRS89
    // ensemble EPSG has it on.
    let poland = r#"PROJCS["ETRF2000-PL_CS92",GEOGCS["ETRF2000-PL",DATUM["ETRF2000_Poland",
        SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],
        UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],
        PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",-5300000.0],
        PARAMETER["Central_Meridian",19.0],PARAMETER["Scale_Factor",0.9993],
        PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
    assert_eq!(identify(poland)[0], (2180, 100));
    // A depth.
    let depth = r#"VERTCS["MSL_Depth",VDATUM["Mean_Sea_Level"],
        PARAMETER["Vertical_Shift",0.0],PARAMETER["Direction",-1.0],UNIT["Meter",1.0]]"#;
    let json: Value = serde_json::from_str(&wkt::to_projjson(depth).unwrap()).unwrap();
    assert_eq!(directions(&json), ["down"]);
    assert_eq!(identify(depth), [(5715, 100)]);
    // PROJCS[…],VERTCS[…]: a compound CRS named from its parts.
    let height = r#"VERTCS["EVRF_2007_PL_height",
        VDATUM["European_Vertical_Reference_Frame_2007_Poland"],
        PARAMETER["Vertical_Shift",0.0],PARAMETER["Direction",1.0],UNIT["Meter",1.0]]"#;
    let both = Crs::from_wkt(&format!("{poland},{height}")).unwrap();
    assert_eq!(both.name(), "ETRS89 / PL-1992 + PL-EVRF2007 height");
    assert_eq!(both.dimension(), 3);
    // The VERTCS inside the PROJCS, as ArcGIS writes a .prj: the same
    // compound CRS, where PROJ drops the heights.
    let nested = poland.replacen(
        r#"UNIT["Meter",1.0]]"#,
        &format!(r#"UNIT["Meter",1.0],{height}]"#),
        1,
    );
    let nested = Crs::from_wkt(&nested).unwrap();
    assert_eq!(nested.name(), both.name());
    assert!(nested.equivalent(&both, false));
    // In centimetres, as in pyproj's test_from_esri_wkt.
    let navd88 = r#"PROJCS["USA_Contiguous_Albers_Equal_Area_Conic_USGS_version",
        GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",
        SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],
        UNIT["Degree",0.0174532925199433]],PROJECTION["Albers"],
        PARAMETER["false_easting",0.0],PARAMETER["false_northing",0.0],
        PARAMETER["central_meridian",-96.0],PARAMETER["standard_parallel_1",29.5],
        PARAMETER["standard_parallel_2",45.5],PARAMETER["latitude_of_origin",23.0],
        UNIT["Meter",1.0],VERTCS["NAVD_1988",VDATUM["North_American_Vertical_Datum_1988"],
        PARAMETER["Vertical_Shift",0.0],PARAMETER["Direction",1.0],UNIT["Centimeter",0.01]]]"#;
    let json: Value = serde_json::from_str(&wkt::to_projjson(navd88).unwrap()).unwrap();
    assert_eq!(json["type"], "CompoundCRS");
    assert_eq!(
        json["components"][1]["coordinate_system"]["axis"][0]["unit"]["conversion_factor"],
        0.01
    );
}

#[test]
fn towgs84_transforms_as_proj_does() {
    let pulkovo = r#"GEOGCS["Pulkovo 1942",DATUM["Pulkovo_1942",
        SPHEROID["Krassowsky 1940",6378245,298.3],
        TOWGS84[33.4,-146.6,-76.3,-0.359,-0.053,0.844,-0.84]],
        PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433],
        AUTHORITY["EPSG","4284"]]"#;
    let crs = Crs::from_wkt(pulkovo).unwrap();
    // A Bound CRS is no EPSG CRS, as in PROJ.
    assert_eq!(crs.identify(), []);
    assert!(!crs.equivalent(&Crs::from_epsg(4284).unwrap(), false));
    // PROJ 9.8.1's results, longitude first.
    for (target, x, y, name) in [
        (
            4326,
            29.998122530511566,
            54.99991936682272,
            "Transformation from Pulkovo 1942 to WGS84",
        ),
        (
            4258,
            29.998122530511566,
            54.999919367708564,
            "Transformation from Pulkovo 1942 to WGS84 + Inverse of ETRS89 to WGS 84 (1)",
        ),
        (
            3857,
            3339375.7248507747,
            7361850.463811354,
            "Transformation from Pulkovo 1942 to WGS84",
        ),
        // On its own datum the transformation isn't used.
        (4284, 30.0, 55.0, ""),
    ] {
        let t =
            Transform::new(&crs, &Crs::from_epsg(target).unwrap(), AxisOrder::EastFirst).unwrap();
        let (cx, cy, _) = t.convert(30.0, 55.0, None).unwrap();
        let tolerance = if target == 3857 { 1e-6 } else { 1e-11 };
        assert!(
            (cx - x).abs() < tolerance && (cy - y).abs() < tolerance,
            "EPSG:{target}: {cx} {cy}"
        );
        assert_eq!(t.operation_for(30.0, 55.0, None).unwrap_or(""), name);
        // And back, to the millimetre: the inverse of PROJ's small-angle
        // Helmert is approximate.
        let (bx, by, _) = t.inverse().unwrap().convert(cx, cy, None).unwrap();
        assert!(
            (bx - 30.0).abs() < 1e-7 && (by - 55.0).abs() < 1e-7,
            "{bx} {by}"
        );
    }
}

/// Where a Bound CRS on these EPSG CRSs knowingly converts otherwise than
/// in PROJ.
const KNOWN_BOUND: &[(&[u32], &str)] = &[
    // As in KNOWN_WKT1: GDAL writes M'poraloko as M_poraloko, which PROJ
    // doesn't take for the same datum, so it goes through WGS 84 to the
    // plain EPSG CRS where we convert.
    (&[4266, 26632, 26692], "a datum equal by id, not by name"),
    // PROJ keeps the signs of TOWGS84 values that are a Coordinate Frame
    // transformation's when the turned values are any Helmert's, ESRI's
    // included; xeibe has EPSG's only, and turns them (CGRS93, EPSG:7721).
    (&[6311, 6312], "ESRI's Helmert transformations"),
];

/// Reads each Bound CRS in `bound.jsonl` (`scripts/gen_wkt_vectors.py`):
/// its PROJJSON as PROJ's, and its point converted to each target as PROJ
/// converts it (2e-9°, or 0.2 mm or 2e-11 relative: PROJ's projections
/// on Bessel Namibia, an ellipsoid in German legal metres, differ by that).
/// Returns the number of conversions
/// compared and a report of the differences.
fn check_bound(path: &Path) -> (usize, String) {
    let mut compared = 0;
    let mut differing: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for r in records(path) {
        let code = r["code"].as_u64().unwrap() as u32;
        let op = r["op"].as_u64().unwrap() as u32;
        // `AxisOrder::EastFirst` keeps south-west axes (Krovak) as they are;
        // pyproj's always_xy turns them round.
        let other_axes =
            |c: u32| xeibe_crs::get(c).is_some_and(|r| r.first_axis == xeibe_crs::FirstAxis::Other);
        if KNOWN_BOUND.iter().any(|(codes, _)| codes.contains(&code)) || other_axes(code) {
            continue;
        }
        let text = r["wkt"].as_str().unwrap();
        // Our WKT1 with TOWGS84, and PROJ's WKT2 (BOUNDCRS) and WKT1_GDAL
        // of it, read back.
        for (version, text) in [
            ("TOWGS84", Some(text)),
            ("WKT2_2019", r["WKT2_2019"].as_str()),
            ("WKT1_GDAL", r["WKT1_GDAL"].as_str()),
        ] {
            let Some(text) = text else { continue };
            let mut ours: Value = match wkt::to_projjson(text) {
                Ok(j) => serde_json::from_str(&j).unwrap(),
                Err(e) => {
                    differing
                        .entry(format!("{version}: {e}"))
                        .or_default()
                        .push(op);
                    continue;
                }
            };
            let mut theirs = r["projjson"].clone();
            // Names as the reader trims them: EPSG's "ETRS89-ALB [2008] " ends
            // in a space, which PROJ keeps. The scale difference to 1e-8 ppm:
            // WKT2's abridged form gives it as a factor, 1 + ppm × 1e-6, to
            // 15 digits.
            for v in [&mut ours, &mut theirs] {
                for p in v["transformation"]["parameters"]
                    .as_array_mut()
                    .into_iter()
                    .flatten()
                {
                    if p["id"]["code"] == 8611
                        && let Some(x) = p["value"].as_f64()
                    {
                        p["value"] = json!((x * 1e8).round() / 1e8);
                    }
                }
                if let Some(name) = v["transformation"]["name"].as_str() {
                    v["transformation"]["name"] =
                        json!(name.split_whitespace().collect::<Vec<_>>().join(" "));
                }
            }
            // What the bound CRS adds; its base is read as without TOWGS84.
            let mut d = Vec::new();
            for key in ["type", "target_crs", "transformation"] {
                diff(
                    &normalise(&ours[key], ""),
                    &normalise(&theirs[key], ""),
                    &format!(".{key}"),
                    &mut d,
                );
            }
            for line in d {
                let key = line.split(':').next().unwrap_or("").to_owned();
                differing
                    .entry(format!("{version} PROJJSON {key}"))
                    .or_default()
                    .push(op);
            }
        }
        let unbuilt = |e: &Error| matches!(e, Error::Unsupported { what, .. } if what.starts_with("the projection method"));
        let src = match Crs::from_wkt(text) {
            Ok(crs) => crs,
            Err(e) if unbuilt(&e) => continue,
            Err(e) => {
                differing.entry(format!("Crs: {e}")).or_default().push(op);
                continue;
            }
        };
        // Written as PROJ writes it, from our reading, which keeps the ids
        // PROJ's PROJJSON leaves out of nested objects.
        let json = wkt::to_projjson(text).unwrap();
        for (version, name) in [
            (wkt::Version::Wkt2_2019, "WKT2_2019"),
            (wkt::Version::Wkt1Gdal, "WKT1_GDAL"),
        ] {
            let Some(theirs) = r[name].as_str() else {
                continue;
            };
            // What the bound CRS adds: TOWGS84, or the target CRS and the
            // abridged transformation. The base is written as without it,
            // from our reading, whose names can differ from PROJ's (and are
            // trimmed: "ETRS89-ALB [2008] " ends in a space).
            let added = |w: &str| -> String {
                let w = w.replace(" ]", "]").replace("  ", " ");
                match version {
                    wkt::Version::Wkt1Gdal => w
                        .find("TOWGS84[")
                        .map(|i| w[i..i + w[i..].find(']').unwrap() + 1].to_owned())
                        .unwrap_or_default(),
                    _ => w
                        .find(",TARGETCRS[")
                        .map(|i| w[i + 1..].to_owned())
                        .unwrap_or_default(),
                }
            };
            match wkt::from_projjson(&json, version, false) {
                Ok(ours) => {
                    if let Some((a, b)) = first_difference(&added(&ours), &added(theirs)) {
                        differing
                            .entry(format!("writing {name}: {a} != PROJ's {b}"))
                            .or_default()
                            .push(op);
                    }
                }
                Err(e) => differing
                    .entry(format!("writing {name}: {e}"))
                    .or_default()
                    .push(op),
            }
        }
        let [x, y] = [
            r["point"][0].as_f64().unwrap(),
            r["point"][1].as_f64().unwrap(),
        ];
        for t in r["to"].as_array().unwrap() {
            let (target, label) = match &t["target"] {
                Value::Number(n) => (
                    Crs::from_epsg(n.as_u64().unwrap() as u32),
                    format!("EPSG:{n}"),
                ),
                Value::String(w) => (Crs::from_wkt(w), "another bound CRS".into()),
                _ => unreachable!(),
            };
            let target = match target {
                Ok(crs) => crs,
                Err(e) if unbuilt(&e) => continue,
                Err(e) => panic!("{e}"),
            };
            if target.code().is_some_and(other_axes) {
                continue;
            }
            let label = if matches!(&t["target"], Value::Number(n) if n.as_u64() == Some(code.into()))
            {
                "its own EPSG CRS".to_owned()
            } else {
                label
            };
            // Without grids: PROJ had none either.
            let ours = Transform::with_grids(&src, &target, AxisOrder::EastFirst, Grids::None)
                .and_then(|t| {
                    t.convert(x, y, None).map_err(|e| Error::Coordinate {
                        index: 0,
                        x,
                        y,
                        reason: e,
                    })
                });
            let theirs = t["result"]
                .as_array()
                .map(|v| [v[0].as_f64().unwrap(), v[1].as_f64().unwrap()]);
            compared += 1;
            let what = match (ours, theirs) {
                (Ok((cx, cy, _)), Some([px, py])) => {
                    // Far outside a projection's area (another bound CRS's
                    // projection), metres agree to 2e-11 relative.
                    let tolerance = if px.abs() <= 360.0 && py.abs() <= 90.0 {
                        2e-9
                    } else {
                        (2e-11 * px.abs().max(py.abs())).max(2e-4)
                    };
                    if (cx - px).abs() <= tolerance && (cy - py).abs() <= tolerance {
                        continue;
                    }
                    format!(
                        "to {label}: off by {:.1e}",
                        (cx - px).abs().max((cy - py).abs())
                    )
                }
                (Err(_), None) => continue,
                (Err(e), Some(_)) => format!("to {label}: error {e}"),
                (Ok(_), None) => format!("to {label}: PROJ fails"),
            };
            differing.entry(what).or_default().push(op);
        }
    }
    let mut report = String::new();
    for (what, ops) in &differing {
        report += &format!("{} x {what}: {:?}\n", ops.len(), &ops[..ops.len().min(10)]);
    }
    (compared, report)
}

#[test]
fn bound_crss_convert_as_proj_converts_them() {
    let (compared, report) = check_bound(&xeibe_testkit::data_dir().join("crs-wkt/bound.jsonl"));
    assert!(compared > 100, "{compared}");
    assert!(report.is_empty(), "{report}");
}

#[test]
#[ignore = "needs target/crs-wkt/bound.jsonl (scripts/gen_wkt_vectors.py --all)"]
fn every_bound_crs_converts_as_proj_converts_it() {
    let (compared, report) =
        check_bound(&xeibe_testkit::data_dir().join("../../target/crs-wkt/bound.jsonl"));
    println!("{compared} conversions\n{report}");
    assert!(report.is_empty(), "{report}");
}
