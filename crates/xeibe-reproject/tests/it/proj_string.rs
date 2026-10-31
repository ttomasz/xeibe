//! Reading PROJ strings (`docs/pyproj-phase3.md`, "PROJ strings"): PROJ's
//! strings for EPSG CRSs and a corpus of hand-written ones, read and
//! compared with PROJ's reading of the same string
//! (`scripts/gen_proj_string_vectors.py`).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use xeibe_reproject::{AxisOrder, Crs, Error, Transform, proj_string};

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
