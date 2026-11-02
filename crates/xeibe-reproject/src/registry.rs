//! EPSG's coordinate operations, read once from `xeibe_crs::operations_json`.
//!
//! The registry mirrors what PROJ's database holds for EPSG (the generator
//! applies PROJ's import rules), because the choice of operation follows
//! PROJ's (`docs/reprojection.md`, level 2).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::extent::Extent;

/// The table PROJ imports an operation into. Its rules sometimes depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Table {
    Helmert,
    Grid,
    Other,
    Concatenated,
    Conversion,
}

/// A parameter value in SI units, or a grid file name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParamValue {
    Number(f64),
    File(String),
    Missing,
}

#[derive(Debug, Clone)]
pub(crate) struct Operation {
    pub(crate) code: u32,
    /// EPSG's, borrowed from `xeibe_crs`'s text, or a bound CRS's own.
    pub(crate) name: Cow<'static, str>,
    pub(crate) table: Table,
    pub(crate) method: u32,
    pub(crate) src: Option<u32>,
    pub(crate) dst: Option<u32>,
    pub(crate) accuracy: Option<f64>,
    /// EPSG's scope of its first usage, e.g. "Transformation of GNSS
    /// coordinates…".
    pub(crate) scope: Option<&'static str>,
    /// EPSG's variant number: the operations between two CRSs are numbered
    /// in the order EPSG added them. 0 when it has none.
    pub(crate) variant: u32,
    pub(crate) deprecated: bool,
    /// EPSG's remarks say "Replaced by".
    pub(crate) replaced: bool,
    /// The operations EPSG's remarks say this one emulates with another
    /// method ("Emulation using NTv2 method of …, code 9327").
    pub(crate) emulates: Vec<u32>,
    /// Extent codes, one per usage.
    pub(crate) extents: Vec<u32>,
    /// The area covering all its usages' areas (`Extent::covering`). PROJ
    /// takes the first usage's area instead, by its database's row order.
    pub(crate) area: Option<Extent>,
    pub(crate) params: Vec<(u32, ParamValue)>,
    /// Steps of a concatenated operation, in order, directions not given.
    pub(crate) steps: Vec<u32>,
    /// Operations that supersede this one: (code, same source and target
    /// CRSs by PROJ's rule, table of the replacement).
    pub(crate) superseded_by: Vec<(u32, bool, Table)>,
}

impl Operation {
    /// A numeric parameter by EPSG code.
    pub(crate) fn param(&self, code: u32) -> Option<f64> {
        self.params.iter().find_map(|(c, v)| match v {
            ParamValue::Number(n) if *c == code => Some(*n),
            _ => None,
        })
    }

    /// A file parameter by EPSG code.
    pub(crate) fn file(&self, code: u32) -> Option<&str> {
        self.params.iter().find_map(|(c, v)| match v {
            ParamValue::File(f) if *c == code => Some(f.as_str()),
            _ => None,
        })
    }

    /// Every grid file the operation reads, as EPSG names them (for NADCON's
    /// pair of files, the latitude one).
    pub(crate) fn grid_names(&self) -> Vec<&str> {
        match crate::operation::grid::file_parameter(self.method) {
            Some(code) => self.file(code).into_iter().collect(),
            None => self
                .params
                .iter()
                .filter_map(|(_, v)| match v {
                    ParamValue::File(f) => Some(f.as_str()),
                    _ => None,
                })
                .collect(),
        }
    }

    /// The grid file PROJ reads for this operation, as EPSG names it: for
    /// NADCON's pair of files, the latitude one, which PROJ's catalog maps
    /// to the single GeoTIFF.
    pub(crate) fn grid_name(&self) -> Option<&str> {
        match crate::operation::grid::file_parameter(self.method) {
            Some(code) => self.file(code),
            None => self.params.iter().find_map(|(_, v)| match v {
                ParamValue::File(f) => Some(f.as_str()),
                _ => None,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GeodeticCrs {
    pub(crate) code: u32,
    pub(crate) name: &'static str,
    pub(crate) kind: GeodeticKind,
    pub(crate) datum: u32,
    pub(crate) deprecated: bool,
}

/// A vertical CRS, with the datum PROJ gives it: a CRS derived from another
/// (depth, another unit) has its base's.
#[derive(Debug, Clone)]
pub(crate) struct VerticalCrs {
    pub(crate) code: u32,
    pub(crate) name: &'static str,
    pub(crate) datum: u32,
    pub(crate) deprecated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GeodeticKind {
    Geographic2d,
    Geographic3d,
    Geocentric,
}

// Some fields are kept for diagnostics and future rules.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct Datum {
    pub(crate) code: u32,
    pub(crate) name: &'static str,
    /// As PROJ derives it: the frame reference epoch, else the publication
    /// date, as `YYYY-MM-DD`. None for ensembles.
    pub(crate) publication_date: Option<&'static str>,
    pub(crate) ellipsoid: u32,
    pub(crate) prime_meridian: u32,
    /// Members, for a datum ensemble.
    pub(crate) members: Option<Vec<u32>>,
    pub(crate) deprecated: bool,
    /// A dynamic reference frame (it has a frame reference epoch).
    pub(crate) dynamic: bool,
    /// For a datum ensemble: how far apart its members may be, in metres.
    pub(crate) ensemble_accuracy: Option<f64>,
    /// The anchor epoch, as a decimal year.
    pub(crate) anchor_epoch: Option<f64>,
    /// A vertical datum, or an ensemble of them.
    pub(crate) vertical: bool,
    /// EPSG's publication date, as `YYYY-MM-DD`: when this realization was
    /// published (`publication_date` gives every ETRF 1989, its frame
    /// reference epoch). None for ensembles.
    pub(crate) published: Option<&'static str>,
}

#[derive(Debug)]
pub(crate) struct Registry {
    pub(crate) ops: HashMap<u32, Operation>,
    /// Operations with this CRS as source or target, by code.
    pub(crate) by_crs: HashMap<u32, Vec<u32>>,
    pub(crate) geodetic: HashMap<u32, GeodeticCrs>,
    pub(crate) vertical: HashMap<u32, VerticalCrs>,
    /// Live vertical CRSs on each datum, by code.
    pub(crate) vertical_by_datum: HashMap<u32, Vec<u32>>,
    /// Live geodetic CRSs on each datum, by code.
    pub(crate) crs_by_datum: HashMap<u32, Vec<u32>>,
    pub(crate) datums: HashMap<u32, Datum>,
    /// Each CRS's area of use: the area covering all its usages'
    /// (`Extent::covering`). PROJ takes the first in its database's order.
    pub(crate) crs_extent: HashMap<u32, Extent>,
}

/// The registry, parsed on first use.
pub(crate) fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let doc: Doc = serde_json::from_str(xeibe_crs::operations_json())
            .expect("xeibe-crs ships valid operations JSON");
        Registry::from_doc(doc)
    })
}

// The parts of `xeibe_crs::operations_json` the registry reads, as
// `scripts/gen_crs_tables.py` writes them; serde skips the rest. Read straight
// into these rather than through a `serde_json::Value` tree, which took most
// of the registry's start-up time to build and free. Rows other than
// operations are arrays, which serde reads into a struct in field order.
// Their text is borrowed from the JSON, which `xeibe_crs` keeps for the life
// of the program ([`kept`]).

#[derive(Deserialize)]
struct Doc<'a> {
    #[serde(borrow)]
    operations: Vec<OperationRow<'a>>,
    #[serde(borrow)]
    geodetic_crs: Vec<GeodeticRow<'a>>,
    #[serde(borrow)]
    vertical_crs: Vec<VerticalRow<'a>>,
    #[serde(borrow)]
    datums: Vec<DatumRow<'a>>,
    #[serde(borrow)]
    extents: Vec<ExtentRow<'a>>,
    crs_extent: Vec<(u32, Vec<u32>)>,
}

#[derive(Deserialize)]
struct OperationRow<'a> {
    code: u32,
    #[serde(borrow)]
    name: Cow<'a, str>,
    table: &'a str,
    method: Option<u32>,
    src: Option<u32>,
    dst: Option<u32>,
    accuracy: Option<f64>,
    #[serde(borrow)]
    scope: Option<Cow<'a, str>>,
    #[serde(default)]
    variant: u32,
    #[serde(default)]
    deprecated: bool,
    #[serde(default)]
    replaced: bool,
    #[serde(default)]
    emulates: Vec<u32>,
    #[serde(default)]
    extents: Vec<u32>,
    #[serde(default)]
    params: Vec<(u32, ParamValue)>,
    #[serde(default)]
    steps: Vec<u32>,
    #[serde(default, borrow)]
    superseded_by: Vec<(u32, bool, &'a str)>,
}

/// A number, a grid file name, or null for a value EPSG leaves out.
impl<'de> Deserialize<'de> for ParamValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = ParamValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a number, a file name or null")
            }
            fn visit_f64<E>(self, v: f64) -> Result<ParamValue, E> {
                Ok(ParamValue::Number(v))
            }
            fn visit_u64<E>(self, v: u64) -> Result<ParamValue, E> {
                Ok(ParamValue::Number(v as f64))
            }
            fn visit_i64<E>(self, v: i64) -> Result<ParamValue, E> {
                Ok(ParamValue::Number(v as f64))
            }
            fn visit_str<E>(self, v: &str) -> Result<ParamValue, E> {
                Ok(ParamValue::File(v.to_owned()))
            }
            fn visit_string<E>(self, v: String) -> Result<ParamValue, E> {
                Ok(ParamValue::File(v))
            }
            fn visit_unit<E>(self) -> Result<ParamValue, E> {
                Ok(ParamValue::Missing)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Deserialize)]
struct GeodeticRow<'a> {
    code: u32,
    #[serde(borrow)]
    name: Cow<'a, str>,
    kind: &'a str,
    datum: u32,
    deprecated: u8,
}

#[derive(Deserialize)]
struct VerticalRow<'a> {
    code: u32,
    #[serde(borrow)]
    name: Cow<'a, str>,
    datum: u32,
    deprecated: u8,
}

#[derive(Deserialize)]
struct DatumRow<'a> {
    code: u32,
    #[serde(borrow)]
    name: Cow<'a, str>,
    publication_date: Option<&'a str>,
    ellipsoid: Option<u32>,
    prime_meridian: Option<u32>,
    members: Option<Vec<u32>>,
    deprecated: u8,
    dynamic: bool,
    ensemble_accuracy: Option<f64>,
    anchor_epoch: Option<f64>,
    vertical: bool,
    published: Option<&'a str>,
}

#[derive(Deserialize)]
struct ExtentRow<'a> {
    code: u32,
    #[serde(borrow)]
    name: Option<Cow<'a, str>>,
    south: Option<f64>,
    west: Option<f64>,
    north: Option<f64>,
    east: Option<f64>,
}

/// A string of the operations JSON: borrowed, or for the few with escapes
/// (13 of 67,000: `\"`, `\r\n`) unescaped once and kept, as the registry
/// keeps everything else.
fn kept(text: Cow<'static, str>) -> &'static str {
    match text {
        Cow::Borrowed(text) => text,
        Cow::Owned(text) => Box::leak(text.into_boxed_str()),
    }
}

fn table(name: &str) -> Table {
    match name {
        "helmert_transformation" => Table::Helmert,
        "grid_transformation" => Table::Grid,
        "other_transformation" => Table::Other,
        "concatenated_operation" => Table::Concatenated,
        _ => Table::Conversion,
    }
}

impl Registry {
    fn from_doc(doc: Doc<'static>) -> Registry {
        let mut ops = HashMap::new();
        let mut by_crs: HashMap<u32, Vec<u32>> = HashMap::new();
        for o in doc.operations {
            let superseded_by = o
                .superseded_by
                .into_iter()
                .map(|(code, same, name)| (code, same, table(name)))
                .collect();
            let op = Operation {
                code: o.code,
                name: Cow::Borrowed(kept(o.name)),
                table: table(o.table),
                method: o.method.unwrap_or(0),
                src: o.src,
                dst: o.dst,
                // EPSG's 999 means unknown; PROJ's factory reads it so.
                accuracy: o.accuracy.filter(|&a| a != 999.0),
                scope: o.scope.map(kept),
                variant: o.variant,
                deprecated: o.deprecated,
                replaced: o.replaced,
                emulates: o.emulates,
                extents: o.extents,
                area: None,
                params: o.params,
                steps: o.steps,
                superseded_by,
            };
            for crs in [op.src, op.dst].into_iter().flatten() {
                let list = by_crs.entry(crs).or_default();
                if !list.contains(&op.code) {
                    list.push(op.code);
                }
            }
            ops.insert(op.code, op);
        }

        let mut geodetic = HashMap::new();
        let mut crs_by_datum: HashMap<u32, Vec<u32>> = HashMap::new();
        for g in doc.geodetic_crs {
            let kind = match g.kind {
                "geographic 3D" => GeodeticKind::Geographic3d,
                "geocentric" => GeodeticKind::Geocentric,
                _ => GeodeticKind::Geographic2d,
            };
            let crs = GeodeticCrs {
                code: g.code,
                name: kept(g.name),
                kind,
                datum: g.datum,
                deprecated: g.deprecated == 1,
            };
            if !crs.deprecated {
                crs_by_datum.entry(crs.datum).or_default().push(crs.code);
            }
            geodetic.insert(crs.code, crs);
        }
        for list in crs_by_datum.values_mut() {
            list.sort_unstable();
        }

        let mut vertical = HashMap::new();
        let mut vertical_by_datum: HashMap<u32, Vec<u32>> = HashMap::new();
        for v in doc.vertical_crs {
            let crs = VerticalCrs {
                code: v.code,
                name: kept(v.name),
                datum: v.datum,
                deprecated: v.deprecated == 1,
            };
            if !crs.deprecated {
                vertical_by_datum
                    .entry(crs.datum)
                    .or_default()
                    .push(crs.code);
            }
            vertical.insert(crs.code, crs);
        }
        for list in vertical_by_datum.values_mut() {
            list.sort_unstable();
        }

        let mut datums = HashMap::new();
        for d in doc.datums {
            let datum = Datum {
                code: d.code,
                name: kept(d.name),
                publication_date: d.publication_date,
                ellipsoid: d.ellipsoid.unwrap_or(0),
                prime_meridian: d.prime_meridian.unwrap_or(0),
                members: d.members,
                deprecated: d.deprecated == 1,
                dynamic: d.dynamic,
                ensemble_accuracy: d.ensemble_accuracy,
                anchor_epoch: d.anchor_epoch,
                vertical: d.vertical,
                published: d.published,
            };
            datums.insert(datum.code, datum);
        }

        let mut extents = HashMap::new();
        for x in doc.extents {
            let extent = Extent::new(
                x.name.map(kept),
                x.west.unwrap_or(-180.0),
                x.south.unwrap_or(-90.0),
                x.east.unwrap_or(180.0),
                x.north.unwrap_or(90.0),
            );
            extents.insert(x.code, extent);
        }
        for op in ops.values_mut() {
            let areas: Vec<&Extent> = op.extents.iter().filter_map(|x| extents.get(x)).collect();
            op.area = Extent::covering(&areas);
        }

        let crs_extent = doc
            .crs_extent
            .into_iter()
            .filter_map(|(code, codes)| {
                let areas: Vec<&Extent> = codes.iter().filter_map(|x| extents.get(x)).collect();
                Some((code, Extent::covering(&areas)?))
            })
            .collect();

        for op in ensemble_operations(&ops, &geodetic, &vertical, &datums, &crs_extent) {
            for crs in [op.src, op.dst].into_iter().flatten() {
                by_crs.entry(crs).or_default().push(op.code);
            }
            ops.insert(op.code, op);
        }

        Registry {
            ops,
            by_crs,
            geodetic,
            vertical,
            vertical_by_datum,
            crs_by_datum,
            datums,
            crs_extent,
        }
    }
}

/// Codes from here on are PROJ's operations, not EPSG's
/// ([`ensemble_operations`]).
pub(crate) const PROJ_CODES: u32 = 0x4000_0000;

/// The null operations PROJ's `build_db.py`
/// (`create_datumensemble_transformations`) adds between the CRS of a datum
/// ensemble and the CRS of each of its members, with the ensemble's
/// accuracy and the member CRS's area: "WGS 84 to WGS 84 (G2296)", "ETRS89
/// to ETRF2014", "BI height to ODN height". PROJ's search uses them like
/// any other operation, so routes may pass through an ensemble anywhere.
///
/// Each datum's CRS is its only live geographic 2D CRS (not a "(lon-lat)"
/// one), or its only live vertical CRS; datums with none or several are
/// skipped, where PROJ's script would stop. A geodetic member is also
/// skipped where EPSG has an operation named "A to B (1)" between the two.
fn ensemble_operations(
    ops: &HashMap<u32, Operation>,
    geodetic: &HashMap<u32, GeodeticCrs>,
    vertical: &HashMap<u32, VerticalCrs>,
    datums: &HashMap<u32, Datum>,
    crs_extent: &HashMap<u32, Extent>,
) -> Vec<Operation> {
    let crs_of = |datum: u32, is_vertical: bool| -> Option<(u32, &str)> {
        let mut found: Vec<(u32, &str)> = if is_vertical {
            vertical
                .values()
                .filter(|c| !c.deprecated && c.datum == datum)
                .map(|c| (c.code, c.name))
                .collect()
        } else {
            geodetic
                .values()
                .filter(|c| {
                    !c.deprecated
                        && c.datum == datum
                        && c.kind == GeodeticKind::Geographic2d
                        && !c.name.ends_with("(lon-lat)")
                })
                .map(|c| (c.code, c.name))
                .collect()
        };
        (found.len() == 1).then(|| found.pop()).flatten()
    };
    let names: std::collections::HashSet<&str> = ops.values().map(|o| &*o.name).collect();
    let mut ensembles: Vec<&Datum> = datums
        .values()
        .filter(|d| d.members.is_some() && d.ensemble_accuracy.is_some())
        .collect();
    ensembles.sort_by_key(|d| d.code);
    let mut out = Vec::new();
    for ensemble in ensembles {
        let Some((src, src_name)) = crs_of(ensemble.code, ensemble.vertical) else {
            continue;
        };
        for &member in ensemble.members.iter().flatten() {
            let Some((dst, dst_name)) = crs_of(member, ensemble.vertical) else {
                continue;
            };
            if !ensemble.vertical
                && (names.contains(format!("{dst_name} to {src_name} (1)").as_str())
                    || names.contains(format!("{src_name} to {dst_name} (1)").as_str()))
            {
                continue;
            }
            let (method, params) = if ensemble.vertical {
                (9616, vec![(8603, ParamValue::Number(0.0))])
            } else {
                let zero = |c| (c, ParamValue::Number(0.0));
                (9603, vec![zero(8605), zero(8606), zero(8607)])
            };
            out.push(Operation {
                code: PROJ_CODES + out.len() as u32,
                name: format!("{src_name} to {dst_name}").into(),
                table: if ensemble.vertical {
                    Table::Other
                } else {
                    Table::Helmert
                },
                method,
                src: Some(src),
                dst: Some(dst),
                accuracy: ensemble.ensemble_accuracy,
                scope: None,
                variant: 0,
                deprecated: false,
                replaced: false,
                emulates: Vec::new(),
                extents: Vec::new(),
                area: crs_extent.get(&dst).cloned(),
                params,
                steps: Vec::new(),
                superseded_by: Vec::new(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_registry() {
        let r = registry();
        let op = &r.ops[&1149];
        assert_eq!(op.name, "ETRS89 to WGS 84 (1)");
        assert_eq!(
            (op.src, op.dst, op.accuracy),
            (Some(4258), Some(4326), Some(1.0))
        );
        assert_eq!(op.param(8605), Some(0.0));
        assert!(r.by_crs[&4258].contains(&1149));
        assert_eq!(r.geodetic[&4258].kind, GeodeticKind::Geographic2d);
        // The ETRS89 ensemble lists ETRF2000-PL among its members.
        assert!(r.datums[&6258].members.as_ref().unwrap().contains(&1305));
        // ETRF2020: frame reference epoch 1989, published in 2024.
        assert_eq!(r.datums[&1382].publication_date, Some("1989-01-01"));
        assert_eq!(r.datums[&1382].published, Some("2024-01-31"));
        let etrs89 = &r.crs_by_datum[&6258];
        assert!(etrs89.is_sorted() && etrs89.contains(&4258));
        let poland = &r.crs_extent[&2180];
        assert_eq!(poland.name, Some("Poland - onshore and offshore."));
        assert_eq!((poland.west, poland.east), (14.14, 24.15));
    }

    #[test]
    fn adds_proj_ensemble_operations() {
        let r = registry();
        let proj: Vec<&Operation> = r.ops.values().filter(|o| o.code >= PROJ_CODES).collect();
        let named = |name: &str| *proj.iter().find(|o| o.name == name).unwrap();
        let g2296 = named("WGS 84 to WGS 84 (G2296)");
        assert_eq!((g2296.src, g2296.accuracy), (Some(4326), Some(2.0)));
        assert_eq!(g2296.param(8605), Some(0.0));
        assert!(r.by_crs[&4326].contains(&g2296.code));
        assert_eq!(named("ETRS89 to ETRF2014").accuracy, Some(0.1));
        let odn = named("BI height to ODN height");
        assert_eq!((odn.method, odn.accuracy), (9616, Some(0.4)));
        // Every one PROJ 9.8.1 adds from EPSG 13.106's ensembles
        // (target/proj-oracle-epsg/proj.db).
        assert_eq!(proj.len(), 91);
    }
}
