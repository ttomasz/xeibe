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

use std::collections::HashMap;
use std::sync::OnceLock;

use xeibe_crs::{CrsKind, DatumKind};

use crate::crs::{Crs, Kind, Relax, oriented_alike};
use crate::name::canonical;
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
        // The ensembles of those datums and their members: EPSG has moved
        // CRSs named "ETRS89 / …" from the ETRS89 ensemble to national
        // members of it, which Esri's and older WKT still have them on.
        let relatives: Vec<u32> = datums
            .iter()
            .flat_map(|&d| crate::datum::relatives(d))
            .filter(|d| !datums.contains(d))
            .collect();
        let candidates = xeibe_crs::CRS.iter().filter(|r| {
            same_kind(r.kind)
                && (datums.is_empty()
                    || r.datum
                        .is_some_and(|d| datums.contains(&d) || relatives.contains(&d)))
                && (method.is_none() || r.method.is_some_and(&same_method))
        });
        for record in candidates {
            let Ok(epsg) = Crs::from_epsg(record.code) else {
                continue;
            };
            let by_name = if record.name.trim() == self.name.trim() {
                100
            } else if named.contains(&record.code) {
                90
            } else {
                70
            };
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
                if self.datum().is_some_and(|d| insignificant(&d.name)) {
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
        out.sort_by_key(|f| {
            (
                u8::MAX - f.confidence,
                deprecated(f.code),
                !f.exact_axes,
                f.code,
            )
        });
        out
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
