//! WKT1 (OGC 01-009, and GDAL's form of it) to PROJJSON
//! (`docs/crs-wkt.md`, WKT1), the way PROJ 9.8.1 reads it (`io.cpp`,
//! `WKTParser`):
//!
//! - projection and parameter names through PROJ's WKT1 table
//!   ([`super::names`]), with its special cases (Mercator_1SP with a
//!   latitude of origin, Polar_Stereographic, Lambert Conformal Conic
//!   without a variant, Krovak's axes, Hotine's rectified grid angle);
//! - default axes where `AXIS` is missing: longitude, latitude for a
//!   `GEOGCS`, unless its `AUTHORITY` names an EPSG CRS, whose axes it
//!   takes; easting, northing for a `PROJCS`, or what its method implies
//!   (south-west for Krovak, along meridians for polar projections);
//! - datum names: GDAL's (`WGS_1984`, `Ordnance_Survey_of_Great_Britain_1936`)
//!   are matched to EPSG's datums, which give the datum its id;
//! - prime meridians: GDAL writes Paris in degrees under a grad unit, and
//!   some meridians in sexagesimal.
//!
//! Web Mercator (`Mercator_1SP` on a sphere with GDAL's `PROJ4` extension)
//! is EPSG:3857's method. Any other `PROJ4` extension, and `TOWGS84`, are
//! errors until PROJ strings and bound CRSs are read.
//!
//! ESRI's form ([`super::esri`]) is read here too: its names of CRSs and
//! datums through Esri's tables, which give them EPSG's names and ids; its
//! projections through PROJ's ESRI table; `VERTCS`; and a `GEOGCS` with a
//! `LINUNIT`, which is 3D.

use serde_json::{Value, json};
use xeibe_crs::DatumKind;

use super::datums;
use super::esri::{self, Dialect, Esri, Table};
use super::names::{METHODS, Method, Param};
use super::parse::{Arg, Node};
use super::wkt2::{Obj, Unit, UnitKind, num, unsupported};
use crate::error::Error;
use crate::name;

/// A WKT1 CRS node to its PROJJSON object.
pub(crate) fn crs(node: &Node) -> Result<Value, Error> {
    crs_in(node, &Dialect::guess(node))
}

fn crs_in(node: &Node, cx: &Dialect) -> Result<Value, Error> {
    match node.keyword.as_str() {
        "GEOGCS" => geographic(node, cx).map(|(v, _)| v),
        "GEOCCS" => geocentric(node, cx),
        "PROJCS" => projected(node, cx),
        "VERT_CS" => vertical(node),
        "VERTCS" => esri_vertical(node),
        "COMPD_CS" => compound(node, cx),
        "LOCAL_CS" => local(node),
        other => Err(unsupported(node, &format!("WKT1 {other}"))),
    }
}

// --- shared ----------------------------------------------------------------

/// `AUTHORITY["EPSG","4326"]` as a PROJJSON `id`.
fn authority(node: &Node, out: &mut Obj) -> Result<(), Error> {
    if let Some(a) = node.child(&["AUTHORITY"]) {
        let code = match a.args.get(1) {
            Some(Arg::Text(s)) => s
                .trim()
                .parse::<i64>()
                .map_or_else(|_| json!(s), |n| json!(n)),
            Some(Arg::Number(v)) => num(*v),
            _ => return Err(a.invalid("expected a code as value 2")),
        };
        out.insert(
            "id".into(),
            json!({"authority": a.text(0)?.trim(), "code": code}),
        );
    }
    Ok(())
}

fn epsg_code(v: &Value) -> Option<u32> {
    (v["id"]["authority"] == "EPSG")
        .then(|| v["id"]["code"].as_u64())
        .flatten()
        .and_then(|c| u32::try_from(c).ok())
}

/// A WKT1 `UNIT`, whose kind its place gives.
fn unit(node: &Node, kind: UnitKind) -> Result<Unit, Error> {
    let name = node.text(0)?;
    let factor = node.number(1)?;
    let close = |want: f64| (factor - want).abs() <= 1e-12 * want;
    // ESRI writes Meter and Degree.
    let json = match kind {
        UnitKind::Linear if name.eq_ignore_ascii_case("metre") && factor == 1.0 => json!("metre"),
        UnitKind::Linear if name == "Meter" && factor == 1.0 => json!("metre"),
        UnitKind::Angular
            if name.eq_ignore_ascii_case("degree") && close(std::f64::consts::PI / 180.0) =>
        {
            json!("degree")
        }
        _ => {
            let mut o = Obj::new();
            let type_name = match kind {
                UnitKind::Linear => "LinearUnit",
                UnitKind::Angular => "AngularUnit",
                UnitKind::Scale => "ScaleUnit",
                _ => "Unit",
            };
            o.insert("type".into(), json!(type_name));
            o.insert("name".into(), json!(name));
            o.insert("conversion_factor".into(), num(factor));
            authority(node, &mut o)?;
            Value::Object(o)
        }
    };
    Ok(Unit { kind, json })
}

fn unit_of(node: &Node, kind: UnitKind) -> Result<Option<Unit>, Error> {
    node.child(&["UNIT"]).map(|u| unit(u, kind)).transpose()
}

/// The size of a unit in SI units.
fn factor(u: &Unit) -> f64 {
    match &u.json {
        Value::String(s) if s == "degree" => std::f64::consts::PI / 180.0,
        Value::Object(o) => o["conversion_factor"].as_f64().unwrap_or(1.0),
        _ => 1.0,
    }
}

fn axis_json(name: &str, abbreviation: &str, direction: &str, unit: &Unit) -> Value {
    json!({
        "name": name,
        "abbreviation": abbreviation,
        "direction": direction,
        "unit": unit.json,
    })
}

fn meridian_axis(name: &str, abbr: &str, direction: &str, longitude: f64, unit: &Unit) -> Value {
    let mut a = axis_json(name, abbr, direction, unit);
    a["meridian"] = json!({"longitude": num(longitude)});
    a
}

/// `AXIS[name, direction]` nodes, as PROJ's `buildAxis` reads WKT1:
/// the name capitalized, latitude and longitude abbreviated, `OTHER` and
/// geocentric axes mapped.
fn axes(node: &Node, unit: &Unit, geocentric: bool) -> Result<Vec<Value>, Error> {
    let mut out = Vec::new();
    for a in node.children(&["AXIS"]) {
        a.only(&[&["AUTHORITY"]])?;
        let label = a.text(0)?.trim();
        let mut name = label.to_owned();
        if let Some(first) = name.get(..1) {
            name.replace_range(..1, &first.to_uppercase());
        }
        let mut abbreviation = match name.as_str() {
            "Latitude" => "lat",
            "Longitude" => "lon",
            "Ellipsoidal height" => "h",
            _ => "",
        };
        let word = a.word(1)?;
        let direction = match (
            geocentric,
            name.as_str(),
            word.to_ascii_uppercase().as_str(),
        ) {
            (true, "Geocentric X", _) => {
                abbreviation = "X";
                "geocentricX"
            }
            (true, "Geocentric Y", _) => {
                abbreviation = "Y";
                "geocentricY"
            }
            (true, "Geocentric Z", "NORTH" | "OTHER") => {
                abbreviation = "Z";
                "geocentricZ"
            }
            (_, _, "NORTH") => "north",
            (_, _, "SOUTH") => "south",
            (_, _, "EAST") => "east",
            (_, _, "WEST") => "west",
            (_, _, "UP") => "up",
            (_, _, "DOWN") => "down",
            (_, _, "OTHER" | "UNKNOWN") => "unspecified",
            _ => return Err(a.invalid(&format!("unknown axis direction {word}"))),
        };
        let mut o = axis_json(&name, abbreviation, direction, unit);
        authority(a, o.as_object_mut().expect("an object"))?;
        out.push(o);
    }
    Ok(out)
}

// --- datums ----------------------------------------------------------------

/// `PRIMEM[name, longitude]`, in the `GEOGCS`'s angle unit, with PROJ's
/// corrections; `None` for Greenwich.
fn prime_meridian(node: Option<&Node>, angle: &Unit) -> Result<Option<Value>, Error> {
    let Some(node) = node else {
        return Ok(None);
    };
    node.only(&[&["AUTHORITY", "UNIT"]])?;
    let name = node.text(0)?;
    let mut value = node.number(1)?;
    let mut unit = unit_of(node, UnitKind::Angular)?.unwrap_or_else(|| angle.clone());
    let grad = (factor(&unit) - std::f64::consts::PI / 200.0).abs() < 1e-15;
    if name == "Paris" && (value - 2.337_229_17).abs() < 1e-8 && grad {
        // GDAL writes Paris in degrees whatever the GEOGCS unit is.
        value = 2.596_921_3;
    } else if let Some(dms) = sexagesimal(name, value) {
        value = dms;
        unit = Unit::degree();
    }
    if value == 0.0 && name.eq_ignore_ascii_case("Greenwich") {
        return Ok(None);
    }
    let mut o = Obj::new();
    o.insert("name".into(), json!(name));
    o.insert("longitude".into(), unit.measure(value, "degree"));
    authority(node, &mut o)?;
    Ok(Some(Value::Object(o)))
}

/// PROJ's prime meridians given in degrees, minutes and seconds: written as
/// DD.MMSS or as decimal degrees, either is their decimal degrees.
pub(super) fn sexagesimal(name: &str, value: f64) -> Option<f64> {
    const DMS: &[(&str, f64, f64, f64)] = &[
        ("Lisbon", -9.0, 7.0, 54.862),
        ("Bogota", -74.0, 4.0, 51.3),
        ("Madrid", -3.0, 41.0, 14.55),
        ("Rome", 12.0, 27.0, 8.4),
        ("Bern", 7.0, 26.0, 22.5),
        ("Jakarta", 106.0, 48.0, 27.79),
        ("Ferro", -17.0, 40.0, 0.0),
        ("Brussels", 4.0, 22.0, 4.71),
        ("Stockholm", 18.0, 3.0, 29.8),
        ("Athens", 23.0, 42.0, 58.815),
        ("Oslo", 10.0, 43.0, 22.5),
        ("Paris RGS", 2.0, 20.0, 13.95),
        ("Paris_RGS", 2.0, 20.0, 13.95),
    ];
    let &(_, d, m, s) = DMS.iter().find(|(n, ..)| *n == name)?;
    let sign = if d >= 0.0 { 1.0 } else { -1.0 };
    let packed = sign * (d.abs() + m / 100.0 + s / 10_000.0);
    let degrees = sign * (d.abs() + m / 60.0 + s / 3600.0);
    ((value - packed).abs() < 1e-8 || (value - degrees).abs() < 1e-8).then_some(degrees)
}

fn spheroid(node: &Node) -> Result<(Value, bool), Error> {
    node.only(&[&["AUTHORITY"]])?;
    let a = node.number(1)?;
    let rf = node.number(2)?;
    let mut o = Obj::new();
    o.insert("name".into(), json!(node.text(0)?));
    if rf == 0.0 {
        o.insert("radius".into(), num(a));
    } else {
        o.insert("semi_major_axis".into(), num(a));
        o.insert("inverse_flattening".into(), num(rf));
    }
    authority(node, &mut o)?;
    Ok((Value::Object(o), rf == 0.0))
}

/// A datum's name and id: GDAL's `WGS_1984`, ensembles' names written
/// without "ensemble", and names with underscores matched to EPSG's
/// datums, as PROJ does; ESRI's names through Esri's table; else the EPSG
/// name of its `AUTHORITY`. `meridian` is the prime meridian's name, which
/// tells EPSG's datums of one ESRI name apart (`D_NTF`).
fn datum_identity(
    node: &Node,
    kind: DatumKind,
    meridian: Option<&str>,
    cx: &Dialect,
    out: &mut Obj,
) -> Result<(), Error> {
    let written = node.text(0)?;
    authority(node, out)?;
    if (cx.names() || written.starts_with("D_"))
        && let Some((name, code)) = esri_datum(written, kind, meridian, cx)
    {
        if let Some(code) = code
            && !out.contains_key("id")
        {
            out.insert("id".into(), json!({"authority": "EPSG", "code": code}));
        }
        out.insert("name".into(), json!(name));
        return Ok(());
    }
    let name = match written {
        "WGS_1984" => Some("World Geodetic System 1984".to_owned()),
        "World Geodetic System 1984 ensemble" => Some("World Geodetic System 1984".into()),
        "European Terrestrial Reference System 1989 ensemble" => {
            Some("European Terrestrial Reference System 1989".into())
        }
        "Greenland Reference 1996 ensemble" => Some("Greenland 1996".into()),
        _ => None,
    };
    let found = if written.contains('_') || name.is_some() {
        datums::by_name(name.as_deref().unwrap_or(written), kind)
    } else {
        None
    };
    let official = match (name, found) {
        (_, Some(d)) => {
            if !out.contains_key("id") {
                out.insert("id".into(), json!({"authority": "EPSG", "code": d.code}));
            }
            d.name.to_owned()
        }
        (Some(n), None) => n,
        (None, None) => out
            .get("id")
            .and_then(|_| epsg_code(&Value::Object(out.clone())))
            .and_then(xeibe_crs::datum)
            .map_or_else(|| written.to_owned(), |d| d.name.to_owned()),
    };
    out.insert("name".into(), json!(official));
    Ok(())
}

/// An ESRI datum's EPSG name and code, as PROJ finds them: four names it
/// knows, then Esri's table (preferring the datum named for a prime
/// meridian other than Greenwich, then a live one), then for `D_…` the
/// name without its prefix as an EPSG name.
fn esri_datum(
    written: &str,
    kind: DatumKind,
    meridian: Option<&str>,
    cx: &Dialect,
) -> Option<(String, Option<u32>)> {
    match written {
        "D_WGS_1984" => return Some(("World Geodetic System 1984".into(), Some(6326))),
        "D_ETRS_1989" => {
            return Some((
                "European Terrestrial Reference System 1989".into(),
                Some(6258),
            ));
        }
        "D_unknown" => return Some(("unknown".into(), None)),
        "D_Unknown_based_on_WGS_84_ellipsoid" => {
            return Some(("Unknown based on WGS 84 ellipsoid".into(), None));
        }
        _ => {}
    }
    let table = match kind {
        DatumKind::Vertical => Table::VerticalDatum,
        _ => Table::Datum,
    };
    let found: Vec<_> = esri::codes(table, written)
        .into_iter()
        .filter_map(xeibe_crs::datum)
        .filter(|d| d.kind == kind)
        .collect();
    let with_meridian =
        |d: &&&xeibe_crs::DatumName| meridian.is_some_and(|m| d.name.ends_with(&format!("({m})")));
    let chosen = found
        .iter()
        .find(with_meridian)
        .or_else(|| found.iter().find(|d| !d.deprecated))
        .or(found.first());
    if let Some(d) = chosen {
        if datums::by_name(written, kind).is_none() {
            cx.confirm();
        }
        // Esri's name is the Greenwich datum's: EPSG's datum on another
        // prime meridian is named for it ("Bogota 1975 (Bogota)").
        if let Some(m) = meridian
            && !d.name.ends_with(&format!("({m})"))
            && let Some(on) = datums::by_name(&format!("{} ({m})", d.name), kind)
        {
            return Some((on.name.to_owned(), Some(on.code)));
        }
        return Some((d.name.to_owned(), Some(d.code)));
    }
    let bare = written.strip_prefix("D_")?;
    datums::by_name(bare, kind).map(|d| (d.name.to_owned(), Some(d.code)))
}

/// `DATUM` with the `PRIMEM` beside it, and whether its ellipsoid is a
/// sphere.
fn datum(node: &Node, pm: Option<Value>, cx: &Dialect) -> Result<(Value, bool), Error> {
    if node.child(&["TOWGS84"]).is_some() {
        return Err(unsupported(node, "TOWGS84 (a bound CRS)"));
    }
    node.only(&[&["SPHEROID", "AUTHORITY", "EXTENSION"]])?;
    if let Some(e) = node.child(&["EXTENSION"]) {
        return Err(unsupported(
            e,
            &format!("the datum EXTENSION {}", e.text(0)?),
        ));
    }
    let mut o = Obj::new();
    o.insert("type".into(), json!("GeodeticReferenceFrame"));
    let meridian = pm.as_ref().and_then(|p| p["name"].as_str());
    datum_identity(node, DatumKind::Geodetic, meridian, cx, &mut o)?;
    let s = node
        .child(&["SPHEROID"])
        .ok_or_else(|| node.invalid("no SPHEROID"))?;
    let (ellipsoid, sphere) = spheroid(s)?;
    o.insert("ellipsoid".into(), ellipsoid);
    if let Some(pm) = pm {
        o.insert("prime_meridian".into(), pm);
    }
    // The id goes last, as elsewhere.
    if let Some(id) = o.remove("id") {
        o.insert("id".into(), id);
    }
    Ok((Value::Object(o), sphere))
}

// --- CRSs ------------------------------------------------------------------

/// An object's type and name, without the " (deprecated)" PROJ appends to
/// deprecated CRSs in WKT1.
fn named(type_name: &str, node: &Node) -> Result<Obj, Error> {
    let mut o = Obj::new();
    o.insert("type".into(), json!(type_name));
    let name = node.text(0)?.trim();
    o.insert(
        "name".into(),
        json!(name.strip_suffix(" (deprecated)").unwrap_or(name)),
    );
    Ok(o)
}

/// A `GEOGCS`, and whether its ellipsoid is a sphere.
fn geographic(node: &Node, cx: &Dialect) -> Result<(Value, bool), Error> {
    node.only(&[&[
        "DATUM",
        "PRIMEM",
        "UNIT",
        "LINUNIT",
        "AXIS",
        "AUTHORITY",
        "EXTENSION",
    ]])?;
    if let Some(e) = node.child(&["EXTENSION"]) {
        return Err(unsupported(
            e,
            &format!("the GEOGCS EXTENSION {}", e.text(0)?),
        ));
    }
    let angle = unit_of(node, UnitKind::Angular)?.unwrap_or_else(Unit::degree);
    let pm = prime_meridian(node.child(&["PRIMEM"]), &angle)?;
    let d = node
        .child(&["DATUM"])
        .ok_or_else(|| node.invalid("no DATUM"))?;
    let (datum, sphere) = datum(d, pm, cx)?;
    let mut o = named("GeographicCRS", node)?;
    // ESRI's 3D form: a LINUNIT for the ellipsoidal height.
    let height = node
        .child(&["LINUNIT"])
        .map(|u| unit(u, UnitKind::Linear))
        .transpose()?;
    if let Some(name) = esri_geographic_name(node.text(0)?, height.is_some(), cx) {
        o.insert("name".into(), json!(name));
    }
    o.insert("datum".into(), datum);
    authority(node, &mut o)?;
    let written = axes(node, &angle, false)?;
    let epsg = epsg_code(&Value::Object(o.clone())).and_then(epsg_axes);
    let axes = match (written.is_empty(), epsg) {
        // No AXIS: the axes of the EPSG CRS the AUTHORITY names, in the
        // GEOGCS's unit; WKT1's longitude, latitude without one.
        (true, Some(epsg)) => {
            let altered: Vec<Value> = epsg
                .iter()
                .map(|a| {
                    let mut a = a.clone();
                    if a["unit"] != json!("metre") && !a["unit"].is_null() {
                        a["unit"] = angle.json.clone();
                    }
                    a
                })
                .collect();
            if !same_units(&epsg, &altered) {
                // A unit other than the authority's: PROJ drops the id.
                o.remove("id");
            }
            altered
        }
        (true, None) => {
            let mut lon_lat = vec![
                axis_json("Longitude", "lon", "east", &angle),
                axis_json("Latitude", "lat", "north", &angle),
            ];
            if let Some(h) = &height {
                lon_lat.push(axis_json("Ellipsoidal height", "h", "up", h));
            }
            lon_lat
        }
        (false, Some(epsg)) => {
            // Axes other than the authority's: PROJ drops the id.
            if !same_directions(&written, &epsg) || !same_units(&written, &epsg) {
                o.remove("id");
            }
            written
        }
        (false, None) => written,
    };
    o.insert(
        "coordinate_system".into(),
        json!({"subtype": "ellipsoidal", "axis": axes}),
    );
    // The id goes last.
    if let Some(id) = o.remove("id") {
        o.insert("id".into(), id);
    }
    Ok((Value::Object(o), sphere))
}

/// EPSG's name for an ESRI `GEOGCS` name, as PROJ gives it: `GCS_WGS_1984`
/// and `GCS_unknown` by hand, others through Esri's table.
fn esri_geographic_name(written: &str, three_d: bool, cx: &Dialect) -> Option<&'static str> {
    match written {
        "GCS_WGS_1984" if !three_d => Some("WGS 84"),
        "GCS_unknown" => Some("unknown"),
        _ if cx.names() || written.starts_with("GCS_") => esri::crs_name(
            Table::GeodeticCrs,
            written,
            Some(if three_d { 3 } else { 2 }),
        ),
        _ => None,
    }
}

/// The axes of an EPSG geographic CRS, for a `GEOGCS` that names it.
fn epsg_axes(code: u32) -> Option<Vec<Value>> {
    let doc: Value = serde_json::from_str(xeibe_crs::projjson(code)?).ok()?;
    if doc["type"] != "GeographicCRS" {
        return None;
    }
    doc["coordinate_system"]["axis"].as_array().cloned()
}

fn same_directions(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x["direction"] == y["direction"])
}

fn same_units(a: &[Value], b: &[Value]) -> bool {
    a.iter().zip(b).all(|(x, y)| {
        let f = |u: &Value| match u {
            Value::String(s) if s == "degree" => std::f64::consts::PI / 180.0,
            Value::Object(o) => o["conversion_factor"].as_f64().unwrap_or(f64::NAN),
            _ => 1.0,
        };
        (f(&x["unit"]) - f(&y["unit"])).abs() <= 1e-12 * f(&x["unit"]).abs()
    })
}

fn geocentric(node: &Node, cx: &Dialect) -> Result<Value, Error> {
    node.only(&[&["DATUM", "PRIMEM", "UNIT", "AXIS", "AUTHORITY"]])?;
    let length = unit_of(node, UnitKind::Linear)?.unwrap_or_else(Unit::metre);
    let pm = prime_meridian(node.child(&["PRIMEM"]), &Unit::degree())?;
    let d = node
        .child(&["DATUM"])
        .ok_or_else(|| node.invalid("no DATUM"))?;
    let (datum, _) = datum(d, pm, cx)?;
    let mut o = named("GeodeticCRS", node)?;
    o.insert("datum".into(), datum);
    let mut axes = axes(node, &length, true)?;
    if axes.is_empty() {
        axes = vec![
            axis_json("Geocentric X", "X", "geocentricX", &length),
            axis_json("Geocentric Y", "Y", "geocentricY", &length),
            axis_json("Geocentric Z", "Z", "geocentricZ", &length),
        ];
    }
    o.insert(
        "coordinate_system".into(),
        json!({"subtype": "Cartesian", "axis": axes}),
    );
    authority(node, &mut o)?;
    Ok(Value::Object(o))
}

/// GDAL's `PROJ4` extension for Web Mercator: a sphere of WGS 84's semi-
/// major axis and no datum shift.
fn web_mercator_extension(node: &Node) -> bool {
    let Some(e) = node.child(&["EXTENSION"]) else {
        return false;
    };
    let (Ok(key), Ok(text)) = (e.text(0), e.text(1)) else {
        return false;
    };
    key.eq_ignore_ascii_case("PROJ4")
        && text.contains("+proj=merc")
        && text.contains("+a=6378137")
        && text.contains("+b=6378137")
        && text.contains("+nadgrids=@null")
}

fn parameters(node: &Node) -> Result<Vec<(String, f64)>, Error> {
    node.children(&["PARAMETER"])
        .map(|p| {
            p.only(&[&["AUTHORITY"]])?;
            Ok((p.text(0)?.trim().to_owned(), p.number(1)?))
        })
        .collect()
}

fn find_method(code: u32) -> Option<&'static Method> {
    METHODS.iter().find(|m| m.epsg == code)
}

/// PROJ's `guessUnitForParameter`: what a WKT1 parameter measures, by name.
fn guess_kind(name: &str) -> Option<UnitKind> {
    let n = name.to_ascii_lowercase();
    if n.contains("scale") || n.contains("scaling factor") {
        Some(UnitKind::Scale)
    } else if [
        "latitude",
        "longitude",
        "meridian",
        "parallel",
        "azimuth",
        "angle",
        "heading",
        "rotation",
    ]
    .iter()
    .any(|w| n.contains(w))
    {
        Some(UnitKind::Angular)
    } else if ["easting", "northing", "height"]
        .iter()
        .any(|w| n.contains(w))
    {
        Some(UnitKind::Linear)
    } else {
        None
    }
}

/// The conversion of a `PROJCS`: PROJ's `buildProjectionFromESRI` for
/// ESRI's WKT, else its `buildProjectionStandard`, which falls back to
/// ESRI's projections too.
fn conversion(
    node: &Node,
    projection: &Node,
    sphere: bool,
    length: &Unit,
    angle: &Unit,
    cx: &Dialect,
) -> Result<(Value, Option<u32>), Error> {
    projection.only(&[&["AUTHORITY"]])?;
    let mut wkt1_name = projection.text(0)?.trim().to_owned();
    let mut params = parameters(node)?;
    let crs_name = node.text(0)?;
    let select = |code: u32, sphere: bool| {
        find_method(code)
            .and_then(|m| select_spherical_or_ellipsoidal(m, sphere))
            .map_or(code, |m| m.epsg)
    };
    // As written: the WKT1 path below rewrites names and parameters.
    let (esri_name, esri_params) = (wkt1_name.clone(), params.clone());
    let from_esri = |maybe: bool| {
        esri::conversion(
            &esri_name,
            crs_name,
            &esri_params,
            sphere,
            length,
            angle,
            maybe,
            select,
        )
    };
    if cx.get() != Esri::No
        && let Some(found) = from_esri(cx.get() == Esri::Maybe)?
    {
        return Ok(found);
    }
    let param_value = |params: &[(String, f64)], wanted: &str| {
        params
            .iter()
            .find(|(n, _)| name::equivalent(n, wanted))
            .map(|(_, v)| *v)
    };
    let mut extra: Vec<(u32, &str, f64, Unit)> = Vec::new();
    let mut mercator_2sp_hack = false;
    let mut method: Option<&'static Method> = None;
    if name::equivalent(&wkt1_name, "Mercator_1SP")
        && param_value(&params, "center_latitude").is_none()
    {
        match param_value(&params, "latitude_of_origin") {
            // GDAL ticket 3026: a Mercator_1SP with a latitude of origin is
            // a Mercator_2SP.
            Some(lat0) if lat0 != 0.0 => {
                wkt1_name = "Mercator_2SP".into();
                mercator_2sp_hack = true;
            }
            // GDAL leaves out the latitude of origin, which is zero.
            _ => extra.push((8801, "Latitude of natural origin", 0.0, Unit::degree())),
        }
    } else if name::equivalent(&wkt1_name, "Polar_Stereographic") {
        let lat0 = param_value(&params, "latitude_of_origin");
        let k = param_value(&params, "scale_factor").unwrap_or(1.0);
        let code = match lat0 {
            Some(_) if k == 1.0 => 9829,
            Some(l) if ((l * factor(angle)).to_degrees().abs() - 90.0).abs() < 1e-10 => 9810,
            _ => return Err(unsupported(projection, "this Polar_Stereographic")),
        };
        method = find_method(code);
        if code == 9829 {
            params.retain(|(n, _)| !name::equivalent(n, "scale_factor"));
        }
    } else if node.child(&["EXTENSION"]).is_some() {
        return Err(unsupported(
            node,
            "a PROJ4 EXTENSION other than Web Mercator's",
        ));
    }
    if method.is_none() {
        method = if wkt1_name.to_ascii_lowercase().starts_with("utm zone") {
            find_method(9807)
        } else {
            METHODS
                .iter()
                .find(|m| name::equivalent(m.wkt1, &wkt1_name))
        };
    }
    if let Some(m) = method {
        method = select_spherical_or_ellipsoidal(m, sphere);
    } else if name::equivalent(&wkt1_name, "Lambert Conformal Conic") {
        // Oracle and Trimble name both variants so; the parameters tell.
        let second = param_value(&params, "standard_parallel_2").is_some();
        let k = param_value(&params, "scale_factor").is_some();
        method = match (second, k) {
            (true, false) => find_method(9802),
            (false, true) => find_method(9801),
            (true, true) => find_method(1051),
            (false, false) => None,
        };
    }
    // Krovak with south and west axes is the south-orientated method.
    if wkt1_name.eq_ignore_ascii_case("Krovak") {
        let dirs: Vec<String> = node
            .children(&["AXIS"])
            .filter_map(|a| a.word(1).ok().map(str::to_ascii_uppercase))
            .collect();
        if dirs == ["SOUTH", "WEST"] {
            method = find_method(9819);
        }
    }
    let Some(method) = method else {
        // Not a WKT1 name: maybe ESRI's, as PROJ tries, else an EPSG one.
        if let Some(found) = from_esri(false)? {
            return Ok(found);
        }
        return by_epsg_names(&wkt1_name, &params, length, angle, projection);
    };
    let mut out = Vec::new();
    let mut found: Vec<u32> = Vec::new();
    for (wkt1, value) in &params {
        let mut wkt1 = wkt1.as_str();
        if mercator_2sp_hack {
            if wkt1.eq_ignore_ascii_case("latitude_of_origin") {
                wkt1 = "standard_parallel_1";
            } else if wkt1.eq_ignore_ascii_case("scale_factor") && *value == 1.0 {
                continue;
            }
        }
        let kind = guess_kind(wkt1);
        let unit = match kind {
            Some(UnitKind::Angular) => Some(angle.clone()),
            Some(UnitKind::Linear) => Some(length.clone()),
            Some(UnitKind::Scale) => Some(Unit::unity()),
            _ => None,
        };
        let mapped: Option<&Param> =
            if method.epsg == 9805 && wkt1.eq_ignore_ascii_case("latitude_of_origin") {
                None
            } else {
                method
                    .params
                    .iter()
                    .find(|p| name::equivalent(p.wkt1, wkt1))
            };
        let mut p = Obj::new();
        match mapped {
            Some(m) => {
                found.push(m.epsg);
                p.insert("name".into(), json!(m.name));
                p.insert("value".into(), num(*value));
                if let Some(u) = unit {
                    p.insert("unit".into(), u.json);
                }
                p.insert("id".into(), json!({"authority": "EPSG", "code": m.epsg}));
            }
            // Mercator_2SP's stray latitude of origin is EPSG's latitude of
            // natural origin.
            None if method.epsg == 9805 && wkt1.eq_ignore_ascii_case("latitude_of_origin") => {
                found.push(8801);
                p.insert("name".into(), json!("Latitude of natural origin"));
                p.insert("value".into(), num(*value));
                p.insert("unit".into(), angle.json.clone());
                p.insert("id".into(), json!({"authority": "EPSG", "code": 8801}));
            }
            None => {
                p.insert("name".into(), json!(wkt1));
                p.insert("value".into(), num(*value));
                if let Some(u) = unit {
                    p.insert("unit".into(), u.json);
                }
            }
        }
        out.push(Value::Object(p));
    }
    for (code, name, value, unit) in extra {
        out.push(json!({"name": name, "value": num(value), "unit": unit.json,
            "id": {"authority": "EPSG", "code": code}}));
    }
    // A missing scale factor defaults to 1, as in PROJ.
    if method.params.iter().any(|p| p.epsg == 8805) && !found.contains(&8805) {
        out.push(
            json!({"name": "Scale factor at natural origin", "value": 1, "unit": "unity",
            "id": {"authority": "EPSG", "code": 8805}}),
        );
    }
    // Hotine with an azimuth but no rectified grid angle: the angle is the
    // azimuth.
    if matches!(method.epsg, 9812 | 9815) && found.contains(&8813) && !found.contains(&8814) {
        let azimuth = out
            .iter()
            .find(|p| p["id"]["code"] == 8813)
            .cloned()
            .expect("found");
        let mut angle = azimuth;
        angle["name"] = json!("Angle from Rectified to Skew Grid");
        angle["id"] = json!({"authority": "EPSG", "code": 8814});
        out.push(angle);
    }
    Ok((
        json!({
            "name": "unnamed",
            "method": {"name": method.name, "id": {"authority": "EPSG", "code": method.epsg}},
            "parameters": out,
        }),
        Some(method.epsg),
    ))
}

/// A method WKT1 has no name for, which PROJ writes under its EPSG name
/// with underscores (`Lambert_Conic_Conformal_(West_Orientated)`) and its
/// parameters under theirs; PROJ finds both by name when reading.
fn by_epsg_names(
    wkt1_name: &str,
    params: &[(String, f64)],
    length: &Unit,
    angle: &Unit,
    projection: &Node,
) -> Result<(Value, Option<u32>), Error> {
    let spaced = |n: &str| n.replace('_', " ");
    let Some(code) = xeibe_crs::method_code(&spaced(wkt1_name)) else {
        return Err(unsupported(
            projection,
            &format!("the WKT1 projection {wkt1_name}"),
        ));
    };
    let mut out = Vec::new();
    for (name, value) in params {
        let mut p = Obj::new();
        p.insert("name".into(), json!(spaced(name)));
        p.insert("value".into(), num(*value));
        match guess_kind(name) {
            Some(UnitKind::Angular) => p.insert("unit".into(), angle.json.clone()),
            Some(UnitKind::Linear) => p.insert("unit".into(), length.json.clone()),
            Some(UnitKind::Scale) => p.insert("unit".into(), json!("unity")),
            _ => None,
        };
        if let Some(c) = xeibe_crs::parameter_code(&spaced(name)) {
            p.insert("id".into(), json!({"authority": "EPSG", "code": c}));
        }
        out.push(Value::Object(p));
    }
    Ok((
        json!({
            "name": "unnamed",
            "method": {"name": spaced(wkt1_name), "id": {"authority": "EPSG", "code": code}},
            "parameters": out,
        }),
        Some(code),
    ))
}

/// PROJ's `selectSphericalOrEllipsoidal`: three methods come in a spherical
/// and an ellipsoidal form under one WKT1 name.
fn select_spherical_or_ellipsoidal(m: &'static Method, sphere: bool) -> Option<&'static Method> {
    let pair = match m.epsg {
        9835 | 9834 => (9835, 9834),
        9820 | 1027 => (9820, 1027),
        1028 | 1029 => (1028, 1029),
        _ => return Some(m),
    };
    find_method(if sphere { pair.1 } else { pair.0 }).or(Some(m))
}

fn projected(node: &Node, cx: &Dialect) -> Result<Value, Error> {
    node.only(&[&[
        "GEOGCS",
        "PROJECTION",
        "PARAMETER",
        "UNIT",
        "AXIS",
        "AUTHORITY",
        "EXTENSION",
    ]])?;
    let g = node
        .child(&["GEOGCS"])
        .ok_or_else(|| node.invalid("no GEOGCS"))?;
    let (mut base, sphere) = geographic(g, cx)?;
    let length = unit_of(node, UnitKind::Linear)?.unwrap_or_else(Unit::metre);
    let angle = Unit {
        kind: UnitKind::Angular,
        json: base["coordinate_system"]["axis"][0]["unit"].clone(),
    };
    let name = node.text(0)?.trim();
    let web_mercator = web_mercator_extension(node)
        || name::equivalent(name, "WGS_84_Pseudo_Mercator")
        || name::equivalent(name, "WGS_1984_Web_Mercator");
    let (conversion, method) = if web_mercator {
        // PROJ's createPseudoMercator: on EPSG:4326, whatever the GEOGCS.
        base = serde_json::from_str(xeibe_crs::projjson(4326).unwrap_or("null"))
            .unwrap_or(Value::Null);
        if let Some(b) = base.as_object_mut() {
            for k in ["$schema", "scope", "area", "bbox", "usages"] {
                b.remove(k);
            }
        }
        let p = |code: u32, name: &str, unit: &str| json!({"name": name, "value": 0, "unit": unit, "id": {"authority": "EPSG", "code": code}});
        (
            json!({
                "name": "unnamed",
                "method": {"name": "Popular Visualisation Pseudo Mercator",
                           "id": {"authority": "EPSG", "code": 1024}},
                "parameters": [
                    p(8801, "Latitude of natural origin", "degree"),
                    p(8802, "Longitude of natural origin", "degree"),
                    p(8806, "False easting", "metre"),
                    p(8807, "False northing", "metre"),
                ],
            }),
            Some(1024),
        )
    } else {
        let projection = node
            .child(&["PROJECTION"])
            .ok_or_else(|| node.invalid("no PROJECTION"))?;
        conversion(node, projection, sphere, &length, &angle, cx)?
    };
    let mut axes = axes(node, &length, false)?;
    if axes.is_empty() {
        axes = default_projected_axes(method, &conversion, &length);
    }
    let mut o = named("ProjectedCRS", node)?;
    if cx.names()
        && let Some(official) = esri_projected_name(name, &length)
    {
        o.insert("name".into(), json!(official));
    }
    o.insert("base_crs".into(), base);
    o.insert("conversion".into(), conversion);
    o.insert(
        "coordinate_system".into(),
        json!({"subtype": "Cartesian", "axis": axes}),
    );
    authority(node, &mut o)?;
    Ok(Value::Object(o))
}

/// EPSG's name for an ESRI `PROJCS` name, through Esri's table. A State
/// Plane name in US feet without its `_Feet` is the one with it, as PROJ
/// reads it (PROJ issue 2086).
fn esri_projected_name(written: &str, length: &Unit) -> Option<&'static str> {
    const US_FOOT: f64 = 12.0 / 39.37;
    if written.contains("_FIPS_")
        && !written.contains("_Feet")
        && (factor(length) - US_FOOT).abs() < 1e-10 * US_FOOT
        && let Some(name) = esri::crs_name(Table::ProjectedCrs, &format!("{written}_Feet"), None)
    {
        return Some(name);
    }
    esri::crs_name(Table::ProjectedCrs, written, None)
}

/// A parameter's value in degrees, from the conversion just built.
fn degrees(conversion: &Value, code: u32) -> Option<f64> {
    let p = conversion["parameters"]
        .as_array()?
        .iter()
        .find(|p| p["id"]["code"] == code)?;
    let v = p["value"].as_f64()?;
    let f = match &p["unit"] {
        Value::String(s) if s == "degree" => std::f64::consts::PI / 180.0,
        Value::Object(o) => o["conversion_factor"].as_f64()?,
        _ => return None,
    };
    Some((v * f).to_degrees())
}

/// The axes a `PROJCS` without `AXIS` has, by its method, as in PROJ.
fn default_projected_axes(method: Option<u32>, conversion: &Value, unit: &Unit) -> Vec<Value> {
    let east_north = || {
        vec![
            axis_json("Easting", "E", "east", unit),
            axis_json("Northing", "N", "north", unit),
        ]
    };
    // PROJ gives these meridians absolutely (90°E and 180° or 0°), right
    // only for a longitude of origin of 0 (its own conversions ignore
    // them). Here they turn with the longitude of origin, as in EPSG's
    // polar CRSs, so the axes point east and north of the projection.
    let origin = [8833, 8802]
        .iter()
        .find_map(|&c| degrees(conversion, c))
        .unwrap_or(0.0);
    let north_pole = || {
        vec![
            meridian_axis("Easting", "E", "south", origin + 90.0, unit),
            meridian_axis("Northing", "N", "south", origin + 180.0, unit),
        ]
    };
    let south_pole = || {
        vec![
            meridian_axis("Easting", "E", "north", origin + 90.0, unit),
            meridian_axis("Northing", "N", "north", origin, unit),
        ]
    };
    match method {
        Some(9819) => vec![
            axis_json("Southing", "", "south", unit),
            axis_json("Westing", "", "west", unit),
        ],
        Some(9810 | 9820) => match degrees(conversion, 8801) {
            Some(l) if (l - 90.0).abs() < 1e-10 => north_pole(),
            Some(l) if (l + 90.0).abs() < 1e-10 => south_pole(),
            _ => east_north(),
        },
        Some(9829) => match degrees(conversion, 8832) {
            Some(l) if l > 0.0 => north_pole(),
            Some(l) if l < 0.0 => south_pole(),
            _ => east_north(),
        },
        Some(9808) => vec![
            axis_json("Easting", "Y", "west", unit),
            axis_json("Northing", "X", "south", unit),
        ],
        _ => east_north(),
    }
}

fn vertical(node: &Node) -> Result<Value, Error> {
    node.only(&[&["VERT_DATUM", "UNIT", "AXIS", "AUTHORITY", "EXTENSION"]])?;
    if let Some(e) = node.child(&["EXTENSION"]) {
        return Err(unsupported(
            e,
            &format!("the VERT_CS EXTENSION {}", e.text(0)?),
        ));
    }
    let length = unit_of(node, UnitKind::Linear)?.unwrap_or_else(Unit::metre);
    let d = node
        .child(&["VERT_DATUM"])
        .ok_or_else(|| node.invalid("no VERT_DATUM"))?;
    d.only(&[&["AUTHORITY", "EXTENSION"]])?;
    if let Some(e) = d.child(&["EXTENSION"]) {
        return Err(unsupported(
            e,
            &format!("the VERT_DATUM EXTENSION {}", e.text(0)?),
        ));
    }
    let mut datum = Obj::new();
    datum.insert("type".into(), json!("VerticalReferenceFrame"));
    datum_identity(d, DatumKind::Vertical, None, &Dialect::none(), &mut datum)?;
    if let Some(id) = datum.remove("id") {
        datum.insert("id".into(), id);
    }
    let mut axes = axes(node, &length, false)?;
    if axes.is_empty() {
        axes = vec![axis_json("Gravity-related height", "H", "up", &length)];
    }
    let mut o = named("VerticalCRS", node)?;
    o.insert("datum".into(), Value::Object(datum));
    o.insert(
        "coordinate_system".into(),
        json!({"subtype": "vertical", "axis": axes}),
    );
    authority(node, &mut o)?;
    Ok(Value::Object(o))
}

/// Whether a `GEOGCS` or `PROJCS`, alone or first in a `COMPD_CS`, has no
/// `AXIS`, so its axes are the defaults [`axes`] and
/// [`default_projected_axes`] give rather than stated.
pub(super) fn implicit_axes(node: &Node) -> bool {
    match node.keyword.as_str() {
        "GEOGCS" | "PROJCS" => node.child(&["AXIS"]).is_none(),
        "COMPD_CS" => node
            .args
            .iter()
            .find_map(|a| match a {
                Arg::Node(n) if n.keyword != "AUTHORITY" => Some(implicit_axes(n)),
                _ => None,
            })
            .unwrap_or(false),
        _ => false,
    }
}

fn compound(node: &Node, cx: &Dialect) -> Result<Value, Error> {
    let mut o = named("CompoundCRS", node)?;
    let mut components = Vec::new();
    for a in &node.args[1..] {
        if let Arg::Node(n) = a
            && n.keyword != "AUTHORITY"
        {
            components.push(crs_in(n, cx)?);
        }
    }
    if components.len() < 2 {
        return Err(node.invalid("fewer than two CRSs"));
    }
    // ESRI's PROJCS[…],VERTCS[…]: named from its parts, as PROJ names it.
    if o["name"] == "" {
        let names: Vec<&str> = components
            .iter()
            .map(|c| c["name"].as_str().unwrap_or("unnamed"))
            .collect();
        o.insert("name".into(), json!(names.join(" + ")));
    }
    o.insert("components".into(), Value::Array(components));
    authority(node, &mut o)?;
    Ok(Value::Object(o))
}

/// ESRI's `VERTCS[name, VDATUM[name], PARAMETER["Vertical_Shift", 0],
/// PARAMETER["Direction", 1], UNIT[…]]`: a direction of −1 is a depth.
/// One on a geodetic `DATUM` (an ellipsoidal height) or with a shift isn't
/// read.
fn esri_vertical(node: &Node) -> Result<Value, Error> {
    node.only(&[&["VDATUM", "DATUM", "PARAMETER", "UNIT", "AUTHORITY"]])?;
    if node.child(&["DATUM"]).is_some() {
        return Err(unsupported(
            node,
            "a VERTCS on a geodetic DATUM (ellipsoidal heights)",
        ));
    }
    let params = parameters(node)?;
    let param = |name: &str| {
        params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    };
    if param("Vertical_Shift").is_some_and(|v| v != 0.0) {
        return Err(unsupported(node, "a VERTCS with a Vertical_Shift"));
    }
    let depth = param("Direction") == Some(-1.0);
    let length = unit_of(node, UnitKind::Linear)?.unwrap_or_else(Unit::metre);
    let d = node
        .child(&["VDATUM"])
        .ok_or_else(|| node.invalid("no VDATUM"))?;
    d.only(&[&["AUTHORITY"]])?;
    let mut datum = Obj::new();
    datum.insert("type".into(), json!("VerticalReferenceFrame"));
    datum_identity(d, DatumKind::Vertical, None, &Dialect::esri(), &mut datum)?;
    if let Some(id) = datum.remove("id") {
        datum.insert("id".into(), id);
    }
    let axis = if depth {
        axis_json("Depth", "D", "down", &length)
    } else {
        axis_json("Gravity-related height", "H", "up", &length)
    };
    let mut o = named("VerticalCRS", node)?;
    if let Some(official) = esri::crs_name(Table::VerticalCrs, node.text(0)?, None) {
        o.insert("name".into(), json!(official));
    }
    o.insert("datum".into(), Value::Object(datum));
    o.insert(
        "coordinate_system".into(),
        json!({"subtype": "vertical", "axis": [axis]}),
    );
    authority(node, &mut o)?;
    Ok(Value::Object(o))
}

fn local(node: &Node) -> Result<Value, Error> {
    node.only(&[&["LOCAL_DATUM", "UNIT", "AXIS", "AUTHORITY"]])?;
    let length = unit_of(node, UnitKind::Linear)?.unwrap_or_else(Unit::metre);
    let d = node
        .child(&["LOCAL_DATUM"])
        .ok_or_else(|| node.invalid("no LOCAL_DATUM"))?;
    d.only(&[&["AUTHORITY"]])?;
    let mut datum = Obj::new();
    datum.insert("type".into(), json!("EngineeringDatum"));
    datum.insert("name".into(), json!(d.text(0)?));
    authority(d, &mut datum)?;
    let mut axes = axes(node, &length, false)?;
    if axes.is_empty() {
        axes = vec![
            axis_json("Easting", "E", "east", &length),
            axis_json("Northing", "N", "north", &length),
        ];
    }
    let mut o = named("EngineeringCRS", node)?;
    o.insert("datum".into(), Value::Object(datum));
    o.insert(
        "coordinate_system".into(),
        json!({"subtype": "Cartesian", "axis": axes}),
    );
    authority(node, &mut o)?;
    Ok(Value::Object(o))
}
