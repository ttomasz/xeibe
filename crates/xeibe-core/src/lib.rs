//! Streaming GML reading primitives shared by all other crates.
//!
//! This crate has no Arrow dependency. See `docs/architecture.md` for the
//! overall data flow.

pub mod archive;
pub mod chunk;
pub mod decode;
pub mod error;
pub mod location;
pub mod namespace;
pub mod qname;
pub mod reader;
pub mod source;
pub mod splitter;
pub mod version;

pub use chunk::{FeatureChunk, RawElement};
pub use error::{Error, Result};
pub use location::Location;
pub use namespace::{NamespaceContext, ns};
pub use qname::QName;
pub use source::{ByteSource, BytesSource, FileSource, ReaderSource, Source, SourceId, Sources};
pub use splitter::{FeatureSplitter, MemberRule, SplitterOptions};
pub use version::{Dialect, GmlVersion};
