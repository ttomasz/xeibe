use crate::Location;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("XML error at {location}: {message}")]
    Xml { location: Location, message: String },

    #[error("unsupported character encoding: {0}")]
    UnsupportedEncoding(String),

    #[error("document type declarations are not supported (at {0})")]
    DtdNotSupported(Location),

    /// The splitter raises it with the [`crate::SourceId`]; a scan or read
    /// of several sources skips such a source and raises it (with names,
    /// see [`Error::no_features_in`]) only when every source was skipped.
    #[error("no feature collection or feature member found in {0}")]
    NoFeatures(String),

    #[error("source can only be read once and was already consumed: {0}")]
    NotReopenable(String),

    #[error("zip archive {0} is not a local file; download it first")]
    RemoteArchive(String),

    #[error("zip member {member} in {archive}: {message}")]
    Zip { archive: String, member: String, message: String },

    #[error("no GML members found in zip archive {0}")]
    NoGmlMembers(String),
}

impl Error {
    /// [`Error::NoFeatures`] for a scan or read in which every source, named
    /// here, was skipped.
    pub fn no_features_in(sources: &[String]) -> Self {
        match sources {
            [one] => Error::NoFeatures(one.clone()),
            many => Error::NoFeatures(format!("any of the {} sources", many.len())),
        }
    }
}
