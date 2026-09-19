pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] xeibe_core::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP {status} for {url}")]
    HttpStatus { url: String, status: u16 },

    #[error("network error for {url}: {message}")]
    Network { url: String, message: String },

    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}
