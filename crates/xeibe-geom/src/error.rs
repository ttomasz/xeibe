use xeibe_core::Location;

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

    #[error("unsupported geometry {element} at {location}")]
    Unsupported { element: String, location: Location },

    #[error("geometry given by reference (xlink:href) at {location}")]
    ByReference { location: Location },

    /// The column's srsNames resolve to more than one CRS (with `MixedCrs::Error`).
    #[error("several CRSs in one column: {0:?}")]
    MixedCrs(Vec<String>),
}
