//! Turning inputs into sources (`docs/architecture.md`, "Sources and remote
//! input"). Nothing here touches the network.

use std::path::PathBuf;

use xeibe_io::{IoOptions, resolve_sources};

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creating the temporary directory");
    dir
}

fn options() -> IoOptions {
    IoOptions {
        http: xeibe_io::HttpOptions {
            timeout: std::time::Duration::from_secs(30),
            max_retries: 3,
            auth: xeibe_io::Auth::None,
            headers: Vec::new(),
            user_agent: "xeibe-test".into(),
        },
        block_size: 8 << 20,
        read_ahead: 4,
    }
}

#[test]
fn local_paths_globs_and_directories_become_sources() {
    let dir = temp_dir("io-resolve");
    std::fs::write(dir.join("a.gml"), "<a/>").unwrap();
    std::fs::write(dir.join("b.gml"), "<a/>").unwrap();

    let one =
        resolve_sources(&[dir.join("a.gml").display().to_string()], &options()).expect("one file");
    assert_eq!(one.len(), 1);

    let all = resolve_sources(&[format!("{}/*.gml", dir.display())], &options()).expect("a glob");
    assert_eq!(all.len(), 2);
}

#[test]
#[cfg(feature = "http")]
fn http_urls_become_streaming_sources() {
    let sources =
        resolve_sources(&["https://example.com/data/roads.gml".into()], &options()).expect("a URL");
    assert_eq!(sources.len(), 1);
    assert!(
        sources[0].name().contains("example.com"),
        "{}",
        sources[0].name()
    );
}

#[test]
fn stdin_is_a_source_too() {
    let sources = resolve_sources(&["-".into()], &options()).expect("stdin");
    assert_eq!(sources.len(), 1);
}

#[test]
fn a_remote_zip_asks_the_user_to_download_it_first() {
    // Zip needs the central directory at the end, so it must be local.
    let error = resolve_sources(
        &["https://example.com/data.zip!/roads.gml".into()],
        &options(),
    )
    .expect_err("a remote archive");
    assert!(error.to_string().contains("download"), "{error}");
}

#[test]
fn a_path_that_does_not_exist_is_an_error() {
    let dir = temp_dir("io-missing");
    assert!(resolve_sources(&[dir.join("nope.gml").display().to_string()], &options()).is_err());
}
