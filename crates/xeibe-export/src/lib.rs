//! A layer's Arrow record batches, as `xeibe-arrow` reads them, written to a
//! file: Parquet with GeoParquet metadata (feature `parquet`) or Arrow IPC
//! (feature `ipc`). See `docs/geometry.md`, "Parquet and GeoParquet output".
//!
//! Writers take one batch at a time, so a caller can stop between batches,
//! and write to any [`std::io::Write`]: a file, or a buffer the caller drains
//! as the file grows.

mod error;
#[cfg(feature = "parquet")]
mod geo_columns;
#[cfg(feature = "ipc")]
mod ipc_writer;
#[cfg(feature = "parquet")]
mod parquet_writer;

pub use error::{Error, Result};
#[cfg(feature = "ipc")]
pub use ipc_writer::IpcWriter;
#[cfg(feature = "parquet")]
pub use parquet_writer::{BboxColumn, ParquetOptions, ParquetWriter};

/// What a finished write did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub rows: u64,
    /// Things written differently than asked, e.g. a CRS GeoParquet can't
    /// name, written as `null`.
    pub warnings: Vec<String>,
}
