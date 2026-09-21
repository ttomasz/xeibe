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

    /// A header, credential or other setting that cannot be used as given.
    #[error("invalid option: {0}")]
    InvalidOption(String),
}

/// For [`xeibe_core::ByteSource::open`], which returns core errors: those pass
/// through, the rest become I/O errors with the same message.
impl From<Error> for xeibe_core::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Core(error) => error,
            Error::Io(error) => xeibe_core::Error::Io(error),
            other => xeibe_core::Error::Io(std::io::Error::other(other.to_string())),
        }
    }
}
