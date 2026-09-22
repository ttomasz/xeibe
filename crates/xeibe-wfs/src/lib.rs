//! WFS bulk-read client: pages streamed into a read. See `docs/wfs.md`.

pub mod capabilities;
pub mod error;
pub mod exception;
#[cfg(feature = "http")]
pub mod http;
pub mod options;
#[cfg(feature = "http")]
pub mod pages;
pub mod paging;
pub mod request;
pub mod response;
mod xml;

pub use capabilities::{Capabilities, FeatureTypeInfo, WfsVersion};
pub use error::{Error, Result};
pub use options::WfsOptions;
#[cfg(feature = "http")]
pub use pages::WfsClient;
pub use paging::PagingStrategy;
