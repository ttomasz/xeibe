//! WKT2 (ISO 19162:2015 and :2019) to PROJJSON, node for node
//! (`docs/crs-wkt.md`, Reading).
//!
//! PROJJSON was designed as the JSON form of WKT2 2019, so most nodes map
//! to one key. What needs work: units, which WKT can leave to an enclosing
//! node and PROJJSON gives each value; the base CRSs of projected and
//! derived CRSs, which WKT gives without a coordinate system and PROJJSON
//! with one (filled in as PROJ fills it in); and keywords that have
//! synonyms. Every child a node may have is listed; any other is an error
//! that names it.

use std::f64::consts::PI;

use serde_json::{Map, Value, json};

use super::parse::{Arg, Node};
use crate::error::Error;

pub(super) type Obj = Map<String, Value>;

/// A CRS node to its PROJJSON object.
pub(crate) fn crs(node: &Node) -> Result<Value, Error> {
    match node.keyword.as_str() {
        "GEOGCRS" | "GEOGRAPHICCRS" | "GEODCRS" | "GEODETICCRS" => geodetic(node),
        "PROJCRS" | "PROJECTEDCRS" => projected(node),
        "VERTCRS" | "VERTICALCRS" => vertical(node),
        "ENGCRS" | "ENGINEERINGCRS" => engineering(node),
        "COMPOUNDCRS" => compound(node),
        "DERIVEDPROJCRS" | "DERIVEDPROJECTEDCRS" => derived_projected(node),
        "BOUNDCRS" => bound(node),
        other => Err(unsupported(node, &format!("WKT2 {other}"))),
    }
}

// --- keywords --------------------------------------------------------------

const GEODETIC_DATUM: &[&str] = &["DATUM", "TRF", "GEODETICDATUM"];
const VERTICAL_DATUM: &[&str] = &["VDATUM", "VRF", "VERTICALDATUM"];
const ENGINEERING_DATUM: &[&str] = &["EDATUM", "ENGINEERINGDATUM"];
const BASE_GEODETIC: &[&str] = &["BASEGEOGCRS", "BASEGEODCRS"];
const UNITS: &[&str] = &[
    "LENGTHUNIT",
    "ANGLEUNIT",
    "SCALEUNIT",
    "TIMEUNIT",
    "TEMPORALQUANTITY",
    "PARAMETRICUNIT",
    "UNIT",
];
/// What any CRS may end with: its usages (2019), the same flattened (2015),
/// its ids and remarks.
const USAGE: &[&str] = &[
    "USAGE",
    "SCOPE",
    "AREA",
    "BBOX",
    "VERTICALEXTENT",
    "TIMEEXTENT",
    "ID",
    "REMARK",
];
/// EPSG's export names the transformation that defines an intermediate
/// datum ("ETRS89 to S34J-IRF (1)"). PROJJSON has no place for it, and
/// PROJ drops it too.
const SKIPPED: &[&str] = &["DEFININGTRANSFORMATION"];

/// The PROJJSON spelling of each axis direction; WKT's are case-insensitive.
const DIRECTIONS: &[&str] = &[
    "north",
    "northNorthEast",
    "northEast",
    "eastNorthEast",
    "east",
    "eastSouthEast",
    "southEast",
    "southSouthEast",
    "south",
    "southSouthWest",
    "southWest",
    "westSouthWest",
    "west",
    "westNorthWest",
    "northWest",
    "northNorthWest",
    "up",
    "down",
    "geocentricX",
    "geocentricY",
    "geocentricZ",
    "columnPositive",
    "columnNegative",
    "rowPositive",
    "rowNegative",
    "displayRight",
    "displayLeft",
    "displayUp",
    "displayDown",
    "forward",
    "aft",
    "port",
    "starboard",
    "clockwise",
    "counterClockwise",
    "towards",
    "awayFrom",
    "future",
    "past",
    "unspecified",
];

const CS_TYPES: &[&str] = &[
    "Cartesian",
    "spherical",
    "ellipsoidal",
    "vertical",
    "ordinal",
    "parametric",
    "affine",
    "TemporalDateTime",
    "TemporalCount",
    "TemporalMeasure",
];

// --- node access -----------------------------------------------------------

impl Node {
    pub(super) fn children<'a, 'k>(
        &'a self,
        keywords: &'k [&'k str],
    ) -> impl Iterator<Item = &'a Node> + 'k
    where
        'a: 'k,
    {
        self.args.iter().filter_map(move |a| match a {
            Arg::Node(n) if keywords.contains(&n.keyword.as_str()) => Some(n),
            _ => None,
        })
    }

    pub(super) fn child(&self, keywords: &[&str]) -> Option<&Node> {
        self.args.iter().find_map(|a| match a {
            Arg::Node(n) if keywords.contains(&n.keyword.as_str()) => Some(n),
            _ => None,
        })
    }

    pub(super) fn text(&self, i: usize) -> Result<&str, Error> {
        match self.args.get(i) {
            Some(Arg::Text(s)) => Ok(s),
            _ => Err(self.invalid(&format!("expected a quoted string as value {}", i + 1))),
        }
    }

    pub(super) fn number(&self, i: usize) -> Result<f64, Error> {
        match self.args.get(i) {
            Some(Arg::Number(v)) => Ok(*v),
            _ => Err(self.invalid(&format!("expected a number as value {}", i + 1))),
        }
    }

    pub(super) fn word(&self, i: usize) -> Result<&str, Error> {
        match self.args.get(i) {
            Some(Arg::Word(w)) => Ok(w),
            _ => Err(self.invalid(&format!("expected a word as value {}", i + 1))),
        }
    }

    pub(super) fn invalid(&self, message: &str) -> Error {
        Error::Wkt {
            offset: self.offset,
            message: format!("{}: {message}", self.keyword),
        }
    }

    /// Fails on a child node not in `allowed` (nor in [`SKIPPED`]).
    pub(super) fn only(&self, allowed: &[&[&str]]) -> Result<(), Error> {
        for a in &self.args {
            if let Arg::Node(n) = a {
                let k = n.keyword.as_str();
                if !SKIPPED.contains(&k) && !allowed.iter().any(|set| set.contains(&k)) {
                    return Err(unsupported(n, &format!("{k} in {}", self.keyword)));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn unsupported(node: &Node, what: &str) -> Error {
    Error::Unsupported {
        crs: format!("WKT at byte {}", node.offset),
        what: what.to_owned(),
    }
}

/// An integral value as a JSON integer, as PROJ writes `6378137`, not
/// `6378137.0`.
pub(super) fn num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9.007_199_254_740_992e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

// --- units -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum UnitKind {
    Linear,
    Angular,
    Scale,
    Time,
    Parametric,
    Any,
}

#[derive(Debug, Clone)]
pub(super) struct Unit {
    pub(super) kind: UnitKind,
    pub(super) json: Value,
}

impl Unit {
    pub(super) fn metre() -> Self {
        Self {
            kind: UnitKind::Linear,
            json: json!("metre"),
        }
    }

    pub(super) fn degree() -> Self {
        Self {
            kind: UnitKind::Angular,
            json: json!("degree"),
        }
    }

    pub(super) fn unity() -> Self {
        Self {
            kind: UnitKind::Scale,
            json: json!("unity"),
        }
    }

    pub(super) fn is(&self, shorthand: &str) -> bool {
        self.json == shorthand
    }

    /// A value in this unit: a bare number in metres or degrees, as PROJJSON
    /// writes the common cases, else `{value, unit}`.
    pub(super) fn measure(&self, v: f64, bare: &str) -> Value {
        if self.is(bare) {
            num(v)
        } else {
            json!({"value": num(v), "unit": self.json})
        }
    }

    /// Whether this unit can apply to a value of `kind`.
    pub(super) fn fits(&self, kind: UnitKind) -> bool {
        self.kind == kind || self.kind == UnitKind::Any
    }
}

pub(super) fn unit(node: &Node) -> Result<Unit, Error> {
    let kind = match node.keyword.as_str() {
        "LENGTHUNIT" => UnitKind::Linear,
        "ANGLEUNIT" => UnitKind::Angular,
        "SCALEUNIT" => UnitKind::Scale,
        "TIMEUNIT" | "TEMPORALQUANTITY" => UnitKind::Time,
        "PARAMETRICUNIT" => UnitKind::Parametric,
        _ => UnitKind::Any,
    };
    node.only(&[&["ID"]])?;
    let name = node.text(0)?;
    let factor = node.number(1)?;
    let close = |want: f64| (factor - want).abs() <= 1e-12 * want;
    let shorthand = match kind {
        UnitKind::Linear | UnitKind::Any if name == "metre" && factor == 1.0 => Some("metre"),
        UnitKind::Angular | UnitKind::Any if name == "degree" && close(PI / 180.0) => {
            Some("degree")
        }
        UnitKind::Scale | UnitKind::Any if name == "unity" && factor == 1.0 => Some("unity"),
        _ => None,
    };
    let json = match shorthand {
        Some(s) => json!(s),
        None => {
            let mut o = Obj::new();
            let type_name = match kind {
                UnitKind::Linear => "LinearUnit",
                UnitKind::Angular => "AngularUnit",
                UnitKind::Scale => "ScaleUnit",
                UnitKind::Time => "TimeUnit",
                UnitKind::Parametric => "ParametricUnit",
                UnitKind::Any => "Unit",
            };
            o.insert("type".into(), json!(type_name));
            o.insert("name".into(), json!(name));
            o.insert("conversion_factor".into(), num(factor));
            ids(node, &mut o)?;
            Value::Object(o)
        }
    };
    Ok(Unit { kind, json })
}

/// The unit node directly inside `node`, if any.
pub(super) fn own_unit(node: &Node) -> Result<Option<Unit>, Error> {
    node.child(UNITS).map(unit).transpose()
}

// --- identifiers, usages ---------------------------------------------------

fn id(node: &Node) -> Result<Value, Error> {
    node.only(&[&["CITATION", "URI"]])?;
    let mut o = Obj::new();
    // EPSG's export writes four ensembles' ids as "EPSG ".
    o.insert("authority".into(), json!(node.text(0)?.trim()));
    let code = match node.args.get(1) {
        Some(Arg::Number(v)) => num(*v),
        Some(Arg::Text(s)) => match s.parse::<i64>() {
            Ok(n) => json!(n),
            Err(_) => json!(s),
        },
        _ => return Err(node.invalid("expected a code as value 2")),
    };
    o.insert("code".into(), code);
    match node.args.get(2) {
        Some(Arg::Number(v)) => {
            o.insert("version".into(), num(*v));
        }
        Some(Arg::Text(s)) => {
            o.insert("version".into(), json!(s));
        }
        _ => {}
    }
    if let Some(c) = node.child(&["CITATION"]) {
        o.insert("authority_citation".into(), json!(c.text(0)?));
    }
    if let Some(u) = node.child(&["URI"]) {
        o.insert("uri".into(), json!(u.text(0)?));
    }
    Ok(Value::Object(o))
}

/// `id`, or `ids` when there are several.
pub(super) fn ids(node: &Node, out: &mut Obj) -> Result<(), Error> {
    let all = node
        .children(&["ID"])
        .map(id)
        .collect::<Result<Vec<_>, _>>()?;
    match all.len() {
        0 => {}
        1 => {
            out.insert("id".into(), all.into_iter().next().unwrap_or_default());
        }
        _ => {
            out.insert("ids".into(), Value::Array(all));
        }
    }
    Ok(())
}

/// The members of one usage, from a `USAGE` node (2019) or the CRS itself
/// (2015, where they come without one).
fn usage_members(node: &Node, out: &mut Obj) -> Result<(), Error> {
    if let Some(s) = node.child(&["SCOPE"]) {
        out.insert("scope".into(), json!(s.text(0)?));
    }
    if let Some(a) = node.child(&["AREA"]) {
        out.insert("area".into(), json!(a.text(0)?));
    }
    if let Some(b) = node.child(&["BBOX"]) {
        out.insert(
            "bbox".into(),
            json!({
                "south_latitude": num(b.number(0)?),
                "west_longitude": num(b.number(1)?),
                "north_latitude": num(b.number(2)?),
                "east_longitude": num(b.number(3)?),
            }),
        );
    }
    if let Some(v) = node.child(&["VERTICALEXTENT"]) {
        let mut e = Obj::new();
        e.insert("minimum".into(), num(v.number(0)?));
        e.insert("maximum".into(), num(v.number(1)?));
        if let Some(u) = own_unit(v)? {
            e.insert("unit".into(), u.json);
        }
        out.insert("vertical_extent".into(), Value::Object(e));
    }
    if let Some(t) = node.child(&["TIMEEXTENT"]) {
        let end = |i: usize| match t.args.get(i) {
            Some(Arg::Text(s)) => Ok(json!(s)),
            Some(Arg::Number(v)) => Ok(json!(v.to_string())),
            _ => Err(t.invalid("expected a time")),
        };
        out.insert(
            "temporal_extent".into(),
            json!({"start": end(0)?, "end": end(1)?}),
        );
    }
    Ok(())
}

/// Usages, ids and remarks, which close every CRS. One usage is written
/// flat, several as `usages`, as PROJ writes them.
fn finish(node: &Node, out: &mut Obj) -> Result<(), Error> {
    let usages: Vec<&Node> = node.children(&["USAGE"]).collect();
    if usages.is_empty() {
        usage_members(node, out)?;
    } else if let [one] = usages[..] {
        one.only(&[&["SCOPE", "AREA", "BBOX", "VERTICALEXTENT", "TIMEEXTENT"]])?;
        usage_members(one, out)?;
    } else {
        let mut list = Vec::new();
        for u in usages {
            u.only(&[&["SCOPE", "AREA", "BBOX", "VERTICALEXTENT", "TIMEEXTENT"]])?;
            let mut o = Obj::new();
            usage_members(u, &mut o)?;
            list.push(Value::Object(o));
        }
        out.insert("usages".into(), Value::Array(list));
    }
    if let Some(r) = node.child(&["REMARK"]) {
        out.insert("remarks".into(), json!(r.text(0)?));
    }
    ids(node, out)
}

/// An object's type and name, without the " (deprecated)" PROJ appends to
/// deprecated CRSs in WKT1.
fn named(type_name: &str, node: &Node) -> Result<Obj, Error> {
    let mut o = Obj::new();
    o.insert("type".into(), json!(type_name));
    let name = node.text(0)?;
    o.insert(
        "name".into(),
        json!(name.strip_suffix(" (deprecated)").unwrap_or(name)),
    );
    Ok(o)
}

// --- datums ----------------------------------------------------------------

fn ellipsoid(node: &Node) -> Result<Value, Error> {
    node.only(&[UNITS, &["ID"]])?;
    let a = node.number(1)?;
    let rf = node.number(2)?;
    let u = own_unit(node)?.unwrap_or_else(Unit::metre);
    let mut o = Obj::new();
    o.insert("name".into(), json!(node.text(0)?));
    // An inverse flattening of 0 marks a sphere.
    if rf == 0.0 {
        o.insert("radius".into(), u.measure(a, "metre"));
    } else {
        o.insert("semi_major_axis".into(), u.measure(a, "metre"));
        o.insert("inverse_flattening".into(), num(rf));
    }
    ids(node, &mut o)?;
    Ok(Value::Object(o))
}

/// `None` for Greenwich, which PROJJSON leaves implicit.
fn prime_meridian(node: &Node) -> Result<Option<Value>, Error> {
    node.only(&[UNITS, &["ID"]])?;
    let name = node.text(0)?;
    let mut longitude = node.number(1)?;
    if longitude == 0.0 && name.eq_ignore_ascii_case("Greenwich") {
        return Ok(None);
    }
    let mut u = own_unit(node)?.unwrap_or_else(Unit::degree);
    // PROJ's correction for meridians defined in degrees, minutes and
    // seconds: EPSG's export rounds Lisbon to -9.131906111.
    if let Some(degrees) = super::wkt1::sexagesimal(name, longitude) {
        longitude = degrees;
        u = Unit::degree();
    }
    let mut o = Obj::new();
    o.insert("name".into(), json!(name));
    o.insert("longitude".into(), u.measure(longitude, "degree"));
    ids(node, &mut o)?;
    Ok(Some(Value::Object(o)))
}

fn anchors(node: &Node, out: &mut Obj) -> Result<(), Error> {
    if let Some(a) = node.child(&["ANCHOR"]) {
        out.insert("anchor".into(), json!(a.text(0)?));
    }
    if let Some(a) = node.child(&["ANCHOREPOCH"]) {
        out.insert("anchor_epoch".into(), num(a.number(0)?));
    }
    Ok(())
}

/// `DYNAMIC[FRAMEEPOCH[…], MODEL[…]]`: the frame's epoch, and deformation
/// models for the CRS.
fn dynamic(node: Option<&Node>) -> Result<(Option<f64>, Vec<Value>), Error> {
    let Some(d) = node else {
        return Ok((None, Vec::new()));
    };
    d.only(&[&["FRAMEEPOCH", "MODEL", "VELOCITYGRID"]])?;
    let epoch = d
        .child(&["FRAMEEPOCH"])
        .ok_or_else(|| d.invalid("no FRAMEEPOCH"))?
        .number(0)?;
    let mut models = Vec::new();
    for m in d.children(&["MODEL", "VELOCITYGRID"]) {
        m.only(&[&["ID"]])?;
        let mut o = Obj::new();
        o.insert("name".into(), json!(m.text(0)?));
        ids(m, &mut o)?;
        models.push(Value::Object(o));
    }
    Ok((Some(epoch), models))
}

/// A geodetic reference frame, with the prime meridian WKT puts next to it.
fn geodetic_frame(node: &Node, epoch: Option<f64>, primem: Option<&Node>) -> Result<Value, Error> {
    node.only(&[&["ELLIPSOID", "SPHEROID", "ANCHOR", "ANCHOREPOCH", "ID"]])?;
    let kind = if epoch.is_some() {
        "DynamicGeodeticReferenceFrame"
    } else {
        "GeodeticReferenceFrame"
    };
    let mut o = named(kind, node)?;
    if let Some(e) = epoch {
        o.insert("frame_reference_epoch".into(), num(e));
    }
    let e = node
        .child(&["ELLIPSOID", "SPHEROID"])
        .ok_or_else(|| node.invalid("no ELLIPSOID"))?;
    o.insert("ellipsoid".into(), ellipsoid(e)?);
    anchors(node, &mut o)?;
    if let Some(pm) = primem.map(prime_meridian).transpose()?.flatten() {
        o.insert("prime_meridian".into(), pm);
    }
    ids(node, &mut o)?;
    Ok(Value::Object(o))
}

fn vertical_frame(node: &Node, epoch: Option<f64>) -> Result<Value, Error> {
    node.only(&[&["ANCHOR", "ANCHOREPOCH", "ID"]])?;
    let kind = if epoch.is_some() {
        "DynamicVerticalReferenceFrame"
    } else {
        "VerticalReferenceFrame"
    };
    let mut o = named(kind, node)?;
    if let Some(e) = epoch {
        o.insert("frame_reference_epoch".into(), num(e));
    }
    anchors(node, &mut o)?;
    ids(node, &mut o)?;
    Ok(Value::Object(o))
}

fn ensemble(node: &Node) -> Result<Value, Error> {
    node.only(&[&["MEMBER", "ELLIPSOID", "SPHEROID", "ENSEMBLEACCURACY", "ID"]])?;
    let mut o = Obj::new();
    o.insert("name".into(), json!(node.text(0)?));
    let mut members = Vec::new();
    for m in node.children(&["MEMBER"]) {
        m.only(&[&["ID"]])?;
        let mut mo = Obj::new();
        mo.insert("name".into(), json!(m.text(0)?));
        ids(m, &mut mo)?;
        members.push(Value::Object(mo));
    }
    o.insert("members".into(), Value::Array(members));
    if let Some(e) = node.child(&["ELLIPSOID", "SPHEROID"]) {
        o.insert("ellipsoid".into(), ellipsoid(e)?);
    }
    let accuracy = node
        .child(&["ENSEMBLEACCURACY"])
        .ok_or_else(|| node.invalid("no ENSEMBLEACCURACY"))?
        .number(0)?;
    // PROJJSON writes the accuracy as a string ("2", "0.1").
    o.insert("accuracy".into(), json!(accuracy.to_string()));
    ids(node, &mut o)?;
    Ok(Value::Object(o))
}

/// The datum or ensemble of a geodetic CRS (or of a base CRS), under its
/// PROJJSON key, plus the CRS's deformation models.
fn geodetic_datum(node: &Node) -> Result<(&'static str, Value, Vec<Value>), Error> {
    let (epoch, models) = dynamic(node.child(&["DYNAMIC"]))?;
    let primem = node.child(&["PRIMEM", "PRIMEMERIDIAN"]);
    if let Some(e) = node.child(&["ENSEMBLE"]) {
        return Ok(("datum_ensemble", ensemble(e)?, models));
    }
    let d = node
        .child(GEODETIC_DATUM)
        .ok_or_else(|| node.invalid("no DATUM or ENSEMBLE"))?;
    Ok(("datum", geodetic_frame(d, epoch, primem)?, models))
}

fn vertical_datum(node: &Node) -> Result<(&'static str, Value, Vec<Value>), Error> {
    let (epoch, models) = dynamic(node.child(&["DYNAMIC"]))?;
    if let Some(e) = node.child(&["ENSEMBLE"]) {
        return Ok(("datum_ensemble", ensemble(e)?, models));
    }
    let d = node
        .child(VERTICAL_DATUM)
        .ok_or_else(|| node.invalid("no VDATUM or ENSEMBLE"))?;
    Ok(("datum", vertical_frame(d, epoch)?, models))
}

// --- coordinate systems ----------------------------------------------------

/// The kind of unit an axis takes: angles for latitude and longitude in an
/// ellipsoidal or spherical CS, nothing for an ordinal one, else lengths.
fn axis_kind(cs: &str, direction: &str) -> Option<UnitKind> {
    match cs {
        "ordinal" => None,
        "ellipsoidal" | "spherical" if matches!(direction, "north" | "south" | "east" | "west") => {
            Some(UnitKind::Angular)
        }
        "TemporalDateTime" | "TemporalCount" | "TemporalMeasure" => Some(UnitKind::Time),
        "parametric" => Some(UnitKind::Parametric),
        _ => Some(UnitKind::Linear),
    }
}

fn axis(node: &Node, cs: &str, shared: Option<&Unit>) -> Result<(Option<f64>, Value), Error> {
    node.only(&[
        UNITS,
        &[
            "ORDER",
            "MERIDIAN",
            "BEARING",
            "AXISMINVALUE",
            "AXISMAXVALUE",
            "RANGEMEANING",
            "ID",
        ],
    ])?;
    // "Geodetic latitude (Lat)": a name and an abbreviation, either of
    // which may be missing. The abbreviation can hold brackets of its own:
    // "Easting (E(X))".
    let label = node.text(0)?.trim();
    let word = node.word(1)?;
    let direction = DIRECTIONS
        .iter()
        .find(|d| d.eq_ignore_ascii_case(word))
        .ok_or_else(|| node.invalid(&format!("unknown axis direction {word}")))?;
    let (name, abbreviation) = axis_name(label, direction);
    let mut o = Obj::new();
    o.insert("name".into(), json!(name));
    o.insert("abbreviation".into(), json!(abbreviation));
    o.insert("direction".into(), json!(direction));
    if let Some(m) = node.child(&["MERIDIAN"]) {
        m.only(&[UNITS])?;
        let u = own_unit(m)?.unwrap_or_else(Unit::degree);
        o.insert(
            "meridian".into(),
            json!({"longitude": u.measure(m.number(0)?, "degree")}),
        );
    }
    let kind = axis_kind(cs, direction);
    let unit = match (own_unit(node)?, kind) {
        (Some(u), _) => Some(u),
        (None, Some(k)) => match shared {
            Some(u) if u.fits(k) => Some(u.clone()),
            _ => match k {
                UnitKind::Angular => Some(Unit::degree()),
                UnitKind::Linear => Some(Unit::metre()),
                _ => None,
            },
        },
        (None, None) => shared.cloned(),
    };
    if let Some(u) = unit {
        o.insert("unit".into(), u.json);
    }
    if let Some(v) = node.child(&["AXISMINVALUE"]) {
        o.insert("minimum_value".into(), num(v.number(0)?));
    }
    if let Some(v) = node.child(&["AXISMAXVALUE"]) {
        o.insert("maximum_value".into(), num(v.number(0)?));
    }
    if let Some(r) = node.child(&["RANGEMEANING"]) {
        o.insert("range_meaning".into(), json!(r.word(0)?));
    }
    ids(node, &mut o)?;
    let order = node.child(&["ORDER"]).map(|o| o.number(0)).transpose()?;
    Ok((order, Value::Object(o)))
}

/// An axis label's name and the abbreviation in the brackets that close it.
/// An axis's name and abbreviation from its label, as PROJ's `buildAxis`
/// gives them: the name capitalised ("latitude" is "Latitude"); without an
/// abbreviation, latitude, longitude and ellipsoidal height take theirs;
/// without a name, a few abbreviations give one ("(E)" is "Easting"), and
/// geocentric axes theirs.
fn axis_name(label: &str, direction: &str) -> (String, String) {
    let capitalised = |n: &str| {
        let mut c = n.chars();
        c.next()
            .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
    };
    let (name, abbreviation) = split_label(label);
    if name.is_empty() {
        let name = match (abbreviation, direction) {
            ("E", _) => "Easting",
            ("N", _) => "Northing",
            ("lat", _) => "Latitude",
            ("lon", _) => "Longitude",
            ("X", "geocentricX") => "Geocentric X",
            ("Y", "geocentricY") => "Geocentric Y",
            ("Z", "geocentricZ") => "Geocentric Z",
            (other, _) => other,
        };
        return (name.to_owned(), abbreviation.to_owned());
    }
    let name = capitalised(name);
    if !abbreviation.is_empty() || label.ends_with(')') {
        return (name, abbreviation.to_owned());
    }
    let abbreviation = match name.as_str() {
        "Latitude" => "lat",
        "Longitude" => "lon",
        "Ellipsoidal height" => "h",
        _ => "",
    };
    (name, abbreviation.to_owned())
}

fn split_label(label: &str) -> (&str, &str) {
    if !label.ends_with(')') {
        return (label, "");
    }
    let mut depth = 0;
    for (i, c) in label.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    return (label[..i].trim(), label[i + 1..label.len() - 1].trim());
                }
            }
            _ => {}
        }
    }
    (label, "")
}

/// `CS[…]`, the `AXIS` nodes after it and the unit they share, all children
/// of `node`.
fn coordinate_system(node: &Node) -> Result<Value, Error> {
    let cs = node.child(&["CS"]).ok_or_else(|| node.invalid("no CS"))?;
    cs.only(&[&["ID"]])?;
    let word = cs.word(0)?;
    let subtype = CS_TYPES
        .iter()
        .find(|t| t.eq_ignore_ascii_case(word))
        .ok_or_else(|| unsupported(cs, &format!("a {word} coordinate system")))?;
    let dimension = cs.number(1)?;
    let shared = own_unit(node)?;
    let mut axes = node
        .children(&["AXIS"])
        .map(|a| axis(a, subtype, shared.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    if axes.len() as f64 != dimension {
        return Err(cs.invalid(&format!(
            "{} axes for a {dimension}-dimensional CS",
            axes.len()
        )));
    }
    if axes.iter().all(|(order, _)| order.is_some()) {
        axes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    }
    let mut o = Obj::new();
    o.insert("subtype".into(), json!(subtype));
    o.insert(
        "axis".into(),
        Value::Array(axes.into_iter().map(|(_, a)| a).collect()),
    );
    ids(cs, &mut o)?;
    Ok(Value::Object(o))
}

fn axis_json(name: &str, abbreviation: &str, direction: &str, unit: &Unit) -> Value {
    json!({
        "name": name,
        "abbreviation": abbreviation,
        "direction": direction,
        "unit": unit.json,
    })
}

// --- CRSs ------------------------------------------------------------------

fn geodetic(node: &Node) -> Result<Value, Error> {
    if node.child(BASE_GEODETIC).is_some() {
        return Err(unsupported(node, "a derived geodetic or geographic CRS"));
    }
    node.only(&[
        GEODETIC_DATUM,
        UNITS,
        USAGE,
        &[
            "DYNAMIC",
            "ENSEMBLE",
            "PRIMEM",
            "PRIMEMERIDIAN",
            "CS",
            "AXIS",
        ],
    ])?;
    let cs = coordinate_system(node)?;
    // GEODCRS with an ellipsoidal CS is how WKT2 2015 writes a geographic
    // CRS; PROJ reads it as one.
    let geographic = node.keyword.starts_with("GEOG") || cs["subtype"] == "ellipsoidal";
    let mut o = named(
        if geographic {
            "GeographicCRS"
        } else {
            "GeodeticCRS"
        },
        node,
    )?;
    let (key, datum, models) = geodetic_datum(node)?;
    o.insert(key.into(), datum);
    if !models.is_empty() {
        o.insert("deformation_models".into(), Value::Array(models));
    }
    o.insert("coordinate_system".into(), cs);
    finish(node, &mut o)?;
    Ok(Value::Object(o))
}

/// `BASEGEOGCRS`: a geographic CRS without a coordinate system, which gets
/// latitude and longitude in its own angle unit, as PROJ gives it.
fn base_geographic(node: &Node) -> Result<Value, Error> {
    node.only(&[
        GEODETIC_DATUM,
        UNITS,
        &["DYNAMIC", "ENSEMBLE", "PRIMEM", "PRIMEMERIDIAN", "ID"],
    ])?;
    let mut o = named("GeographicCRS", node)?;
    let (key, datum, models) = geodetic_datum(node)?;
    o.insert(key.into(), datum);
    if !models.is_empty() {
        o.insert("deformation_models".into(), Value::Array(models));
    }
    let u = own_unit(node)?.unwrap_or_else(Unit::degree);
    o.insert(
        "coordinate_system".into(),
        json!({
            "subtype": "ellipsoidal",
            "axis": [
                axis_json("Geodetic latitude", "Lat", "north", &u),
                axis_json("Geodetic longitude", "Lon", "east", &u),
            ],
        }),
    );
    ids(node, &mut o)?;
    Ok(Value::Object(o))
}

/// What a parameter measures, from its name, for a parameter WKT gives
/// without a unit.
fn parameter_kind(name: &str) -> Option<UnitKind> {
    let n = name.to_ascii_lowercase();
    if n.contains("scale") {
        Some(UnitKind::Scale)
    } else if [
        "latitude",
        "longitude",
        "azimuth",
        "angle",
        "meridian",
        "parallel",
    ]
    .iter()
    .any(|w| n.contains(w))
    {
        Some(UnitKind::Angular)
    } else if ["easting", "northing", "height", "x", "y"]
        .iter()
        .any(|w| n.split(|c: char| !c.is_alphanumeric()).any(|t| t == *w))
    {
        Some(UnitKind::Linear)
    } else {
        None
    }
}

/// `CONVERSION` or `DERIVINGCONVERSION`. A parameter without a unit takes
/// the CRS's length unit or its base's angle unit, by what it measures.
fn conversion(node: &Node, length: &Unit, angle: &Unit) -> Result<Value, Error> {
    node.only(&[&["METHOD", "PROJECTION", "PARAMETER", "ID"]])?;
    let m = node
        .child(&["METHOD", "PROJECTION"])
        .ok_or_else(|| node.invalid("no METHOD"))?;
    m.only(&[&["ID"]])?;
    let mut method = Obj::new();
    method.insert("name".into(), json!(m.text(0)?));
    ids(m, &mut method)?;
    let mut o = Obj::new();
    o.insert("name".into(), json!(node.text(0)?));
    o.insert("method".into(), Value::Object(method));
    let mut parameters = Vec::new();
    for p in node.children(&["PARAMETER"]) {
        p.only(&[UNITS, &["ID"]])?;
        let name = p.text(0)?;
        let mut po = Obj::new();
        po.insert("name".into(), json!(name));
        po.insert("value".into(), num(p.number(1)?));
        let u = match own_unit(p)? {
            Some(u) => Some(u),
            None => match parameter_kind(name) {
                Some(UnitKind::Linear) => Some(length.clone()),
                Some(UnitKind::Angular) => Some(angle.clone()),
                Some(UnitKind::Scale) => Some(Unit::unity()),
                _ => None,
            },
        };
        if let Some(u) = u {
            po.insert("unit".into(), u.json);
        }
        ids(p, &mut po)?;
        parameters.push(Value::Object(po));
    }
    if !parameters.is_empty() {
        o.insert("parameters".into(), Value::Array(parameters));
    }
    ids(node, &mut o)?;
    Ok(Value::Object(o))
}

/// `BOUNDCRS[SOURCECRS[…], TARGETCRS[…], ABRIDGEDTRANSFORMATION[…]]`.
/// The abridged transformation's Helmert parameters have fixed units,
/// whatever is written, as PROJ reads them (`convertFromAbridged`):
/// translations in metres, rotations in arc-seconds, and the scale
/// difference given as a scale factor, which becomes parts per million.
fn bound(node: &Node) -> Result<Value, Error> {
    node.only(&[&["SOURCECRS", "TARGETCRS", "ABRIDGEDTRANSFORMATION"], USAGE])?;
    let inner = |keyword: &str| -> Result<Value, Error> {
        let n = node
            .child(&[keyword])
            .ok_or_else(|| node.invalid(&format!("no {keyword}")))?;
        let found = n.args.iter().find_map(|a| match a {
            Arg::Node(c) => Some(c),
            _ => None,
        });
        crs(found.ok_or_else(|| n.invalid("no CRS"))?)
    };
    let source = inner("SOURCECRS")?;
    let target = inner("TARGETCRS")?;
    let t = node
        .child(&["ABRIDGEDTRANSFORMATION"])
        .ok_or_else(|| node.invalid("no ABRIDGEDTRANSFORMATION"))?;
    t.only(&[&[
        "METHOD",
        "PARAMETER",
        "PARAMETERFILE",
        "ID",
        "VERSION",
        "REMARK",
        "OPERATIONACCURACY",
    ]])?;
    let m = t.child(&["METHOD"]).ok_or_else(|| t.invalid("no METHOD"))?;
    m.only(&[&["ID"]])?;
    let mut method = Obj::new();
    method.insert("name".into(), json!(m.text(0)?));
    ids(m, &mut method)?;
    let arc_second = json!({"type": "AngularUnit", "name": "arc-second",
        "conversion_factor": 4.84813681109536e-06});
    let ppm = json!({"type": "ScaleUnit", "name": "parts per million",
        "conversion_factor": 1e-06});
    let helmert: [(u32, &str); 7] = [
        (8605, "X-axis translation"),
        (8606, "Y-axis translation"),
        (8607, "Z-axis translation"),
        (8608, "X-axis rotation"),
        (8609, "Y-axis rotation"),
        (8610, "Z-axis rotation"),
        (8611, "Scale difference"),
    ];
    let mut parameters = Vec::new();
    for p in t.children(&["PARAMETER", "PARAMETERFILE"]) {
        p.only(&[UNITS, &["ID"]])?;
        let name = p.text(0)?;
        let mut po = Obj::new();
        po.insert("name".into(), json!(name));
        if p.keyword == "PARAMETERFILE" {
            po.insert("value".into(), json!(p.text(1)?));
            ids(p, &mut po)?;
            parameters.push(Value::Object(po));
            continue;
        }
        ids(p, &mut po)?;
        let value = p.number(1)?;
        let code = po["id"]["code"].as_u64().map(|c| c as u32);
        let abridged = helmert
            .iter()
            .find(|(c, n)| code == Some(*c) || crate::name::equivalent(name, n));
        match abridged {
            Some(&(c, n)) => {
                po.insert("name".into(), json!(n));
                let (value, unit) = match c {
                    8605..=8607 => (value, json!("metre")),
                    8608..=8610 => (value, arc_second.clone()),
                    _ => ((value - 1.0) * 1e6, ppm.clone()),
                };
                po.insert("value".into(), num(value));
                po.insert("unit".into(), unit);
                po.insert("id".into(), json!({"authority": "EPSG", "code": c}));
            }
            None => {
                po.insert("value".into(), num(value));
                if let Some(u) = own_unit(p)? {
                    po.insert("unit".into(), u.json);
                }
            }
        }
        // The id goes last, as elsewhere.
        if let Some(id) = po.remove("id") {
            po.insert("id".into(), id);
        }
        parameters.push(Value::Object(po));
    }
    let mut transformation = Obj::new();
    transformation.insert("name".into(), json!(t.text(0)?));
    transformation.insert("method".into(), Value::Object(method));
    transformation.insert("parameters".into(), Value::Array(parameters));
    ids(t, &mut transformation)?;
    let mut o = Obj::new();
    o.insert("type".into(), json!("BoundCRS"));
    o.insert("source_crs".into(), source);
    o.insert("target_crs".into(), target);
    o.insert("transformation".into(), Value::Object(transformation));
    finish(node, &mut o)?;
    Ok(Value::Object(o))
}

/// The angle unit of a base geographic CRS, for its conversion's angles.
fn base_angle_unit(base: &Node) -> Result<Unit, Error> {
    Ok(own_unit(base)?
        .filter(|u| u.fits(UnitKind::Angular))
        .unwrap_or_else(Unit::degree))
}

fn projected(node: &Node) -> Result<Value, Error> {
    node.only(&[BASE_GEODETIC, UNITS, USAGE, &["CONVERSION", "CS", "AXIS"]])?;
    let base = node
        .child(BASE_GEODETIC)
        .ok_or_else(|| node.invalid("no BASEGEOGCRS"))?;
    let length = own_unit(node)?
        .filter(|u| u.fits(UnitKind::Linear))
        .unwrap_or_else(Unit::metre);
    let conv = node
        .child(&["CONVERSION"])
        .ok_or_else(|| node.invalid("no CONVERSION"))?;
    let mut o = named("ProjectedCRS", node)?;
    o.insert("base_crs".into(), base_geographic(base)?);
    o.insert(
        "conversion".into(),
        conversion(conv, &length, &base_angle_unit(base)?)?,
    );
    o.insert("coordinate_system".into(), coordinate_system(node)?);
    finish(node, &mut o)?;
    Ok(Value::Object(o))
}

/// `BASEPROJCRS`: a projected CRS without a coordinate system, which gets
/// easting and northing in its own length unit.
fn base_projected(node: &Node) -> Result<Value, Error> {
    node.only(&[BASE_GEODETIC, UNITS, &["CONVERSION", "ID"]])?;
    let base = node
        .child(BASE_GEODETIC)
        .ok_or_else(|| node.invalid("no BASEGEOGCRS"))?;
    let length = own_unit(node)?
        .filter(|u| u.fits(UnitKind::Linear))
        .unwrap_or_else(Unit::metre);
    let conv = node
        .child(&["CONVERSION"])
        .ok_or_else(|| node.invalid("no CONVERSION"))?;
    let mut o = named("ProjectedCRS", node)?;
    o.insert("base_crs".into(), base_geographic(base)?);
    o.insert(
        "conversion".into(),
        conversion(conv, &length, &base_angle_unit(base)?)?,
    );
    o.insert(
        "coordinate_system".into(),
        json!({
            "subtype": "Cartesian",
            "axis": [
                axis_json("Easting", "E", "east", &length),
                axis_json("Northing", "N", "north", &length),
            ],
        }),
    );
    ids(node, &mut o)?;
    Ok(Value::Object(o))
}

fn derived_projected(node: &Node) -> Result<Value, Error> {
    node.only(&[
        UNITS,
        USAGE,
        &["BASEPROJCRS", "DERIVINGCONVERSION", "CS", "AXIS"],
    ])?;
    let base = node
        .child(&["BASEPROJCRS"])
        .ok_or_else(|| node.invalid("no BASEPROJCRS"))?;
    let conv = node
        .child(&["DERIVINGCONVERSION"])
        .ok_or_else(|| node.invalid("no DERIVINGCONVERSION"))?;
    let length = own_unit(node)?
        .filter(|u| u.fits(UnitKind::Linear))
        .unwrap_or_else(Unit::metre);
    let mut o = named("DerivedProjectedCRS", node)?;
    o.insert("base_crs".into(), base_projected(base)?);
    o.insert(
        "conversion".into(),
        conversion(conv, &length, &Unit::degree())?,
    );
    o.insert("coordinate_system".into(), coordinate_system(node)?);
    finish(node, &mut o)?;
    Ok(Value::Object(o))
}

fn geoid_models(node: &Node, out: &mut Obj) -> Result<(), Error> {
    let mut models = Vec::new();
    for g in node.children(&["GEOIDMODEL"]) {
        g.only(&[&["ID"]])?;
        let mut o = Obj::new();
        o.insert("name".into(), json!(g.text(0)?));
        ids(g, &mut o)?;
        models.push(Value::Object(o));
    }
    match models.len() {
        0 => {}
        1 => {
            out.insert("geoid_model".into(), models.remove(0));
        }
        _ => {
            out.insert("geoid_models".into(), Value::Array(models));
        }
    }
    Ok(())
}

fn vertical(node: &Node) -> Result<Value, Error> {
    if let Some(base) = node.child(&["BASEVERTCRS"]) {
        return derived_vertical(node, base);
    }
    node.only(&[
        VERTICAL_DATUM,
        UNITS,
        USAGE,
        &["DYNAMIC", "ENSEMBLE", "CS", "AXIS", "GEOIDMODEL"],
    ])?;
    let mut o = named("VerticalCRS", node)?;
    let (key, datum, models) = vertical_datum(node)?;
    o.insert(key.into(), datum);
    if !models.is_empty() {
        o.insert("deformation_models".into(), Value::Array(models));
    }
    o.insert("coordinate_system".into(), coordinate_system(node)?);
    geoid_models(node, &mut o)?;
    finish(node, &mut o)?;
    Ok(Value::Object(o))
}

/// `VERTCRS[…, BASEVERTCRS[…], DERIVINGCONVERSION[…], …]`. The base gets a
/// gravity-related height in metres, as PROJ gives it.
fn derived_vertical(node: &Node, base: &Node) -> Result<Value, Error> {
    node.only(&[
        UNITS,
        USAGE,
        &["BASEVERTCRS", "DERIVINGCONVERSION", "CS", "AXIS"],
    ])?;
    base.only(&[VERTICAL_DATUM, UNITS, &["DYNAMIC", "ENSEMBLE", "ID"]])?;
    let mut b = named("VerticalCRS", base)?;
    let (key, datum, models) = vertical_datum(base)?;
    b.insert(key.into(), datum);
    if !models.is_empty() {
        b.insert("deformation_models".into(), Value::Array(models));
    }
    let base_unit = own_unit(base)?
        .filter(|u| u.fits(UnitKind::Linear))
        .unwrap_or_else(Unit::metre);
    b.insert(
        "coordinate_system".into(),
        json!({
            "subtype": "vertical",
            "axis": [axis_json("Gravity-related height", "H", "up", &base_unit)],
        }),
    );
    ids(base, &mut b)?;
    let length = own_unit(node)?
        .filter(|u| u.fits(UnitKind::Linear))
        .unwrap_or_else(Unit::metre);
    let conv = node
        .child(&["DERIVINGCONVERSION"])
        .ok_or_else(|| node.invalid("no DERIVINGCONVERSION"))?;
    let mut o = named("DerivedVerticalCRS", node)?;
    o.insert("base_crs".into(), Value::Object(b));
    o.insert(
        "conversion".into(),
        conversion(conv, &length, &Unit::degree())?,
    );
    o.insert("coordinate_system".into(), coordinate_system(node)?);
    finish(node, &mut o)?;
    Ok(Value::Object(o))
}

fn engineering(node: &Node) -> Result<Value, Error> {
    node.only(&[ENGINEERING_DATUM, UNITS, USAGE, &["CS", "AXIS"]])?;
    let d = node
        .child(ENGINEERING_DATUM)
        .ok_or_else(|| node.invalid("no EDATUM"))?;
    d.only(&[&["ANCHOR", "ID"]])?;
    let mut datum = named("EngineeringDatum", d)?;
    anchors(d, &mut datum)?;
    ids(d, &mut datum)?;
    let mut o = named("EngineeringCRS", node)?;
    o.insert("datum".into(), Value::Object(datum));
    o.insert("coordinate_system".into(), coordinate_system(node)?);
    finish(node, &mut o)?;
    Ok(Value::Object(o))
}

fn compound(node: &Node) -> Result<Value, Error> {
    let mut o = named("CompoundCRS", node)?;
    let mut components = Vec::new();
    for a in &node.args[1..] {
        if let Arg::Node(n) = a
            && !USAGE.contains(&n.keyword.as_str())
        {
            components.push(crs(n)?);
        }
    }
    if components.len() < 2 {
        return Err(node.invalid("fewer than two CRSs"));
    }
    o.insert("components".into(), Value::Array(components));
    finish(node, &mut o)?;
    Ok(Value::Object(o))
}
