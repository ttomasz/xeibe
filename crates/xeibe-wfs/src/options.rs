use std::time::Duration;

use crate::PagingStrategy;

#[derive(Debug, Clone)]
pub struct WfsOptions {
    pub version: Option<crate::WfsVersion>,
    pub page_size: Option<u64>,
    pub strategy: Option<PagingStrategy>,
    pub srs_name: Option<String>,
    pub bbox: Option<([f64; 4], Option<String>)>,
    pub filter: Option<String>,
    /// Property to sort by for stable paging (auto-detected if `None`).
    pub sort_by: Option<String>,
    pub vendor_params: Vec<(String, String)>,
    /// Pages requested ahead of the read (`startIndex` paging only).
    pub concurrency: usize,
    pub max_retries: u32,
    pub timeout: Duration,
    /// Remove duplicate `gml:id`s across pages (memory ∝ id count). Reported either way.
    pub dedupe: bool,
    pub auth: xeibe_io::Auth,
    pub headers: Vec<(String, String)>,
}

impl Default for WfsOptions {
    fn default() -> Self {
        todo!()
    }
}
