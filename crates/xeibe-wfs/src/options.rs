use std::time::Duration;

use crate::PagingStrategy;

#[derive(Debug, Clone)]
pub struct WfsOptions {
    /// Version to request; `None` takes the one the server answers
    /// `GetCapabilities` with (its highest).
    pub version: Option<crate::WfsVersion>,
    pub page_size: Option<u64>,
    /// Forced paging strategy; `None` chooses one (see [`crate::paging::choose`]).
    pub strategy: Option<PagingStrategy>,
    pub srs_name: Option<String>,
    pub bbox: Option<([f64; 4], Option<String>)>,
    pub filter: Option<String>,
    /// Property to sort by for stable offset paging. Not detected yet: `None`
    /// sends no `SORTBY`.
    pub sort_by: Option<String>,
    pub vendor_params: Vec<(String, String)>,
    /// Pages held at once: the one being read plus `concurrency - 1` fetched
    /// ahead of the read, one request at a time, in order. 1 (the default)
    /// fetches a page only when the read asks for it.
    pub concurrency: usize,
    pub max_retries: u32,
    pub timeout: Duration,
    /// Remove duplicate `gml:id`s across pages (memory ∝ id count). Reported
    /// either way. Removal is not implemented yet and fails `pages`.
    pub dedupe: bool,
    pub auth: xeibe_io::Auth,
    pub headers: Vec<(String, String)>,
}

impl Default for WfsOptions {
    /// HTTP settings as in [`xeibe_io::HttpOptions::default`] (30 s, 3 retries).
    fn default() -> Self {
        let http = xeibe_io::HttpOptions::default();
        Self {
            version: None,
            page_size: None,
            strategy: None,
            srs_name: None,
            bbox: None,
            filter: None,
            sort_by: None,
            vendor_params: Vec::new(),
            concurrency: 1,
            max_retries: http.max_retries,
            timeout: http.timeout,
            dedupe: false,
            auth: xeibe_io::Auth::None,
            headers: Vec::new(),
        }
    }
}

impl WfsOptions {
    #[cfg(feature = "http")]
    pub(crate) fn http_options(&self) -> xeibe_io::HttpOptions {
        xeibe_io::HttpOptions {
            timeout: self.timeout,
            max_retries: self.max_retries,
            auth: self.auth.clone(),
            headers: self.headers.clone(),
            ..xeibe_io::HttpOptions::default()
        }
    }
}
