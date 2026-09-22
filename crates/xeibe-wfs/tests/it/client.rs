//! The client against a scripted local HTTP server (`docs/wfs.md`, "Flow",
//! "Paging", "Errors and robustness"): capabilities, hits, pages as lazy
//! sources, exceptions in 200 and 400 responses, truncated pages.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use xeibe_wfs::pages::Progress;
use xeibe_wfs::{Error, PagingStrategy, WfsClient, WfsOptions};

/// `(status, headers, body)`.
type Reply = (u16, Vec<(&'static str, &'static str)>, Vec<u8>);

/// Each connection gets the next reply and is closed.
struct Server {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding a local port");
        let base = format!("http://{}/wfs", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            for (status, headers, body) in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                log.lock().unwrap().push(request_line(&mut stream));
                let _ = respond(&mut stream, status, &headers, &body);
            }
        });
        Self { base, requests }
    }

    /// Request lines (`GET /wfs?… HTTP/1.1`) received so far.
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn request_line(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    head.lines().next().unwrap_or_default().to_string()
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body)?;
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Both)
}

fn ok(body: impl Into<Vec<u8>>) -> Reply {
    (200, Vec::new(), body.into())
}

fn capabilities(paging: bool, count_default: u64) -> String {
    format!(
        r#"<wfs:WFS_Capabilities xmlns:wfs="http://www.opengis.net/wfs/2.0"
    xmlns:ows="http://www.opengis.net/ows/1.1" version="2.0.0">
  <ows:OperationsMetadata>
    <ows:Constraint name="ImplementsResultPaging"><ows:DefaultValue>{}</ows:DefaultValue></ows:Constraint>
    <ows:Constraint name="CountDefault"><ows:DefaultValue>{count_default}</ows:DefaultValue></ows:Constraint>
  </ows:OperationsMetadata>
  <wfs:FeatureTypeList>
    <wfs:FeatureType xmlns:app="http://example.com/app"><wfs:Name>app:Parcel</wfs:Name></wfs:FeatureType>
  </wfs:FeatureTypeList>
</wfs:WFS_Capabilities>"#,
        if paging { "TRUE" } else { "FALSE" }
    )
}

/// A 2.0 page with features `ids`.
fn page(ids: &[u32], matched: &str, next: Option<&str>) -> String {
    let next = next.map(|n| format!(r#" next="{n}""#)).unwrap_or_default();
    let members: String = ids
        .iter()
        .map(|id| format!(r#"<wfs:member><app:Parcel gml:id="p{id}"><app:n>{id}</app:n></app:Parcel></wfs:member>"#))
        .collect();
    format!(
        r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app" numberMatched="{matched}" numberReturned="{}"{next}>{members}</wfs:FeatureCollection>"#,
        ids.len()
    )
}

fn options() -> WfsOptions {
    WfsOptions {
        timeout: Duration::from_secs(10),
        max_retries: 1,
        ..WfsOptions::default()
    }
}

fn no_progress() -> Box<dyn FnMut(&Progress) + Send> {
    Box::new(|_| {})
}

fn read_all(sources: xeibe_core::Sources) -> Result<Vec<String>, xeibe_core::Error> {
    sources
        .map(|source| {
            let mut text = String::new();
            source?.open()?.read_to_string(&mut text)?;
            Ok(text)
        })
        .collect()
}

#[test]
fn layers_and_hits_come_from_the_server() {
    let hits = r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" numberMatched="1234" numberReturned="0"/>"#;
    let server = Server::start(vec![ok(capabilities(true, 1000)), ok(hits)]);
    let client = WfsClient::new(&server.base, options()).unwrap();
    let capabilities = client.capabilities().expect("capabilities");
    assert_eq!(capabilities.feature_types[0].name, "app:Parcel");
    assert_eq!(client.count("Parcel").expect("a count"), Some(1234));

    let requests = server.requests();
    assert_eq!(
        requests.len(),
        2,
        "capabilities are fetched once: {requests:?}"
    );
    assert!(
        requests[0].contains("REQUEST=GetCapabilities"),
        "{}",
        requests[0]
    );
    assert!(requests[1].contains("RESULTTYPE=hits"), "{}", requests[1]);
    assert!(
        requests[1].contains("NAMESPACES=xmlns%28app%2Chttp"),
        "{}",
        requests[1]
    );
}

#[test]
fn start_index_pages_are_fetched_lazily_until_a_short_page() {
    let server = Server::start(vec![
        ok(capabilities(true, 2)),
        ok(page(&[1, 2], "unknown", None)),
        ok(page(&[3, 2], "unknown", None)),
        ok(page(&[5], "unknown", None)),
    ]);
    let client = WfsClient::new(&server.base, options()).unwrap();
    let reports = Arc::new(Mutex::new(Vec::new()));
    let log = reports.clone();
    let mut sources = client
        .pages(
            "app:Parcel",
            Box::new(move |p: &Progress| log.lock().unwrap().push(p.clone())),
        )
        .expect("pages");
    assert_eq!(server.requests().len(), 1, "no page before the read asks");

    let first = sources.next().expect("a page").expect("fetched");
    assert!(first.name().contains("STARTINDEX=0"), "{}", first.name());
    assert_eq!(server.requests().len(), 2);
    let rest = read_all(sources).expect("the other pages");
    assert_eq!(rest.len(), 2);

    let requests = server.requests();
    assert!(
        requests[2].contains("STARTINDEX=2") && requests[2].contains("COUNT=2"),
        "{}",
        requests[2]
    );
    assert!(requests[3].contains("STARTINDEX=4"), "{}", requests[3]);

    let reports = reports.lock().unwrap();
    assert_eq!(reports.len(), 3);
    assert_eq!(reports[2].features, 5);
    assert!(
        reports[1].warnings.iter().any(|w| w.contains("gml:id")),
        "a repeated id is reported: {:?}",
        reports[1].warnings
    );
}

#[test]
fn a_server_generated_next_link_takes_over() {
    // Bind first, so the `next` link can name the port.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/wfs", listener.local_addr().unwrap());
    let next = format!("{base}?page=2");
    let replies = vec![
        ok(capabilities(false, 2)),
        ok(page(&[1, 2], "3", Some(&next))),
        ok(page(&[3], "3", None)),
    ];
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    std::thread::spawn(move || {
        for (status, headers, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            log.lock().unwrap().push(request_line(&mut stream));
            let _ = respond(&mut stream, status, &headers, &body);
        }
    });
    let client = WfsClient::new(&base, options()).unwrap();
    let pages = read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect("pages");
    assert_eq!(pages.len(), 2);
    let requests = requests.lock().unwrap();
    assert!(
        requests[2].starts_with("GET /wfs?page=2 "),
        "{}",
        requests[2]
    );
}

#[test]
fn an_exception_report_in_a_200_response_fails_the_read() {
    let report = r#"<ows:ExceptionReport xmlns:ows="http://www.opengis.net/ows/1.1"><ows:Exception exceptionCode="OperationProcessingFailed"><ows:ExceptionText>boom</ows:ExceptionText></ows:Exception></ows:ExceptionReport>"#;
    let server = Server::start(vec![ok(capabilities(true, 10)), ok(report)]);
    let client = WfsClient::new(&server.base, options()).unwrap();
    let error = read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect_err("an error");
    assert!(
        error.to_string().contains("OperationProcessingFailed"),
        "{error}"
    );
}

#[test]
fn an_exception_report_sent_with_http_400_is_reported() {
    let report = r#"<ows:ExceptionReport xmlns:ows="http://www.opengis.net/ows/1.1"><ows:Exception exceptionCode="InvalidParameterValue" locator="typeNames"/></ows:ExceptionReport>"#;
    let server = Server::start(vec![
        ok(capabilities(true, 10)),
        (400, Vec::new(), report.into()),
    ]);
    let client = WfsClient::new(&server.base, options()).unwrap();
    match client.count("app:Parcel") {
        Err(Error::Exception(report)) => {
            assert_eq!(report.exceptions[0].locator.as_deref(), Some("typeNames"));
        }
        other => panic!("expected the exception report, got {other:?}"),
    }
}

#[test]
fn a_truncated_page_is_retried_with_half_the_page_size() {
    let cut = page(&[1, 2, 3, 4], "4", None);
    let cut = cut[..cut.len() - 40].to_string();
    let server = Server::start(vec![
        ok(capabilities(true, 4)),
        ok(cut),
        ok(page(&[1, 2], "4", None)),
        ok(page(&[3, 4], "4", None)),
    ]);
    let client = WfsClient::new(&server.base, options()).unwrap();
    let pages = read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect("pages");
    assert_eq!(pages.len(), 2);
    let requests = server.requests();
    assert!(requests[1].contains("COUNT=4"), "{}", requests[1]);
    assert!(
        requests[2].contains("COUNT=2") && requests[2].contains("STARTINDEX=0"),
        "{}",
        requests[2]
    );
    assert!(requests[3].contains("STARTINDEX=2"), "{}", requests[3]);
}

#[test]
fn a_page_that_stays_truncated_fails_the_read() {
    let cut = page(&[1, 2], "unknown", None);
    let cut = cut[..cut.len() - 30].to_string();
    let server = Server::start(vec![ok(capabilities(false, 0)), ok(cut.clone()), ok(cut)]);
    let client = WfsClient::new(&server.base, options()).unwrap();
    let error = read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect_err("an error");
    assert!(error.to_string().contains("truncated"), "{error}");
}

#[test]
fn a_gzip_response_is_decoded() {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(page(&[1], "1", None).as_bytes()).unwrap();
    let gzipped = encoder.finish().unwrap();
    let server = Server::start(vec![
        ok(capabilities(false, 0)),
        (200, vec![("Content-Encoding", "gzip")], gzipped),
    ]);
    let client = WfsClient::new(&server.base, options()).unwrap();
    let pages = read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect("pages");
    assert!(pages[0].contains(r#"gml:id="p1""#), "{}", pages[0]);
}

#[test]
fn pages_can_be_fetched_ahead_of_the_read() {
    let server = Server::start(vec![
        ok(capabilities(true, 1)),
        ok(page(&[1], "3", None)),
        ok(page(&[2], "3", None)),
        ok(page(&[3], "3", None)),
    ]);
    let client = WfsClient::new(
        &server.base,
        WfsOptions {
            concurrency: 2,
            strategy: Some(PagingStrategy::StartIndex { page_size: 1 }),
            ..options()
        },
    )
    .unwrap();
    let pages = read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect("pages");
    assert_eq!(pages.len(), 3);
    assert!(pages[2].contains("p3"), "in order");
}

#[test]
fn an_unknown_type_fails_before_any_page() {
    let server = Server::start(vec![ok(capabilities(true, 10))]);
    let client = WfsClient::new(&server.base, options()).unwrap();
    assert!(matches!(
        client.pages("app:Nope", no_progress()),
        Err(Error::UnknownFeatureType(_))
    ));
}
