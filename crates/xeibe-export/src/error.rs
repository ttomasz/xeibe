use arrow_schema::ArrowError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Arrow(#[from] ArrowError),
    #[cfg(feature = "parquet")]
    #[error(transparent)]
    Parquet(#[from] parquet::errors::ParquetError),
    #[cfg(feature = "parquet")]
    #[error(transparent)]
    GeoArrow(#[from] geoarrow_schema::error::GeoArrowError),
    /// Parquet and GeoParquet have no curves; linearize them when reading
    /// (`xeibe_geom::options::CurveMode::Linearize`).
    #[cfg(feature = "parquet")]
    #[error(
        "column {column}: a {kind} (curve) in batch row {row}; Parquet and GeoParquet have only the seven simple geometry types"
    )]
    Curve {
        column: String,
        kind: &'static str,
        row: usize,
    },
    /// A geometry type with no linear equivalent (PolyhedralSurface, TIN, …).
    #[cfg(feature = "parquet")]
    #[error(
        "column {column}: geometry type {kind} can't be written to Parquet, which has only the seven simple geometry types"
    )]
    Unsupported { column: String, kind: String },
    #[cfg(feature = "parquet")]
    #[error("column {column}: malformed WKB in batch row {row}")]
    MalformedWkb { column: String, row: usize },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
