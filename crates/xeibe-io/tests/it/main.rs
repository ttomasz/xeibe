//! Tests for `xeibe-io`: resolving inputs, HTTP sources against a local test
//! server, and object-store sources against an in-memory store. Nothing here
//! touches the internet (`docs/architecture.md`, "Sources and remote input").

#[cfg(feature = "http")]
mod http;
#[cfg(feature = "object-store")]
mod object_store;
mod resolve;
#[cfg(feature = "http")]
mod support;
