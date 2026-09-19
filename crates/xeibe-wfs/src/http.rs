//! WFS requests on top of [`xeibe_io::http::HttpClient`] (retries, backoff, auth).
//! Adds WFS specifics: OWS exception detection in 200 responses.

use url::Url;

use crate::options::WfsOptions;

pub struct HttpClient {
    inner: xeibe_io::http::HttpClient,
}

impl HttpClient {
    pub fn new(options: &WfsOptions) -> crate::Result<Self> {
        todo!()
    }

    pub fn get(&self, url: &Url) -> crate::Result<bytes::Bytes> {
        todo!()
    }
}
