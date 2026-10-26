//! The reproject page's work (`docs/web-reproject.md`), in plain Rust so it
//! is tested natively: a CRS's card, a session converting the points, and
//! what each operation is and does.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use serde_json::{Map, Value, json};
use xeibe_crs::{CrsKind, FirstAxis};
use xeibe_reproject::geodesic::Geodesic;
use xeibe_reproject::{
    AxisOrder, Crs, Explanation, GridRequest, Grids, OperationInfo, OperationStep, Outcome,
    PassReason, Selection, StepCrs, StepKind, Transform,
};

pub type Result<T> = std::result::Result<T, String>;

/// A point as the page sends it: x, y and maybe a third value, in the
/// page's axis order.
pub type Point = (f64, f64, Option<f64>);

/// An EPSG code, typed as `2180` or `EPSG:2180`.
pub fn parse_code(text: &str) -> Result<u32> {
    let text = text.trim();
    let digits = match text.get(..5) {
        Some(prefix) if prefix.eq_ignore_ascii_case("EPSG:") => text[5..].trim(),
        _ => text,
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "{text:?} isn't an EPSG code: type a number such as 2180 or EPSG:2180"
        ));
    }
    digits
        .parse()
        .map_err(|_| format!("{text:?} isn't an EPSG code"))
}

/// What the EPSG tables say about the CRS `text` names, for its card:
/// name, kind, datum and ellipsoid, axes, projection, areas of use,
/// remarks; `error` when the code names no CRS or one that can't be
/// converted.
pub fn crs_card(text: &str) -> Value {
    let code = match parse_code(text) {
        Ok(code) => code,
        Err(error) => return json!({ "error": error }),
    };
    let Some(record) = xeibe_crs::get(code) else {
        return json!({
            "code": code,
            "error": format!("EPSG:{code} isn't a CRS in EPSG {}", xeibe_crs::EPSG_VERSION),
        });
    };
    let projjson: Value = xeibe_crs::projjson(code)
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or(Value::Null);
    let error = match Crs::from_epsg(code) {
        Err(error) => Some(error.to_string()),
        Ok(_) if record.kind == CrsKind::Vertical => Some(
            "a vertical CRS on its own can't be converted: a compound CRS gives its heights a position"
                .to_string(),
        ),
        Ok(_) => None,
    };
    json!({
        "code": code,
        "name": record.name,
        "kind": kind_name(record.kind),
        "deprecated": record.deprecated,
        "replacements": xeibe_crs::replacements(code)
            .map(|c| json!({ "code": c, "name": xeibe_crs::get(c).map(|r| r.name) }))
            .collect::<Vec<_>>(),
        "first_axis_north": record.first_axis == FirstAxis::NorthOrLat,
        "geographic": record.kind.is_geographic(),
        "dimension": record.dimension,
        "datum": datum_json(&projjson),
        "vertical_datum": vertical_datum(&projjson),
        "axes": axes(&projjson),
        "conversion": conversion(&projjson),
        "area_wgs84": record.area_wgs84,
        "usages": xeibe_crs::usages(code)
            .iter()
            .map(|u| json!({ "scope": u.scope, "area": u.area, "bbox": u.bbox }))
            .collect::<Vec<_>>(),
        "remarks": xeibe_crs::crs_remarks(code),
        "error": error,
    })
}

fn kind_name(kind: CrsKind) -> &'static str {
    match kind {
        CrsKind::Geographic2d => "geographic 2D",
        CrsKind::Geographic3d => "geographic 3D",
        CrsKind::Projected => "projected",
        CrsKind::Geocentric => "geocentric",
        CrsKind::Vertical => "vertical",
        CrsKind::Compound => "compound",
        CrsKind::Engineering => "engineering",
        CrsKind::Derived => "derived",
    }
}

/// A unit as PROJJSON writes it: a name, or an object with one.
fn unit_name(unit: &Value) -> Value {
    match unit {
        Value::String(_) => unit.clone(),
        Value::Object(o) => o.get("name").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// A number as PROJJSON writes it: plain, or `{"value": …, "unit": …}`.
fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value["value"].as_f64())
}

/// The axes in the CRS's own order, a compound CRS's horizontal then
/// vertical.
fn axes(json: &Value) -> Vec<Value> {
    if json["type"] == "CompoundCRS" {
        return json["components"]
            .as_array()
            .map(|c| c.iter().flat_map(axes).collect())
            .unwrap_or_default();
    }
    json["coordinate_system"]["axis"]
        .as_array()
        .map(|axes| {
            axes.iter()
                .map(|a| {
                    json!({
                        "name": a["name"],
                        "abbreviation": a["abbreviation"],
                        "direction": a["direction"],
                        "unit": unit_name(&a["unit"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The part of a CRS that has the geodetic datum: itself, a projected or
/// derived CRS's base, a compound CRS's horizontal part's.
fn geodetic(json: &Value) -> Option<&Value> {
    if json.get("datum").is_some() || json.get("datum_ensemble").is_some() {
        if json["type"] != "VerticalCRS" {
            return Some(json);
        }
        return None;
    }
    match json["type"].as_str()? {
        "CompoundCRS" => geodetic(json["components"].get(0)?),
        _ => geodetic(json.get("base_crs")?),
    }
}

fn datum_json(json: &Value) -> Value {
    let Some(geodetic) = geodetic(json) else {
        return Value::Null;
    };
    let (datum, ensemble) = match (geodetic.get("datum"), geodetic.get("datum_ensemble")) {
        (Some(d), _) => (d, false),
        (None, Some(e)) => (e, true),
        (None, None) => return Value::Null,
    };
    let ellipsoid = &datum["ellipsoid"];
    let prime_meridian = datum
        .get("prime_meridian")
        .or_else(|| geodetic.get("prime_meridian"));
    json!({
        "name": datum["name"],
        "code": datum["id"]["code"],
        "ensemble": ensemble,
        "members": datum["members"].as_array().map(Vec::len),
        "accuracy": datum["accuracy"],
        "ellipsoid": {
            "name": ellipsoid["name"],
            "code": ellipsoid["id"]["code"],
            "a": ellipsoid.get("semi_major_axis").or_else(|| ellipsoid.get("radius")).and_then(number),
            "inverse_flattening": ellipsoid.get("inverse_flattening").and_then(number),
            "b": ellipsoid.get("semi_minor_axis").and_then(number),
        },
        "prime_meridian": prime_meridian.map(|p| json!({
            "name": p["name"],
            "longitude": number(&p["longitude"]),
        })),
    })
}

fn vertical_datum(json: &Value) -> Value {
    let vertical = match json["type"].as_str() {
        Some("VerticalCRS") => json,
        Some("CompoundCRS") => match json["components"].get(1) {
            Some(v) => v,
            None => return Value::Null,
        },
        _ => return Value::Null,
    };
    let datum = vertical
        .get("datum")
        .or_else(|| vertical.get("datum_ensemble"));
    datum.map_or(
        Value::Null,
        |d| json!({ "name": d["name"], "code": d["id"]["code"] }),
    )
}

/// A projected CRS's conversion: its method and parameters, as EPSG
/// defines them (with their units).
fn conversion(json: &Value) -> Value {
    let projected = match json["type"].as_str() {
        Some("CompoundCRS") => match json["components"].get(0) {
            Some(h) => h,
            None => return Value::Null,
        },
        _ => json,
    };
    let Some(conversion) = projected.get("conversion") else {
        return Value::Null;
    };
    json!({
        "name": conversion["name"],
        "method": {
            "name": conversion["method"]["name"],
            "code": conversion["method"]["id"]["code"],
        },
        "parameters": conversion["parameters"]
            .as_array()
            .map(|params| params.iter().map(|p| json!({
                "name": p["name"],
                "code": p["id"]["code"],
                "value": p["value"],
                "unit": unit_name(&p["unit"]),
            })).collect::<Vec<_>>()),
    })
}

/// The geographic CRS on a CRS's datum, as PROJJSON: itself for a
/// geographic CRS, a projected or derived CRS's base (theirs in turn), a
/// compound CRS's horizontal part's, and for a geocentric CRS one made on
/// its datum.
fn geographic_json(json: &Value) -> Option<Value> {
    match json["type"].as_str()? {
        "GeographicCRS" => Some(json.clone()),
        "CompoundCRS" => geographic_json(json["components"].get(0)?),
        "GeodeticCRS" => {
            let mut out = Map::new();
            out.insert("type".into(), json!("GeographicCRS"));
            out.insert("name".into(), json["name"].clone());
            for key in ["datum", "datum_ensemble"] {
                if let Some(datum) = json.get(key) {
                    out.insert(key.into(), datum.clone());
                }
            }
            out.insert(
                "coordinate_system".into(),
                json!({
                    "subtype": "ellipsoidal",
                    "axis": [
                        {"name": "Geodetic latitude", "abbreviation": "Lat", "direction": "north", "unit": "degree"},
                        {"name": "Geodetic longitude", "abbreviation": "Lon", "direction": "east", "unit": "degree"},
                    ],
                }),
            );
            Some(Value::Object(out))
        }
        _ => {
            let mut base = json.get("base_crs")?.clone();
            base.as_object_mut()?
                .entry("type")
                .or_insert_with(|| json!("GeographicCRS"));
            geographic_json(&base)
        }
    }
}

/// Semi-major axis and flattening of a geographic CRS's ellipsoid.
fn ellipsoid(geographic: &Value) -> Option<(f64, f64)> {
    let e = geographic
        .get("datum")
        .or_else(|| geographic.get("datum_ensemble"))?
        .get("ellipsoid")?;
    if let Some(radius) = e.get("radius").and_then(number) {
        return Some((radius, 0.0));
    }
    let a = number(e.get("semi_major_axis")?)?;
    let f = match e.get("inverse_flattening").and_then(number) {
        Some(inverse) if inverse != 0.0 => 1.0 / inverse,
        Some(_) => 0.0,
        None => (a - number(e.get("semi_minor_axis")?)?) / a,
    };
    Some((a, f))
}

/// A CRS's points as longitude and latitude on its own datum, for the
/// areas of use and the distances between results.
struct Geo {
    /// The page's axis order has the first two values the other way round
    /// from east first.
    swap: bool,
    /// To the geographic CRS on the same datum: a conversion, exact.
    lonlat: Transform,
    geodesic: Geodesic,
}

impl Geo {
    fn new(crs: &Crs, code: u32, order: AxisOrder) -> Option<Geo> {
        let json: Value = serde_json::from_str(xeibe_crs::projjson(code)?).ok()?;
        let geographic = geographic_json(&json)?;
        let (a, f) = ellipsoid(&geographic)?;
        let target = Crs::from_projjson(&geographic.to_string()).ok()?;
        let lonlat = Transform::new(crs, &target, AxisOrder::EastFirst).ok()?;
        let swap = order == AxisOrder::Authority
            && xeibe_crs::get(code).is_some_and(|r| r.first_axis == FirstAxis::NorthOrLat);
        Some(Geo {
            swap,
            lonlat,
            geodesic: Geodesic::new(a, f),
        })
    }

    fn lonlat(&self, (x, y, z): Point) -> Option<[f64; 2]> {
        let (x, y) = if self.swap { (y, x) } else { (x, y) };
        let (lon, lat, _) = self.lonlat.convert(x, y, z).ok()?;
        (lon.is_finite() && lat.is_finite()).then_some([lon, lat])
    }

    /// In metres, on the ellipsoid.
    fn distance(&self, a: [f64; 2], b: [f64; 2]) -> f64 {
        self.geodesic.inverse(a[1], a[0], b[1], b[0]).0
    }
}

/// Whether a box in degrees (west, south, east, north; east < west across
/// the antimeridian) holds a point.
fn inside(area: [f64; 4], [lon, lat]: [f64; 2]) -> bool {
    let [west, south, east, north] = area;
    let lon = (lon + 180.0).rem_euclid(360.0) - 180.0;
    let in_lon = if west <= east {
        (west..=east).contains(&lon)
    } else {
        lon >= west || lon <= east
    };
    in_lon && (south..=north).contains(&lat)
}

/// A CRS record's area of use as west, south, east, north.
fn record_area(code: u32) -> Option<[f64; 4]> {
    let [south, west, north, east] = xeibe_crs::get(code)?.area_wgs84?.map(f64::from);
    Some([west, south, east, north])
}

/// What the page lets the user choose about the conversion.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// Easting or longitude first, instead of the CRSs' own order.
    pub east_first: bool,
    /// West, south, east, north in degrees.
    pub area_of_interest: Option<[f64; 4]>,
    /// In metres.
    pub accuracy: Option<f64>,
    /// One candidate for every point, by its index.
    pub operation: Option<usize>,
}

impl Options {
    /// From the page's JSON: `{"east_first": …, "area_of_interest": [w, s,
    /// e, n], "accuracy": …, "operation": …}`, any of them left out.
    pub fn from_json(text: &str) -> Result<Options> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let area_of_interest = match v["area_of_interest"].as_array() {
            Some(a) => {
                let numbers: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
                Some(
                    numbers
                        .try_into()
                        .map_err(|_| "the area of interest takes four numbers".to_string())?,
                )
            }
            None => None,
        };
        Ok(Options {
            east_first: v["east_first"].as_bool().unwrap_or(false),
            area_of_interest,
            accuracy: v["accuracy"].as_f64(),
            operation: v["operation"].as_u64().map(|i| i as usize),
        })
    }

    fn order(&self) -> AxisOrder {
        if self.east_first {
            AxisOrder::EastFirst
        } else {
            AxisOrder::Authority
        }
    }
}

/// The points as the page sends them: JSON, one entry per line, `null` for
/// a blank or invalid line, else `[x, y]` or `[x, y, z]`.
pub fn parse_points(text: &str) -> Result<Vec<Option<Point>>> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let lines = v.as_array().ok_or("the points are a JSON array")?;
    lines
        .iter()
        .map(|line| match line {
            Value::Null => Ok(None),
            Value::Array(values) => {
                let n: Vec<f64> = values.iter().filter_map(Value::as_f64).collect();
                match n[..] {
                    [x, y] => Ok(Some((x, y, None))),
                    [x, y, z] => Ok(Some((x, y, Some(z)))),
                    _ => Err(format!("a point has two or three numbers, not {line}")),
                }
            }
            _ => Err(format!("not a point: {line}")),
        })
        .collect()
}

/// Longitude and latitude bounds (west, south, east, north) of points in
/// the CRS `src`, for "the points' bounds" as the area of interest.
pub fn lonlat_bounds(src: u32, east_first: bool, points: &[Option<Point>]) -> Option<[f64; 4]> {
    let crs = Crs::from_epsg(src).ok()?;
    let options = Options {
        east_first,
        ..Options::default()
    };
    let geo = Geo::new(&crs, src, options.order())?;
    let mut bounds: Option<[f64; 4]> = None;
    for p in points.iter().flatten() {
        let Some([lon, lat]) = geo.lonlat(*p) else {
            continue;
        };
        let b = bounds.get_or_insert([lon, lat, lon, lat]);
        *b = [b[0].min(lon), b[1].min(lat), b[2].max(lon), b[3].max(lat)];
    }
    bounds
}

/// One conversion between two EPSG CRSs, with what the page shows about
/// it.
pub struct Session {
    src: u32,
    dst: u32,
    /// The automatic choice; the page lists its candidates.
    all: Transform,
    /// What converts the points: `all`, or one candidate forced.
    used: Transform,
    forced: Option<usize>,
    infos: Vec<OperationInfo>,
    inverse: OnceCell<Result<Transform>>,
    /// Through each candidate alone, made when first asked for.
    each: Vec<OnceCell<Result<Transform>>>,
    src_geo: Option<Geo>,
    dst_geo: Option<Geo>,
    /// For a geographic source CRS: whether the page's points have
    /// latitude first.
    src_geographic: Option<bool>,
    /// Grid files that couldn't be fetched, and why.
    failed: RefCell<BTreeMap<String, String>>,
}

impl Session {
    pub fn new(src: &str, dst: &str, options: &Options) -> Result<Session> {
        let (src, dst) = (parse_code(src)?, parse_code(dst)?);
        let src_crs = Crs::from_epsg(src).map_err(|e| e.to_string())?;
        let dst_crs = Crs::from_epsg(dst).map_err(|e| e.to_string())?;
        let order = options.order();
        let selection = Selection {
            area_of_interest: options.area_of_interest,
            accuracy: options.accuracy,
        };
        let all = Transform::with_selection(&src_crs, &dst_crs, order, Grids::ProjData, selection)
            .map_err(|e| e.to_string())?;
        let infos = all.operation_info();
        let (used, forced) = match options.operation {
            Some(index) => match all.with_operation_index(index) {
                Some(t) => (t.map_err(|e| e.to_string())?, Some(index)),
                None => return Err(format!("there is no operation {}", index + 1)),
            },
            None => (all.clone(), None),
        };
        Ok(Session {
            src,
            dst,
            each: (0..infos.len()).map(|_| OnceCell::new()).collect(),
            src_geo: Geo::new(&src_crs, src, order),
            dst_geo: Geo::new(&dst_crs, dst, order),
            src_geographic: xeibe_crs::get(src)
                .filter(|r| r.kind.is_geographic())
                .map(|r| order == AxisOrder::Authority && r.first_axis == FirstAxis::NorthOrLat),
            all,
            used,
            forced,
            infos,
            inverse: OnceCell::new(),
            failed: RefCell::default(),
        })
    }

    /// The operations to choose among, best first, with their steps,
    /// methods and parameters; empty for a conversion on one datum.
    pub fn info(&self) -> Value {
        let failed = self.failed.borrow();
        json!({
            "source": self.src,
            "target": self.dst,
            "conversion": self.infos.is_empty(),
            "forced": self.forced,
            "source_dimension": self.all.source().dimension(),
            "target_dimension": self.all.target().dimension(),
            "operations": self.infos.iter().map(|info| operation_info_json(info, &failed)).collect::<Vec<_>>(),
        })
    }

    /// The grid data still to fetch before converting `points`, leaving out
    /// files that failed.
    pub fn grid_requests(&self, points: &[Option<Point>]) -> Vec<GridRequest> {
        let mut bounds: Option<[f64; 4]> = None;
        for &(x, y, _) in points.iter().flatten() {
            if !(x.is_finite() && y.is_finite()) {
                continue;
            }
            let b = bounds.get_or_insert([x, y, x, y]);
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        let Some(bounds) = bounds else {
            return Vec::new();
        };
        let failed = self.failed.borrow();
        self.all
            .grid_requests(Some(bounds))
            .into_iter()
            .filter(|r| !failed.contains_key(&r.file))
            .collect()
    }

    pub fn add_grid_data(&self, file: &str, offset: u64, data: Vec<u8>) -> Result<()> {
        self.all
            .add_grid_data(file, offset, data)
            .map_err(|e| e.to_string())
    }

    /// A grid file couldn't be fetched: the points that need it fail, and
    /// say so.
    pub fn grid_failed(&self, file: &str, reason: &str) {
        self.failed
            .borrow_mut()
            .insert(file.to_string(), reason.to_string());
    }

    /// Every line's result: `null` for a line without a point, else
    /// `{"out": [x, y(, z)], "op": index, "warnings": [...]}` or
    /// `{"error": …}`.
    pub fn convert(&self, points: &[Option<Point>]) -> Value {
        Value::Array(
            points
                .iter()
                .map(|p| p.map_or(Value::Null, |p| self.point(p)))
                .collect(),
        )
    }

    fn point(&self, p: Point) -> Value {
        match self.check(p) {
            Ok(c) => json!({
                "out": coords(c.out),
                "op": c.op,
                "warnings": c.warnings,
            }),
            Err(error) => json!({ "error": error }),
        }
    }

    /// The result through the transform used, the operation it went
    /// through, how the choice went, and the warnings: outside the
    /// source's or the target's area of use, and an operation used outside
    /// its own, which the engine's explanation tells apart:
    ///
    /// * `fallback_none`: no operation's area contains the point; the
    ///   first without grids was used, as PROJ does;
    /// * `fallback_failed`: some contain it, but none of those could
    ///   convert it (outside its grid, say), so the same fallback;
    /// * `only_area`: outside the area of the only operation there is,
    ///   which PROJ uses everywhere;
    /// * `forced_area`: outside the area of the operation forced.
    fn check(&self, p: Point) -> Result<Checked> {
        if let Some(lat_first) = self.src_geographic {
            let (lon, lat) = if lat_first { (p.1, p.0) } else { (p.0, p.1) };
            if lat.abs() > 90.0 || lon.abs() > 180.0 {
                return Err(
                    "out of range: latitude is within ±90° and longitude within ±180°; are the axes the other way round?"
                        .into(),
                );
            }
        }
        let input = self.src_geo.as_ref().and_then(|g| g.lonlat(p));
        let out = self.through(&self.used, p)?;
        // The automatic choice, explained; with an operation forced, it
        // still says whether each area contains the point.
        let explanation = self.all.explain_point(p.0, p.1, p.2);
        let op = match self.forced {
            Some(index) => Some(index),
            None => explanation.as_ref().and_then(|e| e.used),
        };
        let mut warnings = Vec::new();
        if let (Some(area), Some(lonlat)) = (record_area(self.src), input)
            && !inside(area, lonlat)
        {
            warnings.push("source_area");
        }
        if let Some(e) = &explanation {
            let candidate = |i: usize| e.candidates.get(i);
            match (self.forced, e.used) {
                (Some(f), _) => {
                    if candidate(f).and_then(|c| c.inside) == Some(false) {
                        warnings.push("forced_area");
                    }
                }
                (None, Some(u)) => match candidate(u) {
                    Some(c) if c.outcome == Outcome::Only && c.inside == Some(false) => {
                        warnings.push("only_area");
                    }
                    Some(c) if c.outcome == Outcome::Fallback => {
                        warnings.push(if e.candidates.iter().any(|c| c.inside == Some(true)) {
                            "fallback_failed"
                        } else {
                            "fallback_none"
                        });
                    }
                    _ => {}
                },
                (None, None) => {}
            }
        }
        let output = self.dst_geo.as_ref().and_then(|g| g.lonlat(out));
        if let (Some(area), Some(lonlat)) = (record_area(self.dst), output)
            && !inside(area, lonlat)
        {
            warnings.push("target_area");
        }
        Ok(Checked {
            out,
            op,
            warnings,
            explanation,
        })
    }

    fn through(&self, transform: &Transform, (x, y, z): Point) -> Result<Point> {
        transform.convert(x, y, z).map_err(|e| {
            let mut message = e.to_string();
            let failed = self.failed.borrow();
            if e == xeibe_reproject::CoordinateError::GridNotLoaded && !failed.is_empty() {
                let list: Vec<String> = failed
                    .iter()
                    .map(|(f, why)| format!("{f} ({why})"))
                    .collect();
                message.push_str(&format!(": couldn't fetch {}", list.join(", ")));
            }
            message
        })
    }

    fn inverse(&self) -> &Result<Transform> {
        self.inverse
            .get_or_init(|| self.used.inverse().map_err(|e| e.to_string()))
    }

    fn candidate(&self, index: usize) -> Option<&Result<Transform>> {
        let cell = self.each.get(index)?;
        Some(
            cell.get_or_init(|| match self.all.with_operation_index(index) {
                Some(t) => t.map_err(|e| e.to_string()),
                None => Err(format!("no operation {}", index + 1)),
            }),
        )
    }

    /// Everything about one point: its longitude and latitude before and
    /// after, the operation used, the result through every candidate and
    /// how far each lands from it, and the round trip.
    pub fn details(&self, p: Point) -> Value {
        let input = self.src_geo.as_ref().and_then(|g| g.lonlat(p));
        let Checked {
            out,
            op,
            warnings,
            explanation,
        } = match self.check(p) {
            Ok(c) => c,
            Err(error) => {
                let explanation = self.all.explain_point(p.0, p.1, p.2);
                return json!({
                    "input_lonlat": input,
                    "error": error,
                    "candidates": self.candidates_json(p, explanation.as_ref(), None),
                });
            }
        };
        let output = self.dst_geo.as_ref().and_then(|g| g.lonlat(out));
        let round_trip = match self.inverse() {
            Err(error) => json!({ "error": error }),
            Ok(inverse) => match self.through(inverse, out) {
                Err(error) => json!({ "error": error }),
                Ok(back) => {
                    let back_lonlat = self.src_geo.as_ref().and_then(|g| g.lonlat(back));
                    json!({
                        "back": coords(back),
                        "delta": [back.0 - p.0, back.1 - p.1, back.2.zip(p.2).map(|(a, b)| a - b)],
                        "distance": self.src_geo.as_ref().zip(input.zip(back_lonlat))
                            .map(|(g, (a, b))| g.distance(a, b)),
                        "operation": inverse.operation_for(out.0, out.1, out.2),
                    })
                }
            },
        };
        json!({
            "input_lonlat": input,
            "out": coords(out),
            "output_lonlat": output,
            "op": op,
            "warnings": warnings,
            "candidates": self.candidates_json(p, explanation.as_ref(), Some((out, output))),
            "round_trip": round_trip,
        })
    }

    /// Each candidate: whether its area contains the point and what the
    /// choice made of it (the engine's explanation; no outcome with an
    /// operation forced, which isn't chosen), and its result.
    fn candidates_json(
        &self,
        p: Point,
        explanation: Option<&Explanation>,
        chosen: Option<(Point, Option<[f64; 2]>)>,
    ) -> Vec<Value> {
        (0..self.infos.len())
            .map(|index| {
                let explained = explanation.and_then(|e| e.candidates.get(index));
                let inside = explained.and_then(|c| c.inside);
                let outcome = match (self.forced, explained) {
                    (None, Some(c)) => outcome_json(c.outcome),
                    _ => Value::Null,
                };
                let result = match self.candidate(index) {
                    Some(Ok(t)) => self.through(t, p),
                    Some(Err(e)) => Err(e.clone()),
                    None => Err("no such operation".into()),
                };
                match result {
                    Err(error) => json!({
                        "index": index,
                        "inside": inside,
                        "outcome": outcome,
                        "error": error,
                    }),
                    Ok(out) => {
                        let lonlat = self.dst_geo.as_ref().and_then(|g| g.lonlat(out));
                        let distance = chosen.and_then(|(_, chosen)| {
                            let g = self.dst_geo.as_ref()?;
                            Some(g.distance(chosen?, lonlat?))
                        });
                        let dz = chosen.and_then(|(c, _)| c.2.zip(out.2).map(|(a, b)| b - a));
                        json!({
                            "index": index,
                            "inside": inside,
                            "outcome": outcome,
                            "out": coords(out),
                            "distance": distance,
                            "dz": dz,
                        })
                    }
                }
            })
            .collect()
    }
}

/// What [`Session::check`] finds for a point.
struct Checked {
    out: Point,
    op: Option<usize>,
    warnings: Vec<&'static str>,
    explanation: Option<Explanation>,
}

/// What the choice made of a candidate, for the page.
fn outcome_json(outcome: Outcome) -> Value {
    match outcome {
        Outcome::Chosen => json!({ "kind": "chosen" }),
        Outcome::Fallback => json!({ "kind": "fallback" }),
        Outcome::Only => json!({ "kind": "only" }),
        Outcome::Outside => json!({ "kind": "outside" }),
        Outcome::NotReached => json!({ "kind": "not_reached" }),
        Outcome::Failed(error) => json!({ "kind": "failed", "error": error.to_string() }),
        Outcome::Passed { by, reason } => json!({
            "kind": "passed",
            "by": by,
            "reason": match reason {
                PassReason::LessAccurate => "less_accurate",
                PassReason::UnknownAccuracy => "unknown_accuracy",
                PassReason::LargerArea => "larger_area",
                PassReason::UnknownAreaName => "unknown_area_name",
                PassReason::Offshore => "offshore",
                PassReason::Priority => "priority",
                PassReason::Earlier => "earlier",
            },
        }),
    }
}

fn coords((x, y, z): Point) -> Value {
    match z {
        Some(z) => json!([x, y, z]),
        None => json!([x, y]),
    }
}

/// EPSG's operations, methods and parameters, from `operations_json`.
struct Registry {
    operations: HashMap<u64, Value>,
    methods: HashMap<u64, Value>,
    /// Name and the kind of unit of its values.
    parameters: HashMap<u64, (Value, Value)>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut all: Value =
            serde_json::from_str(xeibe_crs::operations_json()).expect("operations_json is JSON");
        let rows = |v: &mut Value| match v.take() {
            Value::Array(rows) => rows,
            _ => Vec::new(),
        };
        let operations = rows(&mut all["operations"])
            .into_iter()
            .filter_map(|op| Some((op["code"].as_u64()?, op)))
            .collect();
        let methods = rows(&mut all["methods"])
            .into_iter()
            .filter_map(|m| Some((m[0].as_u64()?, m[1].clone())))
            .collect();
        let parameters = rows(&mut all["parameters"])
            .into_iter()
            .filter_map(|p| Some((p[0].as_u64()?, (p[1].clone(), p[2].clone()))))
            .collect();
        Registry {
            operations,
            methods,
            parameters,
        }
    })
}

fn method_json(code: u64) -> Value {
    json!({ "code": code, "name": registry().methods.get(&code) })
}

/// One EPSG operation: name, method, parameters in SI units with the kind
/// of unit of each, accuracy, remarks; a concatenated operation's steps.
fn epsg_operation(code: u32) -> Value {
    let r = registry();
    let Some(op) = r.operations.get(&u64::from(code)) else {
        return json!({ "code": code });
    };
    let params: Vec<Value> = op["params"]
        .as_array()
        .map(|params| {
            params
                .iter()
                .map(|p| {
                    let code = p[0].as_u64().unwrap_or(0);
                    let (name, unit) = r
                        .parameters
                        .get(&code)
                        .cloned()
                        .unwrap_or((Value::Null, Value::Null));
                    json!({ "code": code, "name": name, "unit": unit, "value": p[1] })
                })
                .collect()
        })
        .unwrap_or_default();
    let steps: Vec<Value> = op["steps"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .filter_map(|s| s.as_u64())
                .map(|s| epsg_operation(s as u32))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "code": code,
        "name": op["name"],
        "table": op["table"],
        "method": op["method"].as_u64().map(method_json),
        "params": params,
        "steps": steps,
        "src": op["src"],
        "dst": op["dst"],
        "accuracy": op["accuracy"],
        "remarks": xeibe_crs::operation_remarks(code),
    })
}

fn step_crs(crs: StepCrs) -> Value {
    let name = |c: u32| xeibe_crs::get(c).map(|r| r.name);
    match crs {
        StepCrs::Epsg(c) => json!({ "label": format!("EPSG:{c}"), "name": name(c) }),
        StepCrs::Promoted(c) => json!({
            "label": format!("EPSG:{c} (3D)"),
            "name": name(c).map(|n| format!("{n}, with ellipsoidal heights")),
        }),
        StepCrs::Compound(h, v) => json!({
            "label": format!("EPSG:{h}+{v}"),
            "name": name(h).zip(name(v)).map(|(h, v)| format!("{h} + {v}")),
        }),
    }
}

fn step_json(step: &OperationStep) -> Value {
    let (kind, detail) = match step.kind {
        StepKind::Epsg { code, inverse } => (
            "epsg",
            json!({ "inverse": inverse, "operation": epsg_operation(code) }),
        ),
        StepKind::EnsembleIdentity { method, inverse } => (
            "ensemble_identity",
            json!({ "inverse": inverse, "method": method_json(u64::from(method)) }),
        ),
        StepKind::Projection { crs, inverse } => {
            let projjson: Value = xeibe_crs::projjson(crs)
                .and_then(|j| serde_json::from_str(j).ok())
                .unwrap_or(Value::Null);
            (
                "projection",
                json!({ "inverse": inverse, "crs": crs, "conversion": conversion(&projjson) }),
            )
        }
        StepKind::SameDatum => ("same_datum", Value::Null),
        StepKind::EnsembleMember => ("ensemble_member", Value::Null),
        StepKind::VerticalConversion => ("vertical_conversion", Value::Null),
        StepKind::BallparkVertical => ("ballpark_vertical", Value::Null),
    };
    json!({
        "name": step.name,
        "kind": kind,
        "detail": detail,
        "source": step_crs(step.source),
        "target": step_crs(step.target),
        "accuracy": step.accuracy,
    })
}

/// Whether an operation shifts nothing: every EPSG transformation it runs
/// has parameters of zero (ETRS89 to WGS 84 (1)), and the other steps are
/// projections, changes on one datum and ensemble null operations. Used
/// outside its area, such an operation costs nothing.
fn is_null(info: &OperationInfo) -> bool {
    fn zero(code: u64) -> bool {
        let Some(op) = registry().operations.get(&code) else {
            return false;
        };
        if let Some(steps) = op["steps"].as_array() {
            return !steps.is_empty() && steps.iter().filter_map(Value::as_u64).all(zero);
        }
        op["params"].as_array().is_some_and(|params| {
            !params.is_empty() && params.iter().all(|p| p[1].as_f64() == Some(0.0))
        })
    }
    info.chain
        .iter()
        .any(|s| matches!(s.kind, StepKind::Epsg { .. }))
        && info.chain.iter().all(|s| match s.kind {
            StepKind::Epsg { code, .. } => zero(u64::from(code)),
            StepKind::BallparkVertical => false,
            _ => true,
        })
}

fn operation_info_json(info: &OperationInfo, failed: &BTreeMap<String, String>) -> Value {
    json!({
        "name": info.name,
        "code": info.code.map(|(code, _)| code),
        "inverse": info.code.map(|(_, inverse)| inverse),
        "accuracy": info.accuracy,
        "area": info.area,
        "area_name": info.area_name,
        "grids": info.grids.iter().map(|g| json!({ "file": g, "failed": failed.get(g) })).collect::<Vec<_>>(),
        "time_dependent": info.time_dependent,
        "scope": info.scope,
        "remarks": info.code.and_then(|(code, _)| xeibe_crs::operation_remarks(code)),
        "null": is_null(info),
        "chain": info.chain.iter().map(step_json).collect::<Vec<_>>(),
    })
}

/// A grid request as the page fetches it.
pub fn grid_request_json(r: &GridRequest) -> Value {
    json!({ "file": r.file, "offset": r.offset, "length": r.length, "url": r.url() })
}
