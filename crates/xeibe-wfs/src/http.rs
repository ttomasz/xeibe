//! WFS requests on top of [`xeibe_io::http::HttpClient`] (retries, backoff, auth).
//! Adds WFS specifics: OWS exception detection in 200 responses, and redirected
//! `POST`s.

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
        Ok(self.get_with_url(url)?.0)
    }

    /// [`Self::get`], with the URL the body came from after redirects.
    pub fn get_with_url(&self, url: &Url) -> crate::Result<(bytes::Bytes, Url)> {
        let (body, final_url) = self.inner.get_with_url(url)?;
        match ExceptionReport::parse(&body) {
            Some(report) => Err(crate::Error::Exception(report)),
            None => Ok((body, final_url)),
        }
    }

    /// An XML request (`text/xml`), answered like [`Self::get`].
    ///
    /// A redirect may have turned the `POST` into a bare `GET` (see
    /// [`xeibe_io::http::HttpClient::post`]). A redirected request that fails,
    /// or whose answer is not a feature collection (a bare `GET` may be
    /// answered with the capabilities, which would otherwise read as one
    /// feature), is [`crate::Error::PostRedirected`]. A feature collection
    /// from another URL came through a redirect that kept the `POST`.
    pub fn post(&self, url: &Url, xml: &str) -> crate::Result<bytes::Bytes> {
        let redirected = |to: &str, error: crate::Error| {
            if to == url.as_str() {
                error
            } else {
                crate::Error::PostRedirected {
                    to: to.to_string(),
                    error: Box::new(error),
                }
            }
        };
        let (body, final_url) = match self.inner.post(url, xml.as_bytes(), "text/xml") {
            Ok(response) => response,
            Err(error) => {
                let to = match &error {
                    xeibe_io::Error::HttpStatus { url, .. } => Some(url.clone()),
                    _ => None,
                };
                let error = crate::Error::from(error);
                return Err(match to {
                    Some(to) => redirected(&to, error),
                    None => error,
                });
            }
        };
        if let Some(report) = ExceptionReport::parse(&body) {
            return Err(redirected(
                final_url.as_str(),
                crate::Error::Exception(report),
            ));
        }
        let collection = crate::xml::utf8(&body)
            .ok()
            .and_then(|document| crate::xml::root_name(&document))
            .is_some_and(|(_, local)| local == "FeatureCollection");
        if final_url != *url && !collection {
            return Err(redirected(
                final_url.as_str(),
                crate::Error::Unsupported("the response is not a feature collection".into()),
            ));
        }
        Ok(body)
    }
}
