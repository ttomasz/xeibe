//! The client against a scripted local HTTP server (`docs/wfs.md`, "Flow",
//! "Paging", "Request encoding (XML POST)", "Errors and robustness"):
//! capabilities, hits, pages as lazy sources, requests sent by POST,
//! exceptions in 200 and 400 responses, truncated pages.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use xeibe_wfs::pages::Progress;
use xeibe_wfs::request::SortOrder;
use xeibe_wfs::{Error, HttpMethod, PagingStrategy, WfsClient, WfsOptions};

/// `(status, headers, body)`.
type Reply = (u16, Vec<(&'static str, &'static str)>, Vec<u8>);

/// Each connection gets the next reply and is closed.
struct Server {
    base: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
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
                log.lock().unwrap().push(read_request(&mut stream));
                let _ = respond(&mut stream, status, &headers, &body);
            }
        });
        Self { base, requests }
    }

    /// Request lines (`GET /wfs?… HTTP/1.1`) received so far.
    fn requests(&self) -> Vec<String> {
        let requests = self.requests.lock().unwrap();
        requests.iter().map(|(line, _)| line.clone()).collect()
    }

    /// Request bodies received so far, empty for a `GET`.
    fn bodies(&self) -> Vec<String> {
        let requests = self.requests.lock().unwrap();
        requests.iter().map(|(_, body)| body.clone()).collect()
    }
}

/// The request line and the body (`Content-Length` bytes).
fn read_request(stream: &mut TcpStream) -> (String, String) {
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    let mut body = vec![0; length];
    let _ = stream.read_exact(&mut body);
    let line = head.lines().next().unwrap_or_default().to_string();
    (line, String::from_utf8_lossy(&body).into_owned())
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

/// A query parameter of a request line (`GET /wfs?… HTTP/1.1`), decoded;
/// keys in any case.
fn param(request: &str, key: &str) -> Option<String> {
    let target = request.split(' ').nth(1)?;
    url::Url::parse(&format!("http://host{target}"))
        .ok()?
        .query_pairs()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.into_owned())
}

type Reports = Arc<Mutex<Vec<Progress>>>;
type Callback = Box<dyn FnMut(&Progress) + Send>;

/// Progress reports, kept for the test to look at.
fn recorded() -> (Reports, Callback) {
    let reports = Arc::new(Mutex::new(Vec::new()));
    let log = reports.clone();
    (
        reports,
        Box::new(move |p: &Progress| log.lock().unwrap().push(p.clone())),
    )
}

fn warnings(reports: &Mutex<Vec<Progress>>) -> Vec<String> {
    reports
        .lock()
        .unwrap()
        .iter()
        .flat_map(|report| report.warnings.clone())
        .collect()
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
fn describe_reads_the_declared_properties_of_the_type() {
    let xsd = r#"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:app="http://example.com/app" elementFormDefault="qualified" targetNamespace="http://example.com/app">
  <xsd:complexType name="ParcelType"><xsd:sequence>
    <xsd:element name="n" type="xsd:int"/>
  </xsd:sequence></xsd:complexType>
  <xsd:element name="Parcel" type="app:ParcelType"/>
</xsd:schema>"#;
    let server = Server::start(vec![ok(capabilities(true, 1000)), ok(xsd)]);
    let client = WfsClient::new(&server.base, options()).expect("a client");
    let schema = client.describe("app:Parcel").expect("the schema");
    assert_eq!(schema.name, "Parcel");
    assert_eq!(schema.namespace.as_deref(), Some("http://example.com/app"));
    let names: Vec<&str> = schema.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["n"]);

    let request = &server.requests()[1];
    assert!(request.contains("REQUEST=DescribeFeatureType"), "{request}");
    assert!(request.contains("TYPENAMES=app%3AParcel"), "{request}");
    assert!(
        request.contains("NAMESPACES=xmlns%28app%2Chttp"),
        "{request}"
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

    assert!(requests.iter().all(|r| param(r, "SORTBY").is_none()));

    let reports = reports.lock().unwrap();
    assert_eq!(reports.len(), 3);
    assert_eq!(reports[2].features, 5);
    assert!(
        reports[1].warnings.iter().any(|w| w.contains("gml:id")),
        "a repeated id is reported: {:?}",
        reports[1].warnings
    );
    // Offsets without SORTBY: said once, before the second page.
    let unsorted = |report: &Progress| {
        report
            .warnings
            .iter()
            .filter(|w| w.contains("without SORTBY"))
            .count()
    };
    assert_eq!(unsorted(&reports[0]), 1, "{:?}", reports[0].warnings);
    assert_eq!(reports.iter().map(unsorted).sum::<usize>(), 1);
}

/// Every request of a read carries the sort keys: the first, the `next` link
/// the server builds from it, and the offsets the read continues with after
/// `ResponseCacheExpired`, so the order never changes partway.
#[test]
fn sort_keys_go_with_every_request_of_a_read() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/wfs", listener.local_addr().unwrap());
    // The server's own link keeps the query, as GeoServer's do.
    let next = format!("{base}?SORTBY=app%3An%20DESC&page=2");
    let expired = r#"<ows:ExceptionReport xmlns:ows="http://www.opengis.net/ows/1.1" version="2.0.0"><ows:Exception exceptionCode="ResponseCacheExpired"/></ows:ExceptionReport>"#;
    let replies = vec![
        ok(capabilities(true, 2)),
        ok(page(&[5, 4], "3", Some(&next))),
        ok(expired),
        ok(page(&[3], "3", None)),
    ];
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    std::thread::spawn(move || {
        for (status, headers, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            log.lock().unwrap().push(read_request(&mut stream).0);
            let _ = respond(&mut stream, status, &headers, &body);
        }
    });
    let options = WfsOptions {
        sort_by: vec![("app:n".into(), SortOrder::Desc)],
        ..options()
    };
    let client = WfsClient::new(&base, options).unwrap();
    let (reports, progress) = recorded();
    let pages = read_all(client.pages("app:Parcel", progress).unwrap()).expect("pages");
    assert_eq!(pages.len(), 2);

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert_eq!(param(&requests[1], "STARTINDEX").as_deref(), Some("0"));
    assert_eq!(
        param(&requests[2], "page").as_deref(),
        Some("2"),
        "the next link"
    );
    assert_eq!(param(&requests[3], "STARTINDEX").as_deref(), Some("2"));
    for request in &requests[1..] {
        assert_eq!(
            param(request, "SORTBY").as_deref(),
            Some("app:n DESC"),
            "{request}"
        );
    }
    let warnings = warnings(&reports);
    assert!(
        warnings.iter().any(|w| w.contains("cache expired")),
        "{warnings:?}"
    );
    assert!(
        !warnings.iter().any(|w| w.contains("without SORTBY")),
        "{warnings:?}"
    );
}

#[test]
fn a_server_that_declares_it_does_not_sort_gets_a_warning() {
    // A single request (no paging) is sorted too.
    let capabilities = capabilities(false, 0).replace(
        "</wfs:WFS_Capabilities>",
        r#"<fes:Filter_Capabilities xmlns:fes="http://www.opengis.net/fes/2.0"><fes:Conformance>
    <fes:Constraint name="ImplementsSorting"><ows:NoValues/><ows:DefaultValue>FALSE</ows:DefaultValue></fes:Constraint>
  </fes:Conformance></fes:Filter_Capabilities></wfs:WFS_Capabilities>"#,
    );
    let server = Server::start(vec![ok(capabilities), ok(page(&[1, 2], "2", None))]);
    let options = WfsOptions {
        sort_by: vec![("app:n".into(), SortOrder::Asc)],
        ..options()
    };
    let client = WfsClient::new(&server.base, options).unwrap();
    let (reports, progress) = recorded();
    read_all(client.pages("app:Parcel", progress).unwrap()).expect("the page");
    let requests = server.requests();
    assert_eq!(param(&requests[1], "SORTBY").as_deref(), Some("app:n ASC"));
    let warnings = warnings(&reports);
    assert!(
        warnings.iter().any(|w| w.contains("ImplementsSorting")),
        "{warnings:?}"
    );
}

#[test]
fn wfs_10_cannot_sort_and_says_so() {
    let capabilities = r#"<WFS_Capabilities xmlns="http://www.opengis.net/wfs" version="1.0.0">
  <Capability><Request><GetFeature><ResultFormat><GML2/></ResultFormat></GetFeature></Request></Capability>
  <FeatureTypeList><FeatureType><Name>app:Parcel</Name></FeatureType></FeatureTypeList>
</WFS_Capabilities>"#;
    let features = r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs" xmlns:gml="http://www.opengis.net/gml" xmlns:app="http://example.com/app"><gml:featureMember><app:Parcel fid="p1"><app:n>1</app:n></app:Parcel></gml:featureMember></wfs:FeatureCollection>"#;
    let server = Server::start(vec![ok(capabilities), ok(features)]);
    let options = WfsOptions {
        sort_by: vec![("app:n".into(), SortOrder::Asc)],
        ..options()
    };
    let client = WfsClient::new(&server.base, options).unwrap();
    let (reports, progress) = recorded();
    read_all(client.pages("app:Parcel", progress).unwrap()).expect("the page");
    assert_eq!(param(&server.requests()[1], "SORTBY"), None);
    let warnings = warnings(&reports);
    assert!(
        warnings.iter().any(|w| w.contains("WFS 1.0 has no SORTBY")),
        "{warnings:?}"
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
            log.lock().unwrap().push(read_request(&mut stream).0);
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

/// A 2.0 page whose counts are the server's to choose; `numberReturned` is
/// the true count unless `returned` is given.
fn page_with_counts(ids: &[u32], matched: &str, returned: Option<&str>) -> String {
    let returned = returned.map_or(ids.len().to_string(), str::to_string);
    let members: String = ids
        .iter()
        .map(|id| format!(r#"<wfs:member><app:Parcel gml:id="p{id}"><app:n>{id}</app:n></app:Parcel></wfs:member>"#))
        .collect();
    format!(
        r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app" numberMatched="{matched}" numberReturned="{returned}">{members}</wfs:FeatureCollection>"#
    )
}

#[test]
fn counts_that_contradict_the_pages_do_not_end_the_read() {
    // deegree: numberReturned="0" on every page. A filtered read whose
    // numberMatched is 0. Either ended the read after the first page.
    for (matched, returned) in [("unknown", Some("0")), ("0", None)] {
        let server = Server::start(vec![
            ok(capabilities(true, 1000)),
            ok(page_with_counts(&[1, 2], matched, returned)),
            ok(page_with_counts(&[3], matched, returned)),
        ]);
        let client = WfsClient::new(
            &server.base,
            WfsOptions {
                page_size: Some(2),
                ..options()
            },
        )
        .unwrap();
        let (reports, callback) = recorded();
        let pages = read_all(client.pages("app:Parcel", callback).unwrap()).expect("pages");
        assert_eq!(pages.len(), 2, "matched={matched} returned={returned:?}");
        let reports = reports.lock().unwrap();
        let last = reports.last().expect("a report");
        assert_eq!(last.features, 3, "matched={matched} returned={returned:?}");
        assert_eq!(last.number_matched, None, "no total to show");
    }
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

/// A FES 2.0 filter on `app:n` whose GET URL is longer than
/// `MAX_GET_URL_LENGTH`.
fn long_filter() -> String {
    let conditions: String = (0..30)
        .map(|n| format!("<fes:PropertyIsEqualTo><fes:ValueReference>app:n</fes:ValueReference><fes:Literal>{n}</fes:Literal></fes:PropertyIsEqualTo>"))
        .collect();
    format!(
        r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0"><fes:Or>{conditions}</fes:Or></fes:Filter>"#
    )
}

fn post_options(method: HttpMethod) -> WfsOptions {
    WfsOptions {
        filter: Some(long_filter()),
        method,
        page_size: Some(2),
        ..options()
    }
}

#[test]
fn a_filter_too_long_for_a_url_is_posted_page_by_page() {
    let server = Server::start(vec![
        ok(capabilities(true, 1000)),
        ok(page(&[1, 2], "3", None)),
        ok(page(&[3], "3", None)),
    ]);
    let client = WfsClient::new(&server.base, post_options(HttpMethod::Auto)).unwrap();
    let (reports, callback) = recorded();
    let sources: Vec<_> = client
        .pages("app:Parcel", callback)
        .unwrap()
        .collect::<Result<_, _>>()
        .expect("pages");
    assert_eq!(sources.len(), 2);
    let requests = server.requests();
    assert!(requests[0].starts_with("GET /wfs?"), "{}", requests[0]);
    let bodies = server.bodies();
    for (i, start) in [(1, 0), (2, 2)] {
        assert_eq!(
            requests[i], "POST /wfs HTTP/1.1",
            "no WFS parameters in the URL"
        );
        assert!(
            bodies[i].contains(&format!(r#"count="2" startIndex="{start}""#)),
            "{}",
            bodies[i]
        );
        assert!(bodies[i].contains(&long_filter()), "{}", bodies[i]);
    }
    assert!(
        sources[1].name().ends_with("/wfs (POST, startIndex=2)"),
        "{}",
        sources[1].name()
    );
    // The test capabilities advertise no POST endpoint.
    let warnings = warnings(&reports);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("doesn't advertise POST")),
        "{warnings:?}"
    );
}

#[test]
fn hits_are_posted_like_the_pages() {
    let hits = r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" numberMatched="30" numberReturned="0"/>"#;
    let server = Server::start(vec![ok(capabilities(true, 1000)), ok(hits)]);
    let client = WfsClient::new(&server.base, post_options(HttpMethod::Auto)).unwrap();
    assert_eq!(client.count("app:Parcel").expect("a count"), Some(30));
    assert_eq!(server.requests()[1], "POST /wfs HTTP/1.1");
    assert!(
        server.bodies()[1].contains(r#"resultType="hits""#),
        "{}",
        server.bodies()[1]
    );
}

#[test]
fn the_method_can_be_forced_either_way() {
    let server = Server::start(vec![ok(capabilities(false, 0)), ok(page(&[1], "1", None))]);
    let client = WfsClient::new(&server.base, post_options(HttpMethod::Get)).unwrap();
    read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect("pages");
    assert!(server.requests()[1].starts_with("GET /wfs?"));
    assert!(server.requests()[1].contains("FILTER="));

    let server = Server::start(vec![ok(capabilities(false, 0)), ok(page(&[1], "1", None))]);
    let client = WfsClient::new(
        &server.base,
        WfsOptions {
            method: HttpMethod::Post,
            ..options()
        },
    )
    .unwrap();
    read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect("pages");
    assert_eq!(server.requests()[1], "POST /wfs HTTP/1.1");
    assert!(
        server.bodies()[1].contains(r#"<wfs:Query typeNames="app:Parcel""#),
        "{}",
        server.bodies()[1]
    );
}

#[test]
fn posts_go_where_the_capabilities_came_from() {
    let server = Server::start(vec![
        (
            301,
            vec![(
                "Location",
                "/moved?map=x.map&SERVICE=WFS&REQUEST=GetCapabilities",
            )],
            Vec::new(),
        ),
        ok(capabilities(false, 0)),
        ok(page(&[1], "1", None)),
    ]);
    let client = WfsClient::new(&server.base, post_options(HttpMethod::Auto)).unwrap();
    read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect("pages");
    assert_eq!(server.requests()[2], "POST /moved?map=x.map HTTP/1.1");
}

#[test]
fn a_post_redirected_into_a_get_is_an_error_that_says_so() {
    // The GET arrives without the request; the server answers with its
    // capabilities, which must not be read as a feature.
    let server = Server::start(vec![
        ok(capabilities(false, 0)),
        (302, vec![("Location", "/elsewhere")], Vec::new()),
        ok(capabilities(false, 0)),
    ]);
    let client = WfsClient::new(&server.base, post_options(HttpMethod::Post)).unwrap();
    let error = read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect_err("an error");
    let message = error.to_string();
    assert!(message.contains("not a feature collection"), "{message}");
    assert!(message.contains("redirected the POST to"), "{message}");
    assert!(message.contains("/elsewhere"), "{message}");
    assert!(server.requests()[2].starts_with("GET /elsewhere"));
}

#[test]
fn a_next_link_that_repeats_a_long_request_leaves_offset_paging_alone() {
    // GeoServer puts the POSTed filter into its `next` links.
    let long_link = format!("http://example.com/wfs?FILTER={}", "x".repeat(2000));
    let server = Server::start(vec![
        ok(capabilities(true, 1000)),
        ok(page(&[1, 2], "3", Some(&long_link))),
        ok(page(&[3], "3", None)),
    ]);
    let client = WfsClient::new(&server.base, post_options(HttpMethod::Auto)).unwrap();
    let (reports, callback) = recorded();
    let pages = read_all(client.pages("app:Parcel", callback).unwrap()).expect("pages");
    assert_eq!(pages.len(), 2);
    assert_eq!(server.requests()[2], "POST /wfs HTTP/1.1");
    assert!(server.bodies()[2].contains(r#"startIndex="2""#));
    let warnings = warnings(&reports);
    assert!(
        !warnings.iter().any(|w| w.contains("next link")),
        "nothing to say: offsets are what the link held: {warnings:?}"
    );
}

#[test]
fn a_next_link_too_long_to_send_is_left_for_offsets() {
    let long_link = format!("http://example.com/wfs?FILTER={}", "x".repeat(2000));
    let server = Server::start(vec![
        ok(capabilities(true, 1000)),
        ok(page(&[1, 2], "3", Some(&long_link))),
        ok(page(&[3], "3", None)),
    ]);
    let client = WfsClient::new(
        &server.base,
        WfsOptions {
            strategy: Some(PagingStrategy::NextLink),
            ..post_options(HttpMethod::Auto)
        },
    )
    .unwrap();
    let (reports, callback) = recorded();
    let pages = read_all(client.pages("app:Parcel", callback).unwrap()).expect("pages");
    assert_eq!(pages.len(), 2);
    assert_eq!(server.requests()[2], "POST /wfs HTTP/1.1");
    assert!(server.bodies()[2].contains(r#"startIndex="2""#));
    let warnings = warnings(&reports);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("too long to send") && w.contains("startIndex=2")),
        "{warnings:?}"
    );
}

#[test]
fn a_next_link_too_long_to_send_ends_a_read_without_offsets() {
    let long_link = format!("http://example.com/wfs?FILTER={}", "x".repeat(2000));
    let server = Server::start(vec![
        ok(capabilities(false, 0)),
        ok(page(&[1, 2], "3", Some(&long_link))),
    ]);
    let client = WfsClient::new(&server.base, post_options(HttpMethod::Auto)).unwrap();
    let error = read_all(client.pages("app:Parcel", no_progress()).unwrap()).expect_err("an error");
    assert!(error.to_string().contains("too long to send"), "{error}");
    assert_eq!(server.requests().len(), 2, "the link is never requested");
}

#[test]
fn vendor_parameters_keep_a_long_request_from_being_posted() {
    let server = Server::start(vec![ok(capabilities(true, 1000))]);
    let client = WfsClient::new(
        &server.base,
        WfsOptions {
            vendor_params: vec![("CQL_FILTER".into(), "n=1".into())],
            ..post_options(HttpMethod::Auto)
        },
    )
    .unwrap();
    let error = client
        .pages("app:Parcel", no_progress())
        .err()
        .expect("an error before any page");
    let message = error.to_string();
    assert!(message.contains("CQL_FILTER"), "{message}");
    assert!(message.contains("force GET"), "{message}");
    assert_eq!(server.requests().len(), 1, "only the capabilities");
}
