//! What the selection needs to know about a CRS: its kind, name, datum, base
//! CRS and area of use. CRSs are EPSG codes, plus the 3D twin PROJ
//! synthesises for a geographic 2D CRS that EPSG has no 3D version of, and
//! compound CRSs made of two EPSG CRSs.

use std::collections::HashMap;

use serde_json::Value;

use crate::extent::Extent;
use crate::registry::{GeodeticKind, registry};

/// A CRS in the search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CrsRef {
    Epsg(u32),
    /// A geographic 2D CRS promoted to 3D without an EPSG 3D twin. It keeps
    /// the name, datum and areas of use of the 2D CRS and has no code.
    Promoted(u32),
    /// A compound CRS of a horizontal and a vertical EPSG CRS, without a
    /// code of its own.
    Compound(u32, u32),
}

impl CrsRef {
    /// The EPSG code with the horizontal facts: the CRS's own, or a code-less
    /// compound CRS's horizontal part's.
    pub(crate) fn code(self) -> u32 {
        match self {
            CrsRef::Epsg(code) | CrsRef::Promoted(code) | CrsRef::Compound(code, _) => code,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrsKind {
    Geographic2d,
    Geographic3d,
    Geocentric,
    Projected,
    Vertical,
    Compound,
    /// A local grid: only EPSG's operations reach it.
    Engineering,
    /// Derived and other CRSs: not handled.
    Other,
}

impl CrsKind {
    pub(crate) fn is_geodetic(self) -> bool {
        matches!(
            self,
            CrsKind::Geographic2d | CrsKind::Geographic3d | CrsKind::Geocentric
        )
    }

    pub(crate) fn is_geographic(self) -> bool {
        matches!(self, CrsKind::Geographic2d | CrsKind::Geographic3d)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CrsFacts {
    pub(crate) name: String,
    pub(crate) kind: CrsKind,
    /// Datum (or datum ensemble) code of a geodetic CRS, or of a projected
    /// CRS's base.
    pub(crate) datum: Option<u32>,
    /// Base geographic CRS of a projected CRS.
    pub(crate) base: Option<u32>,
    /// Name and EPSG code of a projected CRS's deriving conversion.
    pub(crate) conversion: Option<(String, Option<u32>)>,
    /// Area of use: the area covering all its usages' areas.
    pub(crate) extent: Option<Extent>,
    /// Datum (or datum ensemble) code of a vertical CRS, or of a compound
    /// CRS's vertical part.
    pub(crate) vertical_datum: Option<u32>,
    /// A compound CRS's horizontal and vertical CRSs.
    pub(crate) components: Option<(u32, u32)>,
    /// A vertical CRS's axis: the size of its unit in metres, and +1 up or
    /// −1 down.
    pub(crate) vertical_axis: Option<(f64, f64)>,
}

/// Facts about CRSs, looked up once per CRS.
#[derive(Debug, Default)]
pub(crate) struct CrsCache {
    facts: HashMap<CrsRef, Option<CrsFacts>>,
}

impl CrsCache {
    pub(crate) fn get(&mut self, crs: CrsRef) -> Option<&CrsFacts> {
        self.facts
            .entry(crs)
            .or_insert_with(|| look_up(crs))
            .as_ref()
    }

    pub(crate) fn kind(&mut self, crs: CrsRef) -> CrsKind {
        self.get(crs).map_or(CrsKind::Other, |f| f.kind)
    }

    pub(crate) fn name(&mut self, crs: CrsRef) -> String {
        self.get(crs).map(|f| f.name.clone()).unwrap_or_default()
    }

    pub(crate) fn datum(&mut self, crs: CrsRef) -> Option<u32> {
        self.get(crs).and_then(|f| f.datum)
    }

    pub(crate) fn extent(&mut self, crs: CrsRef) -> Option<Extent> {
        self.get(crs).and_then(|f| f.extent.clone())
    }

    pub(crate) fn vertical_datum(&mut self, crs: CrsRef) -> Option<u32> {
        self.get(crs).and_then(|f| f.vertical_datum)
    }

    /// A compound CRS's horizontal and vertical CRSs.
    pub(crate) fn components(&mut self, crs: CrsRef) -> Option<(CrsRef, CrsRef)> {
        self.get(crs)
            .and_then(|f| f.components)
            .map(|(h, v)| (CrsRef::Epsg(h), CrsRef::Epsg(v)))
    }

    pub(crate) fn vertical_axis(&mut self, crs: CrsRef) -> Option<(f64, f64)> {
        self.get(crs).and_then(|f| f.vertical_axis)
    }

    /// PROJ's `demoteTo2D` for a geographic 3D CRS: the registered
    /// geographic 2D CRS of the same name and datum (the 2D CRS a synthetic
    /// 3D twin came from). Other CRSs are returned unchanged.
    pub(crate) fn demote_to_2d(&mut self, crs: CrsRef) -> CrsRef {
        match crs {
            CrsRef::Promoted(code) => CrsRef::Epsg(code),
            CrsRef::Epsg(_) if self.kind(crs) == CrsKind::Geographic3d => {
                let name = self.name(crs);
                let datum = self.datum(crs);
                let mut twins: Vec<_> = registry()
                    .geodetic
                    .values()
                    .filter(|g| {
                        g.kind == GeodeticKind::Geographic2d
                            && g.name == name
                            && Some(g.datum) == datum
                    })
                    .collect();
                twins.sort_by_key(|g| (g.deprecated, g.code));
                twins.first().map_or(crs, |g| CrsRef::Epsg(g.code))
            }
            _ => crs,
        }
    }

    /// `extractGeographicCRS`: the CRS itself when geographic, a projected
    /// CRS's base, a compound CRS's horizontal part's.
    pub(crate) fn geographic(&mut self, crs: CrsRef) -> Option<CrsRef> {
        match self.kind(crs) {
            CrsKind::Geographic2d | CrsKind::Geographic3d => Some(crs),
            CrsKind::Projected => self.get(crs)?.base.map(CrsRef::Epsg),
            CrsKind::Compound => {
                let (h, _) = self.components(crs)?;
                self.geographic(h)
            }
            _ => None,
        }
    }

    /// PROJ's `promoteTo3D` for a geographic 2D CRS: the registered
    /// geographic 3D CRS of the same name and datum, else a synthetic twin.
    /// Other CRSs are returned unchanged.
    pub(crate) fn promote_to_3d(&mut self, crs: CrsRef) -> CrsRef {
        let CrsRef::Epsg(code) = crs else {
            return crs;
        };
        if self.kind(crs) != CrsKind::Geographic2d {
            return crs;
        }
        let name = self.name(crs);
        let datum = self.datum(crs);
        let r = registry();
        let mut twins: Vec<_> = r
            .geodetic
            .values()
            .filter(|g| {
                g.kind == GeodeticKind::Geographic3d && g.name == name && Some(g.datum) == datum
            })
            .collect();
        twins.sort_by_key(|g| (g.deprecated, g.code));
        match twins.first() {
            Some(g) => CrsRef::Epsg(g.code),
            None => CrsRef::Promoted(code),
        }
    }
}

/// A compound CRS from its parts' facts.
fn compound(h: u32, v: u32, name: Option<String>, extent: Option<Extent>) -> Option<CrsFacts> {
    let (hf, vf) = (look_up(CrsRef::Epsg(h))?, look_up(CrsRef::Epsg(v))?);
    Some(CrsFacts {
        name: name.unwrap_or_else(|| format!("{} + {}", hf.name, vf.name)),
        kind: CrsKind::Compound,
        datum: hf.datum,
        base: None,
        conversion: None,
        extent,
        vertical_datum: vf.vertical_datum,
        components: Some((h, v)),
        vertical_axis: None,
    })
}

fn look_up(crs: CrsRef) -> Option<CrsFacts> {
    let r = registry();
    let code = match crs {
        CrsRef::Epsg(code) | CrsRef::Promoted(code) => code,
        CrsRef::Compound(h, v) => return compound(h, v, None, None),
    };
    let extent = r.crs_extent.get(&code).cloned();
    if let Some(v) = r.vertical.get(&code) {
        let axis = crate::crs::Crs::from_epsg(code).ok().and_then(|c| {
            c.axes
                .axes
                .first()
                .map(|a| (a.factor, c.height_direction()))
        });
        return Some(CrsFacts {
            name: v.name.clone(),
            kind: CrsKind::Vertical,
            datum: None,
            base: None,
            conversion: None,
            extent,
            vertical_datum: Some(v.datum),
            components: None,
            vertical_axis: axis,
        });
    }
    if let Some(g) = r.geodetic.get(&code) {
        let kind = match (crs, g.kind) {
            (CrsRef::Promoted(_), _) => CrsKind::Geographic3d,
            (_, GeodeticKind::Geographic2d) => CrsKind::Geographic2d,
            (_, GeodeticKind::Geographic3d) => CrsKind::Geographic3d,
            (_, GeodeticKind::Geocentric) => CrsKind::Geocentric,
        };
        return Some(CrsFacts {
            name: g.name.clone(),
            kind,
            datum: Some(g.datum),
            base: None,
            conversion: None,
            extent,
            vertical_datum: None,
            components: None,
            vertical_axis: None,
        });
    }
    let record = xeibe_crs::get(code)?;
    let id = |v: &Value| v["id"]["code"].as_u64().and_then(|c| u32::try_from(c).ok());
    if record.kind == xeibe_crs::CrsKind::Compound {
        let doc: Value = serde_json::from_str(xeibe_crs::projjson(code)?).ok()?;
        let parts = doc["components"].as_array()?;
        let [h, v] = parts.as_slice() else {
            return None;
        };
        return compound(id(h)?, id(v)?, Some(record.name.to_owned()), extent);
    }
    if record.kind != xeibe_crs::CrsKind::Projected {
        let kind = if record.kind == xeibe_crs::CrsKind::Engineering {
            CrsKind::Engineering
        } else {
            CrsKind::Other
        };
        return Some(CrsFacts {
            name: record.name.to_owned(),
            kind,
            datum: None,
            base: None,
            conversion: None,
            extent,
            vertical_datum: None,
            components: None,
            vertical_axis: None,
        });
    }
    let doc: Value = serde_json::from_str(xeibe_crs::projjson(code)?).ok()?;
    let base = id(&doc["base_crs"]);
    let datum = base.and_then(|b| r.geodetic.get(&b)).map(|g| g.datum);
    let conversion = &doc["conversion"];
    Some(CrsFacts {
        name: record.name.to_owned(),
        kind: CrsKind::Projected,
        datum,
        base,
        conversion: Some((
            conversion["name"].as_str().unwrap_or("").to_owned(),
            id(conversion),
        )),
        extent,
        vertical_datum: None,
        components: None,
        vertical_axis: None,
    })
}
