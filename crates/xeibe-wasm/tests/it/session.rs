//! Scan, read, the map's geometry and its grids, the table's rows.

use xeibe_testkit::read_data;
use xeibe_wasm::session::{Dataset, Reader};

fn dataset(files: &[&str]) -> Dataset {
    let mut dataset = Dataset::default();
    for file in files {
        dataset.add_file(file.to_string(), read_data(file));
    }
    dataset
}

/// Read every batch, loading grids from the committed fixtures as the
/// page loads them from the CDN.
fn read_all(reader: &mut Reader) -> Vec<xeibe_wasm::display::Drawing> {
    let mut drawings = Vec::new();
    while reader.next_batch().unwrap() {
        load_grids(reader);
        drawings.push(reader.take().unwrap());
    }
    drawings
}

fn load_grids(reader: &mut Reader) {
    let dir = xeibe_testkit::data_dir().join("reprojection/grids");
    loop {
        let requests = reader.grid_requests();
        if requests.is_empty() {
            return;
        }
        for request in requests {
            let file = std::fs::read(dir.join(&request.file)).unwrap();
            let start = request.offset as usize;
            let end = request
                .length
                .map_or(file.len(), |n| (start + n as usize).min(file.len()));
            reader
                .add_grid_data(&request.file, request.offset, file[start..end].to_vec())
                .unwrap();
        }
    }
}

#[test]
fn a_layer_goes_onto_the_map_and_into_the_table() {
    let mut dataset = dataset(&["samples/pl/prg-address-points.gml"]);
    let scan = dataset.scan(None).unwrap();
    let layers: Vec<&str> = scan["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        layers,
        ["AD_Miejscowosc", "AD_UlicaPlac", "AD_PunktAdresowy"]
    );
    assert_eq!(scan["layers"][2]["crs"][0]["name"], "ETRS89 / PL-1992");

    let schema = dataset.schema("AD_PunktAdresowy").unwrap();
    assert!(
        schema["explain"]
            .as_str()
            .unwrap()
            .contains("georeferencja")
    );

    let mut reader = dataset.read("AD_PunktAdresowy", None, None).unwrap();
    let info = reader.info();
    assert_eq!(info["geometry_column"], "georeferencja");
    assert_eq!(info["map"]["target"], "EPSG:4326");
    let drawings = read_all(&mut reader);
    let map = drawings[0].map.as_ref().unwrap();
    assert_eq!(map.point_rows, [0, 1]);
    for lonlat in map.points.chunks(2) {
        assert!(
            (14.0..24.2).contains(&lonlat[0]) && (49.0..55.0).contains(&lonlat[1]),
            "{lonlat:?}"
        );
    }
    // The source space starts at the first coordinate.
    assert_eq!(reader.origin(), Some([222442.2, 431274.25]));
    assert_eq!(&drawings[0].source.points[..2], &[0.0, 0.0]);

    let rows = reader.rows(0, 10);
    assert_eq!(rows.len(), 2);
    let columns = info["columns"].as_array().unwrap();
    let cell = |row: usize, name: &str| {
        let i = columns.iter().position(|c| c["name"] == name).unwrap();
        rows[row][i].clone()
    };
    assert_eq!(cell(0, "georeferencja"), "POINT (222442.2 431274.25)");
    // A timestamp with a time zone is a UTC instant.
    assert_eq!(cell(0, "wersjaId"), "2017-04-13T14:10:59Z");
    assert_eq!(reader.rows(1, 10).len(), 1);
    let report = reader.report();
    assert_eq!(report["rows"], 2);
    assert_eq!(report["not_on_map"], 0);
}

#[test]
fn a_limit_stops_the_read() {
    let dataset = dataset(&["samples/be/wallonia-lpis-ecological-focus-area-gml33.gml"]);
    let mut reader = dataset.read("EcologicalFocusArea", None, Some(2)).unwrap();
    read_all(&mut reader);
    assert_eq!(reader.rows_read(), 2);
}

#[test]
fn a_layer_without_geometry_has_no_map() {
    let dataset = dataset(&["samples/pl/rcn-price-register-arcs.gml"]);
    let mut reader = dataset.read("RCN_Transakcja", None, None).unwrap();
    let info = reader.info();
    assert!(info["geometry_column"].is_null());
    assert_eq!(info["map"]["notes"][0], "the layer has no geometry column");
    let drawings = read_all(&mut reader);
    assert!(drawings[0].map.is_none());
    assert_eq!(reader.rows_read(), 1);
}

/// A point on Puerto Rico 1940 (EPSG:4139), latitude first as the URI form
/// says.
fn puerto_rico() -> Vec<u8> {
    r#"<?xml version="1.0"?>
<gml:FeatureCollection xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app">
  <gml:featureMember><app:Site gml:id="s1"><app:name>Ponce</app:name>
    <app:geom><gml:Point srsName="http://www.opengis.net/def/crs/EPSG/0/4139"><gml:pos>18.0 -66.6</gml:pos></gml:Point></app:geom>
  </app:Site></gml:featureMember>
</gml:FeatureCollection>"#
        .as_bytes()
        .to_vec()
}

#[test]
fn the_map_waits_for_the_grids_it_needs() {
    let mut dataset = Dataset::default();
    dataset.add_file("pr.gml".into(), puerto_rico());
    let mut reader = dataset.read("Site", None, None).unwrap();
    assert!(reader.next_batch().unwrap());
    let requests = reader.grid_requests();
    assert!(
        requests.iter().any(|r| r.file == "us_noaa_prvi.tif"),
        "{requests:?}"
    );
    assert!(requests[0].url().starts_with("https://cdn.proj.org/"));
    // The point is on both of Puerto Rico's grids (NADCON and HPGN): the
    // map needs the one its operation reads.
    load_grids(&mut reader);
    let drawing = reader.take().unwrap();
    let map = drawing.map.unwrap();
    assert_eq!(map.point_rows, [0]);
    // The datum shift is a few hundred metres.
    let (lon, lat) = (map.points[0], map.points[1]);
    assert!(
        (lon + 66.6).abs() < 0.01 && (lat - 18.0).abs() < 0.01,
        "{lon} {lat}"
    );
    assert!(
        (lon + 66.6).abs() > 1e-5 || (lat - 18.0).abs() > 1e-5,
        "no shift: {lon} {lat}"
    );
}

#[test]
fn a_grid_that_cant_be_fetched_leaves_the_map_without_grids_and_says_so() {
    let mut dataset = Dataset::default();
    dataset.add_file("pr.gml".into(), puerto_rico());
    let mut reader = dataset.read("Site", None, None).unwrap();
    assert!(reader.next_batch().unwrap());
    assert!(!reader.grid_requests().is_empty());
    reader.without_grids("us_noaa_prvi.tif: network error");
    assert!(reader.grid_requests().is_empty());
    let notes = reader.info()["map"]["notes"].to_string();
    assert!(notes.contains("shown without grids"), "{notes}");
    let drawing = reader.take().unwrap();
    // Converted all the same, by an operation without grids.
    assert_eq!(drawing.map.unwrap().point_rows, [0]);
}
