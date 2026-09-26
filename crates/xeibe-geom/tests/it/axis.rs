//! Axis-order decisions (`docs/geometry.md`, "CRS and axis order").
//!
//! A decision is made per key `(source, srsName as written, dialect)` and never
//! per feature. Output is always normalized to x/y, so `swap` means "the source
//! wrote y/x".

use xeibe_core::{Dialect, SourceId};
use xeibe_geom::axis::{AxisContext, AxisEvidence, decide, first_axis_from_labels};
use xeibe_geom::epsg::FirstAxis;
use xeibe_geom::{AxisKey, AxisOrderMode, AxisOrderOptions};

fn key(srs: &str, dialect: Dialect) -> AxisKey {
    AxisKey {
        source: SourceId(0),
        srs_name: Some(srs.to_string()),
        dialect,
    }
}

fn options(mode: AxisOrderMode) -> AxisOrderOptions {
    AxisOrderOptions {
        mode,
        ..AxisOrderOptions::default()
    }
}

#[track_caller]
fn swaps(mode: AxisOrderMode, srs: &str) -> bool {
    decide(
        &key(srs, Dialect::Gml3),
        None,
        None,
        &AxisEvidence::default(),
        &AxisContext::default(),
        &options(mode),
    )
    .swap
}

#[test]
fn auto_is_the_default_mode() {
    assert_eq!(AxisOrderMode::default(), AxisOrderMode::Auto);
    assert_eq!(AxisOrderOptions::default().mode, AxisOrderMode::Auto);
    assert!(AxisOrderOptions::default().overrides.is_empty());
}

#[test]
fn xy_and_yx_ignore_the_srs_name() {
    for srs in ["EPSG:4326", "urn:ogc:def:crs:EPSG::2180", "nonsense"] {
        assert!(!swaps(AxisOrderMode::XY, srs), "XY never swaps: {srs}");
        assert!(swaps(AxisOrderMode::YX, srs), "YX always swaps: {srs}");
    }
    // Even CRS84, which is lon/lat by definition, is swapped by `YX`.
    assert!(swaps(AxisOrderMode::YX, "urn:ogc:def:crs:OGC:1.3:CRS84"));
}

#[test]
fn crs_mode_follows_the_authority_order_for_every_form() {
    // Including the short form (OGC 09-048r7 §5.3.3).
    assert!(swaps(AxisOrderMode::Crs, "EPSG:4326"));
    assert!(swaps(AxisOrderMode::Crs, "EPSG:2180"));
    assert!(swaps(AxisOrderMode::Crs, "urn:ogc:def:crs:EPSG::4258"));
    assert!(!swaps(AxisOrderMode::Crs, "EPSG:25832"));
    assert!(!swaps(AxisOrderMode::Crs, "urn:ogc:def:crs:OGC:1.3:CRS84"));
}

#[test]
fn a_compound_crs_takes_the_axis_order_of_its_horizontal_part() {
    // NAD83 is latitude-first; the height doesn't matter.
    let nad83 = "urn:ogc:def:crs,crs:EPSG::4269,crs:EPSG::5713";
    assert!(swaps(AxisOrderMode::Crs, nad83));
    assert!(swaps(AxisOrderMode::CrsHeuristic, nad83));
    assert!(swaps(
        AxisOrderMode::CrsHeuristic,
        "http://www.opengis.net/def/crs-compound?1=http://www.opengis.net/def/crs/EPSG/0/4269\
         &2=http://www.opengis.net/def/crs/EPSG/0/5713"
    ));
    // The PROJ spelling is a short form: as written under `CrsHeuristic`.
    assert!(!swaps(AxisOrderMode::CrsHeuristic, "EPSG:4269+5713"));
    assert!(swaps(AxisOrderMode::Crs, "EPSG:4269+5713"));
    assert!(!swaps(AxisOrderMode::Crs, "urn:adv:crs:ETRS89_UTM32*DE_DHHN2016_NH"));
    // An unknown height doesn't matter either; an unknown horizontal part does.
    assert!(swaps(AxisOrderMode::Crs, "urn:ogc:def:crs,crs:EPSG::2180,crs:PL-XYZ"));
    assert!(!swaps(AxisOrderMode::Crs, "urn:ogc:def:crs,crs:PL-XYZ,crs:EPSG::9651"));
}

#[test]
fn an_unknown_crs_is_read_as_written_and_reported() {
    let decision = decide(
        &key("EPSG:98765", Dialect::Gml3),
        None,
        None,
        &AxisEvidence::default(),
        &AxisContext::default(),
        &options(AxisOrderMode::Crs),
    );
    assert!(!decision.swap, "x/y is assumed when the code is unknown");
    assert!(
        !decision.reason.is_empty(),
        "the decision says why, for --explain"
    );
}

#[test]
fn crs_heuristic_decides_by_the_srs_name_form() {
    // Short and legacy forms are read as written; URN and HTTP URI forms follow
    // the CRS. This is GDAL's default behaviour.
    assert!(!swaps(AxisOrderMode::CrsHeuristic, "EPSG:4326"));
    assert!(!swaps(
        AxisOrderMode::CrsHeuristic,
        "http://www.opengis.net/gml/srs/epsg.xml#4326"
    ));
    assert!(!swaps(AxisOrderMode::CrsHeuristic, "25833"));
    assert!(swaps(AxisOrderMode::CrsHeuristic, "urn:ogc:def:crs:EPSG::4326"));
    assert!(swaps(AxisOrderMode::CrsHeuristic, "urn:x-ogc:def:crs:EPSG:3301"));
    assert!(swaps(AxisOrderMode::CrsHeuristic, "urn:EPSG:geographicCRS:4326"));
    assert!(swaps(
        AxisOrderMode::CrsHeuristic,
        "http://www.opengis.net/def/crs/EPSG/0/4258"
    ));
    // …but only where the CRS is northing-first.
    assert!(!swaps(
        AxisOrderMode::CrsHeuristic,
        "http://www.opengis.net/def/crs/EPSG/0/3812"
    ));
    // AdV URNs are easting-first.
    assert!(!swaps(AxisOrderMode::CrsHeuristic, "urn:adv:crs:ETRS89_UTM32"));
    // An unknown srsName is read as written.
    assert!(!swaps(AxisOrderMode::CrsHeuristic, "AUT-GK31-5"));
}

#[test]
fn gml_version_mode_uses_the_dialect_of_the_geometry() {
    let mode = AxisOrderMode::GmlVersion {
        gml2: Box::new(AxisOrderMode::XY),
        gml3: Box::new(AxisOrderMode::Crs),
    };
    let decide_with = |dialect| {
        decide(
            &key("EPSG:4326", dialect),
            None,
            None,
            &AxisEvidence::default(),
            &AxisContext::default(),
            &options(mode.clone()),
        )
        .swap
    };
    assert!(!decide_with(Dialect::Gml2), "GML 2 geometry is x/y");
    assert!(decide_with(Dialect::Gml3), "GML 3 geometry follows the CRS");
}

#[test]
fn overrides_are_keyed_by_the_srs_name_as_written() {
    // The only selector is the srsName: a read has one layer, and a
    // dialect-dependent rule is the `GmlVersion` mode (`docs/geometry.md`,
    // "Decision key and scope").
    let options: AxisOrderOptions =
        serde_json::from_str(r#"{"mode":"XY","overrides":{"EPSG:4326":"YX"}}"#)
            .expect("mode with an srsName override");
    let swap_for = |srs: &str| {
        decide(
            &key(srs, Dialect::Gml3),
            None,
            None,
            &AxisEvidence::default(),
            &AxisContext::default(),
            &options,
        )
        .swap
    };
    assert!(swap_for("EPSG:4326"), "the override for this srsName");
    assert!(!swap_for("EPSG:2180"), "no override: the mode");
    assert!(
        !swap_for("urn:ogc:def:crs:EPSG::4326"),
        "matched on the srsName exactly as written, not on the CRS"
    );
}

#[test]
fn auto_takes_axis_labels_as_decisive() {
    let evidence = AxisEvidence {
        axis_labels: vec!["Lat Long".into()],
        samples: 10,
        ..AxisEvidence::default()
    };
    let decision = decide(
        &key("EPSG:4326", Dialect::Gml3),
        None,
        None,
        &evidence,
        &AxisContext::default(),
        &options(AxisOrderMode::Auto),
    );
    assert!(decision.swap, "axisLabels say latitude comes first");

    let evidence = AxisEvidence {
        axis_labels: vec!["Long Lat".into()],
        samples: 10,
        ..AxisEvidence::default()
    };
    assert!(
        !decide(
            &key("urn:ogc:def:crs:EPSG::4326", Dialect::Gml3),
            None,
            None,
            &evidence,
            &AxisContext::default(),
            &options(AxisOrderMode::Auto),
        )
        .swap,
        "axisLabels beat the srsName form"
    );
}

#[test]
fn auto_uses_the_range_check_when_it_is_decisive() {
    // The BfN case: WFS 1.0 with a short form, written lat/lon. Latitude 6.5
    // is outside EPSG:4258's area of use, so the reading as written is wrong.
    let evidence = AxisEvidence {
        sampled_bbox: Some([49.33, 6.54, 49.40, 6.60]),
        samples: 50,
        ..AxisEvidence::default()
    };
    let decision = decide(
        &key("EPSG:4258", Dialect::Gml3),
        None,
        None,
        &evidence,
        &AxisContext::default(),
        &options(AxisOrderMode::Auto),
    );
    assert!(decision.swap, "the range check rejects the written order");
    assert!(
        !decision.conflicts.is_empty(),
        "the srsName heuristic disagreed, which must be reported"
    );
}

#[test]
fn auto_falls_back_when_the_range_check_cannot_decide() {
    // EPSG:2180 eastings and northings overlap, so both readings fall inside
    // Poland; the fallback (CrsHeuristic) decides.
    let evidence = AxisEvidence {
        sampled_bbox: Some([205249.1, 530976.79, 262968.3, 530543.2]),
        samples: 50,
        ..AxisEvidence::default()
    };
    let decision = decide(
        &key("EPSG:2180", Dialect::Gml3),
        None,
        None,
        &evidence,
        &AxisContext::default(),
        &options(AxisOrderMode::Auto),
    );
    assert!(!decision.swap, "the short form is read as written");
}

#[test]
fn auto_knows_the_fme_quirk() {
    // FME writes the short form in authority order ([GDAL]).
    let context = AxisContext {
        fme_produced: true,
        ..AxisContext::default()
    };
    assert!(
        decide(
            &key("EPSG:4326", Dialect::Gml3),
            None,
            None,
            &AxisEvidence::default(),
            &context,
            &options(AxisOrderMode::Auto),
        )
        .swap,
        "FME + short form + latitude-first CRS"
    );
    // The Geneva sample: FME, EPSG:2056, which is easting-first, so no swap.
    assert!(
        !decide(
            &key("EPSG:2056", Dialect::Gml3),
            None,
            None,
            &AxisEvidence::default(),
            &context,
            &options(AxisOrderMode::Auto),
        )
        .swap
    );
}

#[test]
fn auto_reads_a_gml_2_geometry_as_xy_whatever_the_srs_name_says() {
    // GML 2 predates the authority-order policy (02-069 has no axis-order
    // concept), so a GML 2-dialect geometry is x/y even under a URN. This is the
    // Sandre case, and the only strong evidence a plain file offers.
    assert!(
        !decide(
            &key("urn:ogc:def:crs:EPSG::4326", Dialect::Gml2),
            None,
            None,
            &AxisEvidence::default(),
            &AxisContext::default(),
            &options(AxisOrderMode::Auto),
        )
        .swap,
        "a GML 2 geometry is x/y"
    );

    // The same srsName in a GML 3 geometry keeps the authority order.
    assert!(
        decide(
            &key("urn:ogc:def:crs:EPSG::4326", Dialect::Gml3),
            None,
            None,
            &AxisEvidence::default(),
            &AxisContext::default(),
            &options(AxisOrderMode::Auto),
        )
        .swap
    );

    // Decisive evidence still wins over it. `axisLabels` cannot occur in GML 2,
    // but a range check can: a second ordinate above 90 is not a latitude, so
    // only the swapped reading is possible.
    let out_of_range = AxisEvidence {
        sampled_bbox: Some([10.0, 92.0, 12.0, 95.0]),
        samples: 20,
        ..AxisEvidence::default()
    };
    assert!(
        decide(
            &key("urn:ogc:def:crs:EPSG::4326", Dialect::Gml2),
            None,
            None,
            &out_of_range,
            &AxisContext::default(),
            &options(AxisOrderMode::Auto),
        )
        .swap,
        "the range check is decisive and outranks the dialect"
    );
}

#[test]
fn auto_uses_the_wfs_version_we_requested() {
    // WFS 1.0 with a URN: lon/lat in practice (the Sandre case).
    let wfs10 = AxisContext {
        wfs_version: Some("1.0.0".into()),
        ..AxisContext::default()
    };
    assert!(
        !decide(
            &key("urn:ogc:def:crs:EPSG::4326", Dialect::Gml3),
            None,
            None,
            &AxisEvidence::default(),
            &wfs10,
            &options(AxisOrderMode::Auto),
        )
        .swap,
        "WFS 1.0 is x/y even with a URN"
    );

    // WFS 2.0 follows the authority order.
    let wfs20 = AxisContext {
        wfs_version: Some("2.0.0".into()),
        ..AxisContext::default()
    };
    assert!(
        decide(
            &key("EPSG:2180", Dialect::Gml3),
            None,
            None,
            &AxisEvidence::default(),
            &wfs20,
            &options(AxisOrderMode::Auto),
        )
        .swap
    );
}

#[test]
fn auto_has_no_evidence_switches() {
    // `Auto` always uses all of the evidence and falls back to `CrsHeuristic`;
    // to decide differently, use a fixed mode. An `auto` key in the settings
    // file is not part of the format.
    let with_switches = serde_json::from_str::<AxisOrderOptions>(
        r#"{"mode":"Auto","auto":{"use_axis_labels":false}}"#,
    );
    assert!(with_switches.is_err(), "unknown key accepted: {with_switches:?}");
}

#[test]
fn axis_labels_are_mapped_to_a_first_axis() {
    for labels in ["Lat Long", "lat lon", "N E", "Northing Easting", "y x"] {
        assert_eq!(
            first_axis_from_labels(labels),
            Some(FirstAxis::NorthOrLat),
            "{labels}"
        );
    }
    for labels in ["Long Lat", "lon lat", "E N", "Easting Northing", "x y"] {
        assert_eq!(
            first_axis_from_labels(labels),
            Some(FirstAxis::EastOrLon),
            "{labels}"
        );
    }
    assert_eq!(first_axis_from_labels("foo bar"), None);
    assert_eq!(first_axis_from_labels(""), None);
}

#[test]
fn evidence_merges_associatively() {
    let mut first = AxisEvidence {
        sampled_bbox: Some([0.0, 0.0, 10.0, 10.0]),
        samples: 3,
        axis_labels: vec!["Lat Long".into()],
        envelope_bbox: None,
    };
    first.merge(AxisEvidence {
        sampled_bbox: Some([-5.0, 2.0, 4.0, 20.0]),
        samples: 2,
        axis_labels: vec!["Lat Long".into(), "y x".into()],
        envelope_bbox: Some([0.0, 0.0, 1.0, 1.0]),
    });
    assert_eq!(first.samples, 5);
    assert_eq!(first.sampled_bbox, Some([-5.0, 0.0, 10.0, 20.0]));
    assert_eq!(first.axis_labels.len(), 2, "labels are a set: {:?}", first.axis_labels);
    assert_eq!(first.envelope_bbox, Some([0.0, 0.0, 1.0, 1.0]));
}

#[test]
fn a_bare_mode_is_enough_in_the_settings_file() {
    // `"axis": "YX"`, with overrides only for the rare mixed inputs.
    let options: AxisOrderOptions = serde_json::from_str("\"YX\"").expect("a bare mode");
    assert_eq!(options.mode, AxisOrderMode::YX);
    assert!(options.overrides.is_empty());
    assert_eq!(serde_json::to_string(&options).unwrap(), "\"YX\"");

    let nested: AxisOrderOptions =
        serde_json::from_str(r#"{"GmlVersion":{"gml2":"XY","gml3":"Crs"}}"#).expect("a mode");
    assert!(matches!(nested.mode, AxisOrderMode::GmlVersion { .. }));

    let full: AxisOrderOptions =
        serde_json::from_str(r#"{"mode":"Auto","overrides":{"EPSG:4326":"YX"}}"#)
            .expect("mode with overrides");
    assert_eq!(full.mode, AxisOrderMode::Auto);
    assert_eq!(full.overrides.len(), 1);
    assert_eq!(
        serde_json::to_value(&full).unwrap(),
        serde_json::json!({"mode": "Auto", "overrides": {"EPSG:4326": "YX"}}),
        "written back in the same form"
    );
}

#[test]
fn auto_reports_envelopes_and_request_boxes_in_the_other_order() {
    // Supporting evidence only: never decisive, always reported
    // (`docs/geometry.md`, "`Auto`: evidence-based decision").
    let sampled = [500_000.0, 300_000.0, 500_010.0, 300_010.0];
    let swapped = [300_000.0, 500_000.0, 300_010.0, 500_010.0];
    let evidence = AxisEvidence {
        sampled_bbox: Some(sampled),
        samples: 2,
        axis_labels: Vec::new(),
        envelope_bbox: Some(swapped),
    };
    let context = AxisContext {
        requested_bbox: Some(swapped),
        ..AxisContext::default()
    };
    let decision = decide(
        &key("EPSG:2180", Dialect::Gml3),
        None,
        None,
        &evidence,
        &context,
        &options(AxisOrderMode::XY),
    );
    assert!(!decision.swap, "a fixed mode decides");
    assert!(decision.conflicts.is_empty(), "only `Auto` weighs evidence: {:?}", decision.conflicts);

    let decision = decide(
        &key("EPSG:2180", Dialect::Gml3),
        None,
        None,
        &evidence,
        &context,
        &options(AxisOrderMode::Auto),
    );
    let conflicts = decision.conflicts.join("; ");
    assert!(conflicts.contains("boundedBy envelope"), "{conflicts}");
    assert!(conflicts.contains("requested BBOX"), "{conflicts}");
}
