//! WFS bulk-read client: pages streamed into a read. See `docs/wfs.md`.

// Skeleton phase: signatures only, bodies are `todo!()`.
#![allow(dead_code, unused_variables)]

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

pub use capabilities::{Capabilities, FeatureTypeInfo, WfsVersion};
pub use error::{Error, Result};
pub use options::WfsOptions;
#[cfg(feature = "http")]
pub use pages::WfsClient;
pub use paging::PagingStrategy;
