//! Reading PROJ strings (`docs/pyproj-phase3.md`, "PROJ strings"): PROJ's
//! strings for EPSG CRSs and a corpus of hand-written ones, read and
//! compared with PROJ's reading of the same string
//! (`scripts/gen_proj_string_vectors.py`).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use xeibe_reproject::{AxisOrder, Crs, Error, Transform, proj_string, wkt};

fn records(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Where `ours` and PROJ's differ, as `path: ours != theirs`, numbers
/// within 1e-11 relative (PROJ writes 15 significant digits).
fn diff(a: &Value, b: &Value, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                if k == "$schema" {
                    continue;
                }
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

/// A difference's path without array indices, to group them.
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

/// What went wrong reading `string`, against PROJ's reading `theirs` (its
/// PROJJSON, or `None` where PROJ refused it).
#[derive(Default)]
struct Report {
    read: usize,
    same: usize,
    /// What we refuse, by reason: strings PROJ reads but only with
    /// something this crate doesn't take (decision 6).
    refused: BTreeMap<String, Vec<String>>,
    /// Failures: strings both read differently, ones PROJ reads that we
    /// call invalid, and ones PROJ refuses that we read.
    failed: BTreeMap<String, Vec<String>>,
}

impl Report {
    fn check(&mut self, string: &str, theirs: Option<&Value>) {
        self.read += 1;
        let ours =
            proj_string::to_projjson(string).map(|s| serde_json::from_str::<Value>(&s).unwrap());
        match (ours, theirs) {
            (Ok(ours), Some(theirs)) => {
                let mut d = Vec::new();
                diff(&ours, theirs, "", &mut d);
                if d.is_empty() {
                    self.same += 1;
                }
                for d in d {
                    self.failed
                        .entry(format!("differs at {}", category(&d)))
                        .or_default()
                        .push(format!("{string}  =>  {d}"));
                }
            }
            (Err(Error::Unsupported { what, .. }), Some(_)) => {
                let reason = what.split(" (").next().unwrap_or(&what).to_owned();
                let reason = if reason.starts_with("+proj=") {
                    "+proj=…".to_owned() + &what[what.find(" (").unwrap_or(0)..]
                } else {
                    reason
                };
                self.refused
                    .entry(reason)
                    .or_default()
                    .push(string.to_owned());
            }
            (Err(e), Some(_)) => {
                self.failed
                    .entry(format!("PROJ reads it, we don't: {e}"))
                    .or_default()
                    .push(string.to_owned());
            }
            (Ok(_), None) => {
                self.failed
                    .entry("PROJ refuses it, we read it".into())
                    .or_default()
                    .push(string.to_owned());
            }
            (Err(_), None) => self.same += 1,
        }
    }

    fn text(&self, verbose: bool) -> String {
        let mut out = format!("{} strings, {} as PROJ reads them\n", self.read, self.same);
        for (title, map) in [("refused", &self.refused), ("FAILED", &self.failed)] {
            for (reason, examples) in map {
                out += &format!("{title}: {} x {reason}\n", examples.len());
                for e in examples.iter().take(if verbose { 6 } else { 2 }) {
                    out += &format!("    {e}\n");
                }
            }
        }
        out
    }
}

/// Points converted from each corpus string's geodetic CRS to the CRS,
/// against PROJ's, to 0.1 mm: a backstop for the reading, whose mistakes
/// are metres. The points are around the string's origin, often far outside
/// a projection's area, where Krovak differs from PROJ by up to 22 µm. How
/// many were compared, and the failures. Skipped: strings whose method
/// isn't implemented here, and `+approx`, PROJ's approximate Transverse
/// Mercator, which is converted with the exact one (2 mm apart here).
/// Mercator (Spherical) is compared after `r_c_scale`.
fn coordinates(dir: &Path) -> (usize, Vec<String>) {
    let (mut compared, mut failed) = (0, Vec::new());
    for r in records(&dir.join("corpus.jsonl")) {
        let (Some(srs), Some(points)) = (r["srs"].as_str(), r["points"].as_array()) else {
            continue;
        };
        if srs.contains("+approx") {
            continue;
        }
        let Ok(json) = proj_string::to_projjson(srs) else {
            continue;
        };
        let value: Value = serde_json::from_str(&json).unwrap();
        let crs = if value["type"] == "BoundCRS" {
            &value["source_crs"]
        } else {
            &value
        };
        if crs["type"] != "ProjectedCRS" {
            continue;
        }
        let (Ok(base), Ok(projected)) = (
            Crs::from_projjson(&crs["base_crs"].to_string()),
            Crs::from_projjson(&crs.to_string()),
        ) else {
            continue;
        };
        let t = Transform::new(&base, &projected, AxisOrder::EastFirst).unwrap();
        let (scale, fe, fn_) = r_c_scale(crs);
        for (p, want) in points.iter().zip(r["converted"].as_array().unwrap()) {
            let (lon, lat) = (p[0].as_f64().unwrap(), p[1].as_f64().unwrap());
            let Some(want) = want.as_array() else {
                continue;
            };
            compared += 1;
            let (wx, wy) = (want[0].as_f64().unwrap(), want[1].as_f64().unwrap());
            let (wx, wy) = (fe + (wx - fe) * scale, fn_ + (wy - fn_) * scale);
            match t.convert(lon, lat, None) {
                Ok((x, y, _)) if (x - wx).abs() < 1e-4 && (y - wy).abs() < 1e-4 => {}
                other => failed.push(format!(
                    "{srs} at ({lon}, {lat}): {other:?}, PROJ ({wx}, {wy})"
                )),
            }
        }
    }
    (compared, failed)
}

/// What PROJ's Mercator (Spherical) (`+R_C`) results are scaled by about
/// the false origin to be ours, and that origin: PROJ takes the conformal
/// sphere at the equator whatever the latitude of origin, we take it at the
/// latitude of origin (`docs/pyproj-compat.md`, divergence 10). 1 for
/// another CRS.
fn r_c_scale(crs: &Value) -> (f64, f64, f64) {
    let conversion = &crs["conversion"];
    if !matches!(
        conversion["method"]["id"]["code"].as_u64(),
        Some(1026 | 9841)
    ) {
        return (1.0, 0.0, 0.0);
    }
    let param = |code: u64| {
        conversion["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"]["code"].as_u64() == Some(code))
            .map_or(0.0, |p| p["value"].as_f64().unwrap())
    };
    let ellipsoid = &crs["base_crs"]["datum"]["ellipsoid"];
    let es = ellipsoid["inverse_flattening"]
        .as_f64()
        .map_or(0.0, |rf| (2.0 - 1.0 / rf) / rf);
    let sin = param(8801).to_radians().sin();
    (1.0 / (1.0 - es * sin * sin), param(8806), param(8807))
}

fn check(dir: &Path) -> Report {
    let mut report = Report::default();
    for r in records(&dir.join("corpus.jsonl")) {
        if let Some(srs) = r["srs"].as_str() {
            report.check(srs, r["projjson"].as_object().map(|_| &r["projjson"]));
        }
    }
    for r in records(&dir.join("epsg.jsonl")) {
        if let Some(s) = r["proj4"].as_str() {
            let reading = &r["reading"]["projjson"];
            report.check(s, reading.as_object().map(|_| reading));
        }
    }
    for r in records(&dir.join("methods.jsonl"))
        .into_iter()
        .chain(records(&dir.join("custom.jsonl")))
    {
        report.check(
            r["srs"].as_str().unwrap(),
            r["projjson"].as_object().map(|_| &r["projjson"]),
        );
    }
    report
}

#[test]
fn proj_strings_read_as_proj_reads_them() {
    let report = check(&xeibe_testkit::data_dir().join("proj-string"));
    println!("{}", report.text(true));
    assert!(report.failed.is_empty(), "{}", report.text(false));
}

#[test]
fn proj_strings_convert_as_in_proj() {
    let (compared, failed) = coordinates(&xeibe_testkit::data_dir().join("proj-string"));
    assert!(compared > 300, "{compared}");
    assert!(
        failed.is_empty(),
        "{} of {compared}:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
#[ignore = "needs target/proj-string/ (scripts/gen_proj_string_vectors.py --all)"]
fn every_epsg_proj_string_reads_as_proj_reads_it() {
    let report = check(&xeibe_testkit::data_dir().join("../../target/proj-string"));
    println!("{}", report.text(true));
    assert!(report.failed.is_empty(), "{}", report.text(false));
}

/// `+nadgrids=@null`: a Bound CRS whose transformation to WGS 84 leaves
/// longitude and latitude as they are, even on another ellipsoid. PROJ
/// 9.8.1's results, longitude first.
#[test]
fn null_grid_leaves_longitude_and_latitude() {
    let wgs84 = Crs::from_epsg(4326).unwrap();
    for (string, input, want) in [
        (
            "+proj=utm +zone=18 +datum=NAD83 +units=m +no_defs +nadgrids=null +type=crs",
            (500000.0, 4500000.0),
            (-75.0, 40.65085651660554),
        ),
        (
            "+proj=longlat +ellps=clrk66 +nadgrids=@null +type=crs",
            (-75.5, 40.25),
            (-75.5, 40.25),
        ),
    ] {
        let crs = Crs::from_projjson(&proj_string::to_projjson(string).unwrap()).unwrap();
        let t = Transform::new(&crs, &wgs84, AxisOrder::EastFirst).unwrap();
        let (x, y, _) = t.convert(input.0, input.1, None).unwrap();
        assert!(
            (x - want.0).abs() < 1e-12 && (y - want.1).abs() < 1e-12,
            "{string}: {x} {y}"
        );
        assert_eq!(
            t.operation_for(input.0, input.1, None),
            Some("unknown to WGS84"),
            "{string}"
        );
    }
}

/// A projection EPSG doesn't have, on a datum it does, goes through the
/// EPSG geographic CRS on that datum, as PROJ goes through its base CRS:
/// here WGS 84 to Poland CS92, PROJ 9.8.1's results for 21°E 52°N, within
/// the 0.14 mm Transverse Mercator differs from PROJ by
/// (`docs/reprojection.md`).
#[test]
fn custom_projection_on_an_epsg_datum_converts() {
    let poland = Crs::from_epsg(2180).unwrap();
    for (string, input) in [
        (
            "+proj=tmerc +lon_0=21.5 +k=1 +x_0=123 +datum=WGS84 +type=crs",
            (-34215.903016167365, 5763461.620347131),
        ),
        (
            "+proj=laea +lat_0=50 +lon_0=20 +datum=WGS84 +type=crs",
            (68688.67603560908, 222942.22045496193),
        ),
    ] {
        let crs = Crs::from_projjson(&proj_string::to_projjson(string).unwrap()).unwrap();
        let t = Transform::new(&crs, &poland, AxisOrder::EastFirst).unwrap();
        let (x, y, _) = t.convert(input.0, input.1, None).unwrap();
        assert!(
            (x - 637253.1611040878).abs() < 1.4e-4 && (y - 461197.24291843455).abs() < 1.4e-4,
            "{string}: {x} {y}"
        );
    }
}

/// An unknown datum (decision 8): no transformation to another datum, and
/// an error that says so and what to give instead; on its own datum, a
/// conversion as usual.
#[test]
fn unknown_datum_is_an_error_naming_it() {
    let crs = |s: &str| Crs::from_projjson(&proj_string::to_projjson(s).unwrap()).unwrap();
    let grs80 = crs("+proj=longlat +ellps=GRS80 +type=crs");
    for other in [
        Crs::from_epsg(2180).unwrap(),
        crs("+proj=longlat +ellps=bessel +type=crs"),
    ] {
        let e = Transform::new(&grs80, &other, AxisOrder::EastFirst).unwrap_err();
        assert!(
            matches!(&e, Error::UnknownDatum { datum, .. } if datum == "Unknown based on GRS 1980 ellipsoid"),
            "{e}"
        );
        assert!(e.to_string().contains("+towgs84"), "{e}");
    }
    let e = Transform::new(
        &Crs::from_epsg(4019).unwrap(),
        &Crs::from_epsg(4326).unwrap(),
        AxisOrder::EastFirst,
    )
    .unwrap_err();
    assert!(matches!(e, Error::UnknownDatum { .. }), "{e}");
    let utm = crs("+proj=utm +zone=33 +ellps=GRS80 +type=crs");
    let t = Transform::new(&grs80, &utm, AxisOrder::EastFirst).unwrap();
    let (x, _, _) = t.convert(15.0, 52.0, None).unwrap();
    assert!((x - 500000.0).abs() < 1e-6, "{x}");
}

/// The same tokens, numbers within 1e-13 relative: PROJ writes 15
/// significant digits, and for 15 EPSG CRSs PROJ's database holds an angle
/// one step from `xeibe-crs`'s (`-91.2944444444444` and `…445`).
fn same_tokens(a: &str, b: &str) -> bool {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
    a.len() == b.len()
        && a.iter().zip(&b).all(|(x, y)| {
            x == y
                || match (x.split_once('='), y.split_once('=')) {
                    (Some((kx, vx)), Some((ky, vy))) if kx == ky => {
                        let (vx, vy): (Vec<&str>, Vec<&str>) =
                            (vx.split(',').collect(), vy.split(',').collect());
                        vx.len() == vy.len()
                            && vx.iter().zip(&vy).all(|(p, q)| {
                                match (p.parse::<f64>(), q.parse::<f64>()) {
                                    (Ok(p), Ok(q)) => (p - q).abs() <= 1e-13 * p.abs().max(q.abs()),
                                    _ => p == q,
                                }
                            })
                    }
                    _ => false,
                }
        })
}

/// Where our string for a CRS differs from PROJ's, grouped by the first
/// token that differs.
fn writing(dir: &Path) -> (usize, BTreeMap<String, Vec<String>>) {
    let mut failed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut written = 0;
    let mut check = |label: String, json: &str, theirs: Option<&str>| {
        written += 1;
        let ours = proj_string::from_projjson(json);
        match (ours, theirs) {
            (Ok(ours), Some(theirs)) if same_tokens(&ours, theirs) => {}
            (Err(_), None) => {}
            (Ok(ours), Some(theirs)) => {
                let a: Vec<&str> = ours.split(' ').collect();
                let b: Vec<&str> = theirs.split(' ').collect();
                let i = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
                let key = format!(
                    "{} != {}",
                    a.get(i)
                        .map_or("(end)", |t| t.split('=').next().unwrap_or(t)),
                    b.get(i)
                        .map_or("(end)", |t| t.split('=').next().unwrap_or(t))
                );
                failed
                    .entry(key)
                    .or_default()
                    .push(format!("{label}: {ours}  !=  {theirs}"));
            }
            (Ok(ours), None) => failed
                .entry("PROJ can't write it, we do".into())
                .or_default()
                .push(format!("{label}: {ours}")),
            (Err(e), Some(theirs)) => failed
                .entry(format!("we can't write it: {e}"))
                .or_default()
                .push(format!("{label}: {theirs}")),
        }
    };
    for r in records(&dir.join("epsg.jsonl")) {
        let code = r["code"].as_u64().unwrap() as u32;
        if let Some(json) = xeibe_crs::projjson(code) {
            check(format!("EPSG:{code}"), json, r["proj4"].as_str());
        }
    }
    for r in records(&dir.join("methods.jsonl"))
        .into_iter()
        .chain(records(&dir.join("custom.jsonl")))
    {
        if r["projjson"].is_object() {
            check(
                r["srs"].as_str().unwrap().to_owned(),
                &r["projjson"].to_string(),
                r["to_proj4"].as_str(),
            );
        }
    }
    for r in records(&dir.join("corpus.jsonl")) {
        // What PROJ keeps outside the PROJJSON (an extension: `+over`, a
        // pipeline, a PROJ-only method; `+approx`) can't be written back
        // from it: the strings the reader refuses, and `+approx`.
        let srs = r["srs"].as_str().unwrap_or("");
        if srs.contains("+approx") || proj_string::to_projjson(srs).is_err() {
            continue;
        }
        if r["projjson"].is_object() {
            check(
                r["srs"].as_str().unwrap().to_owned(),
                &r["projjson"].to_string(),
                r["to_proj4"].as_str(),
            );
        }
    }
    (written, failed)
}

fn writing_report(written: usize, failed: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = format!("{written} CRSs written\n");
    for (k, v) in failed {
        out += &format!("{} x {k}\n", v.len());
        for e in v.iter().take(3) {
            out += &format!("    {e}\n");
        }
    }
    out
}

#[test]
fn proj_strings_write_as_proj_writes_them() {
    let (written, failed) = writing(&xeibe_testkit::data_dir().join("proj-string"));
    assert!(failed.is_empty(), "{}", writing_report(written, &failed));
}

#[test]
#[ignore = "needs target/proj-string/ (scripts/gen_proj_string_vectors.py --all)"]
fn every_epsg_crs_writes_as_proj_writes_it_as_a_proj_string() {
    let (written, failed) = writing(&xeibe_testkit::data_dir().join("../../target/proj-string"));
    println!("{}", writing_report(written, &failed));
    assert!(failed.is_empty(), "{}", writing_report(written, &failed));
}

/// Where the EPSG code our identification gives a string's CRS (as
/// `CRS.to_epsg()`: its own code, else the best match at confidence 70 or
/// more) differs from PROJ's `to_epsg()`.
fn identification(dir: &Path) -> (usize, Vec<String>) {
    let mut compared = 0;
    let mut failed = Vec::new();
    let mut check = |s: &str, theirs: Option<u64>| {
        let Ok(crs) = Crs::from_proj_string(s) else {
            return;
        };
        compared += 1;
        let ours = crs
            .code()
            .or_else(|| {
                crs.identify()
                    .into_iter()
                    .find(|&(_, c)| c >= 70)
                    .map(|(code, _)| code)
            })
            .map(u64::from);
        if ours != theirs {
            failed.push(format!("{s}: {ours:?}, PROJ {theirs:?}"));
        }
    };
    for r in records(&dir.join("corpus.jsonl")) {
        if let (Some(srs), true) = (r["srs"].as_str(), r["projjson"].is_object()) {
            check(srs, r["to_epsg"].as_u64());
        }
    }
    for r in records(&dir.join("epsg.jsonl")) {
        if let (Some(s), true) = (r["proj4"].as_str(), r["reading"]["projjson"].is_object()) {
            check(s, r["reading"]["to_epsg"].as_u64());
        }
    }
    (compared, failed)
}

#[test]
#[ignore = "needs target/proj-string/ (scripts/gen_proj_string_vectors.py --all)"]
fn every_proj_string_is_identified_as_proj_identifies_it() {
    let (compared, failed) =
        identification(&xeibe_testkit::data_dir().join("../../target/proj-string"));
    println!("{compared} identified, {} differ", failed.len());
    for f in failed
        .iter()
        .take(if std::env::var_os("ALL_FAILED").is_some() {
            usize::MAX
        } else {
            40
        })
    {
        println!("  {f}");
    }
    assert!(failed.is_empty());
}

/// The projections where we differ from PROJ on purpose, because PROJ's
/// results are wrong there (`docs/pyproj-compat.md`, divergence 10): ISEA
/// on an ellipsoid and with `lon_0`, and its inverse; GS48's, GS50's and
/// Alaska's inverse; S2 on the far side of its face; the bipolar conic
/// where its cones meet. A point of theirs may differ from PROJ's only where our
/// inverse takes our forward back to the point (to 1e-9°) closer than
/// PROJ's takes its own, or where PROJ's comes back more than 1° away, out
/// of the projection's domain, as it must where we refuse a point.
const DIVERGES: &[&str] = &["isea", "gs48", "gs50", "alsk", "s2", "bipc"];

/// How far apart two longitudes and latitudes are, in degrees of arc
/// (near enough).
fn off((l1, p1): (f64, f64), (l2, p2): (f64, f64)) -> f64 {
    let dl = (l1 - l2 + 540.0).rem_euclid(360.0) - 180.0;
    (p1 - p2).abs().max(dl.abs() * p2.to_radians().cos())
}

/// The projections EPSG has no CRS for and those only PROJ defines
/// (`methods.jsonl`): points over the whole globe converted from each
/// string's geodetic CRS and back, against PROJ's (forwards to 2 µm, PROJ's
/// results being rounded to 1 µm; back to 1e-9°, from PROJ's forward
/// results), but where `DIVERGES` lets them differ.
fn method_conversions(path: &Path) -> (usize, Vec<String>) {
    let (mut compared, mut failed) = (0, Vec::new());
    for r in records(path) {
        let srs = r["srs"].as_str().unwrap();
        let Some(points) = r["points"].as_array() else {
            continue;
        };
        let json: Value = serde_json::from_str(&proj_string::to_projjson(srs).unwrap()).unwrap();
        let proj = srs.split(' ').next().unwrap().trim_start_matches("+proj=");
        let diverges = DIVERGES.contains(&proj);
        let base = if json["type"] == "ProjectedCRS" || json["type"] == "DerivedGeographicCRS" {
            &json["base_crs"]
        } else {
            &json
        };
        let (base, crs) = match (
            Crs::from_projjson(&base.to_string()),
            Crs::from_projjson(&json.to_string()),
        ) {
            (Ok(b), Ok(c)) => (b, c),
            (b, c) => {
                failed.push(format!("{srs}: {:?}", b.err().or(c.err())));
                continue;
            }
        };
        let forward = Transform::new(&base, &crs, AxisOrder::EastFirst).unwrap();
        let inverse = r["inverse"].as_array();
        let backward = inverse.map(|_| Transform::new(&crs, &base, AxisOrder::EastFirst).unwrap());
        for (i, (p, want)) in points
            .iter()
            .zip(r["converted"].as_array().unwrap())
            .enumerate()
        {
            let (lon, lat) = (p[0].as_f64().unwrap(), p[1].as_f64().unwrap());
            let ours = forward.convert(lon, lat, None);
            let want = want
                .as_array()
                .map(|w| (w[0].as_f64().unwrap(), w[1].as_f64().unwrap()));
            let back = inverse
                .and_then(|v| v[i].as_array())
                .map(|b| (b[0].as_f64().unwrap(), b[1].as_f64().unwrap()));
            // How far each round trip comes back from the point, ours from
            // our forward result, PROJ's from its own.
            let ours_off = match (&ours, &backward) {
                (Ok((x, y, _)), Some(backward)) => backward
                    .convert(*x, *y, None)
                    .map_or(f64::INFINITY, |(l, p, _)| off((l, p), (lon, lat))),
                _ => f64::INFINITY,
            };
            let proj_off = back.map_or(f64::INFINITY, |b| off(b, (lon, lat)));
            let better = diverges && ours_off < 1e-9 && ours_off < proj_off;
            compared += 1;
            let Some((wx, wy)) = want else {
                if let Ok(ours) = ours
                    && !better
                {
                    failed.push(format!("{srs} at ({lon}, {lat}): {ours:?}, PROJ fails"));
                }
                continue;
            };
            match ours {
                Ok((x, y, _)) if (x - wx).abs() < 2e-6 && (y - wy).abs() < 2e-6 => {}
                Ok(_) if better => continue,
                Err(_) if diverges && proj_off > 1.0 => continue,
                other => {
                    failed.push(format!(
                        "{srs} at ({lon}, {lat}): {other:?}, PROJ ({wx}, {wy})"
                    ));
                    continue;
                }
            }
            let Some(backward) = &backward else {
                continue;
            };
            compared += 1;
            let ours = backward.convert(wx, wy, None);
            let Some((blon, blat)) = back else {
                if let Ok(ours) = ours
                    && !better
                {
                    failed.push(format!(
                        "{srs} back from ({wx}, {wy}): {ours:?}, PROJ fails"
                    ));
                }
                continue;
            };
            let close = |(l, p): (f64, f64)| {
                let dl = (l - blon + 540.0).rem_euclid(360.0) - 180.0;
                (p - blat).abs() < 1e-9 && (dl.abs() < 1e-9 || blat.abs() > 89.999999)
            };
            match ours {
                Ok((l, p, _)) if close((l, p)) => {}
                // Ours inverts our forward, closer than PROJ's inverts its
                // own; or the point is outside where PROJ's inverts at all.
                _ if better || (diverges && proj_off > 1.0) => {}
                other => failed.push(format!(
                    "{srs} back from ({wx}, {wy}): {other:?}, PROJ ({blon}, {blat})"
                )),
            }
        }
    }
    (compared, failed)
}

/// The projections PROJ has no CRS method for (`custom.jsonl`), as
/// `methods.jsonl`'s.
#[test]
fn custom_proj_methods_convert_as_in_proj() {
    let (compared, failed) =
        method_conversions(&xeibe_testkit::data_dir().join("proj-string/custom.jsonl"));
    assert!(compared > 500, "{compared}");
    assert!(
        failed.is_empty(),
        "{} of {compared}:\n{}",
        failed.len(),
        failed
            .iter()
            .take(
                std::env::var("TAKE")
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(60)
            )
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn proj_methods_convert_as_in_proj() {
    let (compared, failed) =
        method_conversions(&xeibe_testkit::data_dir().join("proj-string/methods.jsonl"));
    assert!(
        failed.is_empty(),
        "{} of {compared}:\n{}",
        failed.len(),
        failed
            .iter()
            .take(
                std::env::var("TAKE")
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(60)
            )
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `[path, value]` sets the value at the path, `[path]` removes it: PROJ's
/// reading of a WKT, from the string's PROJJSON (`changes` in
/// `gen_proj_string_vectors.py`).
fn apply(base: &Value, changes: &Value) -> Value {
    let mut out = base.clone();
    for c in changes.as_array().unwrap() {
        let path = c[0].as_array().unwrap();
        let mut at = &mut out;
        for (i, key) in path.iter().enumerate() {
            let last = i + 1 == path.len();
            match key {
                Value::String(k) if last => {
                    let o = at.as_object_mut().unwrap();
                    match c.get(1) {
                        Some(v) => {
                            o.insert(k.clone(), v.clone());
                        }
                        None => {
                            o.remove(k);
                        }
                    }
                    break;
                }
                Value::String(k) => at = at.get_mut(k.as_str()).unwrap(),
                Value::Number(n) if last => {
                    at[n.as_u64().unwrap() as usize] = c[1].clone();
                    break;
                }
                Value::Number(n) => at = at.get_mut(n.as_u64().unwrap() as usize).unwrap(),
                _ => unreachable!(),
            }
        }
        if path.is_empty() {
            out = c[1].clone();
        }
    }
    out
}

/// What a reading of WKT must get as PROJ does: the conversion's method,
/// its parameters in SI units (in any order), the remarks (where PROJ
/// keeps a PROJ4 extension) and the axis directions. Names of the base's
/// axes and units are the reader's own, as `wkt.rs` compares them.
fn reading(crs: &Value) -> Value {
    let si = |p: &Value| {
        let v = p["value"].as_f64().unwrap();
        match &p["unit"] {
            Value::String(u) if u == "degree" => v * 0.0174532925199433,
            Value::Object(o) => v * o["conversion_factor"].as_f64().unwrap(),
            _ => v,
        }
    };
    let c = &crs["conversion"];
    let mut params: Vec<(String, Value, f64)> = c["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            (
                p["name"].as_str().unwrap().to_owned(),
                p["id"].clone(),
                si(p),
            )
        })
        .collect();
    params.sort_by(|a, b| a.0.cmp(&b.0));
    let directions: Vec<&Value> = crs["coordinate_system"]["axis"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| &a["direction"])
        .collect();
    serde_json::json!({
        "method": c["method"],
        "parameters": params,
        "remarks": crs["remarks"],
        "directions": directions,
    })
}

/// Each method's WKT (WKT2 2019 and WKT1_GDAL) written as PROJ writes it,
/// and PROJ's WKT (those and WKT1_ESRI) read as PROJ reads it.
fn method_wkt(dir: &Path) -> (usize, Vec<String>) {
    let (mut compared, mut failed) = (0, Vec::new());
    for r in records(&dir.join("methods.jsonl"))
        .into_iter()
        .chain(records(&dir.join("custom.jsonl")))
    {
        let srs = r["srs"].as_str().unwrap();
        if !r["projjson"].is_object() {
            continue;
        }
        let json = r["projjson"].to_string();
        for (key, version) in [
            ("WKT2_2019", Some(wkt::Version::Wkt2_2019)),
            ("WKT1_GDAL", Some(wkt::Version::Wkt1Gdal)),
            ("WKT1_ESRI", None),
        ] {
            let entry = &r["wkt"][key];
            let theirs = entry["text"].as_str();
            if let Some(version) = version {
                compared += 1;
                match (wkt::from_projjson(&json, version, false), theirs) {
                    (Ok(ours), Some(theirs)) => {
                        if let Some((at, what)) = super::wkt::first_difference(&ours, theirs) {
                            failed.push(format!("{srs} {key} written: at {at}: {what}"));
                        }
                    }
                    (Err(_), None) => {}
                    (Ok(_), None) => failed.push(format!("{srs} {key}: written, PROJ can't")),
                    (Err(e), Some(_)) => failed.push(format!("{srs} {key}: not written: {e}")),
                }
            }
            let Some(text) = theirs else {
                continue;
            };
            compared += 1;
            let ours = wkt::to_projjson(text).map(|s| serde_json::from_str::<Value>(&s).unwrap());
            match (ours, entry.get("changes")) {
                (Ok(ours), Some(changes)) => {
                    let theirs = apply(&r["projjson"], changes);
                    let mut d = Vec::new();
                    diff(&reading(&ours), &reading(&theirs), "", &mut d);
                    for d in d {
                        failed.push(format!("{srs} {key} read: {d}"));
                    }
                    if let (Ok(a), Ok(b)) = (
                        Crs::from_projjson(&ours.to_string()),
                        Crs::from_projjson(&theirs.to_string()),
                    ) && !a.equivalent(&b, false)
                    {
                        failed.push(format!("{srs} {key} read: not equivalent to PROJ's"));
                    }
                }
                (Err(_), None) => {}
                (Ok(_), None) => failed.push(format!("{srs} {key}: read, PROJ can't")),
                // PROJ writes a "PROJ" method in WKT1_ESRI as `PROJ_tpers`,
                // which it reads back as a method of that name it can't
                // convert either.
                (Err(_), Some(_)) if key == "WKT1_ESRI" && text.contains("PROJECTION[\"PROJ_") => {}
                (Err(e), Some(_)) => failed.push(format!("{srs} {key}: not read: {e}")),
            }
        }
    }
    (compared, failed)
}

#[test]
fn proj_methods_wkt_as_in_proj() {
    let (compared, failed) = method_wkt(&xeibe_testkit::data_dir().join("proj-string"));
    assert!(compared > 500, "{compared}");
    assert!(
        failed.is_empty(),
        "{} of {compared}:\n{}",
        failed.len(),
        failed
            .iter()
            .take(
                std::env::var("TAKE")
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(60)
            )
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Mercator (Spherical) (EPSG:1026), which no PROJ string reads into: PROJ
/// writes it as `merc +R_C` and converts it on the conformal sphere at the
/// equator whatever the latitude of origin; we convert it on the one at the
/// latitude of origin, as EPSG has it. PROJ 9.8.1's string, and its results
/// scaled to that sphere (`r_c_scale`).
#[test]
fn mercator_spherical_converts_on_the_sphere_of_its_origin() {
    let wkt = r#"PROJCRS["Spherical Mercator test",BASEGEOGCRS["WGS 84",DATUM["World Geodetic System 1984",ELLIPSOID["WGS 84",6378137,298.257223563,LENGTHUNIT["metre",1]]],PRIMEM["Greenwich",0,ANGLEUNIT["degree",0.0174532925199433]]],CONVERSION["test",METHOD["Mercator (Spherical)",ID["EPSG",1026]],PARAMETER["Latitude of natural origin",30,ANGLEUNIT["degree",0.0174532925199433],ID["EPSG",8801]],PARAMETER["Longitude of natural origin",10,ANGLEUNIT["degree",0.0174532925199433],ID["EPSG",8802]],PARAMETER["False easting",1000,LENGTHUNIT["metre",1],ID["EPSG",8806]],PARAMETER["False northing",2000,LENGTHUNIT["metre",1],ID["EPSG",8807]]],CS[Cartesian,2],AXIS["(E)",east,ORDER[1],LENGTHUNIT["metre",1]],AXIS["(N)",north,ORDER[2],LENGTHUNIT["metre",1]]]"#;
    let json = wkt::to_projjson(wkt).unwrap();
    assert_eq!(
        proj_string::from_projjson(&json).unwrap(),
        "+proj=merc +R_C +lat_0=30 +lon_0=10 +x_0=1000 +y_0=2000 +datum=WGS84 +units=m +no_defs +type=crs"
    );
    let value: Value = serde_json::from_str(&json).unwrap();
    let base = Crs::from_projjson(&value["base_crs"].to_string()).unwrap();
    let crs = Crs::from_wkt(wkt).unwrap();
    let t = Transform::new(&base, &crs, AxisOrder::EastFirst).unwrap();
    let (scale, fe, fn_) = r_c_scale(&value);
    assert!((scale - 1.0 / (1.0 - 0.00669437999014 * 0.25)).abs() < 1e-12);
    for ((lon, lat), (x, y)) in [
        ((10.0, 30.0), (1000.0, 3493803.1042245994)),
        ((40.0, 60.0), (3329387.7285204283, 8373575.159040291)),
        ((-100.0, -45.0), (-12203088.337908238, -5600673.589001055)),
    ] {
        let (x, y) = (fe + (x - fe) * scale, fn_ + (y - fn_) * scale);
        let (ox, oy, _) = t.convert(lon, lat, None).unwrap();
        assert!(
            (ox - x).abs() < 1e-6 && (oy - y).abs() < 1e-6,
            "{lon} {lat}: {ox} {oy}"
        );
    }
}

/// A PROJ string's CRS's forward and inverse, from its geodetic CRS.
fn transforms(srs: &str) -> (Transform, Transform) {
    let json = proj_string::to_projjson(srs).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    let base = Crs::from_projjson(&value["base_crs"].to_string()).unwrap();
    let crs = Crs::from_projjson(&json).unwrap();
    (
        Transform::new(&base, &crs, AxisOrder::EastFirst).unwrap(),
        Transform::new(&crs, &base, AxisOrder::EastFirst).unwrap(),
    )
}

/// Where we differ from PROJ (`DIVERGES`), our inverse inverts our
/// forward, where PROJ's doesn't: ISEA on an ellipsoid and in any
/// orientation, GS48 and Alaska on another sphere or ellipsoid, the
/// bipolar conic where its cones meet (PROJ is 240 km off at 80°W 10°N).
#[test]
fn diverging_projections_invert_their_forward() {
    for (srs, points) in [
        (
            "+proj=isea +ellps=WGS84",
            &[(10.0, 45.0), (-60.0, -20.0), (179.0, 89.0), (0.0, -89.5)][..],
        ),
        (
            "+proj=isea +lat_0=40 +lon_0=-30 +azi=15 +ellps=GRS80",
            &[(10.0, 45.0), (-60.0, -20.0), (150.0, 5.0)],
        ),
        (
            "+proj=isea +orient=pole +R=6371000",
            &[(10.0, 45.0), (-170.0, -70.0)],
        ),
        (
            "+proj=isea +aperture=4 +resolution=6 +R=6371000",
            &[(10.0, 45.0)],
        ),
        ("+proj=gs48 +ellps=WGS84", &[(-100.0, 40.0), (-80.0, 35.0)]),
        ("+proj=gs50 +R=6371000", &[(-100.0, 40.0), (-150.0, 60.0)]),
        ("+proj=alsk +ellps=WGS84", &[(-150.0, 60.0)]),
        (
            "+proj=bipc +R=6371000",
            &[(-80.0, 10.0), (-100.0, 40.0), (-60.0, -10.0), (-85.0, 5.0)],
        ),
        (
            "+proj=bipc +ns +ellps=WGS84",
            &[(-80.0, 10.0), (-75.0, 15.0)],
        ),
    ] {
        let (forward, inverse) = transforms(&format!("{srs} +type=crs"));
        for &(lon, lat) in points {
            let (x, y, _) = forward.convert(lon, lat, None).unwrap();
            let (l, p, _) = inverse.convert(x, y, None).unwrap();
            assert!(
                off((l, p), (lon, lat)) < 1e-9,
                "{srs} at ({lon}, {lat}): ({x}, {y}) back to ({l}, {p})"
            );
        }
    }
}

/// ISEA's `lon_0` only turns the icosahedron: a point moved with it lands
/// where it did. PROJ also subtracts it as a central meridian.
#[test]
fn isea_lon_0_turns_the_icosahedron_once() {
    // PROJ's default, ISEA_STD_LONG, in degrees.
    let standard = 11.25;
    let (plain, _) = transforms("+proj=isea +R=6371000 +type=crs");
    let (turned, _) = transforms("+proj=isea +lon_0=40 +R=6371000 +type=crs");
    for (lon, lat) in [(10.0, 45.0), (-60.0, -20.0), (100.0, 70.0)] {
        let (x0, y0, _) = plain.convert(lon, lat, None).unwrap();
        let (x1, y1, _) = turned.convert(lon + 40.0 - standard, lat, None).unwrap();
        assert!(
            (x1 - x0).abs() < 1e-6 && (y1 - y0).abs() < 1e-6,
            "({lon}, {lat}): ({x0}, {y0}) and ({x1}, {y1})"
        );
    }
}

/// S2 refuses a point on the far side of its face, which PROJ projects onto
/// the face as if it were the antipode.
#[test]
fn s2_refuses_the_far_side_of_its_face() {
    let (forward, _) = transforms("+proj=s2 +R=6371000 +type=crs");
    assert!(forward.convert(30.0, 20.0, None).is_ok());
    assert!(forward.convert(150.0, -20.0, None).is_err());
    let (forward, _) = transforms("+proj=s2 +lat_0=90 +R=6371000 +type=crs");
    assert!(forward.convert(30.0, 60.0, None).is_ok());
    assert!(forward.convert(30.0, -1.0, None).is_err());
}
