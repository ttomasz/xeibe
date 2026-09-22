//! `xeibe wfs layers|count|convert` against a scripted local HTTP server
//! (`docs/wfs.md`). Never a real service.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use parquet::file::reader::FileReader;

use crate::support::*;

/// Each connection gets the next body (HTTP 200) and is closed.
struct Server {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(bodies: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binding a local port");
        let base = format!("http://{}/wfs", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            for body in bodies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                log.lock().unwrap().push(request_line(&mut stream));
                let _ = respond(&mut stream, body.as_bytes());
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

fn respond(stream: &mut TcpStream, body: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Both)
}

/// WFS 2.0 capabilities with one feature type and `startIndex` paging.
fn capabilities() -> String {
    r#"<wfs:WFS_Capabilities xmlns:wfs="http://www.opengis.net/wfs/2.0"
    xmlns:ows="http://www.opengis.net/ows/1.1" version="2.0.0">
  <ows:ServiceIdentification><ows:Title>Test parcels</ows:Title></ows:ServiceIdentification>
  <ows:OperationsMetadata>
    <ows:Constraint name="ImplementsResultPaging"><ows:DefaultValue>TRUE</ows:DefaultValue></ows:Constraint>
    <ows:Constraint name="CountDefault"><ows:DefaultValue>1000</ows:DefaultValue></ows:Constraint>
  </ows:OperationsMetadata>
  <wfs:FeatureTypeList>
    <wfs:FeatureType xmlns:app="http://example.com/app">
      <wfs:Name>app:Parcel</wfs:Name>
      <wfs:Title>Land parcels</wfs:Title>
      <wfs:DefaultCRS>urn:ogc:def:crs:EPSG::2180</wfs:DefaultCRS>
      <ows:WGS84BoundingBox>
        <ows:LowerCorner>14.1 49.0</ows:LowerCorner>
        <ows:UpperCorner>24.2 54.9</ows:UpperCorner>
      </ows:WGS84BoundingBox>
    </wfs:FeatureType>
  </wfs:FeatureTypeList>
</wfs:WFS_Capabilities>"#
        .to_string()
}

/// A 2.0 page with point features `ids` in EPSG:2180 (x/y as written).
fn page(ids: &[u32], matched: u32) -> String {
    let members: String = ids
        .iter()
        .map(|id| {
            format!(
                r#"<wfs:member><app:Parcel gml:id="p{id}"><app:n>{id}</app:n><app:geom><gml:Point gml:id="g{id}" srsName="EPSG:2180"><gml:pos>{} {}</gml:pos></gml:Point></app:geom></app:Parcel></wfs:member>"#,
                500000 + id,
                300000 + id
            )
        })
        .collect();
    format!(
        r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app" numberMatched="{matched}" numberReturned="{}">{members}</wfs:FeatureCollection>"#,
        ids.len()
    )
}

#[test]
fn wfs_layers_lists_feature_types() {
    let server = Server::start(vec![capabilities()]);
    let run = xeibe_ok(&["wfs", "layers", &server.base]);
    assert!(run.stdout.contains("WFS 2.0.0: Test parcels"), "{}", run.stdout);
    let line = run.stdout.lines().find(|line| line.starts_with("app:Parcel")).expect("a line for app:Parcel");
    assert!(line.contains("Land parcels"), "{line}");
    assert!(line.contains("urn:ogc:def:crs:EPSG::2180"), "{line}");
    assert!(line.contains("14.1"), "{line}");
    assert!(server.requests()[0].contains("GetCapabilities"), "{:?}", server.requests());
}

#[test]
fn wfs_count_prints_number_matched() {
    let hits = r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" numberMatched="1234" numberReturned="0"/>"#;
    let server = Server::start(vec![capabilities(), hits.to_string()]);
    let run = xeibe_ok(&["wfs", "count", &server.base, "--type-name", "app:Parcel"]);
    assert_eq!(run.stdout.trim(), "1234");
    let requests = server.requests();
    assert!(requests[1].to_ascii_lowercase().contains("resulttype=hits"), "{requests:?}");
}

#[test]
fn wfs_convert_streams_every_page_into_parquet() {
    let server = Server::start(vec![capabilities(), page(&[1, 2], 3), page(&[3], 3)]);
    let dir = out_dir("wfs_convert");
    let out = dir.join("parcels.parquet");
    let run = xeibe_ok(&[
        "wfs",
        "convert",
        &server.base,
        "--type-name",
        "app:Parcel",
        "--page-size",
        "2",
        // The short form under WFS 2.0 is ambiguous; the flag makes it x/y.
        "--axis-order",
        "xy",
        "-o",
        path_str(&out),
    ]);
    assert!(run.stderr.contains("3 rows written"), "{}", run.stderr);
    assert!(run.stderr.contains("page 2"), "progress per page: {}", run.stderr);

    let requests = server.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert!(requests[1].to_ascii_uppercase().contains("STARTINDEX=0"), "{requests:?}");
    assert!(requests[2].to_ascii_uppercase().contains("STARTINDEX=2"), "{requests:?}");

    assert_eq!(parquet_reader(&out).metadata().file_metadata().num_rows(), 3);
    let geo = geo_metadata(&out);
    let column = &geo["columns"]["geom"];
    assert_eq!(column["geometry_types"], serde_json::json!(["Point"]));
    assert_eq!(column["crs"]["id"]["code"], 2180);
    assert_eq!(column["bbox"], serde_json::json!([500001.0, 300001.0, 500003.0, 300003.0]));
}

#[test]
fn wfs_convert_of_an_unknown_type_fails() {
    let server = Server::start(vec![capabilities()]);
    let dir = out_dir("wfs_unknown");
    let out = dir.join("x.parquet");
    let run = xeibe(&["wfs", "convert", &server.base, "--type-name", "app:Nothing", "-o", path_str(&out)]);
    assert!(!run.success);
    assert!(run.stderr.contains("app:Nothing"), "{}", run.stderr);
    assert!(!out.exists());
}
