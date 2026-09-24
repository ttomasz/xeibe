//! Reads of the real samples in `tests/data`.
//!
//! The axis order of every sample was verified independently of any GML reader
//! (`scripts/corpus/axis_evidence.py`), so `axis.expected_first_xy` in the
//! manifest is the ground truth — not GDAL, which reads 4 of the 17 samples in
//! the wrong order (`docs/geometry.md`, "Observed in real services").

use xeibe_arrow::{ReadOptions, read, scan};
use xeibe_geom::{AxisOrderMode, options::GeomEncoding};
use xeibe_schema::ScanExtent;
use xeibe_testkit::samples::{Sample, sample, samples};
use xeibe_testkit::wkt::{G, Tol};

use crate::support::{Read, collect, extension_name, file_sources};

/// Read geometry as WKB so every sample can be compared the same way.
fn options(axis: AxisOrderMode) -> ReadOptions {
    let mut options = ReadOptions::default();
    options.inference.geometry_encoding = GeomEncoding::Wkb;
    options.geometry.axis.mode = axis;
    options
}

fn read_layer(sample: &Sample, layer: &str, options: &ReadOptions) -> Read {
    let reader = read(file_sources(&sample.file), layer, None, options)
        .unwrap_or_else(|e| panic!("{}: reading {layer}: {e}", sample.name));
    collect(reader)
}

fn layer_names(sample: &Sample) -> Vec<String> {
    sample
        .gdal_report()
        .layers
        .iter()
        .map(|layer| layer.name.clone())
        .collect()
}

/// The first geometry of a layer, if it has one.
fn first_geometry(read: &Read) -> Option<G> {
    let column = read
        .schema
        .fields()
        .iter()
        .find(|field| extension_name(field).is_some())?
        .name()
        .clone();
    read.geometries(&column).into_iter().flatten().next()
}

/// The first geometry of the first layer that has one.
fn first_geometry_of(sample: &Sample, options: &ReadOptions) -> Option<G> {
    layer_names(sample)
        .iter()
        .find_map(|layer| first_geometry(&read_layer(sample, layer, options)))
}

fn tol() -> Tol {
    Tol { abs: 1e-6, rel: 0.0 }
}

#[test]
fn every_layer_of_every_sample_reads_the_features_gdal_counted() {
    for sample in samples() {
        let report = sample.gdal_report();
        for (layer, expected) in report.layers.iter().zip(&sample.gdal.feature_counts) {
            let read = read_layer(&sample, &layer.name, &options(AxisOrderMode::Auto));
            assert_eq!(
                read.rows() as u64,
                *expected,
                "{}: layer {}",
                sample.name,
                layer.name
            );
        }
    }
}

#[test]
fn an_explicit_axis_mode_reads_every_sample_in_the_verified_order() {
    // `--axis-order xy` / `yx` must be enough on its own: one mode per input.
    for sample in samples() {
        let mode = if sample.is_yx() {
            AxisOrderMode::YX
        } else {
            AxisOrderMode::XY
        };
        let Some(geometry) = first_geometry_of(&sample, &options(mode)) else {
            panic!("{}: no geometry", sample.name);
        };
        let first = geometry.first_vertex().expect("a vertex");
        let expected = &sample.axis.expected_first_xy;
        assert!(
            tol().matches(first[0], expected[0]) && tol().matches(first[1], expected[1]),
            "{}: read {first:?}, expected {expected:?} (source order {})",
            sample.name,
            sample.axis_order
        );
    }
}

#[test]
fn auto_finds_the_verified_axis_order() {
    let mut wrong = Vec::new();
    for sample in samples() {
        let Some(geometry) = first_geometry_of(&sample, &options(AxisOrderMode::Auto)) else {
            panic!("{}: no geometry", sample.name);
        };
        let first = geometry.first_vertex().expect("a vertex");
        let expected = &sample.axis.expected_first_xy;
        if !(tol().matches(first[0], expected[0]) && tol().matches(first[1], expected[1])) {
            wrong.push(format!(
                "{}: read {first:?}, expected {expected:?}",
                sample.name
            ));
        }
    }
    assert!(wrong.is_empty(), "Auto read these wrongly:\n  {}", wrong.join("\n  "));
}

#[test]
fn a_gml_2_dialect_outweighs_a_urn_that_says_authority_order() {
    // A station in Mayotte written lon/lat under `urn:ogc:def:crs:EPSG::4326`.
    // Nothing else decides it: no `axisLabels` (GML 2 has none), both orders are
    // legal latitudes, the envelope is written in the same order as the geometry,
    // and the `CrsHeuristic` fallback would read the URN and put the station in
    // the Atlantic, which is what GDAL does. The `<gml:coordinates>` carrier makes
    // it GML 2 dialect, and that decides it (`docs/geometry.md`, "`Auto`:
    // evidence-based decision", row 5).
    let sample = sample("fr-sandre-stations-wfs100-urn-lonlat");
    assert!(!sample.axis.gdal_agrees, "the sample GDAL reads backwards");

    let geometry = first_geometry_of(&sample, &options(AxisOrderMode::Auto)).expect("a geometry");
    let first = geometry.first_vertex().expect("a vertex");
    let expected = &sample.axis.expected_first_xy;
    assert!(
        tol().matches(first[0], expected[0]) && tol().matches(first[1], expected[1]),
        "read {first:?}, expected {expected:?}"
    );
}

#[test]
fn the_mapserver_wfs_11_quirk_is_applied() {
    // Same server, same short srsName, different order per WFS version: 1.0 is
    // x/y and 1.1 is authority order. Only the producer quirk can tell them
    // apart, because EPSG:2180's easting and northing ranges overlap.
    for name in [
        "pl-gugik-mapserver-addresses-wfs100",
        "pl-gugik-mapserver-addresses-wfs110",
        "pl-gugik-mapserver-addresses-wfs200",
    ] {
        let sample = sample(name);
        let geometry = first_geometry_of(&sample, &options(AxisOrderMode::Auto))
            .unwrap_or_else(|| panic!("{name}: no geometry"));
        let first = geometry.first_vertex().expect("a vertex");
        assert!(
            tol().matches(first[0], 547502.59) && tol().matches(first[1], 394384.17),
            "{name}: read {first:?}"
        );
    }
}

#[test]
fn prg_address_points_read_as_the_docs_describe() {
    let sample = sample("pl-prg-address-points");
    let read = read_layer(&sample, "AD_PunktAdresowy", &options(AxisOrderMode::Auto));
    assert_eq!(read.rows(), 2);
    assert_eq!(
        read.strings("kodPocztowy"),
        [Some("68-213".to_string()), Some("68-213".to_string())]
    );
    assert_eq!(
        read.strings("miejscowosc"),
        [
            Some("PL.ZIPIN.3719.EMUiA_0910802_2017-04-13T16_10_26_02_00".to_string()),
            Some("PL.ZIPIN.3719.EMUiA_0910854_2017-04-13T16_10_26_02_00".to_string())
        ],
        "the '#' of a local xlink:href is stripped"
    );
    let geometries = read.geometries("georeferencja");
    xeibe_testkit::wkt::assert_wkt_tol(
        geometries[0].as_ref().expect("a geometry"),
        "POINT (222442.2 431274.25)",
        tol(),
    );
}

#[test]
fn the_ngi_line_is_read_in_three_dimensions() {
    // GDAL 3.13 ignores srsDimension="3" here and scrambles the ordinates
    // (see the BOM); the positions are 3D.
    let sample = sample("be-ngi-electricity-network-3d");
    let read = read_layer(&sample, "UtilityLink", &options(AxisOrderMode::Auto));
    let geometry = first_geometry(&read).expect("a geometry");
    xeibe_testkit::wkt::assert_wkt_tol(
        &geometry,
        concat!(
            "LINESTRING Z (701548.2374999970 711198.8764999993 50.8436999999999,",
            "701594.7454999983 711470.2360000014 51.0999999999999)"
        ),
        Tol { abs: 1e-6, rel: 0.0 },
    );
}

#[test]
fn the_geneva_arcs_keep_the_stored_positions() {
    // ArcByCenterPoint segments are computed, but where they meet a stored
    // position the stored one wins.
    let sample = sample("ch-geneva-arcbycenterpoint-fme");
    let read = read_layer(&sample, "AGR_SPB", &options(AxisOrderMode::Auto));
    assert_eq!(read.rows(), 3);
    let geometries = read.geometries("SHAPE");
    let third = geometries[2].as_ref().expect("a geometry");
    let vertices = third.vertices();
    assert!(
        vertices
            .iter()
            .any(|v| v[0] == 2491721.9602000006 && v[1] == 1115430.192400001),
        "the position stored after the arc is kept exactly"
    );
}

#[test]
fn the_alkis_sample_keeps_its_curves() {
    let sample = sample("de-hamburg-alkis-nas-arcs");
    let read = read_layer(&sample, "AX_Gelaendekante", &options(AxisOrderMode::Auto));
    let geometry = first_geometry(&read).expect("a geometry");
    assert_eq!(
        geometry.canonical().tag(),
        "CIRCULARSTRING",
        "a curve stays a curve: {geometry}"
    );
}

#[test]
fn a_scan_of_a_sample_matches_a_read_of_it() {
    let sample = sample("de-sachsen-anhalt-inspire-su");
    let scan = scan(
        file_sources(&sample.file),
        ScanExtent::Full,
        &ReadOptions::default(),
    )
    .expect("the sample scans");
    let layers = scan.layers();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].feature_count, 2);

    let settings = scan.to_settings().expect("settings");
    assert_eq!(settings.layers.len(), 1);

    let layer = layers[0].name.local.to_string();
    let read = read_layer(&sample, &layer, &options(AxisOrderMode::Auto));
    assert_eq!(read.rows(), 2);
}

#[test]
fn reading_a_zip_member_works_like_reading_a_file() {
    // The samples are not zipped, so build one from a sample.
    let sample = sample("pl-prg-address-points");
    let dir = crate::support::temp_dir("zip-read");
    let path = dir.join("prg.zip");
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.start_file("prg.gml", options).unwrap();
        std::io::Write::write_all(&mut zip, &sample.bytes()).unwrap();
        zip.finish().unwrap();
    }
    let source = xeibe_core::Source::file(format!("{}!/prg.gml", path.display()))
        .expect("a zip member source");
    let reader = read(
        xeibe_core::Sources::from(source),
        "AD_PunktAdresowy",
        None,
        &options(AxisOrderMode::Auto),
    )
    .expect("a reader");
    assert_eq!(collect(reader).rows(), 2);
}
