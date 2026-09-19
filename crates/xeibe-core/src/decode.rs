//! Decompression and character-encoding conversion. Everything downstream
//! sees a UTF-8 byte stream.

use std::io::Read;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Zstd,
    /// Zip archives are expanded into member sources by [`crate::archive`].
    Zip,
}

/// Detect compression from magic bytes.
pub fn detect_compression(prefix: &[u8]) -> Compression {
    todo!()
}

/// Read the XML declaration (and BOM) to find the declared encoding.
pub fn detect_encoding(prefix: &[u8]) -> &'static encoding_rs::Encoding {
    todo!()
}

/// Wrap a raw reader: decompress, then transcode to UTF-8 if needed.
pub fn decoded_reader(raw: Box<dyn Read + Send>) -> crate::Result<Box<dyn Read + Send>> {
    todo!()
}
