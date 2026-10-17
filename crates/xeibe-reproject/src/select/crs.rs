//! What the selection needs to know about a CRS: its kind, name, datum, base
//! CRS and area of use. CRSs are EPSG codes, plus the 3D twin PROJ
//! synthesises for a geographic 2D CRS that EPSG has no 3D version of.

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrsKind {
    Geographic2d,
    Geographic3d,
    Geocentric,
    Projected,
    /// Vertical, compound, engineering…: not handled by level 2.
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

fn look_up(crs: CrsRef) -> Option<CrsFacts> {
    let r = registry();
    let code = match crs {
        CrsRef::Epsg(code) | CrsRef::Promoted(code) => code,
    };
    let extent = r.crs_extent.get(&code).cloned();
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
        });
    }
    let record = xeibe_crs::get(code)?;
    if record.kind != xeibe_crs::CrsKind::Projected {
        return Some(CrsFacts {
            name: record.name.to_owned(),
            kind: CrsKind::Other,
            datum: None,
            base: None,
            conversion: None,
            extent,
        });
    }
    let doc: Value = serde_json::from_str(xeibe_crs::projjson(code)?).ok()?;
    let id = |v: &Value| v["id"]["code"].as_u64().and_then(|c| u32::try_from(c).ok());
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
    })
}
