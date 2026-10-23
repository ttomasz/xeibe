//! PROJJSON to WKT1 as GDAL writes it (`WKT1_GDAL`), following PROJ
//! 9.8.1's export (`crs.cpp`, `datum.cpp`, `conversion.cpp`):
//!
//! - every object with its `AUTHORITY`; units by their EPSG codes, a CRS's
//!   degrees as 9122;
//! - datum names as GDAL wrote them before GDAL 3: EPSG's with special
//!   characters turned to underscores, and `WGS_1984`;
//! - the prime meridian in degrees;
//! - `AXIS` for a projected CRS only when it is easting, northing, and
//!   never for a geographic one (whose WKT1 default is longitude,
//!   latitude); always for geocentric, vertical and local CRSs;
//! - Web Mercator as `Mercator_1SP` with GDAL's `PROJ4` extension.
//!
//! What WKT1 can't say is an error: a geographic or projected 3D CRS, a
//! derived projected CRS, a method PROJ gives no WKT1 name.

use std::f64::consts::PI;

use serde_json::Value;

use super::AxisRule;
use super::names::METHODS;
use super::parse::{Arg, Node};
use crate::error::Error;
use crate::name;

/// A CRS as WKT1_GDAL, with the axes `axes` asks for: `Always` writes
/// those of geographic and projected CRSs whatever they are, and with
/// `Never` a geographic 3D CRS is written as a 2D one. The caller removes
/// the axes for `Never`.
pub(crate) fn crs(v: &Value, axes: AxisRule) -> Result<Node, Error> {
    let always = axes == AxisRule::Always;
    match str_of(v, "type")? {
        "GeographicCRS" => geographic(v, axes),
        "GeodeticCRS" => geocentric(v),
        "ProjectedCRS" => projected(v, always),
        "VerticalCRS" | "DerivedVerticalCRS" => vertical(v),
        "CompoundCRS" => compound(v, axes),
        "EngineeringCRS" => local(v),
        "BoundCRS" => bound(v, axes),
        other => Err(cannot(v, &format!("a {other}"))),
    }
}

/// A part of a CRS on its own, as PROJ writes it in WKT1_GDAL: a datum
/// (without `TOWGS84`), an ellipsoid, a prime meridian, or a conversion's
/// `PROJECTION` and `PARAMETER` nodes, in degrees and metres.
pub(crate) fn object(v: &Value) -> Result<Vec<Node>, Error> {
    match str_of(v, "type")? {
        "Ellipsoid" => Ok(vec![spheroid(v)?]),
        "PrimeMeridian" => Ok(vec![prime_meridian(v)?]),
        "GeodeticReferenceFrame" | "DynamicGeodeticReferenceFrame" | "DatumEnsemble"
            if v["ellipsoid"].is_object() =>
        {
            let mut out = vec![geodetic_datum(v)?];
            if let Some(pm) = super::write::not_greenwich(v) {
                out.push(prime_meridian(pm)?);
            }
            Ok(out)
        }
        "VerticalReferenceFrame" | "DynamicVerticalReferenceFrame" | "DatumEnsemble" => {
            Ok(vec![vertical_datum(v)])
        }
        "EngineeringDatum" => Ok(vec![local_datum(v)?]),
        "Conversion" => {
            let (args, _) = projection(v, v, PI / 180.0, 1.0)?;
            Ok(args
                .into_iter()
                .filter_map(|a| match a {
                    Arg::Node(n) => Some(n),
                    _ => None,
                })
                .collect())
        }
        other => Err(cannot(v, &format!("a {other} on its own"))),
    }
}

fn cannot(v: &Value, what: &str) -> Error {
    Error::Unsupported {
        crs: v["name"].as_str().unwrap_or("PROJJSON").into(),
        what: format!("{what} in WKT1_GDAL"),
    }
}

fn invalid(what: &str) -> Error {
    Error::Projjson {
        crs: "PROJJSON".into(),
        message: format!("cannot write WKT1: {what}"),
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> Result<&'a str, Error> {
    v[key].as_str().ok_or_else(|| invalid(&format!("no {key}")))
}

fn num_of(v: &Value) -> Result<f64, Error> {
    v.as_f64()
        .ok_or_else(|| invalid("a value that is not a number"))
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

fn authority(authority: &str, code: impl ToString) -> Arg {
    sub(node(
        "AUTHORITY",
        vec![text(authority), text(&code.to_string())],
    ))
}

/// An object's `id` as `AUTHORITY`.
fn id(v: &Value, args: &mut Vec<Arg>) {
    let id = &v["id"];
    if let (Some(a), Some(code)) = (id["authority"].as_str(), id.get("code")) {
        let code = match code {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        args.push(authority(a, code));
    }
}

/// A unit's name and size, and its EPSG code: a CRS's degrees are 9122.
fn unit(u: &Value, default: &str) -> Result<(String, f64, Option<u32>), Error> {
    let (name, factor) = match u {
        Value::String(s) if s == "metre" => ("metre".to_owned(), 1.0),
        Value::String(s) if s == "degree" => ("degree".to_owned(), 0.017_453_292_519_943_3),
        Value::String(s) if s == "unity" => ("unity".to_owned(), 1.0),
        Value::Object(o) => (
            str_of(u, "name")?.to_owned(),
            exact(num_of(&o["conversion_factor"])?),
        ),
        Value::Null => return unit(&Value::String(default.into()), default),
        other => return Err(invalid(&format!("the unit {other}"))),
    };
    let code = u["id"]["code"]
        .as_u64()
        .and_then(|c| u32::try_from(c).ok())
        .or_else(|| {
            if name == "degree" {
                Some(9122)
            } else {
                xeibe_crs::unit_code(&name)
            }
        });
    Ok((name, factor, code))
}

/// An angular unit's exact size for one PROJJSON gives to 15 digits
/// (degree, grad, arc-minute, arc-second), so that converting between
/// units gives PROJ's numbers: 52 grads are 46.8°, not 46.7999999999999°.
fn exact(factor: f64) -> f64 {
    [180.0, 200.0, 10_800.0, 648_000.0]
        .iter()
        .map(|n| PI / n)
        .find(|e| (factor - e).abs() <= 1e-14 * e)
        .unwrap_or(factor)
}

fn unit_node(u: &Value, default: &str) -> Result<(Node, f64), Error> {
    let (name, factor, code) = unit(u, default)?;
    let mut args = vec![text(&name), Arg::Number(factor)];
    if let Some(c) = code {
        args.push(authority("EPSG", c));
    }
    Ok((node("UNIT", args), factor))
}

/// A measure in SI units.
fn si(v: &Value, default: &str) -> Result<f64, Error> {
    match v {
        Value::Object(o) => Ok(num_of(&o["value"])? * unit(&o["unit"], default)?.1),
        other => Ok(num_of(other)? * unit(&Value::Null, default)?.1),
    }
}

/// PROJ's `morphNameToESRI`: special characters to single underscores,
/// dropped at the ends, keeping a trailing "(m)", "(ftUS)", "(E-N)" or
/// "(N-E)".
fn morph(name: &str) -> String {
    for suffix in ["(m)", "(ftUS)", "(E-N)", "(N-E)"] {
        if let Some(stem) = name.strip_suffix(suffix) {
            return morph(stem) + suffix;
        }
    }
    let mut out = String::new();
    let mut gap = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '+' || c == '-' {
            if gap && !out.is_empty() {
                out.push('_');
            }
            out.push(c);
            gap = false;
        } else {
            gap = true;
        }
    }
    out
}

/// A datum's WKT1 name: an ensemble's traditional name, and EPSG's names
/// with underscores, as GDAL 2 wrote them.
fn datum_name(d: &Value) -> Result<String, Error> {
    let written = str_of(d, "name")?;
    let name = match written {
        "World Geodetic System 1984 ensemble" => "World Geodetic System 1984",
        "European Terrestrial Reference System 1989 ensemble" => {
            "European Terrestrial Reference System 1989"
        }
        "Greenland Reference 1996 ensemble" => "Greenland 1996",
        other => other,
    };
    let epsg = d["id"]["authority"] == "EPSG";
    let known = d["id"].is_null()
        && super::datums::by_name(name, xeibe_crs::DatumKind::Geodetic)
            .is_some_and(|found| name::equivalent(name, found.name));
    let mut out = if epsg || known {
        morph(name)
    } else {
        name.to_owned()
    };
    if out == "World_Geodetic_System_1984" {
        out = "WGS_1984".into();
    }
    Ok(out)
}

fn spheroid(e: &Value) -> Result<Node, Error> {
    let (a, rf) = if !e["radius"].is_null() {
        (si(&e["radius"], "metre")?, 0.0)
    } else {
        let a = si(&e["semi_major_axis"], "metre")?;
        let rf = match (&e["inverse_flattening"], &e["semi_minor_axis"]) {
            (rf @ Value::Number(_), _) => num_of(rf)?,
            (_, b) if !b.is_null() => a / (a - si(b, "metre")?),
            _ => return Err(invalid("an ellipsoid without its shape")),
        };
        (a, rf)
    };
    let mut args = vec![text(str_of(e, "name")?), Arg::Number(a), Arg::Number(rf)];
    id(e, &mut args);
    Ok(node("SPHEROID", args))
}

/// `DATUM` and `PRIMEM` of a geodetic CRS.
fn datum_and_meridian(v: &Value) -> Result<Vec<Arg>, Error> {
    let d = if v["datum"].is_object() {
        &v["datum"]
    } else if v["datum_ensemble"].is_object() {
        &v["datum_ensemble"]
    } else {
        return Err(invalid("a geodetic CRS without a datum"));
    };
    let pm = &d["prime_meridian"];
    let meridian = if pm.is_object() {
        prime_meridian(pm)?
    } else {
        node(
            "PRIMEM",
            vec![text("Greenwich"), Arg::Number(0.0), authority("EPSG", 8901)],
        )
    };
    Ok(vec![sub(geodetic_datum(d)?), sub(meridian)])
}

fn geodetic_datum(d: &Value) -> Result<Node, Error> {
    let mut datum = vec![text(&datum_name(d)?), sub(spheroid(&d["ellipsoid"])?)];
    id(d, &mut datum);
    Ok(node("DATUM", datum))
}

/// `PRIMEM` in degrees.
fn prime_meridian(pm: &Value) -> Result<Node, Error> {
    let mut args = vec![
        text(str_of(pm, "name")?),
        Arg::Number(si(&pm["longitude"], "degree")?.to_degrees()),
    ];
    id(pm, &mut args);
    Ok(node("PRIMEM", args))
}

/// A geodetic or projected CRS's name, with " (deprecated)" when EPSG has
/// deprecated it, as PROJ writes WKT1.
fn crs_name(v: &Value) -> Result<String, Error> {
    let name = str_of(v, "name")?;
    let deprecated = (v["id"]["authority"] == "EPSG")
        .then(|| v["id"]["code"].as_u64())
        .flatten()
        .and_then(|c| xeibe_crs::get(u32::try_from(c).ok()?))
        .is_some_and(|r| r.deprecated);
    Ok(if deprecated {
        format!("{name} (deprecated)")
    } else {
        name.to_owned()
    })
}

fn axes(v: &Value) -> Result<&Vec<Value>, Error> {
    v["coordinate_system"]["axis"]
        .as_array()
        .ok_or_else(|| invalid("a CRS without axes"))
}

/// `AXIS[name, DIRECTION]`, WKT1's names and directions, as PROJ writes
/// them: EPSG's "Geodetic latitude" as "Latitude".
fn axis_nodes(list: &[Value]) -> Result<Vec<Arg>, Error> {
    list.iter()
        .map(|a| {
            let direction = match str_of(a, "direction")? {
                "north" | "geocentricZ" => "NORTH",
                "south" => "SOUTH",
                "east" => "EAST",
                "west" => "WEST",
                "up" => "UP",
                "down" => "DOWN",
                _ => "OTHER",
            };
            let name = match str_of(a, "name")? {
                "Geodetic latitude" => "Latitude",
                "Geodetic longitude" => "Longitude",
                other => other,
            };
            Ok(sub(node(
                "AXIS",
                vec![text(name), Arg::Word(direction.into())],
            )))
        })
        .collect()
}

fn geographic(v: &Value, rule: AxisRule) -> Result<Node, Error> {
    let list = axes(v)?;
    if list.len() != 2 && rule != AxisRule::Never {
        return Err(cannot(v, "a geographic 3D CRS"));
    }
    let always = rule == AxisRule::Always;
    let mut args = vec![text(&crs_name(v)?)];
    args.extend(datum_and_meridian(v)?);
    args.push(sub(unit_node(&list[0]["unit"], "degree")?.0));
    if always {
        args.extend(axis_nodes(list)?);
    }
    id(v, &mut args);
    Ok(node("GEOGCS", args))
}

fn geocentric(v: &Value) -> Result<Node, Error> {
    let list = axes(v)?;
    let mut args = vec![text(&crs_name(v)?)];
    args.extend(datum_and_meridian(v)?);
    args.push(sub(unit_node(&list[0]["unit"], "metre")?.0));
    args.extend(axis_nodes(list)?);
    id(v, &mut args);
    Ok(node("GEOCCS", args))
}

/// Methods PROJ maps but gives no WKT1 name: it doesn't write them.
const NO_WKT1: &[u32] = &[
    1102, 1051, 9831, 9833, 1043, 1042, 1078, 9838, 1052, 9836, 9837,
];

fn param_code(p: &Value) -> Option<u32> {
    (p["id"]["authority"] == "EPSG")
        .then(|| p["id"]["code"].as_u64())
        .flatten()
        .and_then(|c| u32::try_from(c).ok())
}

/// A parameter's value in the WKT1 units: angles in the base's angle unit,
/// lengths in the CRS's.
fn wkt1_value(p: &Value, angle: f64, length: f64) -> Result<f64, Error> {
    let v = num_of(&p["value"])?;
    let (kind, factor) = match &p["unit"] {
        Value::String(s) if s == "degree" => ("angle", PI / 180.0),
        Value::String(s) if s == "metre" => ("length", 1.0),
        Value::String(_) | Value::Null => ("scale", 1.0),
        Value::Object(o) => (
            match o.get("type").and_then(Value::as_str) {
                Some("AngularUnit") => "angle",
                Some("LinearUnit") => "length",
                _ => "scale",
            },
            exact(num_of(&o["conversion_factor"])?),
        ),
        _ => ("scale", 1.0),
    };
    Ok(match kind {
        "angle" => v * factor / angle,
        "length" => v * factor / length,
        // A scale in another unit (parts per million) as a plain factor.
        _ => v * factor,
    })
}

fn projected(v: &Value, always: bool) -> Result<Node, Error> {
    let base = &v["base_crs"];
    let base_axes = axes(base)?;
    let angle = unit(&base_axes[0]["unit"], "degree")?.1;
    let list = axes(v)?;
    if list.len() != 2 {
        return Err(cannot(v, "a projected 3D CRS"));
    }
    let (linear, length) = unit_node(&list[0]["unit"], "metre")?;
    let mut args = vec![text(&crs_name(v)?), sub(geographic_base(base)?)];
    let (projection, extension) = projection(v, &v["conversion"], angle, length)?;
    args.extend(projection);
    args.push(sub(linear));
    if always
        || (list.len() == 2 && list[0]["direction"] == "east" && list[1]["direction"] == "north")
    {
        args.extend(axis_nodes(list)?);
    }
    if let Some(e) = extension {
        args.push(sub(node("EXTENSION", vec![text("PROJ4"), text(&e)])));
    }
    id(v, &mut args);
    Ok(node("PROJCS", args))
}

/// A conversion's `PROJECTION` and `PARAMETER` nodes, angles in the unit
/// of `angle` and lengths in that of `length` (radians and metres per
/// unit), and GDAL's `PROJ4` extension for Web Mercator. `v` is what
/// errors name.
fn projection(
    v: &Value,
    conversion: &Value,
    angle: f64,
    length: f64,
) -> Result<(Vec<Arg>, Option<String>), Error> {
    let code = param_code(&conversion["method"]);
    let params: Vec<&Value> = conversion["parameters"]
        .as_array()
        .map(|p| p.iter().collect())
        .unwrap_or_default();
    let value = |c: u32| -> Result<Option<f64>, Error> {
        params
            .iter()
            .find(|p| param_code(p) == Some(c))
            .map(|p| wkt1_value(p, angle, length))
            .transpose()
    };
    let mut args = Vec::new();
    let mut extension = None;
    match code {
        Some(1024) => {
            // GDAL's Web Mercator: Mercator_1SP on the datum's ellipsoid,
            // with the sphere in a PROJ4 extension.
            let lon0 = value(8802)?.unwrap_or(0.0);
            let (x0, y0) = (value(8806)?.unwrap_or(0.0), value(8807)?.unwrap_or(0.0));
            args.push(sub(node("PROJECTION", vec![text("Mercator_1SP")])));
            for (n, val) in [
                ("central_meridian", lon0),
                ("scale_factor", 1.0),
                ("false_easting", x0),
                ("false_northing", y0),
            ] {
                args.push(sub(node("PARAMETER", vec![text(n), Arg::Number(val)])));
            }
            extension = Some(format!(
                "+proj=merc +a=6378137 +b=6378137 +lat_ts=0 +lon_0={} +x_0={} +y_0={} +k=1 +units=m +nadgrids=@null +wktext +no_defs",
                super::write::number(lon0),
                super::write::number(x0),
                super::write::number(y0),
            ));
        }
        Some(9804) if value(8801)?.is_some_and(|l| l != 0.0) => {
            return Err(cannot(v, "Mercator (variant A) with a latitude of origin"));
        }
        Some(c) if NO_WKT1.contains(&c) => {
            let name = conversion["method"]["name"]
                .as_str()
                .unwrap_or("its method");
            return Err(cannot(v, name));
        }
        _ => {
            // Mercator (Spherical) on a sphere is Mercator_1SP, and the
            // deprecated Equidistant Cylindrical methods are Equirectangular.
            let as_code = match code {
                Some(1026) => Some(9804),
                Some(9823 | 9842) => Some(1028),
                other => other,
            };
            let method = as_code.and_then(|c| METHODS.iter().find(|m| m.epsg == c));
            match method {
                Some(m) => {
                    args.push(sub(node("PROJECTION", vec![text(m.wkt1)])));
                    for p in &params {
                        // 9842's longitude is "of false origin"; PROJ writes
                        // it as Equirectangular's central meridian.
                        let as_param = |c: u32| {
                            if code == Some(9842) && c == 8822 {
                                8802
                            } else {
                                c
                            }
                        };
                        let Some(mapped) = param_code(p)
                            .and_then(|c| m.params.iter().find(|q| q.epsg == as_param(c)))
                        else {
                            continue;
                        };
                        let val = wkt1_value(p, angle, length)?;
                        // Equidistant Cylindrical writes its latitude of
                        // origin only when it isn't zero.
                        if matches!(code, Some(1028 | 1029)) && mapped.epsg == 8801 && val == 0.0 {
                            continue;
                        }
                        args.push(sub(node(
                            "PARAMETER",
                            vec![text(mapped.wkt1), Arg::Number(val)],
                        )));
                    }
                }
                // A method WKT1 has no name for: EPSG's names, as PROJ
                // writes them.
                None => {
                    let name = str_of(&conversion["method"], "name")?;
                    args.push(sub(node("PROJECTION", vec![text(&name.replace(' ', "_"))])));
                    for p in &params {
                        args.push(sub(node(
                            "PARAMETER",
                            vec![
                                text(str_of(p, "name")?),
                                Arg::Number(wkt1_value(p, angle, length)?),
                            ],
                        )));
                    }
                }
            }
        }
    }
    Ok((args, extension))
}

/// The `GEOGCS` of a projected CRS: its base, 2D.
fn geographic_base(base: &Value) -> Result<Node, Error> {
    let list = axes(base)?;
    if list.len() != 2 {
        return Err(cannot(base, "a projected CRS on a geographic 3D CRS"));
    }
    let mut args = vec![text(&crs_name(base)?)];
    args.extend(datum_and_meridian(base)?);
    args.push(sub(unit_node(&list[0]["unit"], "degree")?.0));
    id(base, &mut args);
    Ok(node("GEOGCS", args))
}

fn vertical(v: &Value) -> Result<Node, Error> {
    // A derived vertical CRS is its base's datum with its own axis.
    let holder = if v["type"] == "DerivedVerticalCRS" {
        &v["base_crs"]
    } else {
        v
    };
    let d = if holder["datum"].is_object() {
        &holder["datum"]
    } else {
        &holder["datum_ensemble"]
    };
    let list = axes(v)?;
    let mut args = vec![text(str_of(v, "name")?), sub(vertical_datum(d))];
    args.push(sub(unit_node(&list[0]["unit"], "metre")?.0));
    args.extend(axis_nodes(list)?);
    id(v, &mut args);
    Ok(node("VERT_CS", args))
}

fn vertical_datum(d: &Value) -> Node {
    let mut datum = vec![
        text(d["name"].as_str().unwrap_or("unknown")),
        Arg::Number(2005.0),
    ];
    id(d, &mut datum);
    node("VERT_DATUM", datum)
}

fn compound(v: &Value, axes: AxisRule) -> Result<Node, Error> {
    let mut args = vec![text(str_of(v, "name")?)];
    for c in v["components"].as_array().into_iter().flatten() {
        args.push(sub(crs(c, axes)?));
    }
    id(v, &mut args);
    Ok(node("COMPD_CS", args))
}

/// A Bound CRS as its base with `TOWGS84` in its `DATUM`, after the
/// `SPHEROID`, as PROJ writes it: always seven Position Vector values.
fn bound(v: &Value, axes: AxisRule) -> Result<Node, Error> {
    let values = towgs84(&v["transformation"]).ok_or_else(|| {
        cannot(
            v,
            "a bound CRS whose transformation isn't a Helmert one to WGS 84",
        )
    })?;
    let mut base = crs(&v["source_crs"], axes)?;
    let geodetic = if base.keyword == "PROJCS" {
        base.args.iter_mut().find_map(|a| match a {
            Arg::Node(n) if n.keyword == "GEOGCS" => Some(n),
            _ => None,
        })
    } else {
        Some(&mut base)
    };
    let datum = geodetic.and_then(|g| {
        g.args.iter_mut().find_map(|a| match a {
            Arg::Node(n) if n.keyword == "DATUM" => Some(n),
            _ => None,
        })
    });
    let Some(datum) = datum else {
        return Err(cannot(v, "a bound CRS on a CRS without a geodetic datum"));
    };
    let at = datum
        .args
        .iter()
        .position(|a| matches!(a, Arg::Node(n) if n.keyword == "SPHEROID"))
        .map_or(datum.args.len(), |i| i + 1);
    datum.args.insert(
        at,
        sub(node(
            "TOWGS84",
            values.iter().map(|&x| Arg::Number(x)).collect(),
        )),
    );
    Ok(base)
}

/// The value of a transformation's parameter in the unit of `factor`.
fn in_unit(p: &Value, factor: f64) -> Option<f64> {
    let raw = p["value"].as_f64()?;
    let f = match &p["unit"] {
        Value::Object(o) => o.get("conversion_factor")?.as_f64()?,
        _ => 1.0,
    };
    Some(if (f - factor).abs() <= 1e-12 * factor {
        raw
    } else {
        raw * f / factor
    })
}

/// A Helmert transformation's seven parameters as `TOWGS84` takes them:
/// Position Vector, metres, arc-seconds and parts per million (PROJ's
/// `getTOWGS84Parameters`). `None` for any other transformation.
pub(super) fn towgs84(t: &Value) -> Option<[f64; 7]> {
    let method = t["method"]["id"]["code"].as_u64()?;
    let sign = match method {
        9603 | 9606 | 1031 | 1033 | 1035 | 1037 => 1.0,
        9607 | 1032 | 1038 => -1.0,
        _ => return None,
    };
    let param = |code: u32, factor: f64| {
        t["parameters"]
            .as_array()?
            .iter()
            .find(|p| p["id"]["code"] == code)
            .and_then(|p| in_unit(p, factor))
    };
    const ARC_SECOND: f64 = 4.848_136_811_095_36e-6;
    let mut out = [0.0; 7];
    for (i, code) in (8605..=8607).enumerate() {
        out[i] = param(code, 1.0)?;
    }
    for (i, code) in (8608..=8611).enumerate() {
        let factor = if code == 8611 { 1e-6 } else { ARC_SECOND };
        out[3 + i] = param(code, factor).unwrap_or(0.0);
    }
    for r in &mut out[3..6] {
        *r *= sign;
    }
    Some(out)
}

fn local(v: &Value) -> Result<Node, Error> {
    let list = axes(v)?;
    let mut args = vec![text(str_of(v, "name")?), sub(local_datum(&v["datum"])?)];
    args.push(sub(unit_node(&list[0]["unit"], "metre")?.0));
    args.extend(axis_nodes(list)?);
    id(v, &mut args);
    Ok(node("LOCAL_CS", args))
}

fn local_datum(d: &Value) -> Result<Node, Error> {
    Ok(node(
        "LOCAL_DATUM",
        vec![text(str_of(d, "name")?), Arg::Number(32767.0)],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_as_gdal_wrote_them() {
        assert_eq!(
            morph("Nouvelle Triangulation Francaise (Paris)"),
            "Nouvelle_Triangulation_Francaise_Paris"
        );
        assert_eq!(morph("M'poraloko"), "M_poraloko");
        assert_eq!(
            morph("NAD83 / UTM zone 10N (ftUS)"),
            "NAD83_UTM_zone_10N(ftUS)"
        );
    }
}
