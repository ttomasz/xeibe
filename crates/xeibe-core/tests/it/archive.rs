//! Sources and zip archives (`docs/architecture.md`, "Sources and remote
//! input" and "Zip archives").
//!
//! Zip is deliberately minimal: local files only, `.gml`/`.xml`(`.gz`) members,
//! `archive.zip!/member` for one member, no zips inside zips.

use std::io::Write;
use std::path::{Path, PathBuf};

use xeibe_core::{ByteSource, Error, Source, archive};

use crate::support::{read_to_string, temp_dir};

const DOC: &str = r#"<?xml version="1.0"?><gml:FeatureCollection xmlns:gml="http://www.opengis.net/gml/3.2"/>"#;

/// An archive with GML members, a non-GML member and a nested zip.
fn build_archive(dir: &Path) -> PathBuf {
    let path = dir.join("data.zip");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for name in ["a.gml", "sub/b.XML", "readme.pdf", "meta/metadata.xml"] {
        zip.start_file(name, options).unwrap();
        zip.write_all(DOC.as_bytes()).unwrap();
    }
    zip.start_file("inner.zip", options).unwrap();
    zip.write_all(b"PK\x03\x04not really").unwrap();
    zip.finish().unwrap();
    path
}

#[test]
fn a_local_file_is_read_as_one_stream() {
    let dir = temp_dir("source-file");
    let path = dir.join("one.gml");
    std::fs::write(&path, DOC).unwrap();

    let source = Source::file(&path).expect("a file source");
    assert!(source.name().contains("one.gml"));
    assert_eq!(read_to_string(&source).unwrap(), DOC);
    // A file source can be opened again (a scan followed by a read).
    assert_eq!(read_to_string(&source).unwrap(), DOC);
}

#[test]
fn a_compressed_file_is_decompressed_by_the_source() {
    let dir = temp_dir("source-gz");
    let path = dir.join("one.gml.gz");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(DOC.as_bytes()).unwrap();
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();

    let source = Source::file(&path).expect("a file source");
    assert_eq!(read_to_string(&source).unwrap(), DOC);
}

#[test]
fn a_one_shot_reader_can_only_be_opened_once() {
    // stdin, or a body handed in by the caller.
    let reader = Box::new(std::io::Cursor::new(DOC.as_bytes().to_vec()));
    let source = Source::reader("-", reader);
    assert_eq!(read_to_string(&source).unwrap(), DOC);
    let error = read_to_string(&source).expect_err("the second open fails");
    assert!(matches!(error, Error::NotReopenable(_)), "got {error:?}");
}

#[test]
fn bytes_already_in_memory_are_a_source() {
    // This is how a fetched WFS page reaches a read.
    let source = xeibe_core::BytesSource {
        name: "page 1".into(),
        bytes: bytes::Bytes::from_static(DOC.as_bytes()),
    };
    assert_eq!(source.name(), "page 1");
    assert_eq!(source.len(), Some(DOC.len() as u64));
    let mut text = String::new();
    std::io::Read::read_to_string(&mut source.open().unwrap(), &mut text).unwrap();
    assert_eq!(text, DOC);
}

#[test]
fn file_sources_report_their_length_for_progress() {
    let dir = temp_dir("source-len");
    let path = dir.join("one.gml");
    std::fs::write(&path, DOC).unwrap();
    let source = xeibe_core::FileSource::open(&path).expect("a file source");
    assert_eq!(source.len(), Some(DOC.len() as u64));
}

#[test]
fn splits_the_member_path_form() {
    assert_eq!(
        archive::split_member_path("data/a.zip!/dir/file.gml"),
        Some(("data/a.zip", "dir/file.gml"))
    );
    assert_eq!(archive::split_member_path("data/a.zip"), None);
    assert_eq!(archive::split_member_path("plain.gml"), None);
}

#[test]
fn lists_gml_candidates_only() {
    let dir = temp_dir("archive-list");
    let path = build_archive(&dir);
    let mut names: Vec<String> = archive::list_candidates(&path, &[])
        .expect("listing")
        .into_iter()
        .map(|member| member.name)
        .collect();
    names.sort();
    // `.xml` members count too, and the match is case-insensitive. Other
    // members, including nested zips, are ignored.
    assert_eq!(names, ["a.gml", "meta/metadata.xml", "sub/b.XML"]);
}

#[test]
fn member_globs_narrow_the_listing() {
    let dir = temp_dir("archive-glob");
    let path = build_archive(&dir);
    let names: Vec<String> = archive::list_candidates(&path, &["sub/*".to_string()])
        .expect("listing")
        .into_iter()
        .map(|member| member.name)
        .collect();
    assert_eq!(names, ["sub/b.XML"]);
}

#[test]
fn every_member_of_an_archive_is_a_source() {
    let dir = temp_dir("archive-expand");
    let path = build_archive(&dir);
    let sources = archive::expand(&path, &[]).expect("sources");
    assert_eq!(sources.len(), 3);
    for source in &sources {
        assert_eq!(read_to_string(source).unwrap(), DOC);
        assert!(matches!(source, Source::ZipMember { .. }));
    }
}

#[test]
fn one_member_can_be_addressed_directly() {
    let dir = temp_dir("archive-member");
    let path = build_archive(&dir);
    let input = format!("{}!/a.gml", path.display());
    let source = Source::file(&input).expect("a member source");
    match &source {
        Source::ZipMember { member, .. } => assert_eq!(member, "a.gml"),
        other => panic!("expected a zip member, got {other:?}"),
    }
    assert_eq!(read_to_string(&source).unwrap(), DOC);
    assert!(source.name().contains("a.gml"));
}

#[test]
fn an_archive_without_gml_members_is_an_error() {
    let dir = temp_dir("archive-empty");
    let path = dir.join("none.zip");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
    zip.start_file("readme.pdf", options).unwrap();
    zip.write_all(b"%PDF-1.4").unwrap();
    zip.finish().unwrap();

    let error = archive::expand(&path, &[]).expect_err("no GML members");
    assert!(matches!(error, Error::NoGmlMembers(_)), "got {error:?}");
}

#[test]
fn a_missing_member_is_an_error_naming_the_archive() {
    let dir = temp_dir("archive-missing");
    let path = build_archive(&dir);
    let input = format!("{}!/nope.gml", path.display());
    let source = Source::file(&input).expect("the path parses");
    let error = read_to_string(&source).expect_err("the member does not exist");
    assert!(matches!(error, Error::Zip { .. }), "got {error:?}");
}

#[test]
fn expanding_inputs_covers_files_directories_and_globs() {
    let dir = temp_dir("expand-sources");
    std::fs::write(dir.join("one.gml"), DOC).unwrap();
    std::fs::write(dir.join("two.gml"), DOC).unwrap();
    std::fs::write(dir.join("notes.txt"), "ignore me").unwrap();

    let by_file = xeibe_core::source::expand_sources(&[dir.join("one.gml").display().to_string()])
        .expect("one file");
    assert_eq!(by_file.len(), 1);

    let by_directory =
        xeibe_core::source::expand_sources(&[dir.display().to_string()]).expect("a directory");
    assert_eq!(by_directory.len(), 2, "only the GML files");

    let by_glob =
        xeibe_core::source::expand_sources(&[format!("{}/*.gml", dir.display())]).expect("a glob");
    assert_eq!(by_glob.len(), 2);
}

#[test]
fn a_remote_archive_asks_the_user_to_download_it() {
    // Zip needs the central directory at the end, so remote zips are refused.
    let error = xeibe_core::source::expand_sources(&["https://example.com/data.zip!/a.gml".into()])
        .expect_err("a remote archive");
    assert!(matches!(error, Error::RemoteArchive(_)), "got {error:?}");
}
