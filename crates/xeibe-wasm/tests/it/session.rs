//! Scan, read, the map's geometry and its grids, the table's rows.

use serde_json::json;
use xeibe_testkit::read_data;
use xeibe_wasm::session::{Choice, Dataset, Reader};

/// The files, all chosen.
fn dataset(files: &[&str]) -> Dataset {
    let mut dataset = Dataset::default();
    for file in files {
        dataset.add_file(file.to_string(), read_data(file));
    }
    choose_all(&mut dataset, files.len());
    dataset
}

fn choose_all(dataset: &mut Dataset, files: usize) {
    let choices: Vec<Choice> = (0..files)
        .map(|file| Choice { file, member: None })
        .collect();
    dataset.choose(&choices).unwrap();
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
    let scan = dataset.scan(None, None).unwrap();
    assert_eq!(scan["complete"], true);
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

    // Each column with the reasons for its type.
    let schema = dataset.schema("AD_PunktAdresowy").unwrap();
    let georeferencja = schema["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "georeferencja")
        .unwrap();
    assert!(!georeferencja["reasons"].as_array().unwrap().is_empty());

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
    choose_all(&mut dataset, 1);
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
    choose_all(&mut dataset, 1);
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

/// A zip of two samples and a PDF, as a dropped file.
fn prg_zip() -> Vec<u8> {
    use std::io::Write;
    let mut zip_bytes = Vec::new();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_bytes));
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, file) in [
        ("data/addresses.gml", "samples/pl/prg-address-points.gml"),
        ("data/more.xml", "samples/pl/prg-address-points.gml"),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(&read_data(file)).unwrap();
    }
    zip.start_file("readme.pdf", options).unwrap();
    zip.write_all(b"%PDF").unwrap();
    zip.finish().unwrap();
    zip_bytes
}

#[test]
fn a_zip_archive_lists_its_members() {
    let mut dataset = Dataset::default();
    dataset.add_file("prg.zip".into(), prg_zip());
    dataset.add_file(
        "a.gml".into(),
        read_data("samples/pl/prg-address-points.gml"),
    );
    let files = dataset.files();
    assert_eq!(files[0]["name"], "prg.zip");
    let members: Vec<(&str, bool)> = files[0]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["name"].as_str().unwrap(), m["gml"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        members,
        [
            ("data/addresses.gml", true),
            ("data/more.xml", true),
            ("readme.pdf", false)
        ]
    );
    assert!(files[0]["members"][0]["unreadable"].is_null());
    assert!(files[1]["members"].is_null());
    assert!(files[1]["size"].as_u64().unwrap() > 0);
}

#[test]
fn a_zip_archive_is_read_from_memory() {
    let mut dataset = Dataset::default();
    dataset.add_file("prg.zip".into(), prg_zip());
    choose_all(&mut dataset, 1);
    let scan = dataset.scan(None, None).unwrap();
    assert_eq!(
        scan["files"],
        json!(["prg.zip!/data/addresses.gml", "prg.zip!/data/more.xml"])
    );
    assert_eq!(scan["layers"][2]["feature_count"], 4);
    let mut reader = dataset.read("AD_PunktAdresowy", None, None).unwrap();
    read_all(&mut reader);
    assert_eq!(reader.rows_read(), 4);

    // One member.
    dataset
        .choose(&[Choice {
            file: 0,
            member: Some("data/more.xml".into()),
        }])
        .unwrap();
    let scan = dataset.scan(None, None).unwrap();
    assert_eq!(scan["files"], json!(["prg.zip!/data/more.xml"]));
    let mut reader = dataset.read("AD_PunktAdresowy", None, None).unwrap();
    read_all(&mut reader);
    assert_eq!(reader.rows_read(), 2);

    // A member that isn't GML can't be read; a zip without GML can't be
    // chosen whole.
    let pdf = dataset.choose(&[Choice {
        file: 0,
        member: Some("missing.gml".into()),
    }]);
    assert!(pdf.unwrap_err().contains("missing.gml"));
    let mut empty = Dataset::default();
    empty.add_file("empty.zip".into(), {
        let mut bytes = Vec::new();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        zip.start_file("readme.pdf", zip::write::FileOptions::<'_, ()>::default())
            .unwrap();
        zip.finish().unwrap();
        bytes
    });
    let error = empty.choose(&[Choice {
        file: 0,
        member: None,
    }]);
    assert!(error.unwrap_err().contains("empty.zip"));

    // Bytes that aren't a zip are listed with the error.
    let mut broken = Dataset::default();
    broken.add_file("broken.zip".into(), b"not a zip".to_vec());
    assert!(broken.files()[0]["error"].is_string());
}

#[test]
fn a_sample_of_each_file_is_enough_for_the_schema() {
    let mut dataset = dataset(&[
        "samples/pl/prg-address-points.gml",
        "samples/be/wallonia-lpis-ecological-focus-area-gml33.gml",
    ]);
    let scan = dataset.scan(None, Some(1)).unwrap();
    assert_eq!(scan["complete"], false);
    assert_eq!(scan["sample"], 1);
    // The first feature of each file: PRG's first layer, and Wallonia's.
    let layers: Vec<&str> = scan["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(layers, ["AD_Miejscowosc", "EcologicalFocusArea"]);
    // The read takes the scan's schema, and reads every feature.
    let mut reader = dataset.read("EcologicalFocusArea", None, None).unwrap();
    let mut schema = dataset.schema("EcologicalFocusArea").unwrap();
    for column in schema["columns"].as_array_mut().unwrap() {
        column.as_object_mut().unwrap().remove("reasons");
    }
    assert_eq!(reader.info()["columns"], schema["columns"]);
    read_all(&mut reader);
    assert!(reader.rows_read() > 1);
}

#[test]
fn a_rows_xml_is_read_back_from_its_file() {
    let prg = read_data("samples/pl/prg-address-points.gml");
    let text = String::from_utf8(prg.clone()).unwrap();
    let gzipped = {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&prg).unwrap();
        encoder.finish().unwrap()
    };
    let mut dataset = Dataset::default();
    dataset.add_file("prg.zip".into(), prg_zip());
    dataset.add_file("a.gml".into(), prg.clone());
    dataset.add_file("a.gml.gz".into(), gzipped);
    let choices = [
        Choice {
            file: 0,
            member: Some("data/more.xml".into()),
        },
        Choice {
            file: 1,
            member: None,
        },
        Choice {
            file: 2,
            member: None,
        },
    ];
    dataset.choose(&choices).unwrap();
    dataset.scan(None, None).unwrap();
    let mut reader = dataset.read("AD_PunktAdresowy", None, None).unwrap();
    read_all(&mut reader);
    assert_eq!(reader.rows_read(), 6);
    // The location isn't one of the table's columns.
    let columns = reader.info()["columns"].to_string();
    assert!(!columns.contains("xeibe:location"), "{columns}");
    assert_eq!(
        reader.rows(0, 1)[0].len(),
        reader.info()["columns"].as_array().unwrap().len()
    );
    let mut sources = Vec::new();
    for row in 0..6 {
        let xml = reader.feature_xml(row, 1 << 20).unwrap();
        let text_xml = xml["xml"].as_str().unwrap();
        assert!(
            text_xml.starts_with("<prgad:AD_PunktAdresowy"),
            "{text_xml}"
        );
        assert!(
            text_xml.ends_with("</prgad:AD_PunktAdresowy>"),
            "{text_xml}"
        );
        assert!(text.contains(text_xml));
        assert_eq!(xml["truncated"], false);
        sources.push(xml["source"].as_str().unwrap().to_string());
    }
    // Natively, on several threads, the files' rows interleave.
    sources.sort();
    sources.dedup();
    assert_eq!(sources, ["a.gml", "a.gml.gz", "prg.zip!/data/more.xml"]);
    let short = reader.feature_xml(0, 10).unwrap();
    assert_eq!(short["xml"].as_str().unwrap().len(), 10);
    assert_eq!(short["truncated"], true);
    assert!(reader.feature_xml(6, 10).is_err());
}

/// Export `layer` of the dataset: the file, and the report.
fn export(
    dataset: &Dataset,
    layer: &str,
    options: Option<&str>,
) -> (bytes::Bytes, serde_json::Value) {
    let mut export = dataset.export(layer, options).unwrap();
    let mut file = Vec::new();
    while export.next_batch().unwrap() {
        file.extend(export.take_bytes());
    }
    let report = export.finish().unwrap();
    file.extend(export.take_bytes());
    (bytes::Bytes::from(file), report)
}

/// The first value of the file's `column`, as WKB.
fn first_wkb(file: &bytes::Bytes, column: &str) -> Vec<u8> {
    use arrow_array::cast::AsArray;
    let batch =
        parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file.clone())
            .unwrap()
            .build()
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
    batch
        .column_by_name(column)
        .unwrap()
        .as_binary::<i32>()
        .value(0)
        .to_vec()
}

#[test]
fn a_layer_is_exported_whole_to_geoparquet() {
    let mut dataset = dataset(&["samples/pl/prg-address-points.gml"]);
    dataset.scan(None, None).unwrap();
    let (file, report) = export(&dataset, "AD_PunktAdresowy", None);
    assert_eq!(report["rows"], 2);
    let reader =
        parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file.clone())
            .unwrap();
    let metadata = reader.metadata().file_metadata();
    assert_eq!(metadata.num_rows(), 2);
    let keys: Vec<&str> = metadata
        .key_value_metadata()
        .unwrap()
        .iter()
        .map(|kv| kv.key.as_str())
        .collect();
    assert!(
        keys.contains(&"geo") && keys.contains(&"gml:settings"),
        "{keys:?}"
    );
    // The location column the page's reads carry stays out.
    assert!(
        reader
            .schema()
            .fields()
            .iter()
            .all(|f| f.name() != "xeibe:location")
    );
}

#[test]
fn an_export_follows_the_axis_order_chosen() {
    let mut dataset = dataset(&["samples/pl/prg-address-points.gml"]);
    dataset.scan(None, None).unwrap();
    let (file, _) = export(&dataset, "AD_PunktAdresowy", None);
    let options = json!({"geometry": {"axis": {"mode": "Auto", "overrides": {"EPSG:2180": "YX"}}}});
    let (swapped, report) = export(&dataset, "AD_PunktAdresowy", Some(&options.to_string()));
    let point = |wkb: &[u8]| -> [f64; 2] {
        let x = f64::from_le_bytes(wkb[5..13].try_into().unwrap());
        let y = f64::from_le_bytes(wkb[13..21].try_into().unwrap());
        [x, y]
    };
    let [x, y] = point(&first_wkb(&file, "georeferencja"));
    assert_eq!(point(&first_wkb(&swapped, "georeferencja")), [y, x]);
    assert!(
        report["axis_decisions"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("override")
    );
}

#[test]
fn curves_are_exported_as_straight_segments() {
    let mut dataset = dataset(&["samples/pl/rcn-price-register-arcs.gml"]);
    dataset.scan(None, None).unwrap();
    let (file, report) = export(&dataset, "RCN_Budynek", None);
    assert!(report["rows"].as_u64().unwrap() > 0);
    // ISO WKB type 3 or 6: Polygon or MultiPolygon, no CurvePolygon (10).
    let wkb = first_wkb(&file, "geometria");
    let code = u32::from_le_bytes(wkb[1..5].try_into().unwrap()) % 1000;
    assert!(code == 3 || code == 6, "WKB type {code}");
}
