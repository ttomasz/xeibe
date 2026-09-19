//! srsName parsing: which CRS, and which *form* it was written in (the form
//! matters for `AxisOrderMode::CrsHeuristic`). See `docs/geometry.md`.

use serde::{Deserialize, Serialize};

/// How an srsName was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SrsNameForm {
    /// `EPSG:2180`
    Short,
    /// `http://www.opengis.net/gml/srs/epsg.xml#2180`
    LegacyUrl,
    /// `urn:ogc:def:crs:EPSG::2180`, versioned `urn:ogc:def:crs:EPSG:6.6:4326`
    OgcUrn,
    /// `urn:x-ogc:def:crs:EPSG::4326`, `urn:x-ogc:def:crs:EPSG:4326`
    ExperimentalUrn,
    /// `urn:EPSG:geographicCRS:4326` (WFS 1.1)
    Wfs11Urn,
    /// `http://www.opengis.net/def/crs/EPSG/0/2180`
    HttpUri,
    /// `http://www.opengis.net/def/crs?authority=EPSG&version=0&code=4326`
    HttpUriKvp,
    /// `urn:ogc:def:crs,crs:EPSG::4269,crs:EPSG::5713`
    CompoundUrn,
    /// `http://www.opengis.net/def/crs-compound?1=…&2=…`
    CompoundUri,
    Unknown,
}

/// A CRS identified by authority and code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CrsRef {
    /// e.g. `EPSG`, `2180`; `OGC`, `CRS84`.
    Code { authority: String, code: String },
    /// Horizontal + vertical (+ …) components, in order.
    Compound(Vec<CrsRef>),
}

impl CrsRef {
    /// `EPSG:2180`, `OGC:CRS84` — the value written to GeoArrow `crs`
    /// metadata with `crs_type = "authority_code"`.
    pub fn authority_code(&self) -> String {
        todo!()
    }

    /// CRS84/CRS83/CRS27: longitude/latitude regardless of authority rules.
    pub fn is_lon_lat_by_definition(&self) -> bool {
        todo!()
    }
}

/// A parsed srsName, keeping the original string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SrsName {
    pub raw: String,
    pub form: SrsNameForm,
    pub crs: Option<CrsRef>,
}

impl SrsName {
    pub fn parse(raw: &str) -> Self {
        todo!()
    }
}
