//! ESRI's form of WKT1 (`docs/crs-wkt.md`, "ESRI names"), as PROJ 9.8.1
//! reads it (`io.cpp`, `WKTParser`, `esriStyle_`): which WKT is ESRI's,
//! Esri's names of EPSG objects (`xeibe-esri`, behind the `esri` feature),
//! and ESRI projections through PROJ's mapping table
//! ([`super::names::ESRI_METHODS`]).

use std::cell::Cell;

use serde_json::{Value, json};

use super::names::{ESRI_METHODS, EsriMethod, METHODS};
use super::parse::{Arg, Node};
use super::wkt2::{Unit, UnitKind, num};
use crate::error::Error;

/// How sure the reader is that WKT1 is ESRI's: PROJ's `esriStyle_` (`Yes`)
/// and `maybeEsriStyle_` (`Maybe`). Names are looked up in Esri's tables
/// for both; `Maybe` takes an ESRI projection only if most of its
/// parameters are there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Esri {
    No,
    Maybe,
    Yes,
}

/// The reader's state while it reads one WKT1 string.
pub(super) struct Dialect(Cell<Esri>);

impl Dialect {
    /// PROJ's `guessDialect` and the markers that make it sure: a
    /// `VERTCS`, a `GEOGCS` named `GCS_…`, a `DATUM` named `D_…`, or
    /// ESRI's `X_Scale` or `Vertical_Shift` parameters make it ESRI's; no
    /// `AXIS` and no `AUTHORITY` anywhere (outside a `LOCAL_CS`, and
    /// without GDAL's Hotine `rectified_grid_angle`) make it maybe ESRI's.
    pub(super) fn guess(root: &Node) -> Dialect {
        let mut seen = Seen::default();
        seen.walk(root);
        let esri = if root.keyword == "VERTCS" || seen.esri_marker {
            Esri::Yes
        } else if root.keyword != "LOCAL_CS"
            && !seen.axis
            && !seen.authority
            && !seen.rectified_grid_angle
        {
            Esri::Maybe
        } else {
            Esri::No
        };
        Dialect(Cell::new(esri))
    }

    /// A dialect that isn't ESRI's (GDAL's `VERT_CS`).
    pub(super) fn none() -> Dialect {
        Dialect(Cell::new(Esri::No))
    }

    /// ESRI's (its `VERTCS`).
    pub(super) fn esri() -> Dialect {
        Dialect(Cell::new(Esri::Yes))
    }

    pub(super) fn get(&self) -> Esri {
        self.0.get()
    }

    /// Whether Esri's names are looked up.
    pub(super) fn names(&self) -> bool {
        self.get() != Esri::No
    }

    /// A name found only in Esri's tables makes `Maybe` sure, as in PROJ.
    pub(super) fn confirm(&self) {
        if self.get() == Esri::Maybe {
            self.0.set(Esri::Yes);
        }
    }
}

#[derive(Default)]
struct Seen {
    axis: bool,
    authority: bool,
    rectified_grid_angle: bool,
    esri_marker: bool,
}

impl Seen {
    fn walk(&mut self, node: &Node) {
        let name = node.text(0).unwrap_or("");
        match node.keyword.as_str() {
            "AXIS" => self.axis = true,
            "AUTHORITY" => self.authority = true,
            "GEOGCS" if name.starts_with("GCS_") => self.esri_marker = true,
            "DATUM" if name.starts_with("D_") => self.esri_marker = true,
            "PARAMETER" if name == "X_Scale" || name == "Vertical_Shift" => self.esri_marker = true,
            "PARAMETER" if name.eq_ignore_ascii_case("rectified_grid_angle") => {
                self.rectified_grid_angle = true
            }
            _ => {}
        }
        for a in &node.args {
            if let Arg::Node(n) = a {
                self.walk(n);
            }
        }
    }
}

// --- Esri's names ------------------------------------------------------------

/// Which of Esri's tables a name is looked up in.
#[derive(Clone, Copy)]
pub(super) enum Table {
    GeodeticCrs,
    ProjectedCrs,
    VerticalCrs,
    Datum,
    VerticalDatum,
}

/// The EPSG codes Esri gives `name` in `table`; none without the `esri`
/// feature.
#[cfg(feature = "esri")]
pub(super) fn codes(table: Table, name: &str) -> Vec<u32> {
    match table {
        Table::GeodeticCrs => xeibe_esri::geodetic_crs(name).collect(),
        Table::ProjectedCrs => xeibe_esri::projected_crs(name).collect(),
        Table::VerticalCrs => xeibe_esri::vertical_crs(name).collect(),
        Table::Datum => xeibe_esri::datum(name).collect(),
        Table::VerticalDatum => xeibe_esri::vertical_datum(name).collect(),
    }
}

#[cfg(not(feature = "esri"))]
pub(super) fn codes(_table: Table, _name: &str) -> Vec<u32> {
    Vec::new()
}

/// EPSG's name for the CRS Esri calls `name`, preferring a live CRS and,
/// with `dimension`, one with that many axes.
pub(super) fn crs_name(table: Table, name: &str, dimension: Option<u8>) -> Option<&'static str> {
    let records: Vec<_> = codes(table, name)
        .into_iter()
        .filter_map(xeibe_crs::get)
        .filter(|r| dimension.is_none_or(|d| r.dimension == d))
        .collect();
    records
        .iter()
        .find(|r| !r.deprecated)
        .or(records.first())
        .map(|r| r.name)
}

// --- projections -------------------------------------------------------------

/// A parameter of an ESRI projection, by name, ignoring case, or else
/// spaces and underscores: PROJ's WKT1_ESRI writes EPSG's names ("False
/// easting") under an ESRI projection for methods Esri names otherwise.
fn value(params: &[(String, f64)], name: &str) -> Option<f64> {
    let canonical = |n: &str| crate::name::canonical(n, false);
    params
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .or_else(|| params.iter().find(|(n, _)| canonical(n) == canonical(name)))
        .map(|(_, v)| *v)
}

/// Parameter names PROJ takes for one another when a projection's own
/// name is missing (`areEquivalentParameters` in `oputils.cpp`): ESRI's
/// UPS writes `Longitude_Of_Origin` for `Central_Meridian`.
const EQUIVALENT_PARAMETERS: &[&[&str]] = &[
    &["latitude_of_point_1", "Latitude_Of_1st_Point"],
    &["longitude_of_point_1", "Longitude_Of_1st_Point"],
    &["latitude_of_point_2", "Latitude_Of_2nd_Point"],
    &["longitude_of_point_2", "Longitude_Of_2nd_Point"],
    &["satellite_height", "height"],
    &[
        "False easting",
        "Easting at false origin",
        "Easting at projection centre",
    ],
    &[
        "False northing",
        "Northing at false origin",
        "Northing at projection centre",
    ],
    &[
        "Scale factor at natural origin",
        "scale_factor",
        "Scale factor on initial line",
        "Scale factor at projection centre",
        "Scale factor on pseudo standard parallel",
    ],
    &[
        "latitude_of_origin",
        "latitude_of_center",
        "Latitude of natural origin",
        "Latitude of false origin",
        "Latitude of projection centre",
        "Central_Parallel",
    ],
    &[
        "central_meridian",
        "longitude_of_center",
        "Longitude of natural origin",
        "Longitude of false origin",
        "Longitude of projection centre",
        "Longitude of origin",
    ],
    &["Azimuth of initial line", "Azimuth at projection centre"],
    &["pseudo_standard_parallel_1", "standard_parallel_1"],
];

/// A parameter by name, else by a name PROJ takes as the same.
fn value_or_alias(params: &[(String, f64)], name: &str) -> Option<f64> {
    value(params, name).or_else(|| {
        let canonical = |n: &str| crate::name::canonical(n, false);
        let wanted = canonical(name);
        let group = EQUIVALENT_PARAMETERS
            .iter()
            .find(|g| g.iter().any(|n| canonical(n) == wanted))?;
        params
            .iter()
            .find(|(n, _)| {
                let n = canonical(n);
                n != wanted && group.iter().any(|g| canonical(g) == n)
            })
            .map(|(_, v)| *v)
    })
}

/// PROJ's `getESRIMapping`: the entry of this ESRI projection name whose
/// parameters match best. Each parameter present counts one; a fixed one
/// without an EPSG counterpart and with another value rules the entry
/// out. When the WKT is only maybe ESRI's, an entry with as many
/// parameters missing as present is passed over.
fn mapping(projection: &str, params: &[(String, f64)], maybe: bool) -> Option<&'static EsriMethod> {
    let mut best: Option<(&EsriMethod, i32)> = None;
    for m in ESRI_METHODS
        .iter()
        .filter(|m| m.esri.eq_ignore_ascii_case(projection))
    {
        let (mut matched, mut unmatched) = (0, 0);
        let mut ruled_out = false;
        for p in m.params {
            match (value(params, p.esri), p.epsg, p.fixed) {
                (Some(v), 0, Some(fixed)) => {
                    if v == fixed {
                        matched += 1;
                    } else {
                        ruled_out = true;
                        break;
                    }
                }
                (Some(_), ..) => matched += 1,
                (None, _, Some(_)) => {}
                (None, _, None) => unmatched += 1,
            }
        }
        if ruled_out || (maybe && unmatched >= matched) {
            continue;
        }
        if best.is_none_or(|(_, b)| matched > b) {
            best = Some((m, matched));
        }
    }
    best.map(|(m, _)| m)
}

/// EPSG's names of the methods ESRI has that WKT1_GDAL doesn't.
const EXTRA_METHODS: &[(u32, &str)] = &[
    (1051, "Lambert Conic Conformal (2SP Michigan)"),
    (1052, "Colombia Urban"),
    (1078, "Equal Earth"),
    (9838, "Vertical Perspective"),
];

/// EPSG's names of the parameters only those methods have.
const EXTRA_PARAMETERS: &[(u32, &str)] = &[
    (1038, "Ellipsoid scaling factor"),
    (1039, "Projection plane origin height"),
    (8821, "Latitude of false origin"),
    (8822, "Longitude of false origin"),
    (8826, "Easting at false origin"),
    (8827, "Northing at false origin"),
    (8834, "Latitude of topocentric origin"),
    (8835, "Longitude of topocentric origin"),
    (8836, "Ellipsoidal height of topocentric origin"),
    (8840, "Viewing point height"),
];

fn parameter_name(code: u32) -> Option<&'static str> {
    METHODS
        .iter()
        .flat_map(|m| m.params)
        .find(|p| p.epsg == code)
        .map(|p| p.name)
        .or_else(|| {
            EXTRA_PARAMETERS
                .iter()
                .find(|(c, _)| *c == code)
                .map(|(_, n)| *n)
        })
}

/// What an EPSG parameter measures, by its name, as PROJ's
/// `guessUnitForParameter` does.
fn kind(name: &str) -> UnitKind {
    let n = name.to_ascii_lowercase();
    if n.contains("scale") || n.contains("scaling") {
        UnitKind::Scale
    } else if [
        "latitude",
        "longitude",
        "meridian",
        "parallel",
        "azimuth",
        "angle",
    ]
    .iter()
    .any(|w| n.contains(w))
    {
        UnitKind::Angular
    } else {
        UnitKind::Linear
    }
}

/// An ESRI parameter's name, its EPSG code and its default.
type EsriPair = (&'static str, u32, Option<f64>);

/// The conversion of an ESRI `PROJCS`: PROJ's `buildProjectionFromESRI`.
/// `None` when the projection isn't an ESRI one (or, for WKT that is only
/// maybe ESRI's, too few of its parameters are there).
#[allow(clippy::too_many_arguments)]
pub(super) fn conversion(
    projection: &str,
    crs_name: &str,
    params: &[(String, f64)],
    sphere: bool,
    length: &Unit,
    angle: &Unit,
    maybe: bool,
    select: impl Fn(u32, bool) -> u32,
) -> Result<Option<(Value, Option<u32>)>, Error> {
    // ESRI's Stereographic at a pole is EPSG's Polar Stereographic
    // (variant A); elsewhere it is a method EPSG doesn't have.
    let stereographic: &[EsriPair] = &[
        ("False_Easting", 8806, None),
        ("False_Northing", 8807, None),
        ("Central_Meridian", 8802, None),
        ("Scale_Factor", 8805, None),
        ("Latitude_Of_Origin", 8801, None),
    ];
    let (method, pairs): (u32, Vec<EsriPair>) = if projection.eq_ignore_ascii_case("Stereographic")
    {
        match value(params, "Latitude_Of_Origin") {
            Some(l) if l.abs() == 90.0 => (9810, stereographic.to_vec()),
            _ => return Ok(None),
        }
    } else {
        let Some(m) = mapping(projection, params, maybe) else {
            return Ok(None);
        };
        let mut method = m.epsg;
        if projection.eq_ignore_ascii_case("Krovak") && crs_name.contains("_East_North") {
            method = 1041;
        }
        let pairs = m
            .params
            .iter()
            .filter(|p| p.epsg != 0)
            .map(|p| (p.esri, p.epsg, p.fixed))
            .collect();
        (method, pairs)
    };
    let method = select(method, sphere);
    // An EPSG parameter two ESRI ones give (Lambert_Conformal_Conic's
    // Standard_Parallel_1 and Latitude_Of_Origin) takes the later one, as
    // in PROJ; a missing one its default (Behrmann's).
    let mut values: Vec<(u32, f64)> = Vec::new();
    for &(esri, code, fixed) in &pairs {
        // Else under EPSG's name for it, as PROJ's WKT1_ESRI writes
        // parameters Esri names otherwise.
        let epsg_name = || parameter_name(code).and_then(|n| value(params, n));
        if let Some(v) = value_or_alias(params, esri).or_else(epsg_name).or(fixed) {
            values.retain(|(c, _)| *c != code);
            values.push((code, v));
        }
    }
    if method == 1028 && projection.eq_ignore_ascii_case("Plate_Carree") {
        // Equidistant Cylindrical's standard parallel, which Plate Carrée
        // fixes at the equator.
        values.push((8823, 0.0));
    } else if matches!(method, 9812 | 9815)
        && !projection.starts_with("Rectified_Skew_Orthomorphic")
        && let Some(azimuth) = value(params, "Azimuth")
    {
        // ESRI's Hotine has no angle to the skew grid: it is the azimuth.
        values.push((8814, azimuth));
    }
    // EPSG's order: the WKT1 table's, else Esri's.
    let order: Vec<u32> = match METHODS.iter().find(|m| m.epsg == method) {
        Some(m) => m.params.iter().map(|p| p.epsg).collect(),
        None => pairs.iter().map(|&(_, c, _)| c).collect(),
    };
    let method_name = METHODS
        .iter()
        .find(|m| m.epsg == method)
        .map(|m| m.name)
        .or_else(|| {
            EXTRA_METHODS
                .iter()
                .find(|(c, _)| *c == method)
                .map(|(_, n)| *n)
        })
        .unwrap_or("unnamed");
    let mut out = Vec::new();
    for code in order {
        let Some(&(_, v)) = values.iter().find(|(c, _)| *c == code) else {
            continue;
        };
        let name = parameter_name(code).unwrap_or("unnamed");
        let unit = match kind(name) {
            UnitKind::Angular => angle.json.clone(),
            UnitKind::Scale => json!("unity"),
            _ => length.json.clone(),
        };
        out.push(json!({"name": name, "value": num(v), "unit": unit,
            "id": {"authority": "EPSG", "code": code}}));
    }
    Ok(Some((
        json!({
            "name": "unnamed",
            "method": {"name": method_name, "id": {"authority": "EPSG", "code": method}},
            "parameters": out,
        }),
        Some(method),
    )))
}
