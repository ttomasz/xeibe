//! Byte sources for GML input beyond local files: HTTP(S) and object stores, each
//! read as one sequential stream. See `docs/architecture.md#sources-and-remote-input`.
//!
//! Nothing here downloads to disk, caches or probes for range support.

pub mod error;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "object-store")]
pub mod object_store;
pub mod options;
pub mod resolve;

pub use error::{Error, Result};
pub use options::{Auth, HttpOptions, IoOptions};
pub use resolve::resolve_sources;
