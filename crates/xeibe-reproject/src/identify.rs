//! Finding the EPSG code of a CRS that comes without one, or checking the
//! one it gives (`docs/crs-wkt.md`, "Identification").
//!
//! The confidence levels are PROJ's, and so is what each means, but the
//! search is simpler. Candidates are the EPSG CRSs of the same kind on the
//! same datum, with the same projection method, from `xeibe-crs`'s table;
//! each is compared in full ([`Crs::matches`]), and its name sets the
//! confidence. Where PROJ differs on purpose:
//!
//! - A WKT1 `GEOGCS` or `PROJCS` without `AXIS` matches by axis units only,
//!   with its `AUTHORITY` as without: GDAL's WKT1 for EPSG:2180 is
//!   EPSG:2180. PROJ applies that rule to a search by name only, so it
//!   rejects the `AUTHORITY` of its own WKT1.
//! - A deprecated CRS is found too, ranked after live ones of the same
//!   confidence. PROJ leaves deprecated CRSs out of its search.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use xeibe_crs::{CrsKind, DatumKind};

use crate::crs::{Crs, Kind, Relax, StoredParam, oriented_alike};
use crate::name::canonical;
use crate::registry::GeodeticKind;
use crate::wkt::datums;

impl Crs {
    /// EPSG CRSs this CRS is, best first, each with a confidence out of
    /// 100, as PROJ's `identify` rates them:
    ///
    /// - 100: equivalent ([`Crs::equivalent`]), with EPSG's name;
    /// - 90: equivalent, with a name that matches EPSG's or one of its
    ///   aliases after normalising (`WGS_1984` for "WGS 84");
    /// - 70: equivalent, under another name;
    /// - 60: equivalent but for the datum, which has no EPSG code or name
    ///   and the same ellipsoid and prime meridian as the EPSG CRS's (70
    ///   when it is named "unknown" or "unnamed");
    /// - 25: not equivalent, but given as this CRS's code, or with its name.
    ///
    /// The CRS's own EPSG code, when it is equivalent, is the only result.
    /// A Bound CRS (WKT1 with `TOWGS84`) has none, as in PROJ.
    /// A WKT1 `GEOGCS` or `PROJCS` without `AXIS` has default axes, which
    /// are compared by unit only. Engineering CRSs are never identified, as
    /// in PROJ. pyproj's `to_epsg()` is the first result at 70 or more.
    ///
    /// ```
    /// use xeibe_reproject::Crs;
    ///
    /// let crs = Crs::from_wkt(
    ///     r#"PROJCS["ETRS89 / PL-1992",GEOGCS["ETRS89",
    ///          DATUM["European_Terrestrial_Reference_System_1989",
    ///            SPHEROID["GRS 1980",6378137,298.257222101]],
    ///          PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],
    ///        PROJECTION["Transverse_Mercator"],
    ///        PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",19],
    ///        PARAMETER["scale_factor",0.9993],PARAMETER["false_easting",500000],
    ///        PARAMETER["false_northing",-5300000],UNIT["metre",1]]"#,
    /// )
    /// .unwrap();
    /// assert_eq!(crs.identify(), [(2180, 100)]);
    /// ```
    pub fn identify(&self) -> Vec<(u32, u8)> {
        self.found()
            .into_iter()
            .map(|f| (f.code, f.confidence))
            .collect()
    }

    /// The EPSG code that operations between datums are looked up by: the
    /// CRS's own, or else that of the best EPSG CRS it is equivalent to.
    /// Not one of a CRS on another datum with the same ellipsoid.
    pub(crate) fn epsg_code(&self) -> Option<u32> {
        self.code.or_else(|| {
            self.found()
                .into_iter()
                .find(|f| f.equivalent)
                .map(|f| f.code)
        })
    }

    fn found(&self) -> Vec<Found> {
        // A Bound CRS is no EPSG CRS, as in PROJ.
        if self.bound.is_some() || self.horizontal().bound.is_some() {
            return Vec::new();
        }
        let Some(kind) = epsg_kind(self) else {
            return Vec::new();
        };
        // Geographic 2D and 3D alike: the axes tell them apart, and EPSG
        // classes some 3D CRSs as 2D (NAD83(FBN), EPSG:8449).
        let same_kind = |k: CrsKind| k == kind || (k.is_geographic() && kind.is_geographic());
        let relax = Relax {
            axis_directions: self.implicit_axes,
            ..Relax::default()
        };
        // Code to confidence, whether the CRS is equivalent (not on another
        // datum with the same ellipsoid, nor merely named alike), and
        // whether its axes are the input's exactly, not only by unit.
        let mut found: HashMap<u32, (u8, bool, bool)> = HashMap::new();
        if let Some(code) = self.code
            && xeibe_crs::get(code).is_some_and(|r| same_kind(r.kind))
        {
            if Crs::from_epsg(code).is_ok_and(|epsg| self.matches(&epsg, relax)) {
                return vec![Found {
                    code,
                    confidence: 100,
                    equivalent: true,
                    exact_axes: true,
                }];
            }
            found.insert(code, (25, false, false));
        }

        // For a CRS without a name (a PROJ string's); a named one keeps
        // its name's CRS (WGS 84 / TMzn35N, not WGS 84 / UTM zone 35N).
        if insignificant(&self.name)
            && let Some(found) = self.utm()
        {
            return vec![found];
        }

        let datums = datum_codes(self);
        let method = match &self.horizontal().kind {
            Kind::Projected { method_code, .. } => *method_code,
            _ => None,
        };
        // Methods that `Crs::matches` may find the same: deprecated
        // methods named as their replacements, and other formulations of
        // one projection.
        let alike = |m: u32| {
            let m = match m {
                9823 => 1029,
                9842 => 1028,
                9802 => 9801,
                9805 => 9804,
                m => m,
            };
            if self.implicit_axes {
                oriented_alike(m)
            } else {
                m
            }
        };
        let same_method = |m: u32| method.is_none_or(|method| alike(method) == alike(m));
        let named = named(&self.name);
        // Esri's names of EPSG CRSs, which PROJ's database has as aliases.
        let esri_named = crate::wkt::esri::crs_codes(&self.name);
        // PROJ's second, approximate search by name: EPSG names containing
        // this one (Esri's "Nord_Maroc" in "Merchich / Nord Maroc"). What
        // it finds isn't sought again in the database.
        let wanted = canonical(&self.name, false);
        let approximately_named = |code: u32| {
            !insignificant(&self.name)
                && wanted.len() > 2
                && xeibe_crs::get(code).is_some_and(|r| canonical(r.name, false).contains(&wanted))
        };
        // The ensembles of those datums and their members: EPSG has moved
        // CRSs named "ETRS89 / …" from the ETRS89 ensemble to national
        // members of it, which Esri's and older WKT still have them on.
        let relatives: Vec<u32> = datums
            .iter()
            .flat_map(|&d| crate::datum::relatives(d))
            .filter(|d| !datums.contains(d))
            .collect();
        let existing = (kind == CrsKind::Projected)
            .then(|| existing_projected(self, &datums))
            .flatten();
        let candidates = xeibe_crs::CRS.iter().filter(|r| {
            same_kind(r.kind)
                && (datums.is_empty()
                    || r.datum
                        .is_some_and(|d| datums.contains(&d) || relatives.contains(&d)))
                && (method.is_none() || r.method.is_some_and(same_method))
        });
        for record in candidates {
            let Ok(epsg) = Crs::from_epsg(record.code) else {
                continue;
            };
            let by_name = if record.name.trim() == self.name.trim() {
                100
            } else if named.contains(&record.code) || esri_named.contains(&record.code) {
                90
            } else {
                70
            };
            // PROJ's searches of geodetic and projected CRSs by datum,
            // ellipsoid and conversion leave deprecated CRSs out; only its
            // search by name finds them. Compound CRSs it finds by their
            // parts, deprecated or not.
            let searched =
                matches!(kind, CrsKind::Projected | CrsKind::Geocentric) || kind.is_geographic();
            if record.deprecated && by_name < 90 && searched {
                continue;
            }
            // Unless found by name, a projected CRS is one PROJ's database
            // query finds.
            if by_name < 90
                && let Some(existing) = &existing
                && !existing.contains(&record.code)
                && !approximately_named(record.code)
            {
                continue;
            }
            let on_relative = record.datum.is_some_and(|d| relatives.contains(&d));
            let datum_only = Relax {
                datum_identity: true,
                ..relax
            };
            let (confidence, equivalent) = if on_relative {
                // A datum's ensemble or member: only with the CRS's name,
                // as PROJ takes ETRS89 for its members only then.
                if by_name < 90 || !self.matches(&epsg, datum_only) {
                    continue;
                }
                (by_name, true)
            } else if self.matches(&epsg, relax) {
                (by_name, true)
            } else if datums.is_empty() && self.matches(&epsg, datum_only) {
                // A projected CRS on a datum named "unknown…" (a PROJ
                // string's "Unknown based on GRS 1980 ellipsoid") is 70 by
                // ellipsoid and conversion; a geographic one only 60, as by
                // ellipsoid alone (PROJ's `ProjectedCRS::identify` and
                // `GeodeticCRS::identify`).
                let projected = matches!(self.horizontal().kind, Kind::Projected { .. });
                if projected && self.datum().is_some_and(|d| insignificant(&d.name)) {
                    (70, false)
                } else {
                    (60, false)
                }
            } else {
                continue;
            };
            // WKT1's default axes match by unit; of two CRSs alike but for
            // their axes (UTM zone 30N and its N-E twin), the one with the
            // default axes comes first, as in PROJ.
            let exact_axes = !relax.axis_directions
                || self.matches(
                    &epsg,
                    Relax {
                        axis_directions: false,
                        datum_identity: true,
                        ..relax
                    },
                );
            let found_now = (confidence, equivalent, exact_axes);
            let level = found.entry(record.code).or_default();
            *level = (*level).max(found_now);
        }
        for &code in named {
            if xeibe_crs::get(code).is_some_and(|r| same_kind(r.kind)) {
                found.entry(code).or_insert((25, false, false));
            }
        }

        let mut out: Vec<Found> = found
            .into_iter()
            .map(|(code, (confidence, equivalent, exact_axes))| Found {
                code,
                confidence,
                equivalent,
                exact_axes,
            })
            .collect();
        let deprecated = |code: u32| xeibe_crs::get(code).is_some_and(|r| r.deprecated);
        // A deprecated CRS with the input's name comes before a live one
        // with another name: EPSG deprecated NAD83 / South Dakota North
        // (ftUS), EPSG:3454, for having South's parameters, so it is
        // equivalent to EPSG:3455, and the name says which was meant.
        // Projected CRSs of equal confidence come by name, as PROJ sorts
        // them, after one with the input's own name.
        // Projected CRSs of equal confidence that PROJ's database query
        // found (for a CRS without a name, as a PROJ string's) come by
        // name, as PROJ sorts them.
        let projected = kind == CrsKind::Projected && insignificant(&self.name);
        let name = |code: u32| xeibe_crs::get(code).map_or("", |r| r.name);
        let other_name = |code: u32| name(code).trim() != self.name.trim();
        out.sort_by(|a, b| {
            let key = |f: &Found| {
                if projected {
                    (
                        u8::MAX - f.confidence,
                        other_name(f.code),
                        false,
                        false,
                        name(f.code),
                    )
                } else {
                    (
                        u8::MAX - f.confidence,
                        false,
                        deprecated(f.code),
                        !f.exact_axes,
                        "",
                    )
                }
            };
            key(a).cmp(&key(b)).then(a.code.cmp(&b.code))
        });
        out
    }
}

/// The live EPSG geographic CRSs (2D, or 3D for a CRS with a height) a
/// projected CRS's base is identified with, which PROJ's
/// `createProjectedCRSFromExisting` looks for projected CRSs on: those on
/// its datum, or for a datum EPSG doesn't have, those on its ellipsoid
/// (the same EPSG ellipsoid, or one equal to it) and prime meridian
/// (`GeodeticCRS::identify`). `None`, no restriction, when there are none.
fn base_candidates(crs: &Crs, datums: &[u32]) -> Option<HashSet<u32>> {
    let datum = crs.datum()?;
    let kind = if crs.dimension() == 3 {
        GeodeticKind::Geographic3d
    } else {
        GeodeticKind::Geographic2d
    };
    let registry = crate::registry::registry();
    let (ellipsoids, meridians) = epsg_shapes();
    let on_ellipsoid = |code: u32| {
        let Some(d) = registry.datums.get(&code) else {
            return false;
        };
        let ellipsoid = match datum.ellipsoid_code {
            Some(e) => d.ellipsoid == e,
            None => ellipsoids
                .get(&d.ellipsoid)
                .is_some_and(|e| e.is(&datum.ellipsoid)),
        };
        ellipsoid
            && meridians
                .get(&d.prime_meridian)
                .is_some_and(|&pm| (pm - datum.prime_meridian).abs() < 1e-10)
    };
    let set: HashSet<u32> = registry
        .geodetic
        .values()
        .filter(|g| !g.deprecated && g.kind == kind)
        .filter(|g| {
            if datums.is_empty() {
                on_ellipsoid(g.datum)
            } else {
                datums.contains(&g.datum)
            }
        })
        .map(|g| g.code)
        .collect();
    (!set.is_empty()).then_some(set)
}

/// An EPSG ellipsoid as PROJ's database stores it, in its own unit (feet
/// for Clarke 1858): the semi-major axis, and the semi-minor axis or the
/// inverse flattening, whichever EPSG gives.
#[derive(Debug, Clone, Copy)]
struct StoredEllipsoid {
    a: f64,
    b: Option<f64>,
    rf: Option<f64>,
}

impl StoredEllipsoid {
    fn of(e: &serde_json::Value) -> Option<Self> {
        let raw = |v: &serde_json::Value| v.as_f64().or_else(|| v["value"].as_f64());
        if let Some(r) = raw(&e["radius"]) {
            return Some(Self {
                a: r,
                b: Some(r),
                rf: None,
            });
        }
        Some(Self {
            a: raw(&e["semi_major_axis"])?,
            b: raw(&e["semi_minor_axis"]),
            rf: e["inverse_flattening"].as_f64(),
        })
    }

    /// `createEllipsoidFromExisting`: the stored values against an
    /// ellipsoid's in metres, within 1e-10.
    fn is(&self, e: &crate::ellipsoid::Ellipsoid) -> bool {
        let close = |stored: f64, x: f64| (stored - x).abs() < 1e-10 * stored.abs();
        let rf = if e.f == 0.0 { 0.0 } else { 1.0 / e.f };
        close(self.a, e.a)
            && (self.b.is_some_and(|b| close(b, e.b))
                || self.rf.is_some_and(|stored| close(stored, rf)))
    }
}

fn ellipsoid_json(crs: &serde_json::Value) -> &serde_json::Value {
    let datum = if crs["datum"].is_object() {
        &crs["datum"]
    } else {
        &crs["datum_ensemble"]
    };
    &datum["ellipsoid"]
}

/// EPSG's ellipsoids as stored and prime meridians in radians, by code,
/// from the geodetic CRSs that use them.
fn epsg_shapes() -> &'static (HashMap<u32, StoredEllipsoid>, HashMap<u32, f64>) {
    static SHAPES: OnceLock<(HashMap<u32, StoredEllipsoid>, HashMap<u32, f64>)> = OnceLock::new();
    SHAPES.get_or_init(|| {
        let registry = crate::registry::registry();
        let (mut ellipsoids, mut meridians) = (HashMap::new(), HashMap::new());
        for g in registry.geodetic.values() {
            let Some(d) = registry.datums.get(&g.datum) else {
                continue;
            };
            if ellipsoids.contains_key(&d.ellipsoid) && meridians.contains_key(&d.prime_meridian) {
                continue;
            }
            let Some(json) = xeibe_crs::projjson(g.code)
                .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            else {
                continue;
            };
            if let Some(e) = StoredEllipsoid::of(ellipsoid_json(&json)) {
                ellipsoids.insert(d.ellipsoid, e);
            }
            if let Some(datum) = Crs::from_epsg(g.code).ok().and_then(|c| c.datum().cloned()) {
                meridians.insert(d.prime_meridian, datum.prime_meridian);
            }
        }
        (ellipsoids, meridians)
    })
}

/// A live EPSG projected CRS as PROJ's database query over projected CRSs
/// sees it: its method, its ellipsoid's semi-major axis as stored (in its
/// own unit, which PROJ compares with metres), its base CRS and
/// its conversion's parameters as written.
struct ProjectedRow {
    code: u32,
    name: &'static str,
    method: u32,
    a: f64,
    base: u32,
    stored: Vec<StoredParam>,
}

fn projected_rows() -> &'static [ProjectedRow] {
    static ROWS: OnceLock<Vec<ProjectedRow>> = OnceLock::new();
    ROWS.get_or_init(|| {
        xeibe_crs::CRS
            .iter()
            .filter(|r| r.kind == CrsKind::Projected && !r.deprecated)
            .filter_map(|r| {
                let crs = Crs::from_epsg(r.code).ok()?;
                let Kind::Projected {
                    datum,
                    method_code,
                    stored,
                    ..
                } = &crs.kind
                else {
                    return None;
                };
                let json: serde_json::Value =
                    serde_json::from_str(xeibe_crs::projjson(r.code)?).ok()?;
                let _ = datum;
                Some(ProjectedRow {
                    code: r.code,
                    name: r.name,
                    method: (*method_code)?,
                    a: StoredEllipsoid::of(ellipsoid_json(&json["base_crs"]))?.a,
                    base: json["base_crs"]["id"]["code"].as_u64()? as u32,
                    stored: stored.clone(),
                })
            })
            .collect()
    })
}

/// PROJ's `createProjectedCRSFromExisting`: the live EPSG projected CRSs
/// with the same method, a semi-major axis within 1e-4, a base its base is
/// identified as ([`base_candidates`]), and each parameter in degrees at
/// the same place, with the same code, within 1 of the value as stored
/// (so not one EPSG stores in grads; Lambert Conic Conformal (2SP)'s
/// standard parallels in either order). Those with the CRS's name, if any;
/// none when more than 200 match, as PROJ then gives up.
fn existing_projected(crs: &Crs, datums: &[u32]) -> Option<HashSet<u32>> {
    let Kind::Projected {
        datum,
        method_code,
        stored,
        ..
    } = &crs.horizontal().kind
    else {
        return None;
    };
    let Some(method) = *method_code else {
        return Some(HashSet::new());
    };
    let bases = base_candidates(crs, datums);
    let mut checks = Vec::new();
    let (mut first, mut second) = (None, None);
    for (i, p) in stored.iter().enumerate() {
        if p.code == 0 || p.value.is_nan() {
            break;
        }
        if !p.degree {
            continue;
        }
        match (method, p.code) {
            (9802, 8823) => first = Some((i, p.value)),
            (9802, 8824) => second = Some((i, p.value)),
            _ => checks.push((i, p.code, p.value)),
        }
    }
    let near = |row: &ProjectedRow, i: usize, code: u32, v: f64| {
        row.stored
            .get(i)
            .is_some_and(|q| q.code == code && (q.value - v).abs() <= 1.0)
    };
    let rows: Vec<&ProjectedRow> = projected_rows()
        .iter()
        .filter(|row| {
            row.method == method
                && (row.a - datum.ellipsoid.a).abs() <= 1e-4 * row.a
                && bases.as_ref().is_none_or(|b| b.contains(&row.base))
                && checks.iter().all(|&(i, code, v)| near(row, i, code, v))
                && match (first, second) {
                    (Some((i1, v1)), Some((i2, v2))) => {
                        (near(row, i1, 8823, v1) && near(row, i2, 8824, v2))
                            || (near(row, i1, 8823, v2) && near(row, i2, 8824, v1))
                    }
                    _ => true,
                }
        })
        .collect();
    let named: HashSet<u32> = rows
        .iter()
        .filter(|row| canonical(row.name, false) == canonical(&crs.name, false))
        .map(|row| row.code)
        .collect();
    if !named.is_empty() {
        return Some(named);
    }
    if rows.len() > 200 {
        return Some(HashSet::new());
    }
    Some(rows.iter().map(|row| row.code).collect())
}

impl Crs {
    /// PROJ's shortcut for UTM (`ProjectedCRS::identify`): a UTM conversion
    /// with easting and northing in metres on WGS 84, or north of the
    /// equator on NAD27 or NAD83, is EPSG's UTM CRS of the zone, 100 by
    /// its name, 90 by an equivalent one, else 70.
    fn utm(&self) -> Option<Found> {
        let Kind::Projected {
            datum,
            method_code: Some(9807),
            params,
            ..
        } = &self.kind
        else {
            return None;
        };
        if !self.axes.easting_northing_metres() || datum.prime_meridian != 0.0 {
            return None;
        }
        let value = |code: u32| params.get(code).ok();
        let lon = value(8802)?.to_degrees();
        let zone = (lon + 183.0) / 6.0;
        let north = match value(8807)? {
            n if n.abs() < 1e-10 => true,
            n if (n - 10_000_000.0).abs() < 1e-10 => false,
            _ => return None,
        };
        if value(8801)?.to_degrees().abs() >= 1e-10
            || (value(8805)? - 0.9996).abs() >= 1e-10
            || value(8806)? != 500_000.0
            || !(zone > 0.9 && zone < 60.1 && (zone - zone.round()).abs() < 1e-10)
        {
            return None;
        }
        let zone = zone.round() as u32;
        let hemisphere = if north { "N" } else { "S" };
        let old_zone = matches!(zone, 59 | 60);
        let (name, code) = match datum.key.as_str() {
            "EPSG:6326" => ("WGS 84", if north { 32600 } else { 32700 } + zone),
            "EPSG:6267" if north && (zone <= 22 || old_zone) => (
                "NAD27",
                if old_zone {
                    3370 + zone - 59
                } else {
                    26700 + zone
                },
            ),
            "EPSG:6269" if north && (zone <= 23 || old_zone) => (
                "NAD83",
                if old_zone {
                    3372 + zone - 59
                } else {
                    26900 + zone
                },
            ),
            _ => return None,
        };
        let name = format!("{name} / UTM zone {zone}{hemisphere}");
        let confidence = if name == self.name {
            100
        } else if canonical(&name, false) == canonical(&self.name, false) {
            90
        } else {
            70
        };
        Some(Found {
            code,
            confidence,
            equivalent: true,
            exact_axes: true,
        })
    }
}

/// An EPSG CRS identification found.
struct Found {
    code: u32,
    confidence: u8,
    equivalent: bool,
    exact_axes: bool,
}

/// The kind of EPSG CRS this one would be; none for an engineering CRS.
fn epsg_kind(crs: &Crs) -> Option<CrsKind> {
    Some(match &crs.kind {
        Kind::Geographic { .. } => CrsKind::Geographic2d,
        Kind::Geocentric { .. } => CrsKind::Geocentric,
        Kind::Projected { .. } => CrsKind::Projected,
        Kind::Vertical { .. } => CrsKind::Vertical,
        Kind::Compound { .. } => CrsKind::Compound,
        Kind::Engineering { .. } => return None,
    })
}

/// The EPSG codes the CRS's datum (its horizontal part's, for a compound
/// CRS) may have: its id, and every datum that bears its name or the id's
/// name. A CRS is compared in full with those on any of them.
fn datum_codes(crs: &Crs) -> Vec<u32> {
    let (key, name, kind) = match (crs.datum(), crs.vertical_datum()) {
        (Some(d), _) => (&d.key, &d.name, DatumKind::Geodetic),
        (None, Some(d)) => (&d.key, &d.name, DatumKind::Vertical),
        (None, None) => return Vec::new(),
    };
    let id = key.strip_prefix("EPSG:").and_then(|code| code.parse().ok());
    let id_name = id.and_then(xeibe_crs::datum).map(|d| d.name);
    let mut codes: Vec<u32> = id.into_iter().collect();
    for name in std::iter::once(name.as_str()).chain(id_name) {
        codes.extend(datums::all_by_name(name, kind).iter().map(|d| d.code));
    }
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// A datum name that says nothing, as PROJ judges it.
fn insignificant(name: &str) -> bool {
    name.get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("unknown"))
        || name == "unnamed"
}

/// The EPSG CRSs whose name or one of whose aliases matches `name` after
/// normalising ([`crate::name::canonical`]).
fn named(name: &str) -> &'static [u32] {
    static INDEX: OnceLock<HashMap<String, Vec<u32>>> = OnceLock::new();
    let index = INDEX.get_or_init(|| {
        let mut index: HashMap<String, Vec<u32>> = HashMap::new();
        let names = xeibe_crs::CRS
            .iter()
            .map(|r| (r.name, r.code))
            .chain(xeibe_crs::ALIASES.iter().copied());
        for (name, code) in names {
            let codes = index.entry(canonical(name, true)).or_default();
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
        index
    });
    index.get(&canonical(name, true)).map_or(&[], Vec::as_slice)
}
