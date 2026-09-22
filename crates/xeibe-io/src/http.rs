//! Plain HTTP(S) sources (blocking `reqwest`), with retries and backoff
//! (5xx, 429 + `Retry-After`, network errors) before the body starts. Also used by
//! `xeibe-wfs`.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use url::Url;
use xeibe_core::ByteSource;

use crate::options::{Auth, HttpOptions};

/// Wait before the first retry when the server gives no `Retry-After`; doubled
/// for each further retry.
const FIRST_BACKOFF: Duration = Duration::from_millis(500);
/// Longest wait between two attempts, `Retry-After` included.
const MAX_WAIT: Duration = Duration::from_secs(60);
/// Read of the body's first bytes before the stream is handed on: a failure in
/// it can still be retried.
const FIRST_READ: usize = 64 << 10;
/// Longest part of an error response's body kept in [`crate::Error::HttpStatus`].
pub const ERROR_BODY_LIMIT: u64 = 64 << 10;

pub struct HttpClient {
    client: reqwest::blocking::Client,
    options: HttpOptions,
}

impl HttpClient {
    pub fn new(options: &HttpOptions) -> crate::Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in &options.headers {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| crate::Error::InvalidOption(format!("header {name:?}: {e}")))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|e| crate::Error::InvalidOption(format!("header {name}: {e}")))?;
            headers.append(name, value);
        }
        // In the blocking client the timeout applies to each operation (connecting,
        // each read), not to the whole body.
        let client = reqwest::blocking::Client::builder()
            .timeout(options.timeout)
            .connect_timeout(options.timeout)
            .user_agent(options.user_agent.clone())
            .default_headers(headers)
            .build()
            .map_err(|e| crate::Error::InvalidOption(format!("HTTP client: {e}")))?;
        Ok(Self {
            client,
            options: options.clone(),
        })
    }

    /// Whole body in memory (capabilities documents, WFS pages).
    ///
    /// Nothing is handed on before the body is complete, so a body that breaks
    /// off is retried like a failed request.
    pub fn get(&self, url: &Url) -> crate::Result<bytes::Bytes> {
        self.fetch(url, |response| {
            response.bytes().map_err(|e| error_chain(&e))
        })
    }

    /// Streaming body of one `GET`. A connection that breaks mid-body is an I/O
    /// error from the returned reader; it is not retried.
    pub fn get_stream(&self, url: &Url) -> crate::Result<Box<dyn std::io::Read + Send>> {
        let (first, response) = self.start_stream(url)?;
        Ok(stream_reader(url, first, response))
    }

    /// The response and its first bytes: everything up to and including the first
    /// read is retried.
    fn start_stream(&self, url: &Url) -> crate::Result<(Vec<u8>, reqwest::blocking::Response)> {
        self.fetch(url, |mut response| {
            let mut first = vec![0; FIRST_READ];
            loop {
                match response.read(&mut first) {
                    Ok(n) => {
                        first.truncate(n);
                        return Ok((first, response));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(error_chain(&e)),
                }
            }
        })
    }

    /// One `GET` with retries. `body` takes a successful response as far as it
    /// may still be retried; its error is a message and always retried.
    fn fetch<T>(
        &self,
        url: &Url,
        mut body: impl FnMut(reqwest::blocking::Response) -> Result<T, String>,
    ) -> crate::Result<T> {
        let mut attempt = 0;
        loop {
            let retries_left = attempt < self.options.max_retries;
            let wait = match self.request(url).send() {
                Ok(response) if response.status().is_success() => match body(response) {
                    Ok(value) => return Ok(value),
                    Err(_) if retries_left => backoff(attempt),
                    Err(message) => return Err(network_error(url, message)),
                },
                Ok(response) => {
                    let status = response.status();
                    let retriable = status.is_server_error()
                        || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
                    if !(retriable && retries_left) {
                        return Err(crate::Error::HttpStatus {
                            url: url.to_string(),
                            status: status.as_u16(),
                            body: error_body(response),
                        });
                    }
                    retry_after(&response).unwrap_or_else(|| backoff(attempt))
                }
                Err(e) if e.is_builder() || e.is_redirect() || !retries_left => {
                    return Err(network_error(url, error_chain(&e)));
                }
                Err(_) => backoff(attempt),
            };
            std::thread::sleep(wait);
            attempt += 1;
        }
    }

    fn request(&self, url: &Url) -> reqwest::blocking::RequestBuilder {
        let request = self.client.get(url.clone());
        match &self.options.auth {
            Auth::None => request,
            Auth::Basic { user, password } => request.basic_auth(user, Some(password)),
            Auth::Bearer(token) => request.bearer_auth(token),
        }
    }
}

/// The first bytes, then the rest of the body. Errors name the URL.
fn stream_reader(
    url: &Url,
    first: Vec<u8>,
    response: reqwest::blocking::Response,
) -> Box<dyn Read + Send> {
    Box::new(NamedReader {
        inner: std::io::Cursor::new(first).chain(response),
        name: url.to_string(),
    })
}

fn backoff(attempt: u32) -> Duration {
    FIRST_BACKOFF
        .saturating_mul(1 << attempt.min(16))
        .min(MAX_WAIT)
}

/// The start of an error response's body; `None` if it is empty or unreadable.
fn error_body(response: reqwest::blocking::Response) -> Option<String> {
    let mut body = Vec::new();
    response
        .take(ERROR_BODY_LIMIT)
        .read_to_end(&mut body)
        .ok()?;
    (!body.is_empty()).then(|| String::from_utf8_lossy(&body).into_owned())
}

/// `Retry-After` in seconds. The HTTP-date form falls back to the backoff.
fn retry_after(response: &reqwest::blocking::Response) -> Option<Duration> {
    let seconds: u64 = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds).min(MAX_WAIT))
}

fn network_error(url: &Url, message: String) -> crate::Error {
    crate::Error::Network {
        url: url.to_string(),
        message,
    }
}

/// `reqwest`'s own message is generic ("error sending request"); the cause says why.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// A reader whose errors name the source.
struct NamedReader<R> {
    inner: R,
    name: String,
}

impl<R: Read> Read for NamedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf).map_err(|e| {
            let message = format!("{}: {}", self.name, error_chain(&e));
            std::io::Error::new(e.kind(), message)
        })
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
    /// Header values and credentials are left out: they may be secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let auth = match self.options.auth {
            Auth::None => "none",
            Auth::Basic { .. } => "basic",
            Auth::Bearer(_) => "bearer",
        };
        let headers: Vec<&str> = self
            .options
            .headers
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        f.debug_struct("HttpClient")
            .field("timeout", &self.options.timeout)
            .field("max_retries", &self.options.max_retries)
            .field("auth", &auth)
            .field("headers", &headers)
            .field("user_agent", &self.options.user_agent)
            .finish()
    }
}

impl HttpSource {
    pub fn new(client: Arc<HttpClient>, url: Url) -> Self {
        Self {
            client,
            name: url.to_string(),
            url,
        }
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
    /// A zip archive, recognised by its first bytes, is
    /// [`xeibe_core::Error::RemoteArchive`] whatever the URL looks like.
    fn open(&self) -> xeibe_core::Result<Box<dyn std::io::Read + Send>> {
        let (first, response) = self.client.start_stream(&self.url)?;
        if xeibe_core::decode::detect_compression(&first) == xeibe_core::decode::Compression::Zip {
            return Err(xeibe_core::Error::RemoteArchive(self.name.clone()));
        }
        Ok(stream_reader(&self.url, first, response))
    }
}
