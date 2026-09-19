use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Index of a source within one scan or read, in the order sources were consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceId(pub u32);

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

/// Raw bytes of one input, before decompression and decoding. Always read as one
/// sequential stream from the start (see `docs/architecture.md#sources-and-remote-input`).
///
/// Implemented here for local files and one-shot readers; `xeibe-io` adds HTTP and
/// object stores. Implementations are synchronous: async stores are bridged in
/// `xeibe-io`, so this crate needs no async runtime.
pub trait ByteSource: Send + Sync + std::fmt::Debug {
    /// Path or URL, used in messages.
    fn name(&self) -> &str;

    /// Size in bytes, if known up front (file metadata, `Content-Length`). For progress only.
    fn len(&self) -> Option<u64>;

    /// A fresh stream from the start. One-shot sources (stdin) fail with
    /// [`crate::Error::NotReopenable`] on the second call.
    fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>>;
}

/// An input to read: a byte source, or one member of a local zip archive.
#[derive(Debug, Clone)]
pub enum Source {
    Stream(Arc<dyn ByteSource>),
    /// Member of a zip archive (see [`crate::archive`]). Archives must be local files.
    ZipMember { archive: PathBuf, member: String },
}

impl Source {
    /// A local file, or `archive.zip!/member` for one zip member.
    pub fn file(path: impl Into<PathBuf>) -> crate::Result<Self> {
        todo!()
    }

    /// A one-shot reader such as stdin.
    pub fn reader(name: impl Into<String>, reader: Box<dyn std::io::Read + Send>) -> Self {
        todo!()
    }

    pub fn name(&self) -> String {
        todo!()
    }

    /// Open a fresh decompressed and decoded (UTF-8) stream.
    pub fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        todo!()
    }
}

/// The inputs of one scan or read: a list, or a lazily produced sequence (WFS pages).
/// Consumed once; a scan followed by a read needs two `Sources` values.
pub struct Sources(Box<dyn Iterator<Item = crate::Result<Source>> + Send>);

impl Sources {
    pub fn lazy(iter: impl Iterator<Item = crate::Result<Source>> + Send + 'static) -> Self {
        Self(Box::new(iter))
    }
}

impl Iterator for Sources {
    type Item = crate::Result<Source>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

impl From<Vec<Source>> for Sources {
    fn from(sources: Vec<Source>) -> Self {
        Self::lazy(sources.into_iter().map(Ok))
    }
}

impl From<Source> for Sources {
    fn from(source: Source) -> Self {
        Self::from(vec![source])
    }
}

/// Local file.
#[derive(Debug)]
pub struct FileSource {
    path: PathBuf,
    name: String,
    len: Option<u64>,
}

impl FileSource {
    pub fn open(path: impl Into<PathBuf>) -> crate::Result<Self> {
        todo!()
    }
}

impl ByteSource for FileSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn len(&self) -> Option<u64> {
        self.len
    }
    fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        todo!()
    }
}

/// Any reader that can be consumed once (stdin, a response body handed in by the
/// caller). The `Mutex` keeps it `Sync`.
pub struct ReaderSource {
    name: String,
    reader: std::sync::Mutex<Option<Box<dyn std::io::Read + Send>>>,
}

impl std::fmt::Debug for ReaderSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl ByteSource for ReaderSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn len(&self) -> Option<u64> {
        None
    }
    fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        todo!()
    }
}

/// Bytes already in memory (a fetched WFS page).
#[derive(Debug)]
pub struct BytesSource {
    pub name: String,
    pub bytes: bytes::Bytes,
}

impl ByteSource for BytesSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn len(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }
    fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        todo!()
    }
}

/// Expand local paths, directories, globs, zip archives and `archive.zip!/member`
/// into individual sources. Remote URLs are resolved by `xeibe-io`.
pub fn expand_sources(inputs: &[String]) -> crate::Result<Vec<Source>> {
    todo!()
}
