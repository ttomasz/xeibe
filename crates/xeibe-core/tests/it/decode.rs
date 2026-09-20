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
    assert_eq!(detect_compression(b"<?xml version=\"1.0\"?>"), Compression::None);
    assert_eq!(detect_compression(&gzip(b"<a/>")), Compression::Gzip);
    assert_eq!(detect_compression(&zstd(b"<a/>")), Compression::Zstd);
    assert_eq!(detect_compression(b"PK\x03\x04rest"), Compression::Zip);
    // Too few bytes to tell: not compressed.
    assert_eq!(detect_compression(b""), Compression::None);
}

#[test]
fn detects_the_declared_encoding() {
    assert_eq!(detect_encoding(b"<?xml version=\"1.0\"?><a/>"), encoding_rs::UTF_8);
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
