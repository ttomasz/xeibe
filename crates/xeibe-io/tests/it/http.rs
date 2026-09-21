//! HTTP sources (`docs/architecture.md`, "Sources and remote input"): one
//! streaming `GET`, retries only before the first byte was handed on, a broken
//! body fails the read, and a remote zip is an error.

use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use xeibe_io::http::{HttpClient, HttpSource};
use xeibe_io::{Auth, HttpOptions, IoOptions, resolve_sources};

use crate::support::{Reply, Server, body};

fn options(max_retries: u32) -> HttpOptions {
    HttpOptions {
        timeout: Duration::from_secs(10),
        max_retries,
        ..HttpOptions::default()
    }
}

fn client(max_retries: u32) -> HttpClient {
    HttpClient::new(&options(max_retries)).expect("an HTTP client")
}

#[test]
fn a_get_streams_the_whole_body() {
    let expected = body(1 << 20);
    let server = Server::start(vec![Reply::ok(expected.clone())]);
    let mut reader = client(0)
        .get_stream(&server.url("/roads.gml"))
        .expect("a stream");
    let mut read = Vec::new();
    reader.read_to_end(&mut read).expect("the body");
    assert_eq!(read, expected);
    assert_eq!(server.requests().len(), 1);
    assert!(
        server.requests()[0].starts_with("GET /roads.gml HTTP/1.1"),
        "{}",
        server.requests()[0]
    );
}

#[test]
fn get_returns_the_body_in_memory() {
    let server = Server::start(vec![Reply::ok("<wfs:WFS_Capabilities/>")]);
    let bytes = client(0)
        .get(&server.url("/wfs?request=GetCapabilities"))
        .expect("a body");
    assert_eq!(&bytes[..], b"<wfs:WFS_Capabilities/>");
}

#[test]
fn a_5xx_is_retried_before_the_body_starts() {
    let server = Server::start(vec![
        Reply::retry_after(503, 0),
        Reply::status(502),
        Reply::ok("<a/>"),
    ]);
    let mut read = String::new();
    client(3)
        .get_stream(&server.url("/a.gml"))
        .expect("a stream after two retries")
        .read_to_string(&mut read)
        .unwrap();
    assert_eq!(read, "<a/>");
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn a_429_waits_as_long_as_retry_after_says() {
    let server = Server::start(vec![Reply::retry_after(429, 1), Reply::ok("<a/>")]);
    let start = Instant::now();
    let bytes = client(1)
        .get(&server.url("/a.gml"))
        .expect("a body after one retry");
    assert_eq!(&bytes[..], b"<a/>");
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn retries_run_out() {
    let server = Server::start(vec![Reply::retry_after(500, 0), Reply::retry_after(500, 0)]);
    let error = client(1)
        .get_stream(&server.url("/a.gml"))
        .err()
        .expect("an error");
    assert!(
        matches!(error, xeibe_io::Error::HttpStatus { status: 500, .. }),
        "{error}"
    );
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_client_error_is_not_retried() {
    let server = Server::start(vec![Reply::status(404), Reply::ok("<a/>")]);
    let error = client(3)
        .get(&server.url("/missing.gml"))
        .expect_err("a 404");
    assert!(error.to_string().contains("404"), "{error}");
    assert!(error.to_string().contains("/missing.gml"), "{error}");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_body_that_breaks_before_its_first_byte_is_retried() {
    let server = Server::start(vec![
        Reply::Broken {
            declared: 100,
            sent: Vec::new(),
        },
        Reply::ok("<a/>"),
    ]);
    let mut read = String::new();
    client(1)
        .get_stream(&server.url("/a.gml"))
        .expect("a stream after one retry")
        .read_to_string(&mut read)
        .unwrap();
    assert_eq!(read, "<a/>");
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_body_that_breaks_midway_fails_the_read_and_is_not_retried() {
    let sent = body(300 << 10);
    let server = Server::start(vec![
        Reply::Broken {
            declared: 1 << 20,
            sent: sent.clone(),
        },
        Reply::ok(body(1 << 20)),
    ]);
    let url = server.url("/big.gml");
    let mut reader = client(3).get_stream(&url).expect("the stream starts");
    let mut read = Vec::new();
    let error = reader
        .read_to_end(&mut read)
        .expect_err("the body is incomplete");
    assert!(error.to_string().contains(url.as_str()), "{error}");
    assert!(read.len() < sent.len() + 1, "{} bytes", read.len());
    assert_eq!(read[..], sent[..read.len()]);
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_refused_connection_is_a_network_error() {
    // A port that was free a moment ago.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = url::Url::parse(&format!("http://127.0.0.1:{port}/a.gml")).unwrap();
    let error = client(0).get(&url).expect_err("nothing listens");
    assert!(matches!(error, xeibe_io::Error::Network { .. }), "{error}");
}

#[test]
fn auth_headers_and_user_agent_are_sent() {
    let server = Server::start(vec![Reply::ok("<a/>")]);
    let client = HttpClient::new(&HttpOptions {
        auth: Auth::Bearer("secret-token".into()),
        headers: vec![("X-Api-Key".into(), "k1".into())],
        user_agent: "xeibe-test/1".into(),
        ..options(0)
    })
    .unwrap();
    client.get(&server.url("/a.gml")).unwrap();
    let head = server.requests()[0].to_ascii_lowercase();
    assert!(
        head.contains("authorization: bearer secret-token"),
        "{head}"
    );
    assert!(head.contains("x-api-key: k1"), "{head}");
    assert!(head.contains("user-agent: xeibe-test/1"), "{head}");
    // Secrets stay out of debug output.
    let debug = format!("{client:?}");
    assert!(
        !debug.contains("secret-token") && !debug.contains("k1"),
        "{debug}"
    );
}

#[test]
fn an_http_source_reopens_with_a_new_get() {
    let server = Server::start(vec![Reply::ok("<a/>"), Reply::ok("<a/>")]);
    let source = HttpSource::new(Arc::new(client(0)), server.url("/a.gml"));
    for _ in 0..2 {
        let mut read = String::new();
        xeibe_core::ByteSource::open(&source)
            .unwrap()
            .read_to_string(&mut read)
            .unwrap();
        assert_eq!(read, "<a/>");
    }
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_resolved_url_reads_through_the_source() {
    let server = Server::start(vec![Reply::ok(body(4096))]);
    let io = IoOptions {
        http: options(0),
        ..IoOptions::default()
    };
    let sources = resolve_sources(&[server.url("/data/roads.gml").to_string()], &io).unwrap();
    assert_eq!(sources.len(), 1);
    assert!(
        server.requests().is_empty(),
        "resolving does not contact the server"
    );
    let mut read = Vec::new();
    sources[0]
        .open()
        .expect("opened")
        .read_to_end(&mut read)
        .unwrap();
    assert_eq!(read, body(4096));
}

#[test]
fn a_remote_zip_is_rejected_by_its_url_or_its_content() {
    let server = Server::start(vec![Reply::ok(b"PK\x03\x04rest of a zip".to_vec())]);
    let io = IoOptions {
        http: options(0),
        ..IoOptions::default()
    };

    let error =
        resolve_sources(&[server.url("/data.zip").to_string()], &io).expect_err("a .zip URL");
    assert!(
        matches!(
            error,
            xeibe_io::Error::Core(xeibe_core::Error::RemoteArchive(_))
        ),
        "{error}"
    );

    // A download URL that says nothing about the format.
    let sources = resolve_sources(&[server.url("/download?id=7").to_string()], &io).unwrap();
    let error = sources[0].open().err().expect("zip content");
    assert!(
        matches!(error, xeibe_core::Error::RemoteArchive(_)),
        "{error}"
    );
    assert!(error.to_string().contains("download it first"), "{error}");
}
