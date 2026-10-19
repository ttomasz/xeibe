//! Streaming GML reading primitives shared by the other
//! [xeibe](https://github.com/ttomasz/xeibe) crates.
//!
//! <div class="warning">
//!
//! **Experimental, not for production use.** This crate is AI-generated and
//! has not been thoroughly tested yet. Its interfaces are not stable and will
//! change in the near future, in breaking ways.
//!
//! </div>
//!
//! This crate has no Arrow dependency. See the
//! [architecture document](https://github.com/ttomasz/xeibe/blob/main/docs/architecture.md) for the overall data flow.

pub mod archive;
pub mod chunk;
pub mod decode;
pub mod error;
pub mod location;
pub mod namespace;
pub mod parallel;
pub mod qname;
pub mod reader;
pub mod source;
pub mod splitter;
pub mod version;

pub use chunk::{FeatureChunk, FeatureOffset, RawElement};
pub use error::{Error, Result};
pub use location::Location;
pub use namespace::{NamespaceContext, ns};
pub use qname::QName;
pub use source::{
    ByteSource, BytesSource, FileSource, RandomAccess, RandomAccessSource, RandomReader,
    ReaderSource, Source, SourceId, Sources,
};
pub use splitter::{FeatureSplitter, MemberRule, SplitterOptions};
pub use version::{Dialect, GmlVersion};
