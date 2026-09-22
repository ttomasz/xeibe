//! Tests for `xeibe-wfs`: capabilities, request building, response inspection,
//! paging, and the client against a local scripted server. Nothing here
//! touches a real service; `docs/wfs.md` is the specification.

mod capabilities;
#[cfg(feature = "http")]
mod client;
mod paging;
mod request;
mod response;
