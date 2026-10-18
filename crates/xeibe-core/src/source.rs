use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::archive::{self, Archive};

/// Index of a source within one scan or read, in the order sources were consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceId(pub u32);

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "source #{}", self.0)
    }
}

/// Raw bytes of one input, before decompression and decoding. Always read as one
/// sequential stream from the start (see `docs/architecture.md#sources-and-remote-input`).
///
/// Implemented here for local files and one-shot readers; `xeibe-io` adds HTTP and
/// object stores. Implementations are synchronous: async stores are bridged in
/// `xeibe-io`, so this crate needs no async runtime.
// `len` is a size hint that may be unknown, so an `is_empty` would mislead.
#[allow(clippy::len_without_is_empty)]
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
    /// Member of a zip archive (see [`crate::archive`]): a local file, or one
    /// held in memory.
    ZipMember {
        archive: Archive,
        member: String,
    },
}

impl Source {
    /// A local file, or `archive.zip!/member` for one zip member.
    ///
    /// A missing file fails here; a missing zip member fails on
    /// [`Source::open`], because the archive is not opened until then.
    pub fn file(path: impl Into<PathBuf>) -> crate::Result<Self> {
        let path = path.into();
        if let Some((archive, member)) = path.to_str().and_then(archive::split_member_path) {
            return Ok(Source::ZipMember {
                archive: Archive::File(PathBuf::from(archive)),
                member: member.to_string(),
            });
        }
        Ok(Source::Stream(Arc::new(FileSource::open(path)?)))
    }

    /// A one-shot reader such as stdin.
    pub fn reader(name: impl Into<String>, reader: Box<dyn std::io::Read + Send>) -> Self {
        Source::Stream(Arc::new(ReaderSource::new(name, reader)))
    }

    pub fn name(&self) -> String {
        match self {
            Source::Stream(source) => source.name().to_string(),
            Source::ZipMember { archive, member } => format!("{}!/{member}", archive.name()),
        }
    }

    /// Open a fresh decompressed and decoded (UTF-8) stream.
    pub fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        self.open_with(None)
    }

    /// [`Source::open`], for a stream that other threads decode ahead of its
    /// reader with `ahead`, beside the other sources open with it.
    pub fn open_ahead(
        &self,
        ahead: &crate::decode::DecodeAhead,
    ) -> crate::Result<Box<dyn std::io::Read + Send>> {
        self.open_with(Some(ahead))
    }

    fn open_with(
        &self,
        ahead: Option<&crate::decode::DecodeAhead>,
    ) -> crate::Result<Box<dyn std::io::Read + Send>> {
        let (raw, compressed) = match self {
            Source::Stream(source) => (source.open()?, false),
            Source::ZipMember { archive, member } => archive::open_member(archive, member)?,
        };
        crate::decode::decoded_reader_with(raw, compressed, ahead)
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
    /// Reads the file's metadata; fails if the file does not exist.
    pub fn open(path: impl Into<PathBuf>) -> crate::Result<Self> {
        let path = path.into();
        let metadata = std::fs::metadata(&path).map_err(|e| with_path(e, &path))?;
        Ok(Self {
            name: path.display().to_string(),
            len: Some(metadata.len()),
            path,
        })
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
        let file = std::fs::File::open(&self.path).map_err(|e| with_path(e, &self.path))?;
        Ok(Box::new(file))
    }
}

/// An I/O error that names the path.
fn with_path(error: std::io::Error, path: &Path) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

/// Any reader that can be consumed once (stdin, a response body handed in by the
/// caller). The `Mutex` keeps it `Sync`.
pub struct ReaderSource {
    name: String,
    reader: std::sync::Mutex<Option<Box<dyn std::io::Read + Send>>>,
}

impl ReaderSource {
    pub fn new(name: impl Into<String>, reader: Box<dyn std::io::Read + Send>) -> Self {
        Self {
            name: name.into(),
            reader: std::sync::Mutex::new(Some(reader)),
        }
    }
}

impl std::fmt::Debug for ReaderSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let consumed = self.reader.lock().map(|r| r.is_none()).unwrap_or(true);
        f.debug_struct("ReaderSource")
            .field("name", &self.name)
            .field("consumed", &consumed)
            .finish()
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
        let mut slot = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.take()
            .ok_or_else(|| crate::Error::NotReopenable(self.name.clone()))
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
        Ok(Box::new(std::io::Cursor::new(self.bytes.clone())))
    }
}

/// Bytes that can be read at any offset without being held whole: bytes in
/// memory, or a browser `File` read in slices. Zip archives need it (their
/// directory is at the end); [`RandomAccessSource`] streams one from the
/// start.
// `len` is always known here, but an `is_empty` would add nothing.
#[allow(clippy::len_without_is_empty)]
pub trait RandomAccess: Send + Sync + std::fmt::Debug {
    fn len(&self) -> u64;

    /// Read into `buf` from `offset`; fewer bytes (0 at the end) are fine.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize>;
}

impl RandomAccess for bytes::Bytes {
    fn len(&self) -> u64 {
        self.as_ref().len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.as_ref().len());
        let n = buf.len().min(self.as_ref().len() - start);
        buf[..n].copy_from_slice(&self.as_ref()[start..start + n]);
        Ok(n)
    }
}

/// [`RandomAccess`] bytes as a reader of the range `pos..end`, seekable
/// within the whole, in reads of [`RANDOM_ACCESS_CHUNK`]: each `read_at` may
/// be costly (a browser `File` read is a call into the browser).
///
/// The bytes of the last `read_at` stay until a read falls outside them,
/// across seeks: a zip's directory, read entry by entry with seeks in
/// between, is one `read_at`, where a `BufReader` (which drops its buffer on
/// every seek) made one per entry.
#[derive(Debug, Clone)]
pub struct RandomReader {
    data: Arc<dyn RandomAccess>,
    pos: u64,
    end: u64,
    window: Vec<u8>,
    /// Where `window` starts in `data`.
    window_start: u64,
}

impl RandomReader {
    /// The whole of `data`.
    pub fn new(data: Arc<dyn RandomAccess>) -> Self {
        let end = data.len();
        Self::range(data, 0, end)
    }

    /// `length` bytes from `start` (fewer if `data` ends first).
    pub fn range(data: Arc<dyn RandomAccess>, start: u64, length: u64) -> Self {
        let end = start.saturating_add(length).min(data.len());
        Self {
            data,
            pos: start.min(end),
            end,
            window: Vec::new(),
            window_start: 0,
        }
    }

    /// Read `window` afresh from `pos`.
    fn fill(&mut self) -> std::io::Result<()> {
        let size = usize::try_from(self.end - self.pos)
            .unwrap_or(usize::MAX)
            .min(RANDOM_ACCESS_CHUNK);
        self.window.resize(size, 0);
        let mut filled = 0;
        while filled < size {
            let n = self
                .data
                .read_at(self.pos + filled as u64, &mut self.window[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        self.window.truncate(filled);
        self.window_start = self.pos;
        Ok(())
    }
}

impl std::io::Read for RandomReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.end || buf.is_empty() {
            return Ok(0);
        }
        let in_window = self.pos >= self.window_start
            && self.pos < self.window_start + self.window.len() as u64;
        if !in_window {
            self.fill()?;
        }
        let at = (self.pos - self.window_start) as usize;
        let n = buf.len().min(self.window.len() - at);
        buf[..n].copy_from_slice(&self.window[at..at + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl std::io::Seek for RandomReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let target = match pos {
            std::io::SeekFrom::Start(offset) => Some(offset),
            std::io::SeekFrom::End(delta) => self.end.checked_add_signed(delta),
            std::io::SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        };
        match target {
            Some(target) => {
                self.pos = target.min(self.end);
                Ok(self.pos)
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the start",
            )),
        }
    }
}

/// [`RandomAccess`] bytes read from the start, as one stream.
#[derive(Debug)]
pub struct RandomAccessSource {
    pub name: String,
    pub data: Arc<dyn RandomAccess>,
}

/// The size of the reads a [`RandomReader`] makes.
pub const RANDOM_ACCESS_CHUNK: usize = 256 << 10;

impl ByteSource for RandomAccessSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn len(&self) -> Option<u64> {
        Some(self.data.len())
    }
    fn open(&self) -> crate::Result<Box<dyn std::io::Read + Send>> {
        Ok(Box::new(RandomReader::new(self.data.clone())))
    }
}

/// File names picked up from directories (case-insensitive); zip archives
/// are expanded into their members.
const DIRECTORY_SUFFIXES: &[&str] = &[
    ".gml", ".xml", ".gml.gz", ".xml.gz", ".gml.zst", ".xml.zst", ".zip",
];

/// Expand local paths, directories, globs, zip archives and `archive.zip!/member`
/// into individual sources. Remote URLs are resolved by `xeibe-io`.
///
/// Directories are walked recursively and contribute files with a GML
/// suffix (`.gml`, `.xml`, either one with `.gz` or `.zst`, and `.zip`);
/// a glob contributes every file it matches. Results of one directory or
/// glob are sorted by path. The member
/// of `archive.zip!/member` may be a glob too (`archive.zip!/data/*.gml`),
/// matched against the archive's candidate members.
pub fn expand_sources(inputs: &[String]) -> crate::Result<Vec<Source>> {
    let mut sources = Vec::new();
    for input in inputs {
        if input.contains("://") {
            let lower = input.to_ascii_lowercase();
            if lower.contains(".zip!/") || lower.ends_with(".zip") {
                return Err(crate::Error::RemoteArchive(input.clone()));
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{input}: remote inputs are resolved by xeibe-io"),
            )
            .into());
        }
        if let Some((archive, member)) = archive::split_member_path(input) {
            // A glob selects among the candidate members; a plain name is
            // that one member, whatever its suffix.
            if is_glob(member) {
                sources.extend(archive::expand(Path::new(archive), &[member.to_string()])?);
            } else {
                sources.push(Source::file(input)?);
            }
            continue;
        }
        let path = Path::new(input);
        if path.is_dir() {
            let mut files = Vec::new();
            walk(path, usize::MAX, &mut files)?;
            files.retain(|file| has_suffix(file, DIRECTORY_SUFFIXES));
            files.sort();
            for file in files {
                push_file(&file, &mut sources)?;
            }
        } else if !path.exists() && is_glob(input) {
            for file in glob_files(input)? {
                push_file(&file, &mut sources)?;
            }
        } else {
            push_file(path, &mut sources)?;
        }
    }
    Ok(sources)
}

/// One file: a zip archive expands into its members.
fn push_file(path: &Path, sources: &mut Vec<Source>) -> crate::Result<()> {
    if has_suffix(path, &[".zip"]) {
        sources.extend(archive::expand(path, &[])?);
    } else {
        sources.push(Source::Stream(Arc::new(FileSource::open(path)?)));
    }
    Ok(())
}

fn has_suffix(path: &Path, suffixes: &[&str]) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    suffixes.iter().any(|suffix| name.ends_with(suffix))
}

fn is_glob(input: &str) -> bool {
    input.contains(['*', '?', '[', '{'])
}

/// All files under `dir`, down to `depth` levels of subdirectories.
fn walk(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) -> crate::Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| with_path(e, dir))? {
        let path = entry?.path();
        if path.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, files)?;
            }
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// Files matching a glob, walked from the longest literal directory prefix.
fn glob_files(pattern: &str) -> crate::Result<Vec<PathBuf>> {
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?
        .compile_matcher();
    let literal_end = pattern.find(['*', '?', '[', '{']).unwrap_or(pattern.len());
    let (base, rest) = match pattern[..literal_end].rfind('/') {
        Some(0) => (PathBuf::from("/"), &pattern[1..]),
        Some(slash) => (PathBuf::from(&pattern[..slash]), &pattern[slash + 1..]),
        None => (PathBuf::from("."), pattern),
    };
    // Without `**`, the pattern says how deep to look.
    let depth = if rest.contains("**") {
        usize::MAX
    } else {
        rest.matches('/').count()
    };
    let mut files = Vec::new();
    if base.is_dir() {
        walk(&base, depth, &mut files)?;
    }
    let relative_base = !pattern.starts_with("./") && base == Path::new(".");
    files.retain(|file| {
        let file = if relative_base {
            file.strip_prefix(".").unwrap_or(file)
        } else {
            file
        };
        matcher.is_match(file)
    });
    files.sort();
    Ok(files)
}
