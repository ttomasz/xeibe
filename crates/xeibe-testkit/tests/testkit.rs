//! Tests for the test support itself. These must pass at all times: they check
//! the oracles the other crates' tests rely on, against the files in
//! `tests/data`.

use xeibe_testkit::gdal;
use xeibe_testkit::samples::{sample, samples};
use xeibe_testkit::wkb;
use xeibe_testkit::wkt::{G, Tol, assert_wkt, diff};

// ------------------------------------------------------------------- WKT

#[test]
fn wkt_round_trips_every_type() {
    for text in [
        "POINT (1 2)",
        "POINT Z (1 2 3)",
        "POINT EMPTY",
        "LINESTRING (1 2,3 4)",
        "LINESTRING EMPTY",
        "CIRCULARSTRING (0 0,1 1,2 0)",
        "COMPOUNDCURVE ((0 0,1 0),CIRCULARSTRING (1 0,2 1,3 0))",
        "POLYGON ((0 0,1 0,1 1,0 0))",
        "POLYGON ((0 0,3 0,3 3,0 0),(1 1,2 1,2 2,1 1))",
        "CURVEPOLYGON (CIRCULARSTRING (0 0,1 1,2 0,1 -1,0 0))",
        "MULTIPOINT ((1 2),(3 4))",
        "MULTILINESTRING ((0 0,1 1),(2 2,3 3))",
        "MULTIPOLYGON (((0 0,1 0,1 1,0 0)))",
        "MULTICURVE (CIRCULARSTRING (0 0,1 1,2 0))",
        "MULTISURFACE (CURVEPOLYGON (CIRCULARSTRING (0 0,1 1,2 0,1 -1,0 0)))",
        "GEOMETRYCOLLECTION (POINT (0 1),LINESTRING (2 3,4 5))",
    ] {
        let geometry = G::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(geometry.to_string(), text, "round trip of {text}");
    }
}

#[test]
fn wkt_accepts_gdal_spellings() {
    // Bare coordinates in a MULTIPOINT, and the trailing `.0` GDAL sometimes prints.
    assert_eq!(
        G::parse("MULTIPOINT (1 2,3 4)").unwrap(),
        G::parse("MULTIPOINT ((1 2),(3 4))").unwrap()
    );
    assert_eq!(
        G::parse("CIRCULARSTRING (1.0 4.0,-1 2.0,1.0 0.0)").unwrap(),
        G::parse("CIRCULARSTRING (1 4,-1 2,1 0)").unwrap()
    );
    // A type we don't model keeps only its tag.
    assert_eq!(
        G::parse("TIN Z (((0 0 0,0 0 1,0 1 0,0 0 0)))").unwrap(),
        G::Other("TIN".into())
    );
    // TRIANGLE is a ring, so it reads as a polygon.
    assert_eq!(
        G::parse("TRIANGLE ((0 0,0 1,1 1,0 0))").unwrap(),
        G::parse("POLYGON ((0 0,0 1,1 1,0 0))").unwrap()
    );
}

#[test]
fn wkt_canonical_matches_our_type_choices() {
    let cases = [
        // A compound curve with one part is that part.
        ("COMPOUNDCURVE (CIRCULARSTRING (0 0,1 1,2 0))", "CIRCULARSTRING (0 0,1 1,2 0)"),
        ("COMPOUNDCURVE ((0 0,1 1,2 0))", "LINESTRING (0 0,1 1,2 0)"),
        // Adjacent parts with the same interpolation merge, sharing one point.
        (
            "COMPOUNDCURVE ((0 0,1 0),(1 0,2 0),CIRCULARSTRING (2 0,3 1,4 0))",
            "COMPOUNDCURVE ((0 0,1 0,2 0),CIRCULARSTRING (2 0,3 1,4 0))",
        ),
        (
            "COMPOUNDCURVE (CIRCULARSTRING (0 0,1 1,2 0),CIRCULARSTRING (2 0,3 1,4 0))",
            "CIRCULARSTRING (0 0,1 1,2 0,3 1,4 0)",
        ),
        // Curve-free curve types become simple-feature types.
        ("CURVEPOLYGON ((0 0,1 0,1 1,0 0))", "POLYGON ((0 0,1 0,1 1,0 0))"),
        ("MULTICURVE ((0 0,1 1))", "MULTILINESTRING ((0 0,1 1))"),
        ("MULTISURFACE (((0 0,1 0,1 1,0 0)))", "MULTIPOLYGON (((0 0,1 0,1 1,0 0)))"),
        // …but not when a curve is present.
        (
            "MULTISURFACE (CURVEPOLYGON (CIRCULARSTRING (0 0,1 1,2 0,1 -1,0 0)))",
            "MULTISURFACE (CURVEPOLYGON (CIRCULARSTRING (0 0,1 1,2 0,1 -1,0 0)))",
        ),
    ];
    for (input, expected) in cases {
        let actual = G::parse(input).unwrap().canonical();
        assert_eq!(actual.to_string(), expected, "canonical form of {input}");
    }
}

#[test]
fn wkt_diff_reports_differences() {
    let a = G::parse("POINT (1 2)").unwrap();
    assert!(diff(&a, &G::parse("LINESTRING (1 2,3 4)").unwrap(), Tol::default()).is_some());
    assert!(diff(&a, &G::parse("POINT (1 3)").unwrap(), Tol::default()).is_some());
    assert!(diff(&a, &G::parse("POINT Z (1 2 0)").unwrap(), Tol::default()).is_some());
    assert!(diff(&a, &G::parse("POINT (1 2)").unwrap(), Tol::default()).is_none());
    // Tolerances.
    let b = G::parse("POINT (1.0000000001 2)").unwrap();
    assert!(diff(&a, &b, Tol::default()).is_none());
    assert!(diff(&a, &b, Tol::exact()).is_some());
    assert!(
        diff(
            &G::parse("LINESTRING (0 0,1 1)").unwrap(),
            &G::parse("LINESTRING (0 0,1 1,2 2)").unwrap(),
            Tol::default()
        )
        .is_some()
    );
}

#[test]
fn wkt_helpers() {
    let geometry = G::parse("LINESTRING Z (1 2 3,4 5 6)").unwrap();
    assert!(geometry.has_z());
    assert_eq!(geometry.first_vertex().unwrap(), vec![1.0, 2.0, 3.0]);
    assert_eq!(geometry.vertices().len(), 2);
    // Only the first two ordinates are swapped.
    assert_wkt(&geometry.swapped_xy(), "LINESTRING Z (2 1 3,5 4 6)");
}

// ------------------------------------------------------------------- WKB

#[test]
fn wkb_decodes_little_and_big_endian() {
    let mut little = vec![1u8];
    little.extend(1u32.to_le_bytes());
    little.extend(1.0f64.to_le_bytes());
    little.extend(2.0f64.to_le_bytes());
    assert_wkt(&wkb::decode(&little).unwrap(), "POINT (1 2)");
    assert_eq!(wkb::type_code(&little).unwrap(), 1);

    let mut big = vec![0u8];
    big.extend(1001u32.to_be_bytes());
    big.extend(1.0f64.to_be_bytes());
    big.extend(2.0f64.to_be_bytes());
    big.extend(3.0f64.to_be_bytes());
    assert_wkt(&wkb::decode(&big).unwrap(), "POINT Z (1 2 3)");
}

#[test]
fn wkb_decodes_curve_types() {
    // CIRCULARSTRING (0 0,1 1,2 0)
    let mut bytes = vec![1u8];
    bytes.extend(8u32.to_le_bytes());
    bytes.extend(3u32.to_le_bytes());
    for (x, y) in [(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)] {
        bytes.extend(f64::to_le_bytes(x));
        bytes.extend(f64::to_le_bytes(y));
    }
    assert_wkt(&wkb::decode(&bytes).unwrap(), "CIRCULARSTRING (0 0,1 1,2 0)");

    // The same, wrapped in a COMPOUNDCURVE.
    let mut compound = vec![1u8];
    compound.extend(9u32.to_le_bytes());
    compound.extend(1u32.to_le_bytes());
    compound.extend(&bytes);
    assert_wkt(
        &wkb::decode(&compound).unwrap(),
        "COMPOUNDCURVE (CIRCULARSTRING (0 0,1 1,2 0))",
    );
}

#[test]
fn wkb_rejects_ewkb_and_trailing_bytes() {
    let mut ewkb = vec![1u8];
    ewkb.extend(0x8000_0001u32.to_le_bytes());
    ewkb.extend(1.0f64.to_le_bytes());
    ewkb.extend(2.0f64.to_le_bytes());
    assert!(wkb::decode(&ewkb).unwrap_err().contains("ISO WKB"));

    let mut trailing = vec![1u8];
    trailing.extend(1u32.to_le_bytes());
    trailing.extend(1.0f64.to_le_bytes());
    trailing.extend(2.0f64.to_le_bytes());
    trailing.push(0);
    assert!(wkb::decode(&trailing).unwrap_err().contains("trailing"));
}

#[test]
fn wkb_reads_an_empty_point_as_nan() {
    let mut bytes = vec![1u8];
    bytes.extend(1u32.to_le_bytes());
    bytes.extend(f64::NAN.to_le_bytes());
    bytes.extend(f64::NAN.to_le_bytes());
    assert_eq!(wkb::decode(&bytes).unwrap(), G::Point(None));
}

// ------------------------------------------------------- samples and GDAL

#[test]
fn every_sample_file_exists_and_matches_the_manifest() {
    let samples = samples();
    assert_eq!(samples.len(), 17, "samples.json lists 17 samples");
    for s in &samples {
        assert!(s.path().exists(), "{} is missing", s.file);
        assert!(
            matches!(s.axis_order.as_str(), "x/y" | "y/x"),
            "{}: odd axis_order {:?}",
            s.name,
            s.axis_order
        );
        assert_eq!(s.axis_order, s.axis.source_order, "{}", s.name);
        assert_eq!(s.axis.expected_first_xy.len(), 2, "{}", s.name);
        // `expected_first_xy` is the written position, swapped for y/x files.
        let written = &s.axis.first_position_as_written;
        let expected = if s.is_yx() {
            vec![written[1], written[0]]
        } else {
            written.clone()
        };
        assert_eq!(expected, s.axis.expected_first_xy, "{}", s.name);
    }
}

#[test]
fn gdal_reference_output_parses_for_every_sample() {
    for s in samples() {
        let report = s.gdal_report();
        assert_eq!(
            report.layers.len(),
            s.gdal.layers,
            "{}: layer count from {}",
            s.name,
            s.gdal.file
        );
        let counts: Vec<u64> = report.layers.iter().map(|l| l.feature_count).collect();
        assert_eq!(counts, s.gdal.feature_counts, "{}: feature counts", s.name);
        for layer in &report.layers {
            assert_eq!(
                layer.features.len() as u64,
                layer.feature_count,
                "{}: features listed for layer {}",
                s.name,
                layer.name
            );
            for feature in &layer.features {
                if let Some(text) = &feature.wkt {
                    G::parse(text).unwrap_or_else(|e| {
                        panic!("{}: layer {}: {e} in {text}", s.name, layer.name)
                    });
                }
            }
        }
    }
}

#[test]
fn gdal_reference_output_keeps_fields_and_values() {
    let s = sample("pl-prg-address-points");
    let report = s.gdal_report();
    let layer = report.layer("AD_Miejscowosc").expect("layer");
    assert_eq!(layer.geometry_type, "Point");
    assert_eq!(layer.geometry_column.as_deref(), Some("georeferencja"));
    assert!(layer.fields.contains(&("lokalnyId".into(), "String".into())));
    assert!(layer.fields.contains(&("wersjaId".into(), "DateTime".into())));
    assert_eq!(layer.attr(0, "nazwa"), Some("Chyrzyno"));
    assert_eq!(
        layer.attr(0, "gml_id"),
        Some("PL.ZIPIN.515.EMUiA_0181728_2017-04-13T12_49_22_02_00")
    );
    assert_wkt(
        &G::parse(layer.features[0].wkt.as_ref().unwrap()).unwrap(),
        "POINT (205249.1 530976.79)",
    );

    // A layer without geometry still has its fields.
    let rcn = sample("pl-rcn-parcels-arcs").gdal_report();
    let transactions = rcn.layer("RCN_Transakcja").expect("layer");
    assert_eq!(transactions.geometry_type, "None");
    assert_eq!(transactions.geometry_column, None);
    assert!(transactions.fields.iter().any(|(n, _)| n == "oznaczenieTransakcji"));
    assert_eq!(transactions.features[0].wkt, None);
}

#[test]
fn gdal_first_position_agrees_with_the_manifest() {
    for s in samples() {
        let report = s.gdal_report();
        let Some(first) = report
            .layers
            .iter()
            .flat_map(|l| &l.features)
            .find_map(|f| f.wkt.as_ref())
        else {
            continue;
        };
        let Some(gdal_first) = &s.axis.gdal_first_xy else {
            continue;
        };
        let vertex = G::parse(first).unwrap().first_vertex().expect("a vertex");
        let tol = Tol { abs: 1e-6, rel: 0.0 };
        assert!(
            tol.matches(vertex[0], gdal_first[0]) && tol.matches(vertex[1], gdal_first[1]),
            "{}: GDAL output starts at {vertex:?}, manifest says {gdal_first:?}",
            s.name
        );
        // The manifest and GDAL disagree for exactly the samples flagged in it.
        let agrees = tol.matches(vertex[0], s.axis.expected_first_xy[0])
            && tol.matches(vertex[1], s.axis.expected_first_xy[1]);
        assert_eq!(agrees, s.axis.gdal_agrees, "{}: gdal_agrees", s.name);
    }
}

#[test]
fn gdal_geometry_cases_load() {
    let cases = gdal::geometry_cases();
    assert_eq!(cases.len(), 292);
    let mut parsed = 0;
    for case in &cases {
        assert!(!case.gml.is_empty(), "{}: empty snippet", case.id);
        if let Some(text) = &case.gdal.wkt {
            G::parse(text).unwrap_or_else(|e| panic!("{}: {e} in {text}", case.id));
            parsed += 1;
        } else {
            assert!(case.gdal.error.is_some(), "{}: neither WKT nor error", case.id);
        }
    }
    assert_eq!(parsed, 181, "GDAL parsed 181 of the 292 snippets");
}
