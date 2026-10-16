//! Decompression and character-encoding conversion: everything downstream sees
//! UTF-8 (`docs/architecture.md`, "Input handling").

use std::io::{Read, Write};

use xeibe_core::decode::{Compression, decoded_reader, detect_compression, detect_encoding};

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn zstd(bytes: &[u8]) -> Vec<u8> {
    zstd::encode_all(bytes, 1).unwrap()
}

fn decode_all(bytes: Vec<u8>) -> String {
    let reader: Box<dyn Read + Send> = Box::new(std::io::Cursor::new(bytes));
    let mut text = String::new();
    decoded_reader(reader)
        .expect("the stream decodes")
        .read_to_string(&mut text)
        .expect("UTF-8 out");
    text
}

#[test]
fn detects_compression_from_magic_bytes() {
    assert_eq!(
        detect_compression(b"<?xml version=\"1.0\"?>"),
        Compression::None
    );
    assert_eq!(detect_compression(&gzip(b"<a/>")), Compression::Gzip);
    assert_eq!(detect_compression(&zstd(b"<a/>")), Compression::Zstd);
    assert_eq!(detect_compression(b"PK\x03\x04rest"), Compression::Zip);
    // Too few bytes to tell: not compressed.
    assert_eq!(detect_compression(b""), Compression::None);
}

#[test]
fn detects_the_declared_encoding() {
    assert_eq!(
        detect_encoding(b"<?xml version=\"1.0\"?><a/>"),
        encoding_rs::UTF_8
    );
    assert_eq!(
        detect_encoding(b"<?xml version=\"1.0\" encoding=\"ISO-8859-2\"?><a/>"),
        encoding_rs::ISO_8859_2
    );
    assert_eq!(
        detect_encoding(b"<?xml version='1.0' encoding='windows-1250'?><a/>"),
        encoding_rs::WINDOWS_1250
    );
    // Labels are case-insensitive, and `encoding_rs` maps them the WHATWG way
    // (iso-8859-1 is windows-1252).
    assert_eq!(
        detect_encoding(b"<?xml version='1.0' encoding='iso-8859-1'?><a/>"),
        encoding_rs::WINDOWS_1252
    );
    // A BOM wins over the declaration, and a document may have only a BOM.
    assert_eq!(detect_encoding(b"\xEF\xBB\xBF<a/>"), encoding_rs::UTF_8);
    assert_eq!(detect_encoding(b"\xFF\xFE<\0a\0"), encoding_rs::UTF_16LE);
    assert_eq!(detect_encoding(b"\xFE\xFF\0<\0a"), encoding_rs::UTF_16BE);
}

#[test]
fn transcodes_other_encodings_to_utf8() {
    let (bytes, _, _) = encoding_rs::ISO_8859_2
        .encode("<?xml version=\"1.0\" encoding=\"ISO-8859-2\"?><a>Żabczyn</a>");
    let text = decode_all(bytes.into_owned());
    assert!(text.contains("Żabczyn"), "{text:?}");
}

#[test]
fn decompresses_gzip_and_zstd_transparently() {
    let document = "<?xml version=\"1.0\"?><a>Chyrzyno</a>";
    assert_eq!(decode_all(gzip(document.as_bytes())), document);
    assert_eq!(decode_all(zstd(document.as_bytes())), document);
    assert_eq!(decode_all(document.as_bytes().to_vec()), document);
}

#[test]
fn a_long_compressed_stream_arrives_whole_and_in_order() {
    // Many blocks of the decompressor's output arrive as they were.
    let document: String = (0..200_000).map(|i| format!("<a>{i}</a>\n")).collect();
    assert_eq!(decode_all(gzip(document.as_bytes())), document);
    assert_eq!(decode_all(zstd(document.as_bytes())), document);
}

#[test]
fn a_truncated_compressed_stream_is_an_error() {
    let document: String = (0..200_000).map(|i| format!("<a>{i}</a>\n")).collect();
    let mut bytes = gzip(document.as_bytes());
    bytes.truncate(bytes.len() / 2);
    let reader: Box<dyn Read + Send> = Box::new(std::io::Cursor::new(bytes));
    let mut text = String::new();
    let result =
        decoded_reader(reader).and_then(|mut stream| Ok(stream.read_to_string(&mut text)?));
    assert!(
        result.is_err(),
        "read {} bytes without an error",
        text.len()
    );
}

#[test]
fn decompresses_and_transcodes_in_one_pass() {
    let (bytes, _, _) = encoding_rs::WINDOWS_1250
        .encode("<?xml version=\"1.0\" encoding=\"windows-1250\"?><a>Żabczyn</a>");
    let text = decode_all(gzip(&bytes));
    assert!(text.contains("Żabczyn"), "{text:?}");
}

#[test]
fn a_utf8_bom_is_removed() {
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend_from_slice(b"<a/>");
    assert_eq!(decode_all(bytes), "<a/>");
}

/// Read a source opened with `DecodeAhead` while `helpers` threads decode
/// ahead of the reader as fast as they can.
fn decode_with_helpers(bytes: Vec<u8>, helpers: usize) -> Vec<u8> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let ahead = Arc::new(xeibe_core::decode::DecodeAhead::default());
    let source = xeibe_core::Source::Stream(Arc::new(xeibe_core::BytesSource {
        name: "test.gml".into(),
        bytes: bytes.into(),
    }));
    let mut reader = source.open_ahead(&ahead).unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let threads: Vec<_> = (0..helpers)
        .map(|_| {
            let (ahead, done) = (ahead.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if !ahead.help() {
                        std::thread::yield_now();
                    }
                }
            })
        })
        .collect();
    let mut out = Vec::new();
    let mut buf = [0u8; 1000];
    loop {
        let n = reader.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    done.store(true, Ordering::Relaxed);
    for thread in threads {
        thread.join().unwrap();
    }
    out
}

#[test]
fn decoding_ahead_on_other_threads_keeps_the_stream_whole_and_in_order() {
    // Several 256 KB blocks, each line numbered.
    let text: String = (0..200_000).map(|i| format!("<n>{i}</n>\n")).collect();
    let document = format!("<?xml version=\"1.0\"?><root>{text}</root>");
    for helpers in [0, 1, 3] {
        let out = decode_with_helpers(gzip(document.as_bytes()), helpers);
        assert!(out == document.as_bytes(), "gzip, {helpers} helpers");
    }
    // Transcoding is decoded ahead too.
    let latin2 = format!("<?xml version=\"1.0\" encoding=\"ISO-8859-2\"?><r>{text}\u{f3}</r>");
    let (encoded, _, _) = encoding_rs::ISO_8859_2.encode(&latin2);
    let out = decode_with_helpers(encoded.into_owned(), 2);
    assert!(out == latin2.as_bytes());
}
