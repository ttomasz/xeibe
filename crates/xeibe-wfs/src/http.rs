//! WFS requests on top of [`xeibe_io::http::HttpClient`] (retries, backoff, auth).
//! Adds WFS specifics: OWS exception detection in 200 responses.

use url::Url;

use crate::exception::ExceptionReport;
use crate::options::WfsOptions;

#[derive(Debug)]
pub struct HttpClient {
    inner: xeibe_io::http::HttpClient,
}

impl HttpClient {
    pub fn new(options: &WfsOptions) -> crate::Result<Self> {
        Ok(Self {
            inner: xeibe_io::http::HttpClient::new(&options.http_options())?,
        })
    }

    /// The whole body (gzip/deflate `Content-Encoding` undone), retried until
    /// complete. An exception report is [`crate::Error::Exception`], whether it
    /// came with an error status or with 200.
    pub fn get(&self, url: &Url) -> crate::Result<bytes::Bytes> {
        let body = self.inner.get(url)?;
        match ExceptionReport::parse(&body) {
            Some(report) => Err(crate::Error::Exception(report)),
            None => Ok(body),
        }
    }
}
