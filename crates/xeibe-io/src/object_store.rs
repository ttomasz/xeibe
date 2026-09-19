//! `object_store` sources (S3, GCS, Azure, HTTP): one streaming `get` per `open`.
//! The async body runs on a tokio runtime and is handed to the synchronous splitter
//! in blocks over a bounded channel. Never call `open` from inside a tokio worker
//! thread without `spawn_blocking`.

use std::sync::Arc;

use xeibe_core::ByteSource;

#[derive(Debug)]
pub struct ObjectStoreSource {
    store: Arc<dyn object_store::ObjectStore>,
    location: object_store::path::Path,
    runtime: tokio::runtime::Handle,
    name: String,
    /// From a listing, if the source came from one.
    len: Option<u64>,
}

impl ObjectStoreSource {
    pub fn new(
        store: Arc<dyn object_store::ObjectStore>,
        location: object_store::path::Path,
        runtime: tokio::runtime::Handle,
        len: Option<u64>,
    ) -> Self {
        todo!()
    }
}

impl ByteSource for ObjectStoreSource {
    fn name(&self) -> &str {
        &self.name
    }
    fn len(&self) -> Option<u64> {
        self.len
    }
    fn open(&self) -> xeibe_core::Result<Box<dyn std::io::Read + Send>> {
        todo!()
    }
}
