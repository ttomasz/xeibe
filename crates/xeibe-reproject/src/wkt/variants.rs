//! WKT2 2015 and the simplified forms, as PROJ writes them: passes over
//! the WKT2 2019 nodes (`docs/crs-wkt.md`, Writing).

use serde_json::Value;

use super::parse::{Arg, Node};

/// WKT2 2019 nodes as WKT2 2015: `GEODCRS` and `BASEGEODCRS` for
/// `GEOGCRS` and `BASEGEOGCRS`, without the base CRS's id; an ensemble as
/// a datum (`DATUM` with its ellipsoid, or `VDATUM`) of its traditional
/// name or its own; no `DYNAMIC` or `ANCHOREPOCH`, which 2015 doesn't have;
/// and the first usage's `SCOPE`, `AREA` and extents in place of the
/// `USAGE` nodes.
pub(crate) fn to_2015(n: &mut Node) {
    if n.keyword == "ENSEMBLE" {
        *n = ensemble_datum(n);
        return;
    }
    match n.keyword.as_str() {
        "GEOGCRS" => n.keyword = "GEODCRS".into(),
        "BASEGEOGCRS" | "BASEGEODCRS" => {
            n.keyword = "BASEGEODCRS".into();
            n.args.retain(|a| !is_node(a, "ID"));
        }
        _ => {}
    }
    let mut first_usage = true;
    let args = std::mem::take(&mut n.args);
    for mut a in args {
        let Arg::Node(c) = &mut a else {
            n.args.push(a);
            continue;
        };
        match c.keyword.as_str() {
            "DYNAMIC" | "ANCHOREPOCH" => {}
            "USAGE" => {
                if std::mem::take(&mut first_usage) {
                    n.args.append(&mut c.args);
                }
            }
            _ => {
                to_2015(c);
                n.args.push(a);
            }
        }
    }
}

/// An ensemble as WKT2 2015 has it: a `DATUM` with its ellipsoid and id,
/// or without an ellipsoid a `VDATUM`.
fn ensemble_datum(e: &Node) -> Node {
    let mut args: Vec<Arg> = Vec::new();
    if let Some(Arg::Text(name)) = e.args.first() {
        args.push(Arg::Text(super::datums::traditional(name).into()));
    }
    let vertical = !e.args.iter().any(|a| is_node(a, "ELLIPSOID"));
    args.extend(
        e.args
            .iter()
            .filter(|a| is_node(a, "ELLIPSOID") || is_node(a, "ID"))
            .cloned(),
    );
    Node {
        keyword: if vertical { "VDATUM" } else { "DATUM" }.into(),
        args,
        offset: e.offset,
    }
}

fn is_node(a: &Arg, keyword: &str) -> bool {
    matches!(a, Arg::Node(c) if c.keyword == keyword)
}

/// WKT2 nodes as PROJ's simplified form (`WKT2_2019_SIMPLIFIED`,
/// `WKT2_2015_SIMPLIFIED`), `v` being the PROJJSON they were written
/// from: ids on the outermost object only, no `ORDER`, no Greenwich
/// `PRIMEM`, no metre on an ellipsoid; a coordinate system's unit once as
/// `UNIT` after its axes when they share it, else each axis's as `UNIT`;
/// a base CRS's angular unit as `UNIT` after its datum; and no unit on a
/// parameter or prime meridian in the unit its axes imply.
pub(crate) fn simplify(n: &mut Node, v: &Value) {
    strip(n);
    crs(n, v);
}

/// The nodes of a part written on its own (`write::object`) in the
/// simplified form, its units in degrees and metres where nothing around
/// it says otherwise: a coordinate system's shared unit after its axes,
/// and a prime meridian with its unit, Greenwich too (where PROJ writes
/// nothing).
pub(crate) fn simplify_object(nodes: &mut Vec<Node>) {
    for n in nodes.iter_mut() {
        strip(n);
        n.args.retain(|a| !is_node(a, "ORDER"));
        match n.keyword.as_str() {
            "ELLIPSOID" => n.args.retain(|a| !is_unit(a, "LENGTHUNIT", "metre")),
            "CONVERSION" => conversion(n, None, None),
            _ => {}
        }
    }
    let mut parent = Node {
        keyword: String::new(),
        args: nodes.drain(..).map(Arg::Node).collect(),
        offset: 0,
    };
    if parent.args.iter().any(|a| is_node(a, "AXIS")) {
        let shared = cs_unit(&parent);
        axes_units(&mut parent, shared);
    }
    nodes.extend(parent.args.into_iter().filter_map(|a| match a {
        Arg::Node(n) => Some(n),
        _ => None,
    }));
}

/// Without ids below the outermost object and without `ORDER`, and an
/// ellipsoid without metres.
fn strip(n: &mut Node) {
    for a in &mut n.args {
        if let Arg::Node(c) = a {
            c.args.retain(|a| !is_node(a, "ID") && !is_node(a, "ORDER"));
            if c.keyword == "ELLIPSOID" {
                c.args.retain(|a| !is_unit(a, "LENGTHUNIT", "metre"));
            }
            strip(c);
        }
    }
    n.args.retain(|a| !is_node(a, "ORDER"));
}

/// A CRS node and those inside it, with their PROJJSON.
fn crs(n: &mut Node, v: &Value) {
    match n.keyword.as_str() {
        "COMPOUNDCRS" => {
            let components = v["components"].as_array().into_iter().flatten();
            for (c, cv) in child_crss(n).zip(components) {
                crs(c, cv);
            }
        }
        "BOUNDCRS" => {
            for a in &mut n.args {
                if let Arg::Node(c) = a {
                    let key = match c.keyword.as_str() {
                        "SOURCECRS" => "source_crs",
                        "TARGETCRS" => "target_crs",
                        _ => continue,
                    };
                    if let Some(Arg::Node(inner)) = c.args.first_mut() {
                        crs(inner, &v[key]);
                    }
                }
            }
        }
        _ => {
            let cs = cs_unit(n);
            let base = &v["base_crs"];
            // A derived projected CRS's base is a projected CRS.
            let geographic_base = if base["base_crs"].is_object() {
                &base["base_crs"]
            } else {
                base
            };
            let angular = match n.args.iter().find(|a| is_base_geographic(a)) {
                Some(_) => unit_of(&geographic_base["coordinate_system"]),
                None => cs.clone(),
            };
            for a in &mut n.args {
                let Arg::Node(c) = a else { continue };
                match c.keyword.as_str() {
                    "BASEGEOGCRS" | "BASEGEODCRS" => base_crs(c, angular.as_ref()),
                    "BASEPROJCRS" => {
                        for b in &mut c.args {
                            if let Arg::Node(bc) = b {
                                match bc.keyword.as_str() {
                                    "BASEGEOGCRS" | "BASEGEODCRS" => base_crs(bc, angular.as_ref()),
                                    "CONVERSION" => conversion(bc, angular.as_ref(), None),
                                    _ => {}
                                }
                            }
                        }
                    }
                    "PRIMEM" => prime_meridian(c, angular.as_ref()),
                    "CONVERSION" | "DERIVINGCONVERSION" => {
                        conversion(c, angular.as_ref(), cs.as_ref());
                    }
                    _ => {}
                }
            }
            axes_units(n, cs);
        }
    }
}

fn child_crss(n: &mut Node) -> impl Iterator<Item = &mut Node> {
    n.args.iter_mut().filter_map(|a| match a {
        Arg::Node(c) if c.keyword.ends_with("CRS") => Some(c),
        _ => None,
    })
}

fn is_base_geographic(a: &Arg) -> bool {
    is_node(a, "BASEGEOGCRS") || is_node(a, "BASEGEODCRS") || is_node(a, "BASEPROJCRS")
}

/// A base geographic CRS: its angular unit after its datum, and its prime
/// meridian unless Greenwich, in degrees without a unit.
fn base_crs(n: &mut Node, angular: Option<&Node>) {
    let degree = degree();
    let unit = angular.unwrap_or(&degree);
    let at = n
        .args
        .iter()
        .position(|a| is_node(a, "PRIMEM"))
        .unwrap_or(n.args.len());
    n.args.insert(at, Arg::Node(as_unit(unit)));
    for a in &mut n.args {
        if let Arg::Node(c) = a
            && c.keyword == "PRIMEM"
        {
            prime_meridian(c, Some(&degree));
        }
    }
    n.args.retain(|a| !is_greenwich(a));
}

/// A prime meridian without its unit when it is `axes`'s; Greenwich goes
/// in `axes_units`.
fn prime_meridian(n: &mut Node, axes: Option<&Node>) {
    n.args
        .retain(|a| !matches!((a, axes), (Arg::Node(u), Some(x)) if same_unit(u, x)));
}

/// A conversion's parameters without the unit of the base CRS's angles,
/// of the CRS's axes (degrees and metres for `None`), or unity, and with
/// any other as `UNIT`.
fn conversion(n: &mut Node, angular: Option<&Node>, linear: Option<&Node>) {
    let degree = degree();
    let angular = angular.unwrap_or(&degree);
    let metre = metre();
    let linear = linear.unwrap_or(&metre);
    for a in &mut n.args {
        if let Arg::Node(p) = a
            && p.keyword == "PARAMETER"
        {
            p.args.retain(|a| match a {
                Arg::Node(u) => match u.keyword.as_str() {
                    "ANGLEUNIT" => !same_unit(u, angular),
                    "LENGTHUNIT" => !same_unit(u, linear),
                    "SCALEUNIT" => !is_unit(a, "SCALEUNIT", "unity"),
                    _ => true,
                },
                _ => true,
            });
            for u in &mut p.args {
                if let Arg::Node(u) = u
                    && is_unit_keyword(&u.keyword)
                {
                    u.keyword = "UNIT".into();
                }
            }
        }
    }
}

/// The axes' units: once after them as `UNIT` when they share one, else
/// each as `UNIT`. A prime meridian at Greenwich goes too.
fn axes_units(n: &mut Node, shared: Option<Node>) {
    n.args.retain(|a| !is_greenwich(a));
    let last_axis = n.args.iter().rposition(|a| is_node(a, "AXIS"));
    for a in &mut n.args {
        if let Arg::Node(axis) = a
            && axis.keyword == "AXIS"
        {
            if shared.is_some() {
                axis.args.retain(|a| !is_unit_node(a));
            } else {
                for u in &mut axis.args {
                    if let Arg::Node(u) = u
                        && is_unit_keyword(&u.keyword)
                    {
                        u.keyword = "UNIT".into();
                    }
                }
            }
        }
    }
    if let (Some(unit), Some(at)) = (shared, last_axis) {
        n.args.insert(at + 1, Arg::Node(as_unit(&unit)));
    }
}

/// The unit all of a CRS node's axes have, if they share one.
fn cs_unit(n: &Node) -> Option<Node> {
    let mut units = n.args.iter().filter_map(|a| match a {
        Arg::Node(axis) if axis.keyword == "AXIS" => Some(axis.args.iter().find_map(|a| match a {
            Arg::Node(u) if is_unit_keyword(&u.keyword) => Some(u),
            _ => None,
        })),
        _ => None,
    });
    let first = units.next()??;
    units
        .all(|u| u.is_some_and(|u| same_unit(u, first)))
        .then(|| first.clone())
}

/// A PROJJSON coordinate system's first axis's unit, as an `ANGLEUNIT`.
fn unit_of(cs: &Value) -> Option<Node> {
    let u = &cs["axis"][0]["unit"];
    let (name, factor) = match u {
        Value::Object(o) => (
            o.get("name").and_then(Value::as_str)?,
            o.get("conversion_factor").and_then(Value::as_f64)?,
        ),
        _ => return Some(degree()),
    };
    Some(Node {
        keyword: "ANGLEUNIT".into(),
        args: vec![Arg::Text(name.into()), Arg::Number(factor)],
        offset: 0,
    })
}

fn degree() -> Node {
    Node {
        keyword: "ANGLEUNIT".into(),
        args: vec![
            Arg::Text("degree".into()),
            Arg::Number(0.017_453_292_519_943_3),
        ],
        offset: 0,
    }
}

fn metre() -> Node {
    Node {
        keyword: "LENGTHUNIT".into(),
        args: vec![Arg::Text("metre".into()), Arg::Number(1.0)],
        offset: 0,
    }
}

fn as_unit(u: &Node) -> Node {
    Node {
        keyword: "UNIT".into(),
        args: u.args.clone(),
        offset: 0,
    }
}

fn is_unit_keyword(k: &str) -> bool {
    k.ends_with("UNIT")
}

fn is_unit_node(a: &Arg) -> bool {
    matches!(a, Arg::Node(u) if is_unit_keyword(&u.keyword))
}

/// The same name and factor (to 1e-12 relative).
fn same_unit(a: &Node, b: &Node) -> bool {
    match (&a.args[..], &b.args[..]) {
        ([Arg::Text(x), Arg::Number(f), ..], [Arg::Text(y), Arg::Number(g), ..]) => {
            x == y && (f - g).abs() <= 1e-12 * f.abs().max(g.abs())
        }
        _ => false,
    }
}

fn is_unit(a: &Arg, keyword: &str, name: &str) -> bool {
    matches!(a, Arg::Node(u) if u.keyword == keyword
        && matches!(u.args.first(), Some(Arg::Text(n)) if n == name))
}

fn is_greenwich(a: &Arg) -> bool {
    matches!(a, Arg::Node(p) if p.keyword == "PRIMEM"
        && matches!(&p.args[..], [Arg::Text(n), Arg::Number(l), ..] if n == "Greenwich" && *l == 0.0))
}
