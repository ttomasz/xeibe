use crate::exception::ExceptionReport;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] xeibe_core::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),

    #[error("HTTP error {status} for {url}")]
    Http { status: u16, url: String },

    /// Network errors after retries, and unusable HTTP settings.
    #[error(transparent)]
    Transport(xeibe_io::Error),

    #[error("server exception: {0}")]
    Exception(ExceptionReport),

    #[error("response truncated (page {page})")]
    Truncated { page: u64 },

    #[error("feature type {0} is not in the capabilities")]
    UnknownFeatureType(String),

    #[error("{0}")]
    Unsupported(String),
}

/// An HTTP error status whose body is an OWS exception report (a WFS 2.0
/// server answers a bad request with 400 and the report) is
/// [`Error::Exception`].
impl From<xeibe_io::Error> for Error {
    fn from(error: xeibe_io::Error) -> Self {
        match error {
            xeibe_io::Error::HttpStatus { url, status, body } => {
                match body
                    .as_deref()
                    .and_then(|b| ExceptionReport::parse(b.as_bytes()))
                {
                    Some(report) => Error::Exception(report),
                    None => Error::Http { status, url },
                }
            }
            xeibe_io::Error::Core(error) => Error::Core(error),
            xeibe_io::Error::Io(error) => Error::Io(error),
            other => Error::Transport(other),
        }
    }
}

/// For [`xeibe_core::Sources`], whose items carry core errors: those pass
/// through, the rest become I/O errors that keep the WFS error as their source.
impl From<Error> for xeibe_core::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Core(error) => error,
            Error::Io(error) => xeibe_core::Error::Io(error),
            other => xeibe_core::Error::Io(std::io::Error::other(other)),
        }
    }
}
