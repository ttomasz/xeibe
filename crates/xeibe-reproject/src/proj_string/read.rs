//! A PROJ string to PROJJSON, the way PROJ 9.8.1 reads it
//! (`PROJStringParser::createFromPROJString` in `io.cpp`), for the strings
//! PROJ reads into a geographic, geocentric or projected CRS with an EPSG
//! method, possibly bound to WGS 84 (`docs/pyproj-phase3.md`, "PROJ
//! strings").
//!
//! PROJ checks a string by building the operation it describes, keeps the
//! parameters that operation looks up and drops the others. Of those kept,
//! one the CRS doesn't use (`+over`, `+lon_wrap`, a `+lon_0` on
//! `+proj=longlat`) is kept only as a `PROJ4` extension of the CRS: such
//! strings are refused here, as are PROJ-only projections, pipelines and
//! `+geoidgrids`.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{Map, Value, json};

use super::names::{GENERIC_READS, METHODS, Method, OTHER_OPERATIONS, PROJECTION_READS, UnitType};
use super::tokens::{self, DEG_TO_RAD, KeyValue, RAD_TO_DEG, Step};
use crate::error::Error;
use crate::proj_tables::{self, Shape};
use crate::wkt::wkt1::towgs84_bound;
use crate::wkt::wkt2::num;

type Obj = Map<String, Value>;

/// A PROJ string's CRS as a PROJJSON value.
pub(crate) fn crs(text: &str) -> Result<Value, Error> {
    let syntax = tokens::syntax(text);
    let unsupported = |what: &str| Error::Unsupported {
        crs: text.to_owned(),
        what: what.to_owned(),
    };
    if syntax.pipeline {
        return Err(unsupported("a PROJ pipeline (+proj=pipeline)"));
    }
    let mut p = Parser {
        text,
        title: syntax.title,
        globals: syntax.globals,
        step: match <[Step; 1]>::try_from(syntax.steps) {
            Ok([step]) => step,
            Err(_) => return Err(invalid("no +proj= or +init=")),
        },
        ignore_nadgrids: false,
    };
    if p.get("type") != "crs" {
        return Err(invalid("not a CRS: +type=crs is missing"));
    }
    if p.step.inverted {
        return Err(invalid("an inverted CRS (+inv)"));
    }
    if p.step.is_init {
        return p.init();
    }
    let name = p.step.name.clone();
    if name == "topocentric" {
        return Err(unsupported("+proj=topocentric"));
    }
    if !(is_geographic(&name) || name == "geocent" || name == "cart" || is_projected(&name)) {
        return Err(if OTHER_OPERATIONS.contains(&name.as_str()) {
            unsupported(&format!(
                "+proj={name} (an operation, not a CRS: it would be a PROJ-based operation)"
            ))
        } else {
            invalid(&format!("unknown projection {name}"))
        });
    }
    p.drop_unread();
    let crs = if is_geographic(&name) {
        if p.has("geoc") {
            return Err(unsupported("+geoc (geocentric latitudes)"));
        }
        let (geographic, _) = p.geographic()?;
        p.bound(geographic, false)?
    } else if name == "geocent" || name == "cart" {
        let geocentric = p.geocentric()?;
        p.bound(geocentric, true)?
    } else {
        if p.has("geoc") {
            return Err(unsupported("+geoc (geocentric latitudes)"));
        }
        let (geodetic, datum) = p.geographic()?;
        let projected = p.projected(geodetic, &datum)?;
        p.bound(projected, false)?
    };
    if p.has("geoidgrids") {
        return Err(unsupported("+geoidgrids (a vertical CRS by grid)"));
    }
    // What PROJ would keep as the CRS's PROJ4 extension.
    if let Some(kv) = p
        .step
        .params
        .iter()
        .find(|kv| kv.key != "no_defs" && !kv.used.get())
    {
        return Err(unsupported(&format!(
            "+{} (PROJ keeps it only as an extension of the CRS)",
            kv.key
        )));
    }
    if is_geographic(&name) && tokens::number(p.get("lon_0")).unwrap_or(0.0) != 0.0 {
        return Err(unsupported("+lon_0 on a geographic CRS"));
    }
    // PROJ checks the string by building its operation; a projection this
    // crate builds is checked the same way.
    if let Some(message) = crate::crs::projection_error(&crs) {
        return Err(invalid(&message));
    }
    Ok(ids(crs))
}

/// PROJ's `JSONFormatter` rule for ids: an object's id is written only when
/// no object around it has one, except for a projected CRS's base CRS, an
/// operation's method and parameters, and a datum ensemble and its members,
/// whose ids are always written.
fn ids(mut crs: Value) -> Value {
    fn walk(v: &mut Value, key: &str, outer_has_id: bool, always: bool) {
        match v {
            Value::Object(o) => {
                let own = o.contains_key("id") || o.contains_key("ids");
                if outer_has_id && !always && key != "datum_ensemble" {
                    o.remove("id");
                    o.remove("ids");
                }
                let inner = outer_has_id || own;
                for (k, child) in o.iter_mut() {
                    let always =
                        matches!(k.as_str(), "base_crs" | "method" | "parameters" | "members");
                    walk(child, k, inner, always);
                }
            }
            Value::Array(a) => {
                for x in a {
                    walk(x, key, outer_has_id, always);
                }
            }
            _ => {}
        }
    }
    walk(&mut crs, "", false, false);
    crs
}

fn invalid(message: &str) -> Error {
    Error::ProjString {
        message: message.to_owned(),
    }
}

fn is_geographic(name: &str) -> bool {
    matches!(name, "longlat" | "lonlat" | "latlong" | "latlon")
}

fn is_projected(name: &str) -> bool {
    name == "etmerc"
        || name == "utm"
        || METHODS.iter().any(|m| m.proj == Some(name))
        || reads(name).is_some()
}

/// The keys PROJ's operation `name` looks up, if PROJ has one by that name.
fn reads(name: &str) -> Option<&'static [&'static str]> {
    PROJECTION_READS
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| PROJECTION_READS[i].1)
}

struct Parser<'a> {
    text: &'a str,
    title: String,
    globals: Vec<KeyValue>,
    step: Step,
    ignore_nadgrids: bool,
}

/// A unit: its PROJJSON and its size in metres or radians.
#[derive(Debug, Clone)]
struct Unit {
    json: Value,
    factor: f64,
}

impl Unit {
    fn metre() -> Self {
        Self {
            json: json!("metre"),
            factor: 1.0,
        }
    }

    fn degree() -> Self {
        Self {
            json: json!("degree"),
            factor: DEG_TO_RAD,
        }
    }

    /// The unit's PROJJSON without its id, as a parameter's (which has one
    /// itself) carries it.
    fn without_id(&self) -> Value {
        let mut v = self.json.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("id");
        }
        v
    }
}

/// PROJ's `linearUnitDescs`: `+units=` names, metres, EPSG names and codes.
const LINEAR_UNITS: &[(&str, &str, &str, u32)] = &[
    ("mm", "0.001", "millimetre", 1025),
    ("cm", "0.01", "centimetre", 1033),
    ("m", "1.0", "metre", 9001),
    ("meter", "1.0", "metre", 9001),
    ("metre", "1.0", "metre", 9001),
    ("ft", "0.3048", "foot", 9002),
    ("us-ft", "0.3048006096012192", "US survey foot", 9003),
    ("fath", "1.8288", "fathom", 9014),
    ("kmi", "1852", "nautical mile", 9030),
    ("us-ch", "20.11684023368047", "US survey chain", 9033),
    ("us-mi", "1609.347218694437", "US survey mile", 9035),
    ("km", "1000.0", "kilometre", 9036),
    ("ind-ft", "0.30479841", "Indian foot (1937)", 9081),
    ("ind-yd", "0.91439523", "Indian yard (1937)", 9085),
    ("mi", "1609.344", "Statute mile", 9093),
    ("yd", "0.9144", "yard", 9096),
    ("ch", "20.1168", "chain", 9097),
    ("link", "0.201168", "link", 9098),
    ("dm", "0.1", "decimetre", 0),
    ("in", "0.0254", "inch", 0),
    ("us-in", "0.025400050800101", "US survey inch", 0),
    ("us-yd", "0.914401828803658", "US survey yard", 0),
    ("ind-ch", "20.11669506", "Indian chain", 0),
];

fn linear_unit(desc: &(&str, &str, &str, u32)) -> Unit {
    let factor: f64 = desc.1.parse().expect("a number");
    if desc.3 == 9001 {
        return Unit::metre();
    }
    let mut o = Obj::new();
    o.insert("type".into(), json!("LinearUnit"));
    o.insert("name".into(), json!(desc.2));
    o.insert("conversion_factor".into(), num(factor));
    if desc.3 != 0 {
        o.insert("id".into(), json!({"authority": "EPSG", "code": desc.3}));
    }
    Unit {
        json: Value::Object(o),
        factor,
    }
}

/// An ellipsoid as PROJ builds it: two axes, an inverse flattening or a
/// radius, and the EPSG id of PROJ's own `Ellipsoid::WGS84` and friends.
#[derive(Debug, Clone)]
struct Ellipsoid {
    name: String,
    a: f64,
    shape: EllShape,
    id: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
enum EllShape {
    InverseFlattening(f64),
    SemiMinor(f64),
}

impl Ellipsoid {
    fn flattened(name: &str, a: f64, rf: f64, id: Option<u32>) -> Self {
        Self {
            name: name.to_owned(),
            a,
            shape: EllShape::InverseFlattening(rf),
            id,
        }
    }

    fn two_axis(name: &str, a: f64, b: f64) -> Self {
        Self {
            name: name.to_owned(),
            a,
            shape: EllShape::SemiMinor(b),
            id: None,
        }
    }

    fn wgs84() -> Self {
        Self::flattened("WGS 84", 6378137.0, 298.257223563, Some(7030))
    }

    fn grs1980() -> Self {
        Self::flattened("GRS 1980", 6378137.0, 298.257222101, Some(7019))
    }

    fn clarke_1866() -> Self {
        let mut e = Self::two_axis("Clarke 1866", 6378206.4, 6356583.8);
        e.id = Some(7008);
        e
    }

    fn is_sphere(&self) -> bool {
        match self.shape {
            EllShape::InverseFlattening(rf) => rf == 0.0,
            EllShape::SemiMinor(b) => b == self.a,
        }
    }

    fn b(&self) -> f64 {
        match self.shape {
            EllShape::InverseFlattening(0.0) => self.a,
            EllShape::InverseFlattening(rf) => self.a * (1.0 - 1.0 / rf),
            EllShape::SemiMinor(b) => b,
        }
    }

    fn rf(&self) -> f64 {
        match self.shape {
            EllShape::InverseFlattening(rf) => rf,
            EllShape::SemiMinor(b) if b == self.a => 0.0,
            EllShape::SemiMinor(b) => self.a / (self.a - b),
        }
    }

    fn squared_eccentricity(&self) -> f64 {
        let f = if self.rf() == 0.0 {
            0.0
        } else {
            1.0 / self.rf()
        };
        f * (2.0 - f)
    }

    /// `Ellipsoid::identify`: an unnamed ellipsoid gets the name of the
    /// PROJ ellipsoid with its axes (`lookForProjWellKnownEllps`).
    fn identify(mut self) -> Self {
        if self.name == "unknown"
            && let Some(e) = well_known(&self)
        {
            self.name = proj_name(e);
        }
        self
    }

    fn json(&self, with_id: bool) -> Value {
        let mut o = Obj::new();
        o.insert("name".into(), json!(self.name));
        if self.is_sphere() {
            o.insert("radius".into(), num(self.a));
        } else {
            o.insert("semi_major_axis".into(), num(self.a));
            match self.shape {
                EllShape::InverseFlattening(rf) => o.insert("inverse_flattening".into(), num(rf)),
                EllShape::SemiMinor(b) => o.insert("semi_minor_axis".into(), num(b)),
            };
        }
        if with_id && let Some(code) = self.id {
            o.insert("id".into(), json!({"authority": "EPSG", "code": code}));
        }
        Value::Object(o)
    }
}

/// PROJ's name for one of its ellipsoids, with "GRS 1980(IUGG, 1980)"
/// shortened as `lookForProjWellKnownEllps` does.
fn proj_name(e: &proj_tables::NamedEllipsoid) -> String {
    if e.description.starts_with("GRS 1980") {
        "GRS 1980".to_owned()
    } else {
        e.description.to_owned()
    }
}

fn well_known(e: &Ellipsoid) -> Option<&'static proj_tables::NamedEllipsoid> {
    well_known_ellipsoid(e.a, e.b(), e.rf())
}

/// `lookForProjWellKnownEllps`: the first of PROJ's ellipsoids with this
/// semi-major axis and, as PROJ defines it, semi-minor axis or inverse
/// flattening.
pub(super) fn well_known_ellipsoid(
    a: f64,
    b: f64,
    rf: f64,
) -> Option<&'static proj_tables::NamedEllipsoid> {
    proj_tables::ELLIPSOIDS.iter().find(|n| {
        (a - n.a).abs() < 1e-10 * n.a
            && match n.shape {
                Shape::SemiMinor(nb) => (b - nb).abs() < 1e-10 * nb,
                Shape::InverseFlattening(nrf) => (rf - nrf).abs() < 1e-10 * nrf,
            }
    })
}

/// A prime meridian: its name, longitude, and whether it is PROJ's
/// `PrimeMeridian::PARIS` (EPSG:8903, in grads).
#[derive(Debug, Clone)]
struct PrimeMeridian {
    name: String,
    degrees: f64,
    paris: bool,
}

impl PrimeMeridian {
    fn greenwich() -> Self {
        Self {
            name: "Greenwich".into(),
            degrees: 0.0,
            paris: false,
        }
    }

    fn is_greenwich(&self) -> bool {
        self.degrees == 0.0
    }

    fn json(&self, with_id: bool) -> Value {
        if self.paris {
            let mut v = json!({"name": "Paris", "longitude": {"value": 2.5969213,
                "unit": {"type": "AngularUnit", "name": "grad", "conversion_factor": 0.015707963267949}}});
            if with_id {
                v["id"] = json!({"authority": "EPSG", "code": 8903});
            }
            v
        } else {
            json!({"name": self.name, "longitude": num(self.degrees)})
        }
    }
}

/// A geodetic reference frame as PROJ builds it from the string.
#[derive(Debug, Clone)]
struct Datum {
    name: String,
    ellipsoid: Ellipsoid,
    pm: PrimeMeridian,
    id: Option<u32>,
}

impl Datum {
    fn epsg(name: &str, code: u32, ellipsoid: Ellipsoid) -> Self {
        Self {
            name: name.to_owned(),
            ellipsoid,
            pm: PrimeMeridian::greenwich(),
            id: Some(code),
        }
    }

    fn json(&self) -> Value {
        let ids = self.id.is_none();
        let mut o = Obj::new();
        o.insert("type".into(), json!("GeodeticReferenceFrame"));
        o.insert("name".into(), json!(self.name));
        o.insert("ellipsoid".into(), self.ellipsoid.json(ids));
        if !self.pm.is_greenwich() {
            o.insert("prime_meridian".into(), self.pm.json(ids));
        }
        if let Some(code) = self.id {
            o.insert("id".into(), json!({"authority": "EPSG", "code": code}));
        }
        Value::Object(o)
    }
}

/// PROJ's `datumDescs`: `+datum=` names that aren't PROJ's static datums,
/// as (name, datum, datum code, ellipsoid, ellipsoid code, a, rf).
const DATUMS: &[(&str, &str, u32, &str, u32, f64, f64)] = &[
    (
        "GGRS87",
        "Greek Geodetic Reference System 1987",
        6121,
        "GRS 1980",
        7019,
        6378137.0,
        298.257222101,
    ),
    (
        "potsdam",
        "Deutsches Hauptdreiecksnetz",
        6314,
        "Bessel 1841",
        7004,
        6377397.155,
        299.1528128,
    ),
    (
        "carthage",
        "Carthage",
        6223,
        "Clarke 1880 (IGN)",
        7011,
        6378249.2,
        293.4660213,
    ),
    (
        "hermannskogel",
        "Militar-Geographische Institut",
        6312,
        "Bessel 1841",
        7004,
        6377397.155,
        299.1528128,
    ),
    (
        "ire65",
        "TM65",
        6299,
        "Airy Modified 1849",
        7002,
        6377340.189,
        299.3249646,
    ),
    (
        "nzgd49",
        "New Zealand Geodetic Datum 1949",
        6272,
        "International 1924",
        7022,
        6378388.0,
        297.0,
    ),
    (
        "OSGB36",
        "OSGB 1936",
        6277,
        "Airy 1830",
        7001,
        6377563.396,
        299.3249646,
    ),
];

/// The conversion code of each EPSG projected CRS.
fn conversion_codes() -> &'static HashMap<u32, u32> {
    static CODES: OnceLock<HashMap<u32, u32>> = OnceLock::new();
    CODES.get_or_init(|| {
        let v: Value = serde_json::from_str(xeibe_crs::operations_json()).expect("operations JSON");
        v["projections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p[0].as_u64()? as u32, p[1].as_u64()? as u32)))
            .collect()
    })
}

impl Parser<'_> {
    // --- looking up parameters, marking what is used -----------------------

    fn find(&self, key: &str) -> Option<&KeyValue> {
        let kv = self
            .globals
            .iter()
            .chain(&self.step.params)
            .find(|kv| kv.key.eq_ignore_ascii_case(key))?;
        kv.used.set(true);
        Some(kv)
    }

    fn has(&self, key: &str) -> bool {
        self.find(key).is_some()
    }

    fn get(&self, key: &str) -> &str {
        self.find(key).map_or("", |kv| kv.value.as_str())
    }

    /// `+k` or `+k_0`, in the step only.
    fn get_k(&self) -> &str {
        match self
            .step
            .params
            .iter()
            .find(|kv| kv.key.eq_ignore_ascii_case("k") || kv.key.eq_ignore_ascii_case("k_0"))
        {
            Some(kv) => {
                kv.used.set(true);
                &kv.value
            }
            None => "",
        }
    }

    /// PROJ's `getAngularValue`: degrees, through radians.
    fn angle_of(value: &str) -> Option<f64> {
        tokens::dmstor(value).map(|r| r * RAD_TO_DEG)
    }

    fn angle(&self, key: &str) -> f64 {
        Self::angle_of(self.get(key)).unwrap_or(0.0)
    }

    /// Drop what PROJ's operation doesn't look up, and repeated keys, as
    /// PROJ does once the operation is built.
    fn drop_unread(&mut self) {
        let name = self.step.name.clone();
        let specific: &[&str] = reads(&name).unwrap_or(&[]);
        let geoidgrids = self.step.params.iter().any(|kv| kv.key == "geoidgrids");
        let mut seen: Vec<String> = Vec::new();
        self.step.params.retain(|kv| {
            if seen.contains(&kv.key) {
                return false;
            }
            seen.push(kv.key.clone());
            GENERIC_READS.contains(&kv.key.as_str())
                || specific.contains(&kv.key.as_str())
                || ((name == "krovak" || name == "mod_krovak") && kv.key == "alpha")
                || (kv.key == "geoid_crs" && geoidgrids)
        });
    }

    // --- units, meridians, datums -------------------------------------------

    fn unit(&self, units: &str, to_meter: &str) -> Result<Unit, Error> {
        let mut found = None;
        let name = self.get(units);
        if !name.is_empty() {
            found = Some(
                LINEAR_UNITS
                    .iter()
                    .find(|u| u.0 == name)
                    .ok_or_else(|| invalid(&format!("unhandled {units}={name}")))?,
            );
        }
        let factor = if to_meter.is_empty() {
            ""
        } else {
            self.get(to_meter)
        };
        if !factor.is_empty() {
            let f = tokens::number(factor)
                .ok_or_else(|| invalid(&format!("invalid value for {to_meter}")))?;
            match LINEAR_UNITS
                .iter()
                .find(|u| (u.1.parse::<f64>().expect("a number") - f).abs() < 1e-10 * f)
            {
                Some(u) => found = Some(u),
                None => {
                    if f == 0.0 {
                        return Err(invalid("invalid unit value"));
                    }
                    return Ok(Unit {
                        json: json!({"type": "LinearUnit", "name": "unknown", "conversion_factor": num(f)}),
                        factor: f,
                    });
                }
            }
        }
        Ok(found.map_or_else(Unit::metre, linear_unit))
    }

    fn prime_meridian(&self) -> Result<PrimeMeridian, Error> {
        let pm = self.get("pm");
        if pm.is_empty() {
            return Ok(PrimeMeridian::greenwich());
        }
        if let Some(degrees) = Self::angle_of(pm) {
            return Ok(PrimeMeridian {
                name: "unknown".into(),
                degrees,
                paris: false,
            });
        }
        if pm == "paris" {
            return Ok(PrimeMeridian {
                name: "Paris".into(),
                degrees: 2.33722917,
                paris: true,
            });
        }
        let (_, defn) = proj_tables::PRIME_MERIDIANS
            .iter()
            .find(|(id, _)| *id == pm)
            .ok_or_else(|| invalid(&format!("unknown pm {pm}")))?;
        let mut name = pm.to_owned();
        name[..1].make_ascii_uppercase();
        Ok(PrimeMeridian {
            name,
            degrees: Self::angle_of(defn).expect("PROJ's meridians are angles"),
            paris: false,
        })
    }

    /// `guessBodyName`: only the Earth's ellipsoids are read.
    fn earth(&self, a: f64) -> Result<(), Error> {
        if (a - 6375000.0).abs() < 0.2 * 6375000.0 {
            Ok(())
        } else {
            Err(Error::Unsupported {
                crs: self.text.to_owned(),
                what: format!("an ellipsoid of another celestial body (a = {a} m)"),
            })
        }
    }

    /// `buildDatum`.
    fn datum(&self, title: &str) -> Result<Datum, Error> {
        let mut ellps = self.get("ellps").to_owned();
        let datum_name = self.get("datum").to_owned();
        let [r, a_s, b_s, rf_s, f_s, es_s, e_s] =
            ["R", "a", "b", "rf", "f", "es", "e"].map(|k| self.get(k).to_owned());
        let numeric = [&r, &a_s, &b_s, &rf_s, &f_s, &es_s, &e_s]
            .iter()
            .any(|s| !s.is_empty());
        if !numeric
            && ellps.is_empty()
            && datum_name.is_empty()
            && (self.step.name == "krovak" || self.step.name == "mod_krovak")
        {
            ellps = "bessel".into();
        }
        let pm = self.prime_meridian()?;
        let nadgrids = self.get("nadgrids");
        let towgs84 = self.get("towgs84");
        let suffix = if !nadgrids.is_empty() {
            format!(" using nadgrids={nadgrids}")
        } else if !towgs84.is_empty() {
            format!(" using towgs84={towgs84}")
        } else {
            String::new()
        };
        let override_pm = |d: Datum| -> Datum {
            if pm.is_greenwich() {
                d
            } else {
                Datum {
                    name: format!("Unknown based on {} ellipsoid{suffix}", d.ellipsoid.name),
                    ellipsoid: d.ellipsoid,
                    pm: pm.clone(),
                    id: None,
                }
            }
        };
        let unknown = |title: &str, ellipsoid: Ellipsoid, pm: PrimeMeridian| -> Datum {
            let name = if !title.is_empty() {
                title.to_owned()
            } else if ellipsoid.name != "unknown" {
                format!("Unknown based on {} ellipsoid{suffix}", ellipsoid.name)
            } else {
                format!("unknown{suffix}")
            };
            Datum {
                name,
                ellipsoid,
                pm,
                id: None,
            }
        };
        if !r.is_empty() {
            let radius = tokens::number(&r).ok_or_else(|| invalid("Invalid R value"))?;
            self.earth(radius)?;
            let name = if title.is_empty() {
                format!("unknown{suffix}")
            } else {
                title.to_owned()
            };
            return Ok(Datum {
                name,
                ellipsoid: Ellipsoid::flattened("unknown", radius, 0.0, None),
                pm,
                id: None,
            });
        }
        let (mut a, mut b, mut rf) = (-1.0, -1.0, -1.0);
        if !datum_name.is_empty() {
            let datum = match datum_name.as_str() {
                "WGS84" => override_pm(Datum::epsg(
                    "World Geodetic System 1984",
                    6326,
                    Ellipsoid::wgs84(),
                )),
                "NAD83" => override_pm(Datum::epsg(
                    "North American Datum 1983",
                    6269,
                    Ellipsoid::grs1980(),
                )),
                "NAD27" => override_pm(Datum::epsg(
                    "North American Datum 1927",
                    6267,
                    Ellipsoid::clarke_1866(),
                )),
                other => {
                    let d = DATUMS
                        .iter()
                        .find(|d| d.0 == other)
                        .ok_or_else(|| invalid(&format!("unknown datum {other}")))?;
                    Datum {
                        name: d.1.to_owned(),
                        ellipsoid: Ellipsoid::flattened(d.3, d.5, d.6, Some(d.4)),
                        pm: pm.clone(),
                        id: Some(d.2),
                    }
                }
            };
            if !numeric {
                return Ok(datum);
            }
            a = datum.ellipsoid.a;
            rf = datum.ellipsoid.rf();
        } else if !ellps.is_empty() {
            let ellipsoid = match ellps.as_str() {
                "WGS84" => Ellipsoid::wgs84(),
                "GRS80" => Ellipsoid::grs1980(),
                other => {
                    let e = proj_tables::ellipsoid(other)
                        .ok_or_else(|| invalid(&format!("unknown ellipsoid {other}")))?;
                    match e.shape {
                        Shape::SemiMinor(eb) => Ellipsoid::two_axis(e.description, e.a, eb),
                        Shape::InverseFlattening(erf) => {
                            Ellipsoid::flattened(e.description, e.a, erf, None)
                        }
                    }
                }
            };
            let datum = Datum {
                name: if title.is_empty() {
                    format!("Unknown based on {} ellipsoid{suffix}", ellipsoid.name)
                } else {
                    title.to_owned()
                },
                ellipsoid,
                pm: pm.clone(),
                id: None,
            };
            if !numeric {
                return Ok(datum);
            }
            a = datum.ellipsoid.a;
            match datum.ellipsoid.shape {
                EllShape::SemiMinor(eb) => b = eb,
                EllShape::InverseFlattening(erf) => rf = erf,
            }
        }
        if !a_s.is_empty() {
            a = tokens::number(&a_s).ok_or_else(|| invalid("Invalid a value"))?;
        }
        if a > 0.0 && (b > 0.0 || !b_s.is_empty()) {
            if !b_s.is_empty() {
                b = tokens::number(&b_s).ok_or_else(|| invalid("Invalid b value"))?;
            }
            self.earth(a)?;
            return Ok(unknown(
                title,
                Ellipsoid::two_axis("unknown", a, b).identify(),
                pm,
            ));
        }
        let flattened = |f: f64| {
            let rf = if f != 0.0 { 1.0 / f } else { 0.0 };
            Ellipsoid::flattened("unknown", a, rf, None).identify()
        };
        if a > 0.0 && (rf >= 0.0 || !rf_s.is_empty()) {
            if !rf_s.is_empty() {
                rf = tokens::number(&rf_s).ok_or_else(|| invalid("Invalid rf value"))?;
            }
            self.earth(a)?;
            return Ok(unknown(
                title,
                Ellipsoid::flattened("unknown", a, rf, None).identify(),
                pm,
            ));
        }
        if a > 0.0 && !f_s.is_empty() {
            let f = tokens::number(&f_s).ok_or_else(|| invalid("Invalid f value"))?;
            self.earth(a)?;
            return Ok(unknown(title, flattened(f), pm));
        }
        if a > 0.0 && !e_s.is_empty() {
            let e = tokens::number(&e_s).ok_or_else(|| invalid("Invalid e value"))?;
            self.earth(a)?;
            return Ok(unknown(title, flattened(1.0 - e.asin().cos()), pm));
        }
        if a > 0.0 && !es_s.is_empty() {
            let es = tokens::number(&es_s).ok_or_else(|| invalid("Invalid es value"))?;
            self.earth(a)?;
            return Ok(unknown(title, flattened(1.0 - (1.0 - es).sqrt()), pm));
        }
        if a > 0.0 {
            self.earth(a)?;
            return Ok(unknown(
                title,
                Ellipsoid::flattened("unknown", a, 0.0, None),
                pm,
            ));
        }
        for (s, k) in [
            (&b_s, "b"),
            (&rf_s, "rf"),
            (&f_s, "f"),
            (&e_s, "e"),
            (&es_s, "es"),
        ] {
            if !s.is_empty() && a_s.is_empty() {
                return Err(invalid(&format!("{k} found, but a missing")));
            }
        }
        Ok(override_pm(Datum::epsg(
            "World Geodetic System 1984",
            6326,
            Ellipsoid::wgs84(),
        )))
    }

    // --- coordinate systems -------------------------------------------------

    /// `processAxisSwap` for a single step: the two horizontal axes, with
    /// `+axis` (unless `ignore_axis`, as for a projected CRS's base),
    /// Krovak's `+czech`, or a polar projection's meridians.
    fn axes(
        &self,
        unit: &Unit,
        geographic: bool,
        pole: Pole,
        ignore_axis: bool,
    ) -> Result<[Value; 2], Error> {
        let axis = |name: &str, abbr: &str, dir: &str, meridian: Option<f64>| {
            let mut v =
                json!({"name": name, "abbreviation": abbr, "direction": dir, "unit": unit.json});
            if let Some(m) = meridian {
                v["meridian"] = json!({"longitude": num(m)});
            }
            v
        };
        let (east, north) = if geographic {
            (
                axis("Longitude", "lon", "east", None),
                axis("Latitude", "lat", "north", None),
            )
        } else {
            match pole {
                Pole::None => (
                    axis("Easting", "E", "east", None),
                    axis("Northing", "N", "north", None),
                ),
                Pole::North => (
                    axis("Easting", "E", "south", Some(90.0)),
                    axis("Northing", "N", "south", Some(180.0)),
                ),
                Pole::South => (
                    axis("Easting", "E", "north", Some(90.0)),
                    axis("Northing", "N", "north", Some(0.0)),
                ),
            }
        };
        let (west, south) = if geographic {
            (
                axis("Longitude", "lon", "west", None),
                axis("Latitude", "lat", "south", None),
            )
        } else {
            (
                axis("Westing", "", "west", None),
                axis("Southing", "", "south", None),
            )
        };
        let spec = self.get("axis");
        if !ignore_axis && !spec.is_empty() {
            let b = spec.as_bytes();
            if b.len() != 3 {
                return Err(invalid(&format!("Unhandled axis={spec}")));
            }
            let pick = |c: u8| match c {
                b'n' => Ok(north.clone()),
                b's' => Ok(south.clone()),
                b'e' => Ok(east.clone()),
                b'w' => Ok(west.clone()),
                _ => Err(invalid(&format!("Unhandled axis={spec}"))),
            };
            return Ok([pick(b[0])?, pick(b[1])?]);
        }
        if (self.step.name == "krovak" || self.step.name == "mod_krovak") && self.has("czech") {
            return Ok([west, south]);
        }
        Ok([east, north])
    }

    // --- CRSs ---------------------------------------------------------------

    /// `buildGeodeticCRS` (a geographic CRS) for the step, or for a
    /// projected step its base.
    fn geographic(&self) -> Result<(Value, Datum), Error> {
        let geographic_step = is_geographic(&self.step.name);
        let title = if geographic_step {
            self.title.clone()
        } else {
            String::new()
        };
        let _ = self.has("units");
        let datum = self.datum(&title)?;
        let mut axes = self
            .axes(&Unit::degree(), true, Pole::None, !geographic_step)?
            .to_vec();
        if !self.has("geoidgrids") && (self.has("vunits") || self.has("vto_meter")) {
            let up = self.unit("vunits", "vto_meter")?;
            axes.push(
                json!({"name": "Ellipsoidal height", "abbreviation": "h", "direction": "up",
                "unit": up.json}),
            );
        }
        let mut o = Obj::new();
        o.insert("type".into(), json!("GeographicCRS"));
        o.insert(
            "name".into(),
            json!(if title.is_empty() { "unknown" } else { &title }),
        );
        o.insert("datum".into(), datum.json());
        o.insert(
            "coordinate_system".into(),
            json!({"subtype": "ellipsoidal", "axis": axes}),
        );
        Ok((Value::Object(o), datum))
    }

    /// `buildGeocentricCRS`.
    fn geocentric(&self) -> Result<Value, Error> {
        let datum = self.datum(&self.title)?;
        let unit = self.unit("units", "")?;
        let axis = |name: &str, abbr: &str, dir: &str| json!({"name": name, "abbreviation": abbr, "direction": dir, "unit": unit.json});
        let name = if self.title.is_empty() {
            "unknown"
        } else {
            &self.title
        };
        let v = json!({
            "type": "GeodeticCRS",
            "name": name,
            "datum": datum.json(),
            "coordinate_system": {"subtype": "Cartesian", "axis": [
                axis("Geocentric X", "X", "geocentricX"),
                axis("Geocentric Y", "Y", "geocentricY"),
                axis("Geocentric Z", "Z", "geocentricZ"),
            ]},
        });
        Ok(v)
    }

    /// `buildBoundOrCompoundCRSIfNeeded`: `+nadgrids` (before `+towgs84`)
    /// makes the CRS a Bound CRS to WGS 84.
    fn bound(&self, crs: Value, geocentric: bool) -> Result<Value, Error> {
        let nadgrids = self.get("nadgrids");
        let towgs84 = self.get("towgs84");
        if !self.ignore_nadgrids && !nadgrids.is_empty() {
            return Ok(json!({
                "type": "BoundCRS",
                "source_crs": crs,
                "target_crs": wgs84_4326(),
                "transformation": {
                    "name": "unknown to WGS84",
                    "method": {"name": "NTv2", "id": {"authority": "EPSG", "code": 9615}},
                    "parameters": [{"name": "Latitude and longitude difference file",
                        "value": nadgrids, "id": {"authority": "EPSG", "code": 8656}}],
                },
            }));
        }
        if towgs84.is_empty() {
            return Ok(crs);
        }
        let mut values = Vec::new();
        for s in towgs84.split(',') {
            values.push(
                tokens::number(s)
                    .ok_or_else(|| invalid("Non numerical value in towgs84 clause"))?,
            );
        }
        match values.len() {
            3 => Ok(towgs84_bound(crs, &values, geocentric)),
            // PROJ doesn't autocorrect a PROJ string's rotations, as it
            // does WKT1's: it reads strings without its database.
            7 => Ok(towgs84_bound(crs, &values, geocentric)),
            n => Err(invalid(&format!("invalid number of +towgs84 values: {n}"))),
        }
    }

    /// `+init=epsg:CODE`: the EPSG CRS, a geographic one with longitude
    /// first in degrees, a projected one with easting first.
    fn init(&self) -> Result<Value, Error> {
        let name = &self.step.name;
        let unsupported = |what: String| Error::Unsupported {
            crs: self.text.to_owned(),
            what,
        };
        let Some(code) = name
            .get(..5)
            .filter(|p| p.eq_ignore_ascii_case("epsg:"))
            .map(|_| &name[5..])
        else {
            return Err(unsupported(format!(
                "+init={name} (only +init=epsg: is read)"
            )));
        };
        let code: u32 = code
            .parse()
            .map_err(|_| invalid(&format!("invalid EPSG code {code}")))?;
        let text = xeibe_crs::projjson(code).ok_or(Error::UnknownCode(code))?;
        let mut significant = false;
        for kv in &self.step.params {
            if kv.key == "over" {
                return Err(unsupported("+over".into()));
            }
            significant |= !((kv.key == "type" && kv.value == "crs")
                || kv.key == "wktext"
                || kv.key == "no_defs");
        }
        if significant {
            // PROJ reads the EPSG CRS's own PROJ string, after the
            // parameters given with it.
            let mut words = Vec::new();
            if !self.title.is_empty() {
                words.push(format!(
                    "title={}",
                    tokens::double_quote_if_needed(&self.title)
                ));
            }
            for kv in &self.step.params {
                words.push(if kv.value.is_empty() {
                    format!("+{}", kv.key)
                } else {
                    format!("+{}={}", kv.key, tokens::double_quote_if_needed(&kv.value))
                });
            }
            words.push(super::write::crs(&serde_json::from_str(text).map_err(
                |e| Error::Projjson {
                    crs: format!("EPSG:{code}"),
                    message: e.to_string(),
                },
            )?)?);
            return crs(&words.join(" "));
        }
        let epsg: Value = serde_json::from_str(text).map_err(|e| Error::Projjson {
            crs: format!("EPSG:{code}"),
            message: e.to_string(),
        })?;
        let name = if self.title.is_empty() {
            epsg["name"].clone()
        } else {
            json!(self.title)
        };
        let usage = epsg["usages"].get(0).unwrap_or(&epsg);
        let extent = |o: &mut Obj| {
            for key in ["area", "bbox"] {
                if let Some(v) = usage.get(key) {
                    o.insert(key.into(), v.clone());
                }
            }
        };
        let lon_lat = |unit: Option<&Value>| {
            let mut axes = vec![
                json!({"name": "Longitude", "abbreviation": "lon", "direction": "east", "unit": "degree"}),
                json!({"name": "Latitude", "abbreviation": "lat", "direction": "north", "unit": "degree"}),
            ];
            if let Some(u) = unit {
                axes.push(json!({"name": "Ellipsoidal height", "abbreviation": "h", "direction": "up", "unit": u}));
            }
            json!({"subtype": "ellipsoidal", "axis": axes})
        };
        match epsg["type"].as_str() {
            Some("GeographicCRS") => {
                let axes = epsg["coordinate_system"]["axis"]
                    .as_array()
                    .map_or(0, Vec::len);
                let mut o = Obj::new();
                o.insert("type".into(), json!("GeographicCRS"));
                o.insert("name".into(), name);
                for key in ["datum", "datum_ensemble", "dynamic"] {
                    if let Some(v) = epsg.get(key) {
                        o.insert(key.into(), v.clone());
                    }
                }
                let height = (axes == 3).then(|| &epsg["coordinate_system"]["axis"][2]["unit"]);
                o.insert("coordinate_system".into(), lon_lat(height));
                extent(&mut o);
                Ok(Value::Object(o))
            }
            Some("ProjectedCRS") if epsg["conversion"]["method"]["id"]["code"] != 9808 => {
                let mut conversion = epsg["conversion"].clone();
                if let Some(c) = conversion_codes().get(&code) {
                    conversion["id"] = json!({"authority": "EPSG", "code": c});
                }
                let unit = epsg["coordinate_system"]["axis"][0]["unit"].clone();
                let mut base = epsg["base_crs"].clone();
                if let Some(remarks) = base["id"]["code"]
                    .as_u64()
                    .and_then(|c| xeibe_crs::crs_remarks(c as u32))
                {
                    base["remarks"] = json!(remarks);
                }
                let mut o = Obj::new();
                o.insert("type".into(), json!("ProjectedCRS"));
                o.insert("name".into(), name);
                o.insert("base_crs".into(), base);
                o.insert("conversion".into(), conversion);
                o.insert("coordinate_system".into(), json!({"subtype": "Cartesian", "axis": [
                    {"name": "Easting", "abbreviation": "E", "direction": "east", "unit": unit},
                    {"name": "Northing", "abbreviation": "N", "direction": "north", "unit": unit},
                ]}));
                extent(&mut o);
                Ok(Value::Object(o))
            }
            _ => Ok(epsg),
        }
        .map(ids)
    }

    /// A key's value, without marking it used.
    fn peek(&self, key: &str) -> &str {
        self.globals
            .iter()
            .chain(&self.step.params)
            .find(|kv| kv.key.eq_ignore_ascii_case(key))
            .map_or("", |kv| kv.value.as_str())
    }

    /// PROJ's `omerc` set-up without `+alpha` or `+gamma`: the line through
    /// two points, whose latitudes it checks.
    fn omerc_two_points(&self) -> Result<(), Error> {
        let present = |k: &str| {
            self.globals
                .iter()
                .chain(&self.step.params)
                .any(|kv| kv.key == k)
        };
        if present("alpha") || present("gamma") {
            return Ok(());
        }
        const TOL: f64 = 1e-7;
        let radians = |k: &str| tokens::dmstor(self.peek(k)).unwrap_or(0.0);
        let (phi1, phi2, phi0) = (radians("lat_1"), radians("lat_2"), radians("lat_0"));
        let half_pi = std::f64::consts::FRAC_PI_2;
        let fail = |m: &str| Err(invalid(&format!("omerc: {m}")));
        if phi1.abs() > half_pi - TOL {
            return fail("Invalid value for lat_1: |lat_1| should be < 90°");
        }
        if phi2.abs() > half_pi - TOL {
            return fail("Invalid value for lat_2: |lat_2| should be < 90°");
        }
        if (phi1 - phi2).abs() <= TOL {
            return fail("Invalid value for lat_1/lat_2: lat_1 should be different from lat_2");
        }
        if phi1.abs() <= TOL {
            return fail("Invalid value for lat_1: lat_1 should be different from 0");
        }
        if (phi0.abs() - half_pi).abs() <= TOL {
            return fail("Invalid value for lat_0: |lat_0| should be < 90°");
        }
        Ok(())
    }

    /// `buildProjectedCRS`.
    fn projected(&mut self, geodetic: Value, datum: &Datum) -> Result<Value, Error> {
        let name = self.step.name.clone();
        let sphere = datum.ellipsoid.is_sphere();
        let candidates: Vec<&'static Method> = METHODS
            .iter()
            .filter(|m| m.proj == Some(name.as_str()))
            .collect();
        let mut mapping = candidates.first().copied();
        let mut strict = false;
        if candidates.len() >= 2 {
            let mut all_aux = true;
            for m in &candidates {
                match m.aux {
                    None => all_aux = false,
                    Some(aux) if !aux.contains('=') => {
                        if self.has(aux) {
                            strict = true;
                            mapping = Some(m);
                            break;
                        }
                    }
                    Some(aux) => {
                        let (k, v) = aux.split_once('=').expect("key=value");
                        if self.get(k) == v {
                            strict = true;
                            mapping = Some(m);
                            break;
                        }
                    }
                }
            }
            if all_aux && !strict {
                mapping = None;
            }
        }
        if let Some(m) = mapping
            && !strict
        {
            mapping = Some(spherical_or_ellipsoidal(m, sphere));
        }
        if (self.prime_meridian()?.degrees - datum.pm.degrees).abs()
            > 1e-10 * datum.pm.degrees.abs().max(1.0)
        {
            return Err(invalid(
                "inconsistent pm values between projectedCRS and its base geographicalCRS",
            ));
        }
        let mut pole = Pole::None;
        let mut web_mercator = false;
        let mut web_mercator_name = "WGS 84 / Pseudo-Mercator".to_owned();
        let by_code = |code: u32| METHODS.iter().find(|m| m.epsg == code);
        let by_name = |n: &str| METHODS.iter().find(|m| m.name == n);
        match name.as_str() {
            "tmerc" if self.get("axis") == "wsu" => mapping = by_code(9808),
            "etmerc" => mapping = by_code(9807),
            "lcc" => {
                let (lat_0, lat_1, lat_2) = (
                    self.get("lat_0").to_owned(),
                    self.get("lat_1").to_owned(),
                    self.get("lat_2").to_owned(),
                );
                let k = self.get_k().to_owned();
                mapping = if lat_2.is_empty() && !lat_0.is_empty() && !lat_1.is_empty() {
                    if lat_0 == lat_1 || Self::angle_of(&lat_0) == Self::angle_of(&lat_1) {
                        by_code(9801)
                    } else {
                        by_code(1102)
                    }
                } else if !k.is_empty() && tokens::number(&k).unwrap_or(0.0) != 1.0 {
                    by_code(1051)
                } else {
                    by_code(9802)
                };
            }
            "aeqd" if self.has("guam") => mapping = by_code(9831),
            "geos" if self.get("sweep") == "x" => {
                mapping = by_name("Geostationary Satellite (Sweep X)")
            }
            "geos" => mapping = by_name("Geostationary Satellite (Sweep Y)"),
            "omerc" => {
                self.omerc_two_points()?;
                mapping = if self.has("no_rot") {
                    None
                } else if self.has("no_uoff") || self.has("no_off") {
                    by_code(9812)
                } else if self.has("lat_1")
                    && self.has("lon_1")
                    && self.has("lat_2")
                    && self.has("lon_2")
                {
                    by_name("Hotine Oblique Mercator Two Point Natural Origin")
                } else {
                    by_code(9815)
                };
            }
            "somerc" => {
                mapping = by_code(9815);
                if !self.has("alpha") && !self.has("gamma") && !self.has("lonc") {
                    let lon_0 = self.get("lon_0").to_owned();
                    for (k, v) in [("alpha", "90"), ("gamma", "90"), ("lonc", lon_0.as_str())] {
                        self.step.params.push(KeyValue {
                            key: k.into(),
                            value: v.into(),
                            used: Default::default(),
                        });
                    }
                }
            }
            "krovak" | "mod_krovak" => {
                let code = if name == "krovak" { 9819 } else { 1042 };
                let czech = self.has("czech");
                if (self.get("axis") == "swu" && !czech) || (czech && !self.has("axis")) {
                    mapping = by_code(code);
                }
            }
            "merc" => {
                if self.has("a")
                    && self.has("b")
                    && self.get("a") == self.get("b")
                    && (!self.has("lat_ts") || self.angle("lat_ts") == 0.0)
                    && tokens::number(self.get_k()).unwrap_or(0.0) == 1.0
                    && self.get("nadgrids") == "@null"
                {
                    mapping = by_code(1024);
                    self.ignore_nadgrids = true;
                    if tokens::number(self.get("a")).unwrap_or(0.0) == 6378137.0
                        && self.angle("lon_0") == 0.0
                        && self.angle("lat_0") == 0.0
                        && self.angle("x_0") == 0.0
                        && self.angle("y_0") == 0.0
                    {
                        web_mercator = true;
                        if self.has("units") && self.get("units") != "m" {
                            web_mercator_name += &format!(" (unit {})", self.get("units"));
                        }
                    }
                } else if self.has("lat_ts") {
                    if self.has("R_C") && !sphere && self.angle("lat_ts") != 0.0 {
                        return Err(invalid(
                            "lat_ts != 0 not supported for spherical Mercator on an ellipsoid",
                        ));
                    }
                    mapping = by_code(9805);
                } else if self.has("R_C") {
                    let k = self.get_k().to_owned();
                    if !k.is_empty() && tokens::number(&k).unwrap_or(0.0) != 1.0 {
                        if sphere {
                            mapping = by_code(9804);
                        } else {
                            return Err(invalid(
                                "k_0 != 1 not supported for spherical Mercator on an ellipsoid",
                            ));
                        }
                    } else {
                        mapping = by_code(1026);
                    }
                } else {
                    mapping = by_code(9804);
                }
            }
            "stere" => {
                if self.has("lat_0") && (self.angle("lat_0").abs() - 90.0).abs() < 1e-10 {
                    let lat_0 = self.angle("lat_0");
                    pole = if lat_0 > 0.0 {
                        Pole::North
                    } else {
                        Pole::South
                    };
                    let lat_ts = self.get("lat_ts").to_owned();
                    let k = self.get_k().to_owned();
                    let k_is_1 =
                        k.is_empty() || (tokens::number(&k).unwrap_or(0.0) - 1.0).abs() < 1e-10;
                    if !lat_ts.is_empty()
                        && (Self::angle_of(&lat_ts).unwrap_or(0.0) - lat_0).abs() > 1e-10
                        && !k.is_empty()
                        && (tokens::number(&k).unwrap_or(0.0) - 1.0).abs() > 1e-10
                    {
                        return Err(invalid(
                            "lat_ts != lat_0 and k != 1 not supported for Polar Stereographic",
                        ));
                    }
                    mapping = if !lat_ts.is_empty() && k_is_1 {
                        by_code(9829)
                    } else {
                        by_code(9810)
                    };
                } else {
                    mapping = by_name("Stereographic");
                }
            }
            "laea" => {
                if self.has("lat_0") && (self.angle("lat_0").abs() - 90.0).abs() < 1e-10 {
                    pole = if self.angle("lat_0") > 0.0 {
                        Pole::North
                    } else {
                        Pole::South
                    };
                }
            }
            "ortho" => {
                let k = self.get_k().to_owned();
                if (!k.is_empty() && tokens::number(&k).unwrap_or(0.0) != 1.0)
                    || (self.has("alpha")
                        && tokens::number(self.get("alpha")).unwrap_or(0.0) != 0.0)
                {
                    mapping = by_code(1130);
                }
            }
            _ => {}
        }
        let unit = self.unit("units", "to_meter")?;
        let unsupported = |what: String| Error::Unsupported {
            crs: self.text.to_owned(),
            what,
        };
        let conversion = if name == "utm" {
            let zone: i64 = atoi(self.get("zone"));
            let north = !self.has("south");
            if !(1..=60).contains(&zone) {
                return Err(invalid(&format!("invalid UTM zone {zone}")));
            }
            let _ = self.has("approx");
            utm(zone as u32, north)
        } else {
            let Some(m) = mapping else {
                return Err(unsupported(format!(
                    "+proj={name} (a projection PROJ has no CRS method for: it would be a PROJ-based operation)"
                )));
            };
            if m.epsg == 0 {
                return Err(unsupported(format!(
                    "+proj={name} ({}, a projection only PROJ defines)",
                    m.name
                )));
            }
            let mut parameters = Vec::new();
            for param in m.params {
                let key = param.proj.unwrap_or("");
                let given = if key == "k" || key == "k_0" {
                    self.get_k().to_owned()
                } else if !key.is_empty() {
                    self.get(key).to_owned()
                } else {
                    String::new()
                };
                let mut value = 0.0;
                if !given.is_empty() {
                    let v = if param.unit == UnitType::Angular {
                        Self::angle_of(&given)
                    } else {
                        tokens::number(&given)
                    };
                    value = v.ok_or_else(|| invalid(&format!("invalid value for {key}")))?;
                } else if name == "omerc" && key == "gamma" {
                    let alpha = self.get("alpha");
                    if !alpha.is_empty() {
                        value = Self::angle_of(alpha).unwrap_or(0.0);
                    }
                } else if name == "krovak" || name == "mod_krovak" {
                    // The defaults of PROJ's `krovak.cpp`, as `io.cpp` writes them.
                    #[allow(clippy::excessive_precision)]
                    let default = match param.epsg {
                        8811 => 49.5,
                        8833 => 24.833333333333333333,
                        1036 => 30.28813975277777776,
                        8818 => 78.5,
                        8819 => 0.9999,
                        _ => 0.0,
                    };
                    value = default;
                } else if name == "cea" && key == "lat_ts" {
                    let k = self.get_k().to_owned();
                    if !k.is_empty() {
                        let k =
                            tokens::number(&k).ok_or_else(|| invalid("invalid value for k/k_0"))?;
                        if !(0.0..=1.0).contains(&k) {
                            return Err(invalid("k/k_0 should be in [0,1]"));
                        }
                        let es = datum.ellipsoid.squared_eccentricity();
                        if es < 0.0 || es == 1.0 {
                            return Err(invalid("Invalid flattening"));
                        }
                        value = (k * ((1.0 - es) / (1.0 - k * k * es)).sqrt()).acos() * RAD_TO_DEG;
                    }
                } else if param.unit == UnitType::Scale {
                    value = 1.0;
                }
                let mut rounded = if param.unit == UnitType::Linear {
                    value / unit.factor
                } else {
                    value
                };
                if (rounded - rounded.round()).abs() < 1e-8 {
                    rounded = rounded.round();
                }
                let unit_json = match param.unit {
                    UnitType::Angular => json!("degree"),
                    UnitType::Linear if param.epsg != 0 => unit.without_id(),
                    UnitType::Linear => unit.json.clone(),
                    UnitType::Scale => json!("unity"),
                    UnitType::None => Value::Null,
                };
                let mut p = Obj::new();
                p.insert("name".into(), json!(param.name));
                p.insert("value".into(), num(rounded));
                if !unit_json.is_null() {
                    p.insert("unit".into(), unit_json);
                }
                if param.epsg != 0 {
                    p.insert(
                        "id".into(),
                        json!({"authority": "EPSG", "code": param.epsg}),
                    );
                }
                parameters.push(Value::Object(p));
            }
            if name == "tmerc" {
                let _ = self.has("approx");
            }
            json!({
                "name": "unknown",
                "method": {"name": m.name, "id": {"authority": "EPSG", "code": m.epsg}},
                "parameters": parameters,
            })
        };
        let [e, n] = self.axes(&unit, false, pole, false)?;
        let mut axes = vec![e, n];
        if let Some(h) = geodetic["coordinate_system"]["axis"].get(2) {
            axes.push(h.clone());
        }
        let cs = json!({"subtype": "Cartesian", "axis": axes});
        if web_mercator {
            return Ok(json!({
                "type": "ProjectedCRS",
                "name": web_mercator_name,
                "base_crs": wgs84_4326(),
                "conversion": {
                    "name": "unnamed",
                    "method": {"name": "Popular Visualisation Pseudo Mercator",
                        "id": {"authority": "EPSG", "code": 1024}},
                    "parameters": [
                        {"name": "Latitude of natural origin", "value": 0, "unit": "degree",
                            "id": {"authority": "EPSG", "code": 8801}},
                        {"name": "Longitude of natural origin", "value": 0, "unit": "degree",
                            "id": {"authority": "EPSG", "code": 8802}},
                        {"name": "False easting", "value": 0, "unit": "metre",
                            "id": {"authority": "EPSG", "code": 8806}},
                        {"name": "False northing", "value": 0, "unit": "metre",
                            "id": {"authority": "EPSG", "code": 8807}},
                    ],
                },
                "coordinate_system": cs,
            }));
        }
        let name_json = if self.title.is_empty() {
            "unknown"
        } else {
            &self.title
        };
        Ok(json!({
            "type": "ProjectedCRS",
            "name": name_json,
            "base_crs": geodetic,
            "conversion": conversion,
            "coordinate_system": cs,
        }))
    }
}

#[derive(Debug, Clone, Copy)]
enum Pole {
    None,
    North,
    South,
}

/// `atoi`: the leading integer of `s`, or 0.
fn atoi(s: &str) -> i64 {
    let t = s.trim_start();
    let end = t
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .map_or(t.len(), |(i, _)| i);
    t[..end].parse().unwrap_or(0)
}

/// `selectSphericalOrEllipsoidal`.
fn spherical_or_ellipsoidal(m: &'static Method, sphere: bool) -> &'static Method {
    let pick = |spherical: u32, ellipsoidal: u32| {
        let code = if sphere { spherical } else { ellipsoidal };
        METHODS.iter().find(|x| x.epsg == code).unwrap_or(m)
    };
    match m.epsg {
        9834 | 9835 => pick(9834, 9835),
        1027 | 9820 => pick(1027, 9820),
        1029 | 1028 => pick(1029, 1028),
        _ => m,
    }
}

/// `Conversion::createUTM`.
fn utm(zone: u32, north: bool) -> Value {
    let p = |name: &str, value: f64, unit: &str, code: u32| json!({"name": name, "value": num(value), "unit": unit, "id": {"authority": "EPSG", "code": code}});
    json!({
        "name": format!("UTM zone {zone}{}", if north { "N" } else { "S" }),
        "method": {"name": "Transverse Mercator", "id": {"authority": "EPSG", "code": 9807}},
        "parameters": [
            p("Latitude of natural origin", 0.0, "degree", 8801),
            p("Longitude of natural origin", f64::from(zone) * 6.0 - 183.0, "degree", 8802),
            p("Scale factor at natural origin", 0.9996, "unity", 8805),
            p("False easting", 500000.0, "metre", 8806),
            p("False northing", if north { 0.0 } else { 10000000.0 }, "metre", 8807),
        ],
        "id": {"authority": "EPSG", "code": if north { 16000 + zone } else { 16100 + zone }},
    })
}

/// PROJ's `GeographicCRS::EPSG_4326`, as a Bound CRS's hub and Web
/// Mercator's base.
fn wgs84_4326() -> Value {
    json!({"type": "GeographicCRS", "name": "WGS 84",
        "datum": {"type": "GeodeticReferenceFrame", "name": "World Geodetic System 1984",
            "ellipsoid": {"name": "WGS 84", "semi_major_axis": 6378137, "inverse_flattening": 298.257223563}},
        "coordinate_system": {"subtype": "ellipsoidal", "axis": [
            {"name": "Latitude", "abbreviation": "lat", "direction": "north", "unit": "degree"},
            {"name": "Longitude", "abbreviation": "lon", "direction": "east", "unit": "degree"}]},
        "id": {"authority": "EPSG", "code": 4326}})
}
