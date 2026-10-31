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
        for (p, want) in points.iter().zip(r["converted"].as_array().unwrap()) {
            let (lon, lat) = (p[0].as_f64().unwrap(), p[1].as_f64().unwrap());
            let Some(want) = want.as_array() else {
                continue;
            };
            compared += 1;
            let (wx, wy) = (want[0].as_f64().unwrap(), want[1].as_f64().unwrap());
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

/// The projections EPSG has no CRS for and those only PROJ defines
/// (`methods.jsonl`): points over the whole globe converted from each
/// string's geodetic CRS and back, against PROJ's (forwards to 2 µm, PROJ's
/// results being rounded to 1 µm; back to 1e-9°, from PROJ's forward
/// results). Every string must build a CRS here, but those of `NOT_PORTED`,
/// which must not.
fn method_conversions(path: &Path, not_ported: &[&str]) -> (usize, Vec<String>) {
    let (mut compared, mut failed) = (0, Vec::new());
    for r in records(path) {
        let srs = r["srs"].as_str().unwrap();
        let Some(points) = r["points"].as_array() else {
            continue;
        };
        let json: Value = serde_json::from_str(&proj_string::to_projjson(srs).unwrap()).unwrap();
        let proj = srs.split(' ').next().unwrap().trim_start_matches("+proj=");
        let not_ported = not_ported.contains(&proj);
        let base = if json["type"] == "ProjectedCRS" || json["type"] == "DerivedGeographicCRS" {
            &json["base_crs"]
        } else {
            &json
        };
        let (base, crs) = match (
            Crs::from_projjson(&base.to_string()),
            Crs::from_projjson(&json.to_string()),
        ) {
            (Ok(_), Ok(_)) if not_ported => {
                failed.push(format!("{srs}: builds, but {proj} is in NOT_PORTED"));
                continue;
            }
            (Ok(b), Ok(c)) => (b, c),
            (_, Err(Error::Unsupported { .. })) if not_ported => continue,
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
            compared += 1;
            let ours = forward.convert(lon, lat, None);
            let Some(want) = want.as_array() else {
                if let Ok(ours) = ours {
                    failed.push(format!("{srs} at ({lon}, {lat}): {ours:?}, PROJ fails"));
                }
                continue;
            };
            let (wx, wy) = (want[0].as_f64().unwrap(), want[1].as_f64().unwrap());
            match ours {
                Ok((x, y, _)) if (x - wx).abs() < 2e-6 && (y - wy).abs() < 2e-6 => {}
                other => failed.push(format!(
                    "{srs} at ({lon}, {lat}): {other:?}, PROJ ({wx}, {wy})"
                )),
            }
            let (Some(backward), Some(back)) = (&backward, inverse.map(|v| &v[i])) else {
                continue;
            };
            compared += 1;
            let ours = backward.convert(wx, wy, None);
            let Some(back) = back.as_array() else {
                if let Ok(ours) = ours {
                    failed.push(format!(
                        "{srs} back from ({wx}, {wy}): {ours:?}, PROJ fails"
                    ));
                }
                continue;
            };
            let (blon, blat) = (back[0].as_f64().unwrap(), back[1].as_f64().unwrap());
            let close = |(l, p): (f64, f64)| {
                let dl = (l - blon + 540.0).rem_euclid(360.0) - 180.0;
                (p - blat).abs() < 1e-9 && (dl.abs() < 1e-9 || blat.abs() > 89.999999)
            };
            match ours {
                Ok((l, p, _)) if close((l, p)) => {}
                other => failed.push(format!(
                    "{srs} back from ({wx}, {wy}): {other:?}, PROJ ({blon}, {blat})"
                )),
            }
        }
    }
    (compared, failed)
}

/// The projections of `methods.jsonl` not yet ported from PROJ.
const NOT_PORTED: &[&str] = &[];

/// The projections of `custom.jsonl` not yet ported from PROJ.
const CUSTOM_NOT_PORTED: &[&str] = &[
    "airocean",
    "bertin1953",
    "healpix",
    "rhealpix",
    "imoll",
    "imoll_o",
    "isea",
    "calcofi",
    "s2",
    "spilhaus",
    "lsat",
    "misrsom",
    "som",
];

/// The projections PROJ has no CRS method for (`custom.jsonl`), as
/// `methods.jsonl`'s.
#[test]
fn custom_proj_methods_convert_as_in_proj() {
    let (compared, failed) = method_conversions(
        &xeibe_testkit::data_dir().join("proj-string/custom.jsonl"),
        CUSTOM_NOT_PORTED,
    );
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
    let (compared, failed) = method_conversions(
        &xeibe_testkit::data_dir().join("proj-string/methods.jsonl"),
        NOT_PORTED,
    );
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
/// converts it with `merc +R_C`, on the conformal sphere at the equator
/// whatever the latitude of origin. PROJ 9.8.1's string and results.
#[test]
fn mercator_spherical_converts_as_in_proj() {
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
    for ((lon, lat), (x, y)) in [
        ((10.0, 30.0), (1000.0, 3493803.1042245994)),
        ((40.0, 60.0), (3329387.7285204283, 8373575.159040291)),
        ((-100.0, -45.0), (-12203088.337908238, -5600673.589001055)),
    ] {
        let (ox, oy, _) = t.convert(lon, lat, None).unwrap();
        assert!(
            (ox - x).abs() < 1e-6 && (oy - y).abs() < 1e-6,
            "{lon} {lat}: {ox} {oy}"
        );
    }
}
