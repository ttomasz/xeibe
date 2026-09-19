pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] xeibe_core::Error),

    #[error(transparent)]
    Geom(#[from] xeibe_geom::Error),

    #[error(transparent)]
    Schema(#[from] xeibe_schema::Error),

    #[error(transparent)]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("feature error at {location}: {message}")]
    Feature {
        location: xeibe_core::Location,
        message: String,
    },

    #[error("settings file {path}: {message}")]
    Settings { path: String, message: String },

    #[error("column {column}: invalid type {type_string:?}: {message}")]
    ColumnType { column: String, type_string: String, message: String },

    /// A field marked non-null in a given Arrow schema has no value in this feature.
    /// Handled per `OnFeatureError`, like any feature error.
    #[error("missing value for non-null field {column} at {location}")]
    MissingValue {
        location: xeibe_core::Location,
        column: String,
    },

    #[error("value does not match the schema at {location}: {message}")]
    SchemaMismatch {
        location: xeibe_core::Location,
        message: String,
    },
}
