//! Plain HTTP(S) sources (blocking `reqwest`), with retries and backoff
//! (5xx, 429 + `Retry-After`, network errors) before the body starts. Also used by
//! `xeibe-wfs`.

use std::sync::Arc;

use xeibe_core::ByteSource;
use url::Url;

use crate::options::HttpOptions;

pub struct HttpClient {
    client: reqwest::blocking::Client,
    options: HttpOptions,
}

impl HttpClient {
    pub fn new(options: &HttpOptions) -> crate::Result<Self> {
        todo!()
    }

    /// Whole body in memory (capabilities documents, WFS pages).
    pub fn get(&self, url: &Url) -> crate::Result<bytes::Bytes> {
        todo!()
    }

    /// Streaming body of one `GET`. A connection that breaks mid-body is an I/O
    /// error from the returned reader; it is not retried.
    pub fn get_stream(&self, url: &Url) -> crate::Result<Box<dyn std::io::Read + Send>> {
        todo!()
    }
}

/// One HTTP resource as a [`ByteSource`]: every `open` is a new `GET`.
#[derive(Debug)]
pub struct HttpSource {
    client: Arc<HttpClient>,
    url: Url,
    name: String,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl HttpSource {
    pub fn new(client: Arc<HttpClient>, url: Url) -> Self {
        todo!()
    }
}

impl ByteSource for HttpSource {
    fn name(&self) -> &str {
        &self.name
    }
    /// Unknown until the response arrives.
    fn len(&self) -> Option<u64> {
        None
    }
    fn open(&self) -> xeibe_core::Result<Box<dyn std::io::Read + Send>> {
        todo!()
    }
}
