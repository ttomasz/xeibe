//! Pages of one feature type streamed into a read: capabilities → hits → pages,
//! each page fetched whole into memory, checked for exceptions and truncation,
//! and handed on as a source. Nothing is saved (see `docs/wfs.md`).

use xeibe_core::{Source, Sources};

use crate::{Capabilities, WfsOptions};

/// Progress callback payload.
#[derive(Debug, Clone)]
pub struct Progress {
    pub type_name: String,
    pub pages: u64,
    pub features: u64,
    pub number_matched: Option<u64>,
}

pub struct WfsClient {
    endpoint: url::Url,
    options: WfsOptions,
    http: crate::http::HttpClient,
}

impl WfsClient {
    pub fn new(endpoint: &str, options: WfsOptions) -> crate::Result<Self> {
        todo!()
    }

    pub fn capabilities(&self) -> crate::Result<Capabilities> {
        todo!()
    }

    /// `resultType=hits`.
    pub fn count(&self, type_name: &str) -> crate::Result<Option<u64>> {
        todo!()
    }

    /// Every page of `type_name`, lazily: the next page is requested when the read
    /// needs more features (up to `options.concurrency` pages ahead, kept in order).
    /// A page that fails after retries ends the sequence with an error.
    pub fn pages(&self, type_name: &str, progress: Box<dyn FnMut(&Progress) + Send>) -> crate::Result<Sources> {
        todo!()
    }

    /// Fetch one page completely; retried whole, or with half the page size after
    /// a truncated response.
    fn fetch_page(&self, url: &url::Url, index: u64) -> crate::Result<Source> {
        todo!()
    }
}
