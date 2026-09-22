use xeibe_core::{Location, SourceId};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] xeibe_core::Error),

    #[error("invalid coordinates at {location}: {message}")]
    InvalidCoordinates { location: Location, message: String },

    #[error("wrong number of positions in {element} at {location}: {found} (expected {expected})")]
    PositionCount {
        element: &'static str,
        location: Location,
        found: usize,
        expected: String,
    },

    /// Structurally wrong geometry: a missing required part, a member of the
    /// wrong kind, collinear `Circle` points, …
    #[error("invalid geometry at {location}: {message}")]
    InvalidGeometry { location: Location, message: String },

    #[error("unsupported geometry {element} at {location}")]
    Unsupported { element: String, location: Location },

    #[error("geometry given by reference (xlink:href) at {location}")]
    ByReference { location: Location },

    /// The column's srsNames resolve to more than one CRS (with `MixedCrs::Error`).
    #[error("several CRSs in one column: {0:?}")]
    MixedCrs(Vec<String>),
}

impl Error {
    /// An invalid-coordinates error raised by code that works on text alone
    /// (e.g. [`crate::parse::parse_pos_list`]); its location is a placeholder
    /// until [`Error::at`] sets the real one.
    pub(crate) fn invalid_coordinates(message: impl Into<String>) -> Self {
        Error::InvalidCoordinates { location: unlocated(), message: message.into() }
    }

    /// Replace the location of a located error (the parser knows where the
    /// element was; the string-level helpers don't).
    pub fn at(mut self, at: Location) -> Self {
        match &mut self {
            Error::InvalidCoordinates { location, .. }
            | Error::InvalidGeometry { location, .. }
            | Error::PositionCount { location, .. }
            | Error::Unsupported { location, .. }
            | Error::ByReference { location } => *location = at,
            Error::Core(_) | Error::MixedCrs(_) => {}
        }
        self
    }
}

/// Placeholder location for errors raised without access to the reader.
pub(crate) fn unlocated() -> Location {
    Location { source: SourceId(0), byte_offset: 0, feature_seq: None, gml_id: None }
}
