//! A browser `Blob` (a dropped `File`) read in slices, so a file is never
//! held whole: no copy of it in JavaScript or in the module's memory, and no
//! limit on its size from the largest `ArrayBuffer` a browser allocates.
//!
//! `FileReaderSync` reads synchronously, which only a worker may do; the page
//! runs the module in one.

use std::cell::{Cell, RefCell};

use wasm_bindgen::JsCast;
use xeibe_core::RandomAccess;

/// A blob and its size.
pub struct BlobData {
    blob: web_sys::Blob,
    reader: web_sys::FileReaderSync,
    size: u64,
}

// The module has one thread: built without atomics, wasm32-unknown-unknown
// can't start another, and reads run with `threads = 1`. A build with
// atomics fails here rather than share a `Blob` across threads.
#[cfg(not(target_feature = "atomics"))]
unsafe impl Send for BlobData {}
#[cfg(not(target_feature = "atomics"))]
unsafe impl Sync for BlobData {}

impl std::fmt::Debug for BlobData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobData")
            .field("size", &self.size)
            .finish()
    }
}

impl BlobData {
    pub fn new(blob: web_sys::Blob) -> Result<Self, String> {
        let reader = web_sys::FileReaderSync::new()
            .map_err(|_| "FileReaderSync is only available in a worker".to_string())?;
        let size = blob.size() as u64;
        Ok(BlobData { blob, reader, size })
    }
}

impl RandomAccess for BlobData {
    fn len(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        let end = offset.saturating_add(buf.len() as u64).min(self.size);
        if offset >= end {
            return Ok(0);
        }
        let error = |e: wasm_bindgen::JsValue| {
            let message = e
                .dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
                .unwrap_or_else(|| format!("{e:?}"));
            std::io::Error::other(message)
        };
        let slice = self
            .blob
            .slice_with_f64_and_f64(offset as f64, end as f64)
            .map_err(error)?;
        let buffer = self.reader.read_as_array_buffer(&slice).map_err(error)?;
        let bytes = js_sys::Uint8Array::new(&buffer);
        let n = bytes.length() as usize;
        bytes.copy_to(&mut buf[..n]);
        progress(n as u64);
        Ok(n)
    }
}

thread_local! {
    /// Bytes read from blobs since the last reset, and the callback told
    /// about them.
    static READ: Cell<u64> = const { Cell::new(0) };
    static REPORTED: Cell<u64> = const { Cell::new(0) };
    static CALLBACK: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

/// Called with the bytes read so far, every 8 MiB of them.
const STEP: u64 = 8 << 20;

/// Report reads to `callback(bytes)` from now on, counting from zero.
pub fn on_progress(callback: Option<js_sys::Function>) {
    READ.set(0);
    REPORTED.set(0);
    CALLBACK.replace(callback);
}

fn progress(n: u64) {
    let read = READ.get() + n;
    READ.set(read);
    if read - REPORTED.get() < STEP {
        return;
    }
    REPORTED.set(read);
    CALLBACK.with_borrow(|callback| {
        if let Some(callback) = callback {
            let _ = callback.call1(&wasm_bindgen::JsValue::NULL, &(read as f64).into());
        }
    });
}
