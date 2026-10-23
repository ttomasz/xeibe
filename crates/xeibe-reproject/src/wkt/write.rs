//! PROJJSON to WKT2 2019, written the way PROJ writes it
//! (`docs/crs-wkt.md`, Writing).
//!
//! The mapping is the reader's in reverse, plus PROJ's choices where WKT
//! leaves room: ids only on the CRS, its base CRS, methods and parameters;
//! every axis with its own unit and, when there are several, its `ORDER`;
//! axis names in lower case, and left out for easting and northing in
//! their usual places and for geocentric X/Y/Z; numbers with 15
//! significant digits.

use serde_json::Value;

use super::parse::{Arg, Node};
use crate::error::Error;

/// A CRS's PROJJSON as one WKT2 2019 node.
pub(crate) fn crs(v: &Value) -> Result<Node, Error> {
    let mut w = Writer { root: true };
    w.crs(v)
}

/// The text of `node`, on one line or indented as PROJ indents it.
pub(crate) fn format(node: &Node, pretty: bool) -> String {
    let mut out = String::new();
    write_node(node, pretty, 0, &mut out);
    out
}

fn write_node(node: &Node, pretty: bool, depth: usize, out: &mut String) {
    out.push_str(&node.keyword);
    out.push('[');
    let mut first = true;
    // PROJ indents the axes one level deeper than the CS they follow.
    let mut after_cs = false;
    for a in &node.args {
        if !first {
            out.push(',');
        }
        match a {
            Arg::Node(n) => {
                after_cs |= n.keyword == "CS";
                let extra = usize::from(after_cs && n.keyword == "AXIS");
                if pretty {
                    out.push('\n');
                    out.push_str(&"    ".repeat(depth + 1 + extra));
                }
                write_node(n, pretty, depth + 1 + extra, out);
            }
            Arg::Text(s) => {
                out.push('"');
                out.push_str(&s.replace('"', "\"\""));
                out.push('"');
            }
            Arg::Number(v) => out.push_str(&number(*v)),
            Arg::Word(w) => out.push_str(w),
        }
        first = false;
    }
    out.push(']');
}

/// C's `%.15g`, as PROJ formats numbers: 15 significant digits, trailing
/// zeros dropped, an exponent below 1e-4 and from 1e15.
pub(crate) fn number(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let sci = format!("{v:.14e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-5..15).contains(&exp) {
        let mantissa = trim_zeros(mantissa);
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exp.abs());
    }
    let decimals = (14 - exp).max(0) as usize;
    trim_zeros(&format!("{v:.decimals$}")).to_owned()
}

fn trim_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

fn node(keyword: &str, args: Vec<Arg>) -> Node {
    Node {
        keyword: keyword.into(),
        args,
        offset: 0,
    }
}

fn text(s: &str) -> Arg {
    Arg::Text(s.into())
}

fn sub(n: Node) -> Arg {
    Arg::Node(n)
}

struct Writer {
    /// Whether the CRS being written is the outermost, which keeps its id.
    root: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Length,
    Angle,
    Scale,
}

fn invalid(what: &str) -> Error {
    Error::Projjson {
        crs: "PROJJSON".into(),
        message: format!("cannot write WKT: {what}"),
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> Result<&'a str, Error> {
    v[key].as_str().ok_or_else(|| invalid(&format!("no {key}")))
}

fn num_of(v: &Value) -> Result<f64, Error> {
    v.as_f64()
        .ok_or_else(|| invalid("a value that is not a number"))
}

/// `UNIT` nodes: PROJJSON's shorthands, or a named unit with its factor.
fn unit(u: &Value, kind: Kind) -> Result<Node, Error> {
    let keyword = |type_name: &str| match type_name {
        "LinearUnit" => "LENGTHUNIT",
        "AngularUnit" => "ANGLEUNIT",
        "ScaleUnit" => "SCALEUNIT",
        "TimeUnit" => "TIMEUNIT",
        "ParametricUnit" => "PARAMETRICUNIT",
        _ => "UNIT",
    };
    let (kw, name, factor) = match u {
        Value::String(s) if s == "metre" => ("LENGTHUNIT", "metre", 1.0),
        Value::String(s) if s == "degree" => ("ANGLEUNIT", "degree", 0.017_453_292_519_943_3),
        Value::String(s) if s == "unity" => ("SCALEUNIT", "unity", 1.0),
        Value::Object(o) => (
            keyword(o.get("type").and_then(Value::as_str).unwrap_or("Unit")),
            str_of(u, "name")?,
            num_of(&o["conversion_factor"])?,
        ),
        Value::Null => {
            return unit(
                &Value::String(
                    match kind {
                        Kind::Length => "metre",
                        Kind::Angle => "degree",
                        Kind::Scale => "unity",
                    }
                    .into(),
                ),
                kind,
            );
        }
        other => return Err(invalid(&format!("the unit {other}"))),
    };
    Ok(node(kw, vec![text(name), Arg::Number(factor)]))
}

/// A measure: a bare number in the default unit, or `{value, unit}`.
fn measure(v: &Value, kind: Kind) -> Result<(f64, Node), Error> {
    match v {
        Value::Object(o) => Ok((num_of(&o["value"])?, unit(&o["unit"], kind)?)),
        other => Ok((num_of(other)?, unit(&Value::Null, kind)?)),
    }
}

fn id(v: &Value) -> Result<Node, Error> {
    let mut args = vec![text(str_of(v, "authority")?)];
    args.push(match &v["code"] {
        Value::Number(n) => Arg::Number(n.as_f64().unwrap_or_default()),
        Value::String(s) => text(s),
        _ => return Err(invalid("an id without a code")),
    });
    match &v["version"] {
        Value::Number(n) => args.push(Arg::Number(n.as_f64().unwrap_or_default())),
        Value::String(s) => args.push(text(s)),
        _ => {}
    }
    if let Some(c) = v["authority_citation"].as_str() {
        args.push(sub(node("CITATION", vec![text(c)])));
    }
    if let Some(u) = v["uri"].as_str() {
        args.push(sub(node("URI", vec![text(u)])));
    }
    Ok(node("ID", args))
}

fn ids(v: &Value, args: &mut Vec<Arg>) -> Result<(), Error> {
    if v["id"].is_object() {
        args.push(sub(id(&v["id"])?));
    }
    for i in v["ids"].as_array().into_iter().flatten() {
        args.push(sub(id(i)?));
    }
    Ok(())
}

/// One usage's `SCOPE`, `AREA`, `BBOX` and extents, from `v`.
fn usage(v: &Value) -> Result<Vec<Arg>, Error> {
    let mut args = Vec::new();
    if let Some(s) = v["scope"].as_str() {
        args.push(sub(node("SCOPE", vec![text(s)])));
    }
    if let Some(a) = v["area"].as_str() {
        args.push(sub(node("AREA", vec![text(a)])));
    }
    let b = &v["bbox"];
    if b.is_object() {
        let n = |k: &str| num_of(&b[k]).map(Arg::Number);
        args.push(sub(node(
            "BBOX",
            vec![
                n("south_latitude")?,
                n("west_longitude")?,
                n("north_latitude")?,
                n("east_longitude")?,
            ],
        )));
    }
    let e = &v["vertical_extent"];
    if e.is_object() {
        let mut a = vec![
            Arg::Number(num_of(&e["minimum"])?),
            Arg::Number(num_of(&e["maximum"])?),
        ];
        if !e["unit"].is_null() {
            a.push(sub(unit(&e["unit"], Kind::Length)?));
        }
        args.push(sub(node("VERTICALEXTENT", a)));
    }
    let t = &v["temporal_extent"];
    if t.is_object() {
        args.push(sub(node(
            "TIMEEXTENT",
            vec![text(str_of(t, "start")?), text(str_of(t, "end")?)],
        )));
    }
    Ok(args)
}

impl Writer {
    /// The usages, id and remarks that close a CRS. Only the outermost CRS
    /// writes them, as PROJ does.
    fn finish(&self, v: &Value, args: &mut Vec<Arg>) -> Result<(), Error> {
        if !self.root {
            return Ok(());
        }
        match v["usages"].as_array() {
            Some(list) => {
                for u in list {
                    args.push(sub(node("USAGE", usage(u)?)));
                }
            }
            None => {
                let one = usage(v)?;
                if !one.is_empty() {
                    args.push(sub(node("USAGE", one)));
                }
            }
        }
        ids(v, args)?;
        if let Some(r) = v["remarks"].as_str() {
            args.push(sub(node("REMARK", vec![text(r)])));
        }
        Ok(())
    }

    fn crs(&mut self, v: &Value) -> Result<Node, Error> {
        match str_of(v, "type")? {
            "GeographicCRS" => self.geodetic(v, "GEOGCRS"),
            "GeodeticCRS" => self.geodetic(v, "GEODCRS"),
            "ProjectedCRS" => self.projected(v),
            "VerticalCRS" => self.vertical(v),
            "DerivedVerticalCRS" => self.derived_vertical(v),
            "CompoundCRS" => self.compound(v),
            "EngineeringCRS" => self.engineering(v),
            "DerivedProjectedCRS" => self.derived_projected(v),
            "BoundCRS" => bound(v),
            other => Err(Error::Unsupported {
                crs: v["name"].as_str().unwrap_or("PROJJSON").into(),
                what: format!("writing a {other} as WKT"),
            }),
        }
    }

    fn geodetic(&mut self, v: &Value, keyword: &str) -> Result<Node, Error> {
        let mut args = vec![text(str_of(v, "name")?)];
        args.extend(datum_args(v)?);
        args.extend(cs_args(&v["coordinate_system"])?);
        self.finish(v, &mut args)?;
        Ok(node(keyword, args))
    }

    fn projected(&mut self, v: &Value) -> Result<Node, Error> {
        let mut args = vec![text(str_of(v, "name")?)];
        args.push(sub(base_geographic(&v["base_crs"])?));
        args.push(sub(conversion("CONVERSION", &v["conversion"])?));
        args.extend(cs_args(&v["coordinate_system"])?);
        self.finish(v, &mut args)?;
        Ok(node("PROJCRS", args))
    }

    fn derived_projected(&mut self, v: &Value) -> Result<Node, Error> {
        let base = &v["base_crs"];
        let mut b = vec![text(str_of(base, "name")?)];
        b.push(sub(base_geographic(&base["base_crs"])?));
        b.push(sub(conversion("CONVERSION", &base["conversion"])?));
        ids(base, &mut b)?;
        let mut args = vec![text(str_of(v, "name")?), sub(node("BASEPROJCRS", b))];
        args.push(sub(conversion("DERIVINGCONVERSION", &v["conversion"])?));
        args.extend(cs_args(&v["coordinate_system"])?);
        self.finish(v, &mut args)?;
        Ok(node("DERIVEDPROJCRS", args))
    }

    fn vertical(&mut self, v: &Value) -> Result<Node, Error> {
        let mut args = vec![text(str_of(v, "name")?)];
        args.extend(vertical_datum_args(v)?);
        args.extend(cs_args(&v["coordinate_system"])?);
        let models: Vec<&Value> = match (&v["geoid_model"], v["geoid_models"].as_array()) {
            (m @ Value::Object(_), _) => vec![m],
            (_, Some(list)) => list.iter().collect(),
            _ => Vec::new(),
        };
        for m in models {
            let mut a = vec![text(str_of(m, "name")?)];
            ids(m, &mut a)?;
            args.push(sub(node("GEOIDMODEL", a)));
        }
        self.finish(v, &mut args)?;
        Ok(node("VERTCRS", args))
    }

    fn derived_vertical(&mut self, v: &Value) -> Result<Node, Error> {
        let base = &v["base_crs"];
        let mut b = vec![text(str_of(base, "name")?)];
        b.extend(vertical_datum_args(base)?);
        ids(base, &mut b)?;
        let mut args = vec![text(str_of(v, "name")?), sub(node("BASEVERTCRS", b))];
        args.push(sub(conversion("DERIVINGCONVERSION", &v["conversion"])?));
        args.extend(cs_args(&v["coordinate_system"])?);
        self.finish(v, &mut args)?;
        Ok(node("VERTCRS", args))
    }

    fn compound(&mut self, v: &Value) -> Result<Node, Error> {
        let mut args = vec![text(str_of(v, "name")?)];
        let root = std::mem::replace(&mut self.root, false);
        for c in v["components"].as_array().into_iter().flatten() {
            args.push(sub(self.crs(c)?));
        }
        self.root = root;
        self.finish(v, &mut args)?;
        Ok(node("COMPOUNDCRS", args))
    }

    fn engineering(&mut self, v: &Value) -> Result<Node, Error> {
        let d = &v["datum"];
        let mut datum = vec![text(str_of(d, "name")?)];
        if let Some(a) = d["anchor"].as_str() {
            datum.push(sub(node("ANCHOR", vec![text(a)])));
        }
        let mut args = vec![text(str_of(v, "name")?), sub(node("EDATUM", datum))];
        args.extend(cs_args(&v["coordinate_system"])?);
        self.finish(v, &mut args)?;
        Ok(node("ENGCRS", args))
    }
}

fn ellipsoid(e: &Value) -> Result<Node, Error> {
    let (a, u, rf) = if !e["radius"].is_null() {
        let (a, u) = measure(&e["radius"], Kind::Length)?;
        (a, u, 0.0)
    } else {
        let (a, u) = measure(&e["semi_major_axis"], Kind::Length)?;
        let rf = match (&e["inverse_flattening"], &e["semi_minor_axis"]) {
            (rf @ Value::Number(_), _) => num_of(rf)?,
            (_, b) if !b.is_null() => {
                let (b, _) = measure(b, Kind::Length)?;
                a / (a - b)
            }
            _ => return Err(invalid("an ellipsoid without its shape")),
        };
        (a, u, rf)
    };
    Ok(node(
        "ELLIPSOID",
        vec![
            text(str_of(e, "name")?),
            Arg::Number(a),
            Arg::Number(rf),
            sub(u),
        ],
    ))
}

fn ensemble(e: &Value) -> Result<Node, Error> {
    let mut args = vec![text(str_of(e, "name")?)];
    for m in e["members"].as_array().into_iter().flatten() {
        args.push(sub(node("MEMBER", vec![text(str_of(m, "name")?)])));
    }
    if e["ellipsoid"].is_object() {
        args.push(sub(ellipsoid(&e["ellipsoid"])?));
    }
    // PROJJSON keeps the accuracy as text ("2.0"), which PROJ writes as is.
    let accuracy = match &e["accuracy"] {
        Value::String(s) => Arg::Word(s.clone()),
        other => Arg::Number(num_of(other)?),
    };
    args.push(sub(node("ENSEMBLEACCURACY", vec![accuracy])));
    Ok(node("ENSEMBLE", args))
}

fn anchors(d: &Value, args: &mut Vec<Arg>) -> Result<(), Error> {
    if let Some(a) = d["anchor"].as_str() {
        args.push(sub(node("ANCHOR", vec![text(a)])));
    }
    if !d["anchor_epoch"].is_null() {
        args.push(sub(node(
            "ANCHOREPOCH",
            vec![Arg::Number(num_of(&d["anchor_epoch"])?)],
        )));
    }
    Ok(())
}

/// `DYNAMIC[FRAMEEPOCH[…], MODEL[…]]` for a dynamic frame or a CRS with
/// deformation models.
fn dynamic(crs: &Value, datum: &Value) -> Result<Option<Node>, Error> {
    if datum["frame_reference_epoch"].is_null() {
        return Ok(None);
    }
    let mut args = vec![sub(node(
        "FRAMEEPOCH",
        vec![Arg::Number(num_of(&datum["frame_reference_epoch"])?)],
    ))];
    for m in crs["deformation_models"].as_array().into_iter().flatten() {
        let mut a = vec![text(str_of(m, "name")?)];
        ids(m, &mut a)?;
        args.push(sub(node("MODEL", a)));
    }
    Ok(Some(node("DYNAMIC", args)))
}

/// The datum (or ensemble) and prime meridian of a geodetic CRS, with
/// `DYNAMIC` before them for a dynamic frame.
fn datum_args(v: &Value) -> Result<Vec<Arg>, Error> {
    let mut args = Vec::new();
    let d = &v["datum"];
    let pm = if d.is_object() {
        if let Some(n) = dynamic(v, d)? {
            args.push(sub(n));
        }
        let mut a = vec![text(str_of(d, "name")?), sub(ellipsoid(&d["ellipsoid"])?)];
        anchors(d, &mut a)?;
        args.push(sub(node("DATUM", a)));
        &d["prime_meridian"]
    } else if v["datum_ensemble"].is_object() {
        args.push(sub(ensemble(&v["datum_ensemble"])?));
        &Value::Null
    } else {
        return Err(invalid("a geodetic CRS without a datum"));
    };
    let (name, longitude, u) = if pm.is_object() {
        let (l, u) = measure(&pm["longitude"], Kind::Angle)?;
        (str_of(pm, "name")?, l, u)
    } else {
        ("Greenwich", 0.0, unit(&Value::Null, Kind::Angle)?)
    };
    args.push(sub(node(
        "PRIMEM",
        vec![text(name), Arg::Number(longitude), sub(u)],
    )));
    Ok(args)
}

fn vertical_datum_args(v: &Value) -> Result<Vec<Arg>, Error> {
    let mut args = Vec::new();
    let d = &v["datum"];
    if d.is_object() {
        if let Some(n) = dynamic(v, d)? {
            args.push(sub(n));
        }
        let mut a = vec![text(str_of(d, "name")?)];
        anchors(d, &mut a)?;
        args.push(sub(node("VDATUM", a)));
    } else if v["datum_ensemble"].is_object() {
        args.push(sub(ensemble(&v["datum_ensemble"])?));
    } else {
        return Err(invalid("a vertical CRS without a datum"));
    }
    Ok(args)
}

/// `BASEGEOGCRS` (or `BASEGEODCRS`): datum and prime meridian, and the
/// base's id, which PROJ keeps.
fn base_geographic(v: &Value) -> Result<Node, Error> {
    let keyword = if v["type"] == "GeodeticCRS" {
        "BASEGEODCRS"
    } else {
        "BASEGEOGCRS"
    };
    let mut args = vec![text(str_of(v, "name")?)];
    args.extend(datum_args(v)?);
    ids(v, &mut args)?;
    Ok(node(keyword, args))
}

/// What a parameter measures, by its unit, else by its name.
fn parameter_kind(p: &Value) -> Kind {
    match &p["unit"] {
        Value::String(s) if s == "degree" => Kind::Angle,
        Value::String(s) if s == "unity" => Kind::Scale,
        Value::Object(o) => match o.get("type").and_then(Value::as_str) {
            Some("AngularUnit") => Kind::Angle,
            Some("ScaleUnit") => Kind::Scale,
            _ => Kind::Length,
        },
        _ => {
            let n = p["name"].as_str().unwrap_or("").to_ascii_lowercase();
            if n.contains("scale") {
                Kind::Scale
            } else if ["latitude", "longitude", "azimuth", "angle"]
                .iter()
                .any(|w| n.contains(w))
            {
                Kind::Angle
            } else {
                Kind::Length
            }
        }
    }
}

/// `BOUNDCRS[SOURCECRS[…], TARGETCRS[…], ABRIDGEDTRANSFORMATION[…]]`,
/// each CRS written whole. A Helmert transformation's parameters go in the
/// abridged form's fixed units, without a unit: metres, arc-seconds, and
/// the scale difference as a scale factor (PROJ's `convertToAbridged`).
fn bound(v: &Value) -> Result<Node, Error> {
    let source = Writer { root: true }.crs(&v["source_crs"])?;
    let target = Writer { root: true }.crs(&v["target_crs"])?;
    let t = &v["transformation"];
    let m = &t["method"];
    let mut method = vec![text(str_of(m, "name")?)];
    ids(m, &mut method)?;
    let mut args = vec![text(str_of(t, "name")?), sub(node("METHOD", method))];
    const ARC_SECOND: f64 = 4.848_136_811_095_36e-6;
    for p in t["parameters"].as_array().into_iter().flatten() {
        let code = p["id"]["code"].as_u64().unwrap_or(0);
        let mut a = vec![text(str_of(p, "name")?)];
        if let Some(file) = p["value"].as_str() {
            a.push(text(file));
            ids(p, &mut a)?;
            args.push(sub(node("PARAMETERFILE", a)));
            continue;
        }
        let raw = num_of(&p["value"])?;
        let factor = match &p["unit"] {
            Value::Object(o) => o
                .get("conversion_factor")
                .and_then(Value::as_f64)
                .unwrap_or(1.0),
            _ => 1.0,
        };
        let in_unit = |wanted: f64| {
            if (factor - wanted).abs() <= 1e-12 * wanted {
                raw
            } else {
                raw * factor / wanted
            }
        };
        match code {
            8605..=8607 => a.push(Arg::Number(in_unit(1.0))),
            8608..=8610 => a.push(Arg::Number(in_unit(ARC_SECOND))),
            8611 => a.push(Arg::Number(1.0 + in_unit(1e-6) * 1e-6)),
            _ => {
                a.push(Arg::Number(raw));
                a.push(sub(unit(&p["unit"], parameter_kind(p))?));
            }
        }
        ids(p, &mut a)?;
        args.push(sub(node("PARAMETER", a)));
    }
    ids(t, &mut args)?;
    Ok(node(
        "BOUNDCRS",
        vec![
            sub(node("SOURCECRS", vec![sub(source)])),
            sub(node("TARGETCRS", vec![sub(target)])),
            sub(node("ABRIDGEDTRANSFORMATION", args)),
        ],
    ))
}

fn conversion(keyword: &str, c: &Value) -> Result<Node, Error> {
    let m = &c["method"];
    let mut method = vec![text(str_of(m, "name")?)];
    ids(m, &mut method)?;
    let mut args = vec![text(str_of(c, "name")?), sub(node("METHOD", method))];
    for p in c["parameters"].as_array().into_iter().flatten() {
        let mut a = vec![text(str_of(p, "name")?), Arg::Number(num_of(&p["value"])?)];
        a.push(sub(unit(&p["unit"], parameter_kind(p))?));
        ids(p, &mut a)?;
        args.push(sub(node("PARAMETER", a)));
    }
    Ok(node(keyword, args))
}

/// Whether PROJ leaves out an axis's name: Easting (E) as the first axis,
/// Northing (N) as the second, whatever their directions (polar CRSs point
/// both along meridians), and Geocentric X, Y and Z in their places.
fn bare_axis(position: usize, name: &str, abbreviation: &str) -> bool {
    matches!(
        (position, name, abbreviation),
        (0, "Easting", "E")
            | (1, "Northing", "N")
            | (0, "Geocentric X", "X")
            | (1, "Geocentric Y", "Y")
            | (2, "Geocentric Z", "Z")
    )
}

/// `CS[…]` and its axes, each with its own unit.
fn cs_args(cs: &Value) -> Result<Vec<Arg>, Error> {
    let axes = cs["axis"]
        .as_array()
        .ok_or_else(|| invalid("a coordinate system without axes"))?;
    let subtype = str_of(cs, "subtype")?;
    let mut args = vec![sub(node(
        "CS",
        vec![Arg::Word(subtype.into()), Arg::Number(axes.len() as f64)],
    ))];
    for (i, a) in axes.iter().enumerate() {
        let name = str_of(a, "name")?;
        let abbreviation = a["abbreviation"].as_str().unwrap_or("");
        let mut lower = name.to_owned();
        if let Some(first) = lower.get(..1) {
            lower.replace_range(..1, &first.to_lowercase());
        }
        // PROJ writes a geodetic CS's "Latitude" and "Longitude" without
        // their abbreviations.
        let plain = matches!(name, "Latitude" | "Longitude");
        let label = match (
            bare_axis(i, name, abbreviation),
            abbreviation.is_empty() || plain,
        ) {
            (true, _) => format!("({abbreviation})"),
            (false, true) => lower,
            (false, false) => format!("{lower} ({abbreviation})"),
        };
        let direction = str_of(a, "direction")?;
        let mut ax = vec![text(&label), Arg::Word(direction.into())];
        if a["meridian"].is_object() {
            let (l, u) = measure(&a["meridian"]["longitude"], Kind::Angle)?;
            ax.push(sub(node("MERIDIAN", vec![Arg::Number(l), sub(u)])));
        }
        if axes.len() > 1 {
            ax.push(sub(node("ORDER", vec![Arg::Number(i as f64 + 1.0)])));
        }
        let angular =
            subtype == "ellipsoidal" && matches!(direction, "north" | "south" | "east" | "west");
        if !a["unit"].is_null() || subtype != "ordinal" {
            let kind = if angular { Kind::Angle } else { Kind::Length };
            ax.push(sub(unit(&a["unit"], kind)?));
        }
        args.push(sub(node("AXIS", ax)));
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_percent_15g() {
        assert_eq!(number(6378137.0), "6378137");
        assert_eq!(number(0.017_453_292_519_943_295), "0.0174532925199433");
        assert_eq!(number(40.166_666_666_666_67), "40.1666666666667");
        assert_eq!(number(-5_300_000.0), "-5300000");
        assert_eq!(number(1.193845912e-7), "1.193845912e-07");
        assert_eq!(number(0.0001), "0.0001");
        assert_eq!(number(1e15), "1e+15");
        assert_eq!(number(2010.0), "2010");
        assert_eq!(number(-0.0), "0");
    }
}
