//! Response-level facts read from the collection root (without full parsing).

use crate::exception::ExceptionReport;

#[derive(Debug, Clone, Default)]
pub struct ResponseInfo {
    /// `numberMatched` (2.0); `None` for "unknown".
    pub number_matched: Option<u64>,
    /// `numberReturned` (2.0) / `numberOfFeatures` (1.1).
    pub number_returned: Option<u64>,
    pub next: Option<String>,
    pub previous: Option<String>,
    pub time_stamp: Option<String>,
    /// Inner `wfs:FeatureCollection`s (multi-query responses).
    pub nested_collections: u32,
    /// `wfs:member xlink:href` (referenced members).
    pub referenced_members: u64,
    pub truncated: bool,
    /// Closing collection element present (complete page).
    pub complete: bool,
}

pub enum Response {
    Features(ResponseInfo),
    Exception(ExceptionReport),
}

/// Inspect a fetched page (start and end of the document).
pub fn inspect(page: &[u8]) -> crate::Result<Response> {
    todo!()
}
