//! Object-store sources (`docs/architecture.md`, "Sources and remote input"):
//! one streaming `get`, handed to a synchronous reader in blocks. Uses the
//! in-memory store, so nothing leaves the process.

use std::io::Read;
use std::sync::Arc;

use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use xeibe_core::ByteSource;
use xeibe_io::IoOptions;
use xeibe_io::object_store::{ObjectStoreSource, store_sources};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .unwrap()
}

fn store(runtime: &tokio::runtime::Runtime, objects: &[(&str, &[u8])]) -> Arc<dyn ObjectStore> {
    let store = InMemory::new();
    for (key, bytes) in objects {
        runtime
            .block_on(store.put(&Path::from(*key), bytes.to_vec().into()))
            .unwrap();
    }
    Arc::new(store)
}

fn small_blocks() -> IoOptions {
    IoOptions {
        block_size: 7,
        read_ahead: 2,
        ..IoOptions::default()
    }
}

#[test]
fn an_object_is_read_in_blocks_from_start_to_end() {
    let runtime = runtime();
    let data = b"<gml:FeatureCollection><f>1</f><f>2</f></gml:FeatureCollection>";
    let store = store(&runtime, &[("dir/a.gml", data)]);
    let source = ObjectStoreSource::new(
        store,
        Path::from("dir/a.gml"),
        runtime.handle().clone(),
        None,
    )
    .with_name("memory:///dir/a.gml")
    .with_blocks(7, 2);
    assert_eq!(source.name(), "memory:///dir/a.gml");
    for _ in 0..2 {
        let mut read = Vec::new();
        source.open().unwrap().read_to_end(&mut read).unwrap();
        assert_eq!(read, data);
    }
}

#[test]
fn an_empty_object_is_an_empty_stream() {
    let runtime = runtime();
    let store = store(&runtime, &[("empty.gml", b"")]);
    let source = ObjectStoreSource::new(
        store,
        Path::from("empty.gml"),
        runtime.handle().clone(),
        None,
    );
    let mut read = Vec::new();
    source.open().unwrap().read_to_end(&mut read).unwrap();
    assert!(read.is_empty());
}

#[test]
fn a_missing_object_fails_on_open() {
    let runtime = runtime();
    let store = store(&runtime, &[]);
    let source = ObjectStoreSource::new(
        store,
        Path::from("nope.gml"),
        runtime.handle().clone(),
        None,
    );
    let error = source.open().err().expect("no such object");
    assert!(error.to_string().contains("nope.gml"), "{error}");
}

#[test]
fn a_zip_object_is_a_remote_archive() {
    let runtime = runtime();
    let store = store(&runtime, &[("download", b"PK\x03\x04rest of a zip")]);
    let source = ObjectStoreSource::new(
        store,
        Path::from("download"),
        runtime.handle().clone(),
        None,
    );
    let error = source.open().err().expect("zip content");
    assert!(
        matches!(error, xeibe_core::Error::RemoteArchive(_)),
        "{error}"
    );
}

#[test]
fn a_prefix_lists_gml_objects_in_key_order_with_sizes() {
    let runtime = runtime();
    let store = store(
        &runtime,
        &[
            ("data/b.gml", b"<b/>"),
            ("data/sub/c.xml.gz", b"xx"),
            ("data/a.gml", b"<a/>"),
            ("data/readme.txt", b"no"),
            ("other/d.gml", b"<d/>"),
        ],
    );
    let sources = store_sources(
        store,
        "s3://bucket",
        "data/",
        runtime.handle(),
        &small_blocks(),
    )
    .unwrap();
    let names: Vec<_> = sources.iter().map(|s| s.name()).collect();
    assert_eq!(
        names,
        [
            "s3://bucket/data/a.gml",
            "s3://bucket/data/b.gml",
            "s3://bucket/data/sub/c.xml.gz"
        ]
    );
    let xeibe_core::Source::Stream(first) = &sources[0] else {
        panic!("a stream source");
    };
    assert_eq!(first.len(), Some(4));
}

#[test]
fn a_glob_matches_keys_and_star_stays_within_one_level() {
    let runtime = runtime();
    let store = store(
        &runtime,
        &[
            ("data/a.gml", b"<a/>"),
            ("data/b.xml", b"<b/>"),
            ("data/sub/c.gml", b"<c/>"),
        ],
    );
    let names = |key: &str| -> Vec<String> {
        store_sources(
            store.clone(),
            "s3://bucket",
            key,
            runtime.handle(),
            &small_blocks(),
        )
        .unwrap()
        .iter()
        .map(|s| s.name())
        .collect()
    };
    assert_eq!(names("data/*.gml"), ["s3://bucket/data/a.gml"]);
    assert_eq!(
        names("data/**/*.gml"),
        ["s3://bucket/data/a.gml", "s3://bucket/data/sub/c.gml"]
    );
}

#[test]
fn a_zip_under_a_prefix_is_a_remote_archive() {
    let runtime = runtime();
    let store = store(&runtime, &[("data/a.gml", b"<a/>"), ("data/b.zip", b"PK")]);
    let error = store_sources(
        store,
        "s3://bucket",
        "data/",
        runtime.handle(),
        &small_blocks(),
    )
    .expect_err("a zip");
    assert!(error.to_string().contains("download it first"), "{error}");
}
