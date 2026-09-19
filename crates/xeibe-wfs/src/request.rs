//! KVP request building (see the table in `docs/wfs.md` "Request encoding").

use url::Url;

use crate::WfsVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultType {
    Results,
    Hits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct GetFeature {
    pub version: WfsVersion,
    pub type_name: String,
    /// `(prefix, uri)` for `NAMESPACES` (2.0) / `NAMESPACE` (1.1).
    pub namespace: Option<(String, String)>,
    pub srs_name: Option<String>,
    pub output_format: Option<String>,
    /// In the axis order assumed for `srs_name`; optional CRS as 5th value.
    pub bbox: Option<([f64; 4], Option<String>)>,
    /// FES/OGC filter XML, passed through.
    pub filter: Option<String>,
    pub property_names: Vec<String>,
    pub sort_by: Vec<(String, SortOrder)>,
    pub result_type: ResultType,
    /// `COUNT` (2.0) / `MAXFEATURES` (1.x).
    pub count: Option<u64>,
    /// `STARTINDEX` (2.0, or vendor on 1.x).
    pub start_index: Option<u64>,
    /// Vendor parameters, e.g. `CQL_FILTER`.
    pub vendor: Vec<(String, String)>,
}

impl GetFeature {
    pub fn to_url(&self, base: &Url) -> crate::Result<Url> {
        todo!()
    }
}

pub fn get_capabilities_url(base: &Url, version: Option<WfsVersion>) -> crate::Result<Url> {
    todo!()
}
