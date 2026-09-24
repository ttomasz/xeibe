//! `scan` and `read`, the two user-facing operations
//! (`docs/architecture.md`, "User-facing API").

use arrow_array::RecordBatchReader;
use arrow_schema::{DataType, Field, TimeUnit};
use xeibe_arrow::{OnFeatureError, ReadOptions, read, scan};
use xeibe_schema::{SampleOptions, ScanExtent};
use xeibe_testkit::gml;
use xeibe_testkit::wkt::assert_wkt;

use crate::support::{extension_name, read_document, read_with, scan_document, schema_of, sources};

fn parcel(id: &str, body: &str) -> String {
    gml::feature("Parcel", id, body)
}

fn document() -> String {
    gml::gml32_collection(&[
        &parcel(
            "p1",
            concat!(
                "<app:area>12.5</app:area><app:code>0012</app:code>",
                "<app:geom><gml:Point srsName=\"EPSG:2180\"><gml:pos>1 2</gml:pos></gml:Point></app:geom>"
            ),
        ),
        &gml::feature("Road", "r1", "<app:name>A</app:name>"),
        &parcel(
            "p2",
            concat!(
                "<app:area>3</app:area><app:code>0034</app:code>",
                "<app:geom><gml:Point srsName=\"EPSG:2180\"><gml:pos>3 4</gml:pos></gml:Point></app:geom>"
            ),
        ),
    ])
}

#[test]
fn a_scan_lists_every_layer_with_its_summary() {
    let scan = scan_document(&document());
    assert!(scan.is_complete(), "a full scan lists every layer");

    let layers = scan.layers();
    let names: Vec<String> = layers.iter().map(|l| l.name.local.to_string()).collect();
    assert_eq!(names, ["Parcel", "Road"]);

    let parcels = &layers[0];
    assert_eq!(parcels.feature_count, 2);
    assert_eq!(parcels.geometry_columns, ["geom"]);
    assert_eq!(parcels.crs, ["EPSG:2180"]);
    assert_eq!(parcels.extent, Some([1.0, 2.0, 3.0, 4.0]));

    let roads = &layers[1];
    assert_eq!(roads.feature_count, 1);
    assert!(roads.geometry_columns.is_empty());
    assert_eq!(roads.extent, None);
}

#[test]
fn a_sampled_scan_says_it_may_be_incomplete() {
    let scan = scan(
        sources(&document()),
        ScanExtent::Sample { max_features: 1 },
        &ReadOptions::default(),
    )
    .expect("a sampled scan");
    assert!(!scan.is_complete());
    assert_eq!(scan.layers().len(), 1, "only the first feature was read");
}

#[test]
fn a_scan_produces_an_arrow_schema_per_layer() {
    let scan = scan_document(&document());
    let schema = scan.arrow_schema("Parcel").expect("a schema");
    let names: Vec<&String> = schema.fields().iter().map(|f| f.name()).collect();
    assert_eq!(names, ["@id", "area", "code", "geom"]);
    assert_eq!(schema.field(1).data_type(), &DataType::Float64);
    assert_eq!(
        schema.field(2).data_type(),
        &DataType::Utf8View,
        "leading zeros stay text"
    );
    assert!(scan.arrow_schema("Nope").is_err());
}

#[test]
fn a_scan_can_produce_schemas_for_other_options_without_another_pass() {
    let scan = scan_document(&document());
    let strings = xeibe_schema::InferenceOptions::strings();
    let schema = scan.schema_with("Parcel", &strings).expect("a schema");
    assert_eq!(schema.schema.field(1).data_type(), &DataType::Utf8View);
}

#[test]
fn explain_names_the_reason_for_each_column() {
    let scan = scan_document(&document());
    let explanation = scan.explain("Parcel").expect("an explanation");
    assert!(explanation.contains("code"), "{explanation}");
    assert!(explanation.contains("geom"), "{explanation}");
}

#[test]
fn a_read_returns_one_layer_as_record_batches() {
    let read = read_document(&document(), "Parcel");
    assert_eq!(read.rows(), 2);
    assert_eq!(read.column_names(), ["@id", "area", "code", "geom"]);
    assert_eq!(read.f64s("area"), [Some(12.5), Some(3.0)]);
    assert_eq!(
        read.strings("code"),
        [Some("0012".to_string()), Some("0034".to_string())]
    );
    assert_eq!(
        read.strings("@id"),
        [Some("p1".to_string()), Some("p2".to_string())]
    );
    assert_eq!(read.report.features_per_layer.get("Parcel"), Some(&2));
}

#[test]
fn a_read_without_a_schema_infers_one_from_the_layers_first_features() {
    // The sample is taken from the requested layer, not from the start of the
    // file (`docs/schema-inference.md` §6.1).
    let read = read_document(&document(), "Road");
    assert_eq!(read.rows(), 1);
    assert_eq!(read.strings("name"), [Some("A".to_string())]);
    assert!(
        read.report.inferred.is_some(),
        "the inferred schema is in the report, ready to save"
    );
}

#[test]
fn a_given_schema_is_used_as_it_is() {
    let schema = schema_of(vec![
        Field::new("@id", DataType::Utf8View, true),
        Field::new("area", DataType::Float64, true),
    ]);
    let read = read_with(&document(), "Parcel", Some(schema.clone()), &ReadOptions::default());
    assert_eq!(
        read.column_names(),
        ["@id", "area"],
        "the schema is the projection: nothing is appended"
    );
    assert_eq!(read.f64s("area"), [Some(12.5), Some(3.0)]);
}

#[test]
fn nested_elements_are_read_into_flat_columns() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        concat!(
            "<app:idIIP><app:AD_IdentyfikatorIIP>",
            "<app:lokalnyId>a339e481</app:lokalnyId><app:przestrzenNazw>PL.PZGIK.200</app:przestrzenNazw>",
            "</app:AD_IdentyfikatorIIP></app:idIIP>",
            r##"<app:miejscowosc xlink:href="#PL.X.1"/>"##
        ),
    )]);
    let read = read_document(&document, "Parcel");
    assert_eq!(
        read.column_names(),
        ["@id", "lokalnyId", "przestrzenNazw", "miejscowosc"]
    );
    assert_eq!(read.strings("lokalnyId"), [Some("a339e481".to_string())]);
    assert_eq!(
        read.strings("miejscowosc"),
        [Some("PL.X.1".to_string())],
        "the href, '#' stripped"
    );
}

#[test]
fn a_scan_writes_geometry_options_under_options_geometry() {
    // Axis order, CRS and curves are read parameters, not inference.
    let settings = scan_document(&document()).to_settings().expect("settings");
    let json = serde_json::to_value(&settings).expect("JSON");
    assert!(json["options"]["geometry"]["axis"].is_string(), "{json}");
    assert!(
        json["options"]["inference"].get("geometry").is_none(),
        "{}",
        json["options"]["inference"]
    );
}

#[test]
fn the_reader_reports_its_schema_before_the_first_batch() {
    let reader = read(
        sources(&document()),
        "Parcel",
        None,
        &ReadOptions::default(),
    )
    .expect("a reader");
    let schema = reader.schema();
    assert_eq!(schema.fields().len(), 4);
}

#[test]
fn reading_an_unknown_layer_is_an_error() {
    assert!(read(sources(&document()), "Nope", None, &ReadOptions::default()).is_err());
}

#[test]
fn geometry_columns_carry_their_crs_and_encoding() {
    let read = read_document(&document(), "Parcel");
    let geometry = read.field("geom");
    assert_eq!(extension_name(&geometry), Some("geoarrow.wkb"));
    let geometries = read.geometries("geom");
    assert_wkt(geometries[0].as_ref().expect("a geometry"), "POINT (1 2)");
    assert_wkt(geometries[1].as_ref().expect("a geometry"), "POINT (3 4)");
}

#[test]
fn several_sources_share_one_schema() {
    let first = gml::gml32_collection(&[&parcel("p1", "<app:area>1</app:area>")]);
    let second = gml::gml32_collection(&[&parcel("p2", "<app:area>2</app:area>")]);
    let sources = xeibe_core::Sources::from(vec![
        xeibe_core::Source::reader("a.gml", Box::new(std::io::Cursor::new(first.into_bytes()))),
        xeibe_core::Source::reader("b.gml", Box::new(std::io::Cursor::new(second.into_bytes()))),
    ]);
    let reader = read(sources, "Parcel", None, &ReadOptions::default()).expect("a reader");
    let read = crate::support::collect(reader);
    assert_eq!(read.rows(), 2);
    assert_eq!(read.i64s("area"), [Some(1), Some(2)]);
}

#[test]
fn batch_size_bounds_the_batches() {
    let features: Vec<String> = (0..10)
        .map(|i| parcel(&format!("p{i}"), &format!("<app:area>{i}</app:area>")))
        .collect();
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let options = ReadOptions {
        batch_size: 4,
        ..ReadOptions::default()
    };
    let read = read_with(&gml::gml32_collection(&refs), "Parcel", None, &options);
    assert_eq!(read.rows(), 10);
    assert!(
        read.batches.iter().all(|batch| batch.num_rows() <= 4),
        "batch sizes: {:?}",
        read.batches.iter().map(|b| b.num_rows()).collect::<Vec<_>>()
    );
}

#[test]
fn rows_keep_their_source_order_by_default() {
    let features: Vec<String> = (0..20)
        .map(|i| parcel(&format!("p{i}"), &format!("<app:area>{i}</app:area>")))
        .collect();
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let options = ReadOptions {
        threads: 4,
        batch_size: 3,
        splitter: xeibe_core::SplitterOptions {
            target_chunk_bytes: 1,
            ..xeibe_core::SplitterOptions::default()
        },
        ..ReadOptions::default()
    };
    let read = read_with(&gml::gml32_collection(&refs), "Parcel", None, &options);
    let areas: Vec<i64> = read.i64s("area").into_iter().flatten().collect();
    assert_eq!(areas, (0..20).collect::<Vec<_>>());
    assert!(ReadOptions::default().preserve_order);
}

#[test]
fn a_projection_builds_only_the_columns_asked_for() {
    let options = ReadOptions {
        projection: Some(vec!["area".into()]),
        ..ReadOptions::default()
    };
    let read = read_with(&document(), "Parcel", None, &options);
    assert_eq!(read.column_names(), ["area"]);
}

#[test]
fn a_feature_error_can_be_skipped_instead_of_stopping_the_read() {
    // A coordinate that is not a number is a feature error.
    let broken = gml::gml32_collection(&[
        &parcel(
            "p1",
            "<app:geom><gml:Point><gml:pos>1 2</gml:pos></gml:Point></app:geom>",
        ),
        &parcel(
            "p2",
            "<app:geom><gml:Point><gml:pos>x y</gml:pos></gml:Point></app:geom>",
        ),
    ]);
    assert_eq!(OnFeatureError::default(), OnFeatureError::Error);
    let strict = read(sources(&broken), "Parcel", None, &ReadOptions::default());
    let strict_failed = match strict {
        Err(_) => true,
        Ok(reader) => reader.into_iter().any(|batch| batch.is_err()),
    };
    assert!(strict_failed, "the default policy stops the read");

    let skipping = ReadOptions {
        on_feature_error: OnFeatureError::Skip,
        ..ReadOptions::default()
    };
    let read = read_with(&broken, "Parcel", None, &skipping);
    assert_eq!(read.rows(), 1, "the broken feature is skipped");
    assert_eq!(read.report.skipped.len(), 1);

    let null_geometry = ReadOptions {
        on_feature_error: OnFeatureError::NullGeometry,
        ..ReadOptions::default()
    };
    let read = read_with(&broken, "Parcel", None, &null_geometry);
    assert_eq!(read.rows(), 2, "the attributes are kept");
    assert_eq!(read.geometries("geom")[1], None);
}

#[test]
fn the_sample_size_can_be_changed() {
    let options = ReadOptions {
        sample: SampleOptions {
            features_per_layer: 1,
            min_typed_values: 1,
            ..SampleOptions::default()
        },
        ..ReadOptions::default()
    };
    let document = gml::gml32_collection(&[
        &parcel("p1", "<app:n>1</app:n>"),
        &parcel("p2", "<app:n>abc</app:n>"),
    ]);
    // Only the first feature was sampled, so the column is typed as an
    // integer, and the second value doesn't fit it: a feature error.
    let skipping = ReadOptions {
        on_feature_error: OnFeatureError::Skip,
        ..options
    };
    let read = read_with(&document, "Parcel", None, &skipping);
    assert_eq!(read.data_type("n"), DataType::Int64);
    assert_eq!(read.i64s("n"), [Some(1)]);
    assert_eq!(read.report.skipped.len(), 1);
}

#[test]
fn the_defaults_match_the_documented_ones() {
    let options = ReadOptions::default();
    assert_eq!(options.on_feature_error, OnFeatureError::Error);
    assert_eq!(options.geometry.axis.mode, xeibe_geom::AxisOrderMode::Auto);
    assert!(options.geometry.crs_override.is_none());
    assert!(options.geometry.primary.is_none());
    assert!(options.batch_size > 0);
    assert!(options.threads > 0);
    assert!(options.queue_depth > 0);
    assert!(options.projection.is_none());
}

#[test]
fn a_timestamp_column_holds_the_instant_in_microseconds() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        "<app:t>1970-01-01T00:00:01+00:00</app:t>",
    )]);
    let read = read_document(&document, "Parcel");
    assert_eq!(
        read.data_type("t"),
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
    );
    assert_eq!(read.timestamps("t"), [Some(1_000_000)]);
}
