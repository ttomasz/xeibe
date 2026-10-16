//! Decompression and character-encoding conversion. Everything downstream
//! sees a UTF-8 byte stream.

use std::collections::VecDeque;
use std::io::{Cursor, Read};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Zstd,
    /// Zip archives are expanded into member sources by [`crate::archive`].
    Zip,
}

/// Bytes needed to recognize every compression format.
const MAGIC_LEN: usize = 4;

/// Bytes read to find the XML declaration. Real declarations are far shorter.
const DECLARATION_LEN: usize = 1024;

/// Detect compression from magic bytes.
pub fn detect_compression(prefix: &[u8]) -> Compression {
    if prefix.starts_with(&[0x1f, 0x8b]) {
        Compression::Gzip
    } else if prefix.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Compression::Zstd
    } else if prefix.starts_with(b"PK\x03\x04") || prefix.starts_with(b"PK\x05\x06") {
        Compression::Zip
    } else {
        Compression::None
    }
}

/// Read the XML declaration (and BOM) to find the declared encoding.
///
/// A BOM wins over the declaration. Without either, or with a label
/// `encoding_rs` does not know, the answer is UTF-8.
pub fn detect_encoding(prefix: &[u8]) -> &'static Encoding {
    sniff_encoding(prefix).ok().flatten().unwrap_or(UTF_8)
}

/// `Ok(None)`: nothing declared. `Err(label)`: an unknown label.
fn sniff_encoding(prefix: &[u8]) -> Result<Option<&'static Encoding>, String> {
    if let Some((encoding, _)) = Encoding::for_bom(prefix) {
        return Ok(Some(encoding));
    }
    // UTF-16 without a BOM: `<?` as two-byte units.
    if prefix.starts_with(b"<\0?\0") {
        return Ok(Some(UTF_16LE));
    }
    if prefix.starts_with(b"\0<\0?") {
        return Ok(Some(UTF_16BE));
    }
    let Some(label) = declared_label(prefix) else {
        return Ok(None);
    };
    match Encoding::for_label(label) {
        // A UTF-16 label in a declaration that could be read as ASCII is a
        // mislabelled single-byte document.
        Some(encoding) if encoding == UTF_16LE || encoding == UTF_16BE => Ok(Some(UTF_8)),
        Some(encoding) => Ok(Some(encoding)),
        None => Err(String::from_utf8_lossy(label).into_owned()),
    }
}

/// The `encoding` pseudo-attribute of an XML declaration at the very start.
fn declared_label(prefix: &[u8]) -> Option<&[u8]> {
    let rest = prefix.strip_prefix(b"<?xml")?;
    let end = rest.windows(2).position(|w| w == b"?>")?;
    let decl = &rest[..end];
    let at = decl.windows(8).position(|w| w == b"encoding")?;
    let mut rest = decl[at + 8..].trim_ascii_start();
    rest = rest.strip_prefix(b"=")?.trim_ascii_start();
    let quote = *rest.first()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.iter().position(|&b| b == quote)?;
    Some(rest[..end].trim_ascii())
}

/// Wrap a raw reader: decompress, then transcode to UTF-8 if needed.
///
/// A UTF-8 BOM is removed. A zip archive is an error here: archives are
/// expanded into member sources before anything is opened. Decompression
/// and transcoding happen as the stream is read, on the thread that reads it.
pub fn decoded_reader(raw: Box<dyn Read + Send>) -> crate::Result<Box<dyn Read + Send>> {
    decoded_reader_with(raw, false, None)
}

/// [`decoded_reader`] for a source that may be `compressed` already (a
/// deflated zip member). With `ahead`, a stream that needs decompressing or
/// transcoding is decoded in blocks that other threads can decode ahead of
/// the reader ([`DecodeAhead`]).
pub(crate) fn decoded_reader_with(
    raw: Box<dyn Read + Send>,
    compressed: bool,
    ahead: Option<&DecodeAhead>,
) -> crate::Result<Box<dyn Read + Send>> {
    let (magic, raw) = peek(raw, MAGIC_LEN)?;
    let compression = detect_compression(&magic);
    let decompressed: Box<dyn Read + Send> = match compression {
        Compression::None => raw,
        Compression::Gzip => Box::new(flate2::read::MultiGzDecoder::new(raw)),
        Compression::Zstd => Box::new(zstd::stream::read::Decoder::new(raw)?),
        Compression::Zip => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "a zip archive cannot be read as one stream; pass it as an archive input",
            )
            .into());
        }
    };

    let (prefix, stream) = peek(decompressed, DECLARATION_LEN)?;
    let encoding = sniff_encoding(&prefix)
        .map_err(crate::Error::UnsupportedEncoding)?
        .unwrap_or(UTF_8);
    let decoded: Box<dyn Read + Send> = if encoding == UTF_8 {
        let mut stream = stream;
        if prefix.starts_with(b"\xEF\xBB\xBF") {
            let mut bom = [0u8; 3];
            stream.read_exact(&mut bom)?;
        }
        stream
    } else {
        Box::new(Utf8Transcoder::new(stream, encoding))
    };
    let Some(ahead) = ahead else {
        return Ok(decoded);
    };
    if !compressed && compression == Compression::None && encoding == UTF_8 {
        // Nothing to decode: the reader is as fast as the source.
        ahead.set(None);
        return Ok(decoded);
    }
    let shared = Arc::new(AheadShared {
        decoder: Mutex::new(AheadDecoder {
            reader: decoded,
            done: false,
            error: None,
        }),
        blocks: Mutex::new(VecDeque::new()),
    });
    ahead.set(Some(shared.clone()));
    Ok(Box::new(AheadReader {
        shared,
        current: Cursor::new(Vec::new()),
    }))
}

/// Bytes decoded at a time, and blocks that may wait for the reader.
const AHEAD_BLOCK: usize = 256 * 1024;
const AHEAD_BLOCKS: usize = 16;

/// Decoding ahead of the reader, by the workers of a scan or read.
///
/// The worker that splits a source reads its decoded stream, and
/// decompression or transcoding costs about as much as splitting. A source
/// opened with [`crate::Source::open_ahead`] decodes in blocks of 256 KB, and
/// [`DecodeAhead::help`] decodes the next block into a queue of up to 16
/// (4 MB). Workers call it before they take their next chunk and while
/// another worker splits, so that the splitting worker mostly finds its
/// input decoded. Nothing runs on a thread of its own
/// (`docs/architecture.md`, "Parallel pipeline").
#[derive(Default)]
pub struct DecodeAhead {
    /// The stream of the source opened last.
    current: Mutex<Option<Arc<AheadShared>>>,
}

impl DecodeAhead {
    /// Decode one block ahead of the reader of the source opened last.
    /// `false` when there is nothing to do: the queue is full, another thread
    /// is decoding, the stream has ended or needs no decoding.
    pub fn help(&self) -> bool {
        let Some(shared) = lock(&self.current).clone() else {
            return false;
        };
        shared.decode_ahead()
    }

    fn set(&self, shared: Option<Arc<AheadShared>>) {
        *lock(&self.current) = shared;
    }
}

impl std::fmt::Debug for DecodeAhead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodeAhead").finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A decoded stream shared by its reader and the threads that decode ahead.
/// Blocks are queued while the decoder is locked, so they stay in order.
struct AheadShared {
    decoder: Mutex<AheadDecoder>,
    blocks: Mutex<VecDeque<std::io::Result<Vec<u8>>>>,
}

struct AheadDecoder {
    reader: Box<dyn Read + Send>,
    done: bool,
    /// An error after the data read with it, for the next block.
    error: Option<std::io::Error>,
}

impl AheadDecoder {
    /// The next block; `None` at the end of the stream.
    fn block(&mut self) -> Option<std::io::Result<Vec<u8>>> {
        if let Some(error) = self.error.take() {
            return Some(Err(error));
        }
        if self.done {
            return None;
        }
        let mut block = vec![0; AHEAD_BLOCK];
        let mut len = 0;
        while len < block.len() {
            match self.reader.read(&mut block[len..]) {
                Ok(0) => {
                    self.done = true;
                    break;
                }
                Ok(n) => len += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.done = true;
                    if len == 0 {
                        return Some(Err(e));
                    }
                    self.error = Some(e);
                    break;
                }
            }
        }
        block.truncate(len);
        (len > 0).then_some(Ok(block))
    }
}

impl AheadShared {
    fn decode_ahead(&self) -> bool {
        if lock(&self.blocks).len() >= AHEAD_BLOCKS {
            return false;
        }
        let mut decoder = match self.decoder.try_lock() {
            Ok(decoder) => decoder,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return false,
        };
        match decoder.block() {
            Some(block) => {
                lock(&self.blocks).push_back(block);
                true
            }
            None => false,
        }
    }

    /// The next block for the reader: a queued one, or one decoded now.
    fn next_block(&self) -> Option<std::io::Result<Vec<u8>>> {
        if let Some(block) = lock(&self.blocks).pop_front() {
            return Some(block);
        }
        let mut decoder = lock(&self.decoder);
        // Queued while this thread waited for the decoder.
        if let Some(block) = lock(&self.blocks).pop_front() {
            return Some(block);
        }
        decoder.block()
    }
}

/// The reader's end of a stream decoded ahead.
struct AheadReader {
    shared: Arc<AheadShared>,
    current: Cursor<Vec<u8>>,
}

impl Read for AheadReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let n = self.current.read(out)?;
            if n > 0 || out.is_empty() {
                return Ok(n);
            }
            match self.shared.next_block() {
                Some(block) => self.current = Cursor::new(block?),
                None => return Ok(0),
            }
        }
    }
}

/// Read up to `len` bytes, and return them with a reader that still yields
/// the whole stream.
fn peek(
    mut reader: Box<dyn Read + Send>,
    len: usize,
) -> std::io::Result<(Vec<u8>, Box<dyn Read + Send>)> {
    let mut prefix = Vec::with_capacity(len);
    (&mut reader).take(len as u64).read_to_end(&mut prefix)?;
    let rest: Box<dyn Read + Send> = Box::new(Cursor::new(prefix.clone()).chain(reader));
    Ok((prefix, rest))
}

/// Streaming conversion of any `encoding_rs` encoding to UTF-8. Malformed
/// input becomes U+FFFD, as `encoding_rs` does.
struct Utf8Transcoder<R> {
    inner: R,
    decoder: encoding_rs::Decoder,
    input: Box<[u8]>,
    input_start: usize,
    input_end: usize,
    input_eof: bool,
    output: Box<[u8]>,
    output_start: usize,
    output_end: usize,
    finished: bool,
}

impl<R: Read> Utf8Transcoder<R> {
    const BUFFER: usize = 64 * 1024;

    fn new(inner: R, encoding: &'static Encoding) -> Self {
        Self {
            inner,
            decoder: encoding.new_decoder_with_bom_removal(),
            input: vec![0; Self::BUFFER].into_boxed_slice(),
            input_start: 0,
            input_end: 0,
            input_eof: false,
            output: vec![0; Self::BUFFER].into_boxed_slice(),
            output_start: 0,
            output_end: 0,
            finished: false,
        }
    }

    /// Decode more output; returns `false` at the end of the stream.
    fn fill_output(&mut self) -> std::io::Result<bool> {
        while !self.finished {
            if self.input_start == self.input_end && !self.input_eof {
                self.input_start = 0;
                self.input_end = loop {
                    match self.inner.read(&mut self.input) {
                        Ok(n) => break n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) => return Err(e),
                    }
                };
                self.input_eof = self.input_end == 0;
            }
            let last = self.input_eof;
            let (result, read, written, _) = self.decoder.decode_to_utf8(
                &self.input[self.input_start..self.input_end],
                &mut self.output,
                last,
            );
            self.input_start += read;
            self.output_start = 0;
            self.output_end = written;
            if last && result == encoding_rs::CoderResult::InputEmpty {
                self.finished = true;
            }
            if written > 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl<R: Read> Read for Utf8Transcoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.output_start == self.output_end && !self.fill_output()? {
            return Ok(0);
        }
        let n = buf.len().min(self.output_end - self.output_start);
        buf[..n].copy_from_slice(&self.output[self.output_start..self.output_start + n]);
        self.output_start += n;
        Ok(n)
    }
}
