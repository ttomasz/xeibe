//! `object_store` sources (S3, GCS, Azure, HTTP): one streaming `get` per `open`.
//! The async body runs on a tokio runtime and is handed to the synchronous splitter
//! in blocks over a bounded channel. Never call `open` from inside a tokio worker
//! thread without `spawn_blocking`.

use std::io::Read;
use std::sync::{Arc, OnceLock};

use bytes::{Bytes, BytesMut};
use futures_util::{StreamExt, TryStreamExt};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use xeibe_core::{ByteSource, Source};

/// Keys a directory-like prefix contributes (case-insensitive), as for local
/// directories. A `.zip` under the prefix is an error: archives must be local.
const PREFIX_SUFFIXES: &[&str] = &[".gml", ".xml", ".gml.gz", ".xml.gz", ".gml.zst", ".xml.zst"];

/// One message from the body task: a block, the end of the body, or an error.
type Block = std::io::Result<Option<Bytes>>;

#[derive(Debug)]
pub struct ObjectStoreSource {
    store: Arc<dyn object_store::ObjectStore>,
    location: object_store::path::Path,
    runtime: tokio::runtime::Handle,
    name: String,
    /// From a listing, if the source came from one.
    len: Option<u64>,
    block_size: usize,
    read_ahead: usize,
}

impl ObjectStoreSource {
    /// Named after the location until [`Self::with_name`] gives it a URL; reads
    /// blocks of 8 MiB, 4 ahead, until [`Self::with_blocks`] says otherwise.
    pub fn new(
        store: Arc<dyn object_store::ObjectStore>,
        location: object_store::path::Path,
        runtime: tokio::runtime::Handle,
        len: Option<u64>,
    ) -> Self {
        Self {
            name: location.to_string(),
            store,
            location,
            runtime,
            len,
            block_size: 8 << 20,
            read_ahead: 4,
        }
    }

    /// The name used in messages, usually the URL.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Block size and number of blocks buffered ahead of the reader (see
    /// [`crate::IoOptions`]).
    pub fn with_blocks(mut self, block_size: usize, read_ahead: usize) -> Self {
        self.block_size = block_size.max(1);
        self.read_ahead = read_ahead.max(1);
        self
    }
}

impl ByteSource for ObjectStoreSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn len(&self) -> Option<u64> {
        self.len
    }
    /// Waits for the first block, so a missing object fails here; a zip archive,
    /// recognised by its first bytes, is [`xeibe_core::Error::RemoteArchive`].
    fn open(&self) -> xeibe_core::Result<Box<dyn std::io::Read + Send>> {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<Block>(self.read_ahead);
        let store = self.store.clone();
        let location = self.location.clone();
        let block_size = self.block_size;
        self.runtime.spawn(async move {
            let blocks = async {
                let mut body = store
                    .get(&location)
                    .await
                    .map_err(std::io::Error::other)?
                    .into_stream();
                let mut block = BytesMut::with_capacity(block_size);
                while let Some(chunk) = body.next().await {
                    block.extend_from_slice(&chunk.map_err(std::io::Error::other)?);
                    if block.len() >= block_size {
                        let full = block.split().freeze();
                        block.reserve(block_size);
                        if sender.send(Ok(Some(full))).await.is_err() {
                            // The reader was dropped.
                            return Ok(());
                        }
                    }
                }
                if !block.is_empty() && sender.send(Ok(Some(block.freeze()))).await.is_err() {
                    return Ok(());
                }
                let _ = sender.send(Ok(None)).await;
                Ok(())
            };
            if let Err(error) = blocks.await {
                let _ = sender.send(Err(error)).await;
            }
        });

        let first = match receiver.blocking_recv() {
            Some(Ok(block)) => block.unwrap_or_default(),
            Some(Err(error)) => return Err(named(&self.name, error).into()),
            None => return Err(named(&self.name, ended_early()).into()),
        };
        if xeibe_core::decode::detect_compression(&first) == xeibe_core::decode::Compression::Zip {
            return Err(xeibe_core::Error::RemoteArchive(self.name.clone()));
        }
        let done = first.is_empty();
        Ok(Box::new(BlockReader {
            receiver,
            current: first,
            done,
            name: self.name.clone(),
        }))
    }
}

/// The synchronous end of the channel.
struct BlockReader {
    receiver: tokio::sync::mpsc::Receiver<Block>,
    current: Bytes,
    done: bool,
    name: String,
}

impl Read for BlockReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.current.is_empty() {
            if self.done {
                return Ok(0);
            }
            match self.receiver.blocking_recv() {
                Some(Ok(Some(block))) => self.current = block,
                Some(Ok(None)) => self.done = true,
                Some(Err(error)) => return Err(named(&self.name, error)),
                // The task ended without saying so (it panicked or the runtime
                // shut down): the body is incomplete.
                None => return Err(named(&self.name, ended_early())),
            }
        }
        let n = buf.len().min(self.current.len());
        buf[..n].copy_from_slice(&self.current.split_to(n));
        Ok(n)
    }
}

fn ended_early() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "the object's body ended before it was complete",
    )
}

fn named(name: &str, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{name}: {error}"))
}

/// Sources for one object-store input, relative to the store: `key` is one
/// object, `prefix/` every GML object under the prefix, and a key with glob
/// characters (`*`, `?`, `[`, `{`; `*` does not cross `/`, `**` does) every
/// object it matches. Listings are sorted by key and give each source its
/// size. `base` is the URL of the store's root, used to name the sources.
pub fn store_sources(
    store: Arc<dyn ObjectStore>,
    base: &str,
    key: &str,
    runtime: &tokio::runtime::Handle,
    options: &crate::IoOptions,
) -> crate::Result<Vec<Source>> {
    let base = base.trim_end_matches('/');
    let source = |location: Path, len: Option<u64>| {
        let name = format!("{base}/{location}");
        let source = ObjectStoreSource::new(store.clone(), location, runtime.clone(), len)
            .with_name(name)
            .with_blocks(options.block_size, options.read_ahead);
        Source::Stream(Arc::new(source))
    };

    let glob_start = key.find(['*', '?', '[', '{']);
    if glob_start.is_none() && !key.is_empty() && !key.ends_with('/') {
        let location =
            Path::parse(key).map_err(|e| crate::Error::InvalidUrl(format!("{base}/{key}: {e}")))?;
        return Ok(vec![source(location, None)]);
    }

    // The literal directory part is listed; the pattern filters the listing.
    let literal = &key[..glob_start.unwrap_or(key.len())];
    let prefix = &literal[..literal.rfind('/').map_or(0, |slash| slash + 1)];
    let prefix = Path::parse(prefix.trim_end_matches('/'))
        .map_err(|e| crate::Error::InvalidUrl(format!("{base}/{key}: {e}")))?;
    let matcher = match glob_start {
        Some(_) => Some(
            globset::GlobBuilder::new(key)
                .literal_separator(true)
                .build()
                .map_err(|e| crate::Error::InvalidUrl(format!("{base}/{key}: {e}")))?
                .compile_matcher(),
        ),
        None => None,
    };
    let listing = runtime
        .block_on(store.list(Some(&prefix)).try_collect::<Vec<_>>())
        .map_err(|e| crate::Error::Network {
            url: format!("{base}/{key}"),
            message: e.to_string(),
        })?;
    let mut objects: Vec<_> = listing
        .into_iter()
        .filter(|meta| match &matcher {
            Some(matcher) => matcher.is_match(meta.location.as_ref()),
            None => {
                let lower = meta.location.as_ref().to_ascii_lowercase();
                PREFIX_SUFFIXES.iter().any(|s| lower.ends_with(s)) || lower.ends_with(".zip")
            }
        })
        .collect();
    objects.sort_by(|a, b| a.location.cmp(&b.location));
    if let Some(zip) = objects.iter().find(|meta| {
        meta.location
            .as_ref()
            .to_ascii_lowercase()
            .ends_with(".zip")
    }) {
        return Err(xeibe_core::Error::RemoteArchive(format!("{base}/{}", zip.location)).into());
    }
    Ok(objects
        .into_iter()
        .map(|meta| source(meta.location, Some(meta.size)))
        .collect())
}

/// The current tokio runtime, or one of this crate's own, started on first use
/// and kept for the life of the process.
pub(crate) fn runtime() -> crate::Result<tokio::runtime::Handle> {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return Ok(handle);
    }
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime.handle().clone());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .thread_name("xeibe-io")
        .enable_all()
        .build()?;
    Ok(RUNTIME.get_or_init(|| runtime).handle().clone())
}
