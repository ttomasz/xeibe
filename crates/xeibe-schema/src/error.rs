pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] xeibe_core::Error),

    #[error(transparent)]
    Geom(#[from] xeibe_geom::Error),

    #[error("invalid path pattern {pattern:?}: {message}")]
    Pattern { pattern: String, message: String },

    #[error("given schema does not fit layer {layer}: {message}")]
    Bind { layer: String, message: String },

    #[error("layer not found: {0}")]
    UnknownLayer(String),
}
