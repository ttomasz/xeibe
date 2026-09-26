//! Decompression and character-encoding conversion. Everything downstream
//! sees a UTF-8 byte stream.

use std::io::{Cursor, Read};

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
/// and transcoding run on a thread of their own ([`read_ahead`]).
pub fn decoded_reader(raw: Box<dyn Read + Send>) -> crate::Result<Box<dyn Read + Send>> {
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
    if compression != Compression::None || encoding != UTF_8 {
        return Ok(read_ahead(decoded)?);
    }
    Ok(decoded)
}

/// Bytes a [`read_ahead`] thread reads at a time, and blocks it may be ahead.
const AHEAD_BLOCK: usize = 256 * 1024;
const AHEAD_BLOCKS: usize = 8;

/// Run `reader` on a thread of its own, up to a few blocks ahead of the
/// consumer, so that decompression overlaps with the splitter, the one
/// sequential stage of a read. Where there are no threads (WebAssembly),
/// `reader` itself.
pub(crate) fn read_ahead(mut reader: Box<dyn Read + Send>) -> std::io::Result<Box<dyn Read + Send>> {
    if cfg!(target_family = "wasm") {
        return Ok(reader);
    }
    let (sender, blocks) = std::sync::mpsc::sync_channel(AHEAD_BLOCKS);
    std::thread::Builder::new()
        .name("xeibe-decode".into())
        .spawn(move || loop {
            let mut block = vec![0; AHEAD_BLOCK];
            let mut len = 0;
            let error = loop {
                match reader.read(&mut block[len..]) {
                    Ok(0) => break None,
                    Ok(n) => {
                        len += n;
                        if len == block.len() {
                            break None;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => break Some(e),
                }
            };
            // A short block: the end of input, or an error.
            let last = len < block.len();
            block.truncate(len);
            // A failed send: the consumer is gone.
            if len > 0 && sender.send(Ok(block)).is_err() {
                return;
            }
            if let Some(error) = error {
                let _ = sender.send(Err(error));
            }
            if last {
                return;
            }
        })?;
    Ok(Box::new(ReadAhead { blocks, current: Cursor::new(Vec::new()) }))
}

/// The consumer's end of [`read_ahead`].
struct ReadAhead {
    blocks: std::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>,
    current: Cursor<Vec<u8>>,
}

impl Read for ReadAhead {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let n = self.current.read(out)?;
            if n > 0 || out.is_empty() {
                return Ok(n);
            }
            match self.blocks.recv() {
                Ok(block) => self.current = Cursor::new(block?),
                // The thread has finished: the end of input.
                Err(_) => return Ok(0),
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
