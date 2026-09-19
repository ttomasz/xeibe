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

    #[cfg(feature = "http")]
    #[error(transparent)]
    Transport(#[from] reqwest::Error),

    #[error("server exception: {0}")]
    Exception(ExceptionReport),

    #[error("response truncated (page {page})")]
    Truncated { page: u64 },

    #[error("{0}")]
    Unsupported(String),
}
