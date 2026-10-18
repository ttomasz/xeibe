//! WFS bulk-read client: pages streamed into a read. See the
//! [WFS document](https://github.com/ttomasz/xeibe/blob/main/docs/wfs.md).
//!
//! <div class="warning">
//!
//! **Experimental, not for production use.** This crate is AI-generated and
//! has not been thoroughly tested yet. Its interfaces are not stable and will
//! change in the near future, in breaking ways.
//!
//! </div>

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
