//! A CRS's PROJJSON as a PROJ string, the way PROJ 9.8.1 writes it
//! (`IPROJStringExportable::exportToPROJString` for a CRS: the
//! `_exportToPROJString` of each CRS class in `crs.cpp`, `Conversion`'s in
//! `conversion.cpp`, and the ellipsoid's and prime meridian's in
//! `datum.cpp`). PROJ writes the same string in its PROJ_4 and PROJ_5
//! conventions for every CRS.

use serde_json::Value;

use super::names::{METHODS, Method, UnitType};
use super::read::well_known_ellipsoid;
use super::tokens::{self, DEG_TO_RAD};
use crate::error::Error;
use crate::proj_tables;

/// PROJ's linear units (`pj_units` in `units.cpp`), in its order: the
/// first within 1e-10 of a unit's size names it.
const PROJ_UNITS: &[(&str, f64)] = &[
    ("km", 1000.0),
    ("m", 1.0),
    ("dm", 0.1),
    ("cm", 0.01),
    ("mm", 0.001),
    ("kmi", 1852.0),
    ("in", 0.0254),
    ("ft", 0.3048),
    ("yd", 0.9144),
    ("mi", 1609.344),
    ("fath", 1.8288),
    ("ch", 20.1168),
    ("link", 0.201168),
    ("us-in", 100.0 / 3937.0),
    ("us-ft", 1200.0 / 3937.0),
    ("us-yd", 3600.0 / 3937.0),
    ("us-ch", 79200.0 / 3937.0),
    ("us-mi", 6336000.0 / 3937.0),
    ("ind-yd", 0.91439523),
    ("ind-ft", 0.30479841),
    ("ind-ch", 20.11669506),
];

/// A CRS's PROJJSON as a PROJ string.
pub(crate) fn crs(v: &Value) -> Result<String, Error> {
    // A CRS PROJ keeps a PROJ string for (in its remarks): that string, as
    // its `exportToPROJString` gives it back.
    if let Some(text) = extension(v) {
        return Ok(format!("{} +type=crs", text.replace(" +type=crs", "")));
    }
    let mut f = Formatter::default();
    f.crs(v)?;
    if !f.has("no_defs") {
        f.flag("no_defs");
    }
    if !f.has("type") {
        f.param("type", "crs");
    }
    Ok(f.finish())
}

/// The `PROJ4` extension PROJ writes in WKT1 for a projected CRS whose
/// method is "PROJ …" (`createPROJExtensionFromCustomProj`): the
/// conversion's step, flags and parameters, its base's datum, `+wktext` and
/// `+no_defs`.
pub(crate) fn custom_extension(crs: &Value) -> Result<String, Error> {
    let base = &crs["base_crs"];
    if base["type"] != "GeographicCRS" {
        return Err(cannot());
    }
    let mut f = Formatter::default();
    f.custom(&crs["conversion"])?;
    f.datum_info(base)?;
    f.flag("wktext");
    f.flag("no_defs");
    Ok(f.finish())
}

/// The PROJ string PROJ keeps as a CRS's extension, from its remarks
/// ("PROJ CRS string: …", up to ". "), as PROJ's `CRS::setProperties`
/// reads it.
pub(crate) fn extension(v: &Value) -> Option<&str> {
    const PREFIX: &str = "PROJ CRS string: ";
    let remarks = v["remarks"].as_str()?;
    let rest = &remarks[remarks.find(PREFIX)? + PREFIX.len()..];
    Some(rest.find(". ").map_or(rest, |end| &rest[..end]))
}

fn cannot() -> Error {
    Error::ProjString {
        message: "CRS cannot be converted to a PROJ string".into(),
    }
}

/// The parameters written so far, and what a Bound CRS hands its base.
#[derive(Default)]
struct Formatter {
    step: Option<String>,
    params: Vec<(String, Option<String>)>,
    towgs84: Vec<f64>,
    nadgrids: String,
    vertical_grids: Option<(String, &'static str)>,
}

impl Formatter {
    fn add_step(&mut self, name: &str) {
        self.step = Some(name.to_owned());
    }

    fn param(&mut self, key: &str, value: impl Into<String>) {
        self.params.push((key.to_owned(), Some(value.into())));
    }

    fn flag(&mut self, key: &str) {
        self.params.push((key.to_owned(), None));
    }

    fn number(&mut self, key: &str, value: f64) {
        self.param(key, number(value));
    }

    fn has(&self, key: &str) -> bool {
        self.params.iter().any(|(k, _)| k == key)
    }

    fn finish(self) -> String {
        let mut out: Vec<String> = Vec::new();
        if let Some(step) = &self.step {
            out.push(format!("+proj={step}"));
        }
        for (k, v) in self.params {
            out.push(match v {
                Some(v) => format!("+{k}={v}"),
                None => format!("+{k}"),
            });
        }
        out.join(" ")
    }

    fn crs(&mut self, v: &Value) -> Result<(), Error> {
        match v["type"].as_str() {
            Some("GeographicCRS") => {
                if v["coordinate_system"]["subtype"] != "ellipsoidal" {
                    return Err(cannot());
                }
                self.add_step("longlat");
                self.datum_info(v)
            }
            Some("GeodeticCRS") => {
                if v["coordinate_system"]["subtype"] != "Cartesian" {
                    return Err(cannot());
                }
                self.add_step("geocent");
                self.datum_info(v)?;
                let unit = &v["coordinate_system"]["axis"][0]["unit"];
                if (unit_factor(unit) - 1.0).abs() > 1e-10 {
                    return Err(cannot());
                }
                self.param("units", "m");
                Ok(())
            }
            Some("ProjectedCRS") => self.projected(v),
            // A rotated pole (`ob_tran`): the conversion, then the base's datum.
            Some("DerivedGeographicCRS") => {
                let conv = &v["conversion"];
                if !conv["method"]["name"]
                    .as_str()
                    .is_some_and(|n| n.starts_with("PROJ "))
                {
                    return Err(cannot());
                }
                self.custom(conv)?;
                self.datum_info(&v["base_crs"])
            }
            Some("BoundCRS") => self.bound(v),
            Some("CompoundCRS") => {
                for c in v["components"].as_array().into_iter().flatten() {
                    self.crs(c)?;
                }
                Ok(())
            }
            // A derived vertical CRS (a unit change, a height turned into a
            // depth) as a vertical one on its own axis: PROJ can't write a
            // derived one, but its database holds EPSG's as vertical CRSs,
            // which it writes so.
            Some("VerticalCRS" | "DerivedVerticalCRS") => {
                if let Some((grids, crs)) = self.vertical_grids.take() {
                    self.param("geoidgrids", grids);
                    self.param("geoid_crs", crs);
                }
                if let Some(unit) = v["coordinate_system"]["axis"].get(0).map(|a| &a["unit"]) {
                    match unit_name(unit_factor(unit)) {
                        Some(name) => self.param("vunits", name),
                        None => self.number("vto_meter", unit_factor(unit)),
                    }
                }
                Ok(())
            }
            _ => Err(cannot()),
        }
    }

    /// `BoundCRS::_exportToPROJString`: to WGS 84, a vertical grid, a
    /// horizontal grid (`+nadgrids`) or a Helmert transformation
    /// (`+towgs84`) for the base to write.
    fn bound(&mut self, v: &Value) -> Result<(), Error> {
        let base = &v["source_crs"];
        let hub_wgs84 = v["target_crs"]["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case("WGS 84"));
        let t = &v["transformation"];
        let file = t["parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|p| p["value"].as_str().map(str::to_owned));
        if base["type"] == "VerticalCRS" && hub_wgs84 {
            if let Some(file) = file {
                self.vertical_grids = Some((file, "WGS84"));
            }
            return self.crs(base);
        }
        if hub_wgs84 && let Some(file) = file.filter(|_| is_grid_method(t)) {
            self.nadgrids = file;
            self.crs(base)?;
            self.nadgrids.clear();
            return Ok(());
        }
        let hub_geodetic = matches!(
            v["target_crs"]["type"].as_str(),
            Some("GeographicCRS" | "GeodeticCRS")
        );
        if hub_geodetic && hub_wgs84 {
            self.towgs84 = towgs84(t)?;
        }
        self.crs(base)?;
        self.towgs84.clear();
        Ok(())
    }

    /// `GeodeticCRS::addDatumInfoToPROJString`.
    fn datum_info(&mut self, crs: &Value) -> Result<(), Error> {
        let datum = datum_of(crs).ok_or_else(cannot)?;
        let name = datum["name"].as_str().unwrap_or("");
        let mut written = false;
        if self.towgs84.is_empty() && self.nadgrids.is_empty() && name != "unknown" {
            for (proj, epsg_name, a, shape) in [
                (
                    "WGS84",
                    "World Geodetic System 1984",
                    6378137.0,
                    Shape::Rf(298.257223563),
                ),
                (
                    "NAD27",
                    "North American Datum 1927",
                    6378206.4,
                    Shape::B(6356583.8),
                ),
                (
                    "NAD83",
                    "North American Datum 1983",
                    6378137.0,
                    Shape::Rf(298.257222101),
                ),
            ] {
                if equivalent_name(name, epsg_name)
                    && prime_meridian(datum)? == 0.0
                    && same_ellipsoid(&datum["ellipsoid"], a, shape)?
                {
                    self.param("datum", proj);
                    written = true;
                    break;
                }
            }
        }
        if !written {
            self.ellipsoid(&datum["ellipsoid"])?;
            let pm = prime_meridian(datum)?;
            if pm != 0.0 {
                match proj_tables::PRIME_MERIDIANS.iter().find(|(_, defn)| {
                    tokens::dmstor(defn).is_some_and(|r| (pm * DEG_TO_RAD - r).abs() < 1e-10)
                }) {
                    Some((id, _)) => self.param("pm", *id),
                    None => self.number("pm", pm),
                }
            }
        }
        if self.towgs84.len() == 7 {
            let v = self
                .towgs84
                .iter()
                .map(|&x| number(x))
                .collect::<Vec<_>>()
                .join(",");
            self.param("towgs84", v);
        }
        if !self.nadgrids.is_empty() {
            let grids = self.nadgrids.clone();
            self.param("nadgrids", grids);
        }
        Ok(())
    }

    /// `Ellipsoid::_exportToPROJString`.
    fn ellipsoid(&mut self, e: &Value) -> Result<(), Error> {
        let (a, shape) = ellipsoid_shape(e)?;
        let (b, rf) = match shape {
            Shape::Rf(rf) => (if rf == 0.0 { a } else { a * (1.0 - 1.0 / rf) }, rf),
            Shape::B(b) => (b, if a == b { 0.0 } else { a / (a - b) }),
        };
        if let Some(n) = well_known_ellipsoid(a, b, rf) {
            self.param("ellps", n.id);
        } else if b == a || rf == 0.0 {
            self.number("R", a);
        } else {
            self.number("a", a);
            match shape {
                Shape::Rf(rf) => self.number("rf", rf),
                Shape::B(b) => self.number("b", b),
            }
        }
        Ok(())
    }

    /// `ProjectedCRS::_exportToPROJString`, which is `Conversion`'s with the
    /// CRS as its target.
    fn projected(&mut self, crs: &Value) -> Result<(), Error> {
        let conv = &crs["conversion"];
        let base = &crs["base_crs"];
        let code = conv["method"]["id"]["code"].as_u64().map(|c| c as u32);
        let method_name = conv["method"]["name"].as_str().unwrap_or("");
        let p = Params(conv);
        let axes = &crs["coordinate_system"]["axis"];
        let dir = |i: usize| axes[i]["direction"].as_str().unwrap_or("");
        let mut done = false;
        let mut ellipsoid_done = false;
        let mut target_modifiers = true;
        let mut insert_wsu = false;
        let mut negate_k = false;
        match code {
            Some(9807) => {
                if let Some((zone, north)) = utm(&p) {
                    done = true;
                    self.add_step("utm");
                    self.param("zone", zone.to_string());
                    if !north {
                        self.flag("south");
                    }
                } else if p.value(8805, UnitType::Scale)? < 0.0
                    && p.value(8806, UnitType::Linear)? == 0.0
                    && p.value(8807, UnitType::Linear)? == 0.0
                    && dir(0) == "east"
                    && dir(1) == "north"
                {
                    insert_wsu = true;
                    negate_k = true;
                }
            }
            Some(9812 | 9815) => {
                let azimuth = p.value(8813, UnitType::Angular)?;
                let gamma = p.value(8814, UnitType::Angular)?;
                if (azimuth - 90.0).abs() < 1e-4 && (gamma - 90.0).abs() < 1e-4 {
                    done = true;
                    self.add_step("somerc");
                    self.number("lat_0", p.value(8811, UnitType::Angular)?);
                    self.number("lon_0", p.value(8812, UnitType::Angular)?);
                    self.number("k_0", p.value(8815, UnitType::Scale)?);
                    let (x, y) = if code == Some(9812) {
                        (8806, 8807)
                    } else {
                        (8816, 8817)
                    };
                    self.number("x_0", p.value(x, UnitType::Linear)?);
                    self.number("y_0", p.value(y, UnitType::Linear)?);
                }
            }
            Some(1041 | 1043) => {
                if (p.value(1036, UnitType::Angular)? - 30.2881397).abs() > 1e-7
                    || (p.value(8818, UnitType::Angular)? - 78.5).abs() > 1e-8
                {
                    return Err(cannot());
                }
            }
            Some(9804) => {
                if p.value(8801, UnitType::Angular)? != 0.0 {
                    return Err(cannot());
                }
            }
            Some(9805) => {
                if p.find_name("Scale factor at natural origin").is_some()
                    && (p.value(8805, UnitType::Scale)? - 1.0).abs() > 1e-10
                {
                    return Err(cannot());
                }
                if p.value(8801, UnitType::Angular)? != 0.0 {
                    return Err(cannot());
                }
            }
            Some(9808) => {
                if p.value(8806, UnitType::Linear)? != 0.0
                    || p.value(8807, UnitType::Linear)? != 0.0
                {
                    return Err(cannot());
                }
            }
            Some(1024) => {
                self.web_mercator(crs, &p)?;
                done = true;
                ellipsoid_done = true;
                target_modifiers = false;
            }
            _ if conv["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case("Popular Visualisation Mercator")) =>
            {
                self.web_mercator(crs, &p)?;
                done = true;
                ellipsoid_done = true;
                target_modifiers = false;
            }
            _ if method_name.starts_with("PROJ ") => {
                self.custom(conv)?;
                done = true;
            }
            _ if equivalent_name(method_name, "Peirce Quincuncial (Square)")
                || equivalent_name(method_name, "Peirce Quincuncial (Diamond)") =>
            {
                if p.find_code(8805).is_some()
                    && (p.value(8805, UnitType::Scale)? - 1.0).abs() > 1e-10
                {
                    return Err(cannot());
                }
                if p.find_code(8801).is_some()
                    && (p.value(8801, UnitType::Angular)? - 90.0).abs() > 1e-10
                {
                    return Err(cannot());
                }
            }
            _ => {}
        }
        if !done {
            let m = mapping(code, method_name).ok_or_else(cannot)?;
            let Some(proj) = m.proj else {
                return Err(cannot());
            };
            self.add_step(proj);
            if let Some(aux) = m.aux {
                let mut add = true;
                if aux.starts_with("axis=")
                    && (m.epsg == 9819 || m.epsg == 1042)
                    && dir(0) == "west"
                    && dir(1) == "south"
                {
                    self.flag("czech");
                    add = false;
                }
                if (aux == "f=0" || aux == "R_A") && is_sphere(base)? {
                    add = false;
                }
                if add {
                    match aux.split_once('=') {
                        Some((k, v)) => self.param(k, v),
                        None => self.flag(aux),
                    }
                }
            }
            if insert_wsu {
                self.param("axis", "wsu");
            }
            if m.epsg == 9829 {
                let parallel = p.value(8832, UnitType::Angular)?;
                self.number("lat_0", if parallel >= 0.0 { 90.0 } else { -90.0 });
            }
            for param in m.params {
                let Some(key) = param.proj else {
                    continue;
                };
                let value = match p.get(param) {
                    Some(v) => v.si_or_degrees(param.unit)?,
                    None if (m.epsg == 9812 || m.epsg == 9815) && param.epsg == 8814 => continue,
                    None if param.epsg == 8805 => 1.0,
                    None => 0.0,
                };
                if m.epsg == 9801 && key == "lat_1" {
                    self.number(key, value);
                    self.number("lat_0", value);
                } else if negate_k && param.epsg == 8805 {
                    self.number(key, -value);
                } else {
                    self.number(key, value);
                }
            }
        }
        if target_modifiers {
            if !ellipsoid_done {
                match base["type"].as_str() {
                    Some("GeographicCRS") => self.datum_info(base)?,
                    _ => {
                        let datum = datum_of(base).ok_or_else(cannot)?;
                        self.ellipsoid(&datum["ellipsoid"])?;
                    }
                }
            }
            let factor = unit_factor(&axes[0]["unit"]);
            if (factor - 1.0).abs() > 1e-10 {
                match unit_name(factor) {
                    Some(name) => self.param("units", name),
                    None => self.number("to_meter", factor),
                }
            } else {
                self.param("units", "m");
            }
        }
        Ok(())
    }

    /// `createPROJExtensionFromCustomProj`: a "PROJ" conversion's step,
    /// the flags of its name and its parameters, lengths in metres and
    /// angles in degrees.
    fn custom(&mut self, conv: &Value) -> Result<(), Error> {
        let name = conv["method"]["name"].as_str().ok_or_else(cannot)?;
        let mut tokens = name.split(' ').skip(1);
        self.add_step(tokens.next().ok_or_else(cannot)?);
        for t in tokens {
            match t.split_once('=') {
                Some((k, v)) => self.param(k, v),
                None => self.flag(t),
            }
        }
        for p in conv["parameters"].as_array().into_iter().flatten() {
            let key = p["name"].as_str().ok_or_else(cannot)?;
            let value = p["value"].as_f64().ok_or_else(cannot)?;
            let unit = &p["unit"];
            let kind = match unit {
                Value::String(s) if s == "degree" => "angle",
                Value::String(s) if s == "metre" => "length",
                Value::Object(o) => match o.get("type").and_then(Value::as_str) {
                    Some("AngularUnit") => "angle",
                    Some("LinearUnit") => "length",
                    _ => "other",
                },
                _ => "other",
            };
            let factor = unit_factor(unit);
            self.number(
                key,
                match kind {
                    "angle" => value * factor / (std::f64::consts::PI / 180.0),
                    "length" => value * factor,
                    _ => value,
                },
            );
        }
        Ok(())
    }

    /// `createPROJ4WebMercator`.
    fn web_mercator(&mut self, crs: &Value, p: &Params) -> Result<(), Error> {
        let base = &crs["base_crs"];
        if base["type"] != "GeographicCRS" {
            return Err(cannot());
        }
        let factor = unit_factor(&crs["coordinate_system"]["axis"][0]["unit"]);
        let units = if (factor - 1.0).abs() <= 1e-10 {
            "m"
        } else {
            unit_name(factor).ok_or_else(cannot)?
        };
        let (a, _) = ellipsoid_shape(&datum_of(base).ok_or_else(cannot)?["ellipsoid"])?;
        self.add_step("merc");
        self.number("a", a);
        self.number("b", a);
        self.number("lat_ts", 0.0);
        self.number("lon_0", p.value(8802, UnitType::Angular)?);
        self.number("x_0", p.value(8806, UnitType::Linear)?);
        self.number("y_0", p.value(8807, UnitType::Linear)?);
        self.number("k", 1.0);
        self.param("units", units);
        self.param("nadgrids", "@null");
        self.flag("wktext");
        self.flag("no_defs");
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Rf(f64),
    B(f64),
}

/// PROJ's `formatToString`: `%.15g`, after rounding to a tenth a value
/// within 1e-8 of one.
fn number(v: f64) -> String {
    let v = if (v * 10.0 - (v * 10.0).round()).abs() < 1e-8 {
        (v * 10.0).round() / 10.0
    } else {
        v
    };
    crate::number::to_string(v)
}

fn unit_name(factor: f64) -> Option<&'static str> {
    PROJ_UNITS
        .iter()
        .find(|(_, f)| (f - factor).abs() < 1e-10 * factor)
        .map(|(n, _)| *n)
}

/// A unit's size in metres, radians or unity.
fn unit_factor(unit: &Value) -> f64 {
    match unit {
        Value::String(s) => match s.as_str() {
            "degree" => std::f64::consts::PI / 180.0,
            _ => 1.0,
        },
        Value::Object(o) => o
            .get("conversion_factor")
            .and_then(Value::as_f64)
            .unwrap_or(1.0),
        _ => 1.0,
    }
}

/// A `{value, unit}` measure, or a bare number in the default unit, in
/// SI units.
fn measure(v: &Value, default_factor: f64) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().map(|x| x * default_factor),
        Value::Object(o) => Some(o.get("value")?.as_f64()? * unit_factor(o.get("unit")?)),
        _ => None,
    }
}

fn datum_of(crs: &Value) -> Option<&Value> {
    [&crs["datum"], &crs["datum_ensemble"]]
        .into_iter()
        .find(|d| d.is_object())
}

fn prime_meridian(datum: &Value) -> Result<f64, Error> {
    match &datum["prime_meridian"] {
        Value::Null => Ok(0.0),
        pm => measure(&pm["longitude"], std::f64::consts::PI / 180.0)
            .map(|r| r.to_degrees())
            .ok_or_else(cannot),
    }
}

fn ellipsoid_shape(e: &Value) -> Result<(f64, Shape), Error> {
    if let Some(r) = measure(&e["radius"], 1.0) {
        return Ok((r, Shape::Rf(0.0)));
    }
    let a = measure(&e["semi_major_axis"], 1.0).ok_or_else(cannot)?;
    if let Some(rf) = e["inverse_flattening"].as_f64() {
        return Ok((a, Shape::Rf(rf)));
    }
    let b = measure(&e["semi_minor_axis"], 1.0).ok_or_else(cannot)?;
    Ok((a, Shape::B(b)))
}

fn same_ellipsoid(e: &Value, a: f64, shape: Shape) -> Result<bool, Error> {
    let (ea, es) = ellipsoid_shape(e)?;
    let rf = |a: f64, s: Shape| match s {
        Shape::Rf(rf) => rf,
        Shape::B(b) if b == a => 0.0,
        Shape::B(b) => a / (a - b),
    };
    Ok((ea - a).abs() <= 1e-10 * a && (rf(ea, es) - rf(a, shape)).abs() <= 1e-10 * rf(a, shape))
}

fn is_sphere(base: &Value) -> Result<bool, Error> {
    let crs = if base["type"] == "BoundCRS" {
        &base["source_crs"]
    } else {
        base
    };
    let Some(datum) = datum_of(crs) else {
        return Ok(false);
    };
    Ok(match ellipsoid_shape(&datum["ellipsoid"])? {
        (_, Shape::Rf(rf)) => rf == 0.0,
        (a, Shape::B(b)) => a == b,
    })
}

/// PROJ's `Identifier::isEquivalentName`, enough for EPSG's datum names:
/// letters and digits, ignoring case; a datum ensemble's name without
/// " ensemble".
fn equivalent_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.strip_suffix(" ensemble")
            .unwrap_or(s)
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>()
    };
    norm(a) == norm(b)
}

fn mapping(code: Option<u32>, name: &str) -> Option<&'static Method> {
    METHODS
        .iter()
        .find(|m| (code.is_some_and(|c| c != 0 && m.epsg == c)) || equivalent_name(m.name, name))
}

fn is_grid_method(t: &Value) -> bool {
    let name = t["method"]["name"].as_str().unwrap_or("");
    let code = t["method"]["id"]["code"].as_u64();
    matches!(code, Some(9613..=9615)) || ["NTv1", "NTv2", "NADCON"].iter().any(|n| name.contains(n))
}

/// `Transformation::getTOWGS84Parameters`: a Helmert transformation's
/// seven values in Position Vector form (metres, arc-seconds, parts per
/// million), or none for another method.
fn towgs84(t: &Value) -> Result<Vec<f64>, Error> {
    let name = t["method"]["name"].as_str().unwrap_or("");
    let code = t["method"]["id"]["code"].as_u64().unwrap_or(0);
    let params = t["parameters"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let count = params.len();
    let lower = name.to_ascii_lowercase();
    let (seven, invert) = if (count == 7 && lower.contains("coordinate frame"))
        || matches!(code, 1032 | 1132 | 9607 | 1133 | 1140 | 1038 | 1149)
    {
        (true, true)
    } else if (count == 7 && lower.contains("position vector"))
        || matches!(code, 1033 | 9606 | 1037)
    {
        (true, false)
    } else if (count == 3 && lower.contains("geocentric translations"))
        || matches!(code, 1031 | 9603 | 1035)
    {
        (false, false)
    } else {
        return Ok(Vec::new());
    };
    let arc_second = std::f64::consts::PI / 648_000.0;
    let sign = if invert { -1.0 } else { 1.0 };
    let mut out = vec![0.0; 7];
    let mut found = [false; 7];
    for p in params {
        let Some(code) = p["id"]["code"].as_u64() else {
            continue;
        };
        let i = match code {
            8605..=8611 => (code - 8605) as usize,
            _ => continue,
        };
        let si = p["value"].as_f64().ok_or_else(cannot)? * unit_factor(&p["unit"]);
        out[i] = match i {
            0..=2 => si,
            3..=5 => sign * si / arc_second,
            _ => si / 1e-6,
        };
        found[i] = true;
    }
    if seven && !found.iter().all(|&f| f) || !seven && !found[..3].iter().all(|&f| f) {
        return Err(cannot());
    }
    Ok(out
        .into_iter()
        .map(|x| if x == 0.0 { 0.0 } else { x })
        .collect())
}

/// `Conversion::isUTM`.
fn utm(p: &Params) -> Option<(u32, bool)> {
    let lat = p.raw(8801)?;
    let lon = p.raw(8802)?;
    let k = p.raw(8805)?;
    let fe = p.raw(8806)?;
    let fn_ = p.raw(8807)?;
    let degree =
        |u: &Value| (unit_factor(u) - std::f64::consts::PI / 180.0).abs() <= 1e-10 * unit_factor(u);
    let metre = |u: &Value| (unit_factor(u) - 1.0).abs() <= 1e-10;
    let unity = |u: &Value| (unit_factor(u) - 1.0).abs() <= 1e-10;
    if (lat.0 - 0.0).abs() >= 1e-10
        || !degree(&lon.1)
        || !unity(&k.1)
        || (k.0 - 0.9996).abs() >= 1e-10
    {
        return None;
    }
    let zone = (lon.0 + 183.0) / 6.0;
    if !(zone > 0.9 && zone < 60.1 && (zone - zone.round()).abs() < 1e-10) {
        return None;
    }
    if fe.0 != 500000.0 || !metre(&fe.1) || !metre(&fn_.1) {
        return None;
    }
    let north = if (fn_.0 - 0.0).abs() < 1e-10 {
        true
    } else if (fn_.0 - 10_000_000.0).abs() < 1e-10 {
        false
    } else {
        return None;
    };
    Some((zone.round() as u32, north))
}

/// A conversion's parameters, by EPSG code or name.
struct Params<'a>(&'a Value);

struct ParamValue<'a>(&'a Value);

impl ParamValue<'_> {
    fn si_or_degrees(&self, unit: UnitType) -> Result<f64, Error> {
        let v = self.0["value"].as_f64().ok_or_else(cannot)?;
        let factor = match &self.0["unit"] {
            Value::Null => 1.0,
            u => unit_factor(u),
        };
        Ok(match unit {
            UnitType::Angular => v * factor / (std::f64::consts::PI / 180.0),
            _ => v * factor,
        })
    }
}

impl Params<'_> {
    fn all(&self) -> impl Iterator<Item = &Value> {
        self.0["parameters"].as_array().into_iter().flatten()
    }

    fn find_code(&self, code: u32) -> Option<&Value> {
        self.all()
            .find(|p| p["id"]["code"].as_u64() == Some(u64::from(code)))
    }

    fn find_name(&self, name: &str) -> Option<&Value> {
        self.all()
            .find(|p| p["name"].as_str().is_some_and(|n| equivalent_name(n, name)))
    }

    fn get(&self, param: &super::names::Param) -> Option<ParamValue<'_>> {
        (if crate::methods::is_epsg(param.epsg) {
            self.find_code(param.epsg)
        } else {
            None
        })
        .or_else(|| self.find_name(param.name))
        .map(ParamValue)
    }

    /// The value in its own unit, and the unit.
    fn raw(&self, code: u32) -> Option<(f64, Value)> {
        let p = self.find_code(code)?;
        Some((p["value"].as_f64()?, p["unit"].clone()))
    }

    /// `parameterValueNumeric`: in degrees for an angle, else in SI units;
    /// 0 when missing.
    fn value(&self, code: u32, unit: UnitType) -> Result<f64, Error> {
        match self.find_code(code) {
            Some(p) => ParamValue(p).si_or_degrees(unit),
            None => Ok(0.0),
        }
    }
}
