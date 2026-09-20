//! Scanning and schema inference against the real samples in `tests/data`.

use arrow_schema::{DataType, TimeUnit};
use xeibe_schema::{InferenceOptions, ScanOptions, Scanner};
use xeibe_testkit::samples::{sample, samples};

use crate::support::{column_names, data_type, extension_name, field, nested, scan_file};

#[test]
fn every_sample_scans_into_the_layers_gdal_found() {
    for sample in samples() {
        let observation = scan_file(&sample.file);
        let mut found: Vec<String> = observation
            .layers
            .keys()
            .map(|name| name.local.to_string())
            .collect();
        found.sort();
        let mut expected: Vec<String> = sample
            .gdal_report()
            .layers
            .iter()
            .map(|layer| layer.name.clone())
            .collect();
        expected.sort();
        assert_eq!(found, expected, "{}: layers", sample.name);

        let total: u64 = observation
            .layers
            .values()
            .map(|layer| layer.feature_count)
            .sum();
        assert_eq!(total, sample.total_features(), "{}: features", sample.name);
        assert!(!observation.sampled, "{}: a full scan", sample.name);
    }
}

#[test]
fn every_sample_produces_a_schema_for_each_of_its_layers() {
    let options = InferenceOptions::default();
    for sample in samples() {
        let observation = scan_file(&sample.file);
        for (name, _) in &observation.layers {
            let schema = xeibe_schema::infer_schema(&observation, name, &options, None)
                .unwrap_or_else(|e| panic!("{}: {name}: {e}", sample.name));
            assert!(
                !schema.schema.fields().is_empty(),
                "{}: {name} has no columns",
                sample.name
            );
            assert!(
                schema.schema.fields().iter().all(|f| f.is_nullable()),
                "{}: {name} has a non-null column",
                sample.name
            );
        }
    }
}

#[test]
fn the_prg_worked_example_from_the_docs() {
    // `docs/schema-inference.md` §5, on the two features in the sample.
    let observation = scan_file(&sample("pl-prg-address-points").file);
    let (name, _) = observation.layer("AD_PunktAdresowy").expect("the layer");
    let schema = xeibe_schema::infer_schema(&observation, name, &InferenceOptions::default(), None)
        .expect("a schema")
        .schema;

    assert_eq!(
        column_names(&schema),
        [
            "@id",
            "idIIP",
            "poczatekWersjiObiektu",
            "numerPorzadkowy",
            "georeferencja",
            "kodPocztowy",
            "dataNadania",
            "miejscowosc",
        ]
    );
    assert_eq!(data_type(&schema, "@id"), DataType::Utf8View);

    // The INSPIRE type wrapper AD_IdentyfikatorIIP is collapsed away.
    assert_eq!(nested(&schema, "idIIP.lokalnyId").data_type(), &DataType::Utf8View);
    assert_eq!(
        nested(&schema, "idIIP.przestrzenNazw").data_type(),
        &DataType::Utf8View,
        "a constant text element is still data, unlike a constant attribute"
    );
    assert_eq!(
        nested(&schema, "idIIP.wersjaId").data_type(),
        &DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
    );

    assert_eq!(
        data_type(&schema, "poczatekWersjiObiektu"),
        DataType::Timestamp(TimeUnit::Microsecond, None),
        "written without a time zone"
    );
    assert_eq!(
        data_type(&schema, "kodPocztowy"),
        DataType::Utf8View,
        "\"68-213\" is not numeric"
    );
    assert_eq!(data_type(&schema, "dataNadania"), DataType::Date32);
    assert_eq!(
        data_type(&schema, "miejscowosc"),
        DataType::Utf8View,
        "an xlink:href foreign key to AD_Miejscowosc.@id"
    );
    // In the full file this column holds "27a" as well and stays text; the two
    // sampled features only have plain numbers.
    assert_eq!(data_type(&schema, "numerPorzadkowy"), DataType::Int64);

    let geometry = field(&schema, "georeferencja");
    assert_eq!(extension_name(geometry), Some("geoarrow.point"));
    assert_eq!(
        geometry
            .metadata()
            .get(xeibe_schema::rules::meta::SRS_NAME)
            .map(String::as_str),
        Some("EPSG:2180")
    );
}

#[test]
fn repeated_references_become_a_list() {
    // AD_UlicaPlac repeats `adres2` once per address on the street.
    let observation = scan_file(&sample("pl-prg-address-points").file);
    let (name, _) = observation.layer("AD_UlicaPlac").expect("the layer");
    let schema = xeibe_schema::infer_schema(&observation, name, &InferenceOptions::default(), None)
        .expect("a schema")
        .schema;
    match data_type(&schema, "adres2") {
        DataType::List(item) => assert_eq!(item.data_type(), &DataType::Utf8View),
        other => panic!("expected a list of hrefs, got {other}"),
    }
}

#[test]
fn a_sample_with_curves_is_written_as_wkb() {
    // RCN parcels contain gml:Arc.
    let observation = scan_file(&sample("pl-rcn-parcels-arcs").file);
    let (name, _) = observation.layer("RCN_Budynek").expect("the layer");
    let schema = xeibe_schema::infer_schema(&observation, name, &InferenceOptions::default(), None)
        .expect("a schema")
        .schema;
    let geometry = schema
        .fields()
        .iter()
        .find(|field| extension_name(field).is_some())
        .expect("a geometry column");
    assert_eq!(extension_name(geometry), Some("geoarrow.wkb"));
}

#[test]
fn a_scan_records_the_srs_names_it_saw() {
    let observation = scan_file(&sample("de-hamburg-alkis-nas-arcs").file);
    let srs: Vec<String> = observation
        .layers
        .values()
        .flat_map(|layer| layer.root.children.values())
        .filter_map(|node| node.geometry.as_ref())
        .flat_map(|geometry| geometry.srs.keys().cloned())
        .collect();
    assert!(
        srs.iter().any(|name| name == "urn:adv:crs:ETRS89_UTM32"),
        "the AdV CRS URN is kept as written: {srs:?}"
    );
}

#[test]
fn a_sampled_scan_of_a_sample_stops_early() {
    let sample = sample("pl-prg-address-points");
    let source = xeibe_core::Source::file(sample.path()).expect("a file source");
    let observation = Scanner::new(ScanOptions {
        extent: xeibe_schema::ScanExtent::Sample { max_features: 2 },
        ..ScanOptions::default()
    })
    .run(xeibe_core::Sources::from(source))
    .expect("the sample scans");
    assert!(observation.sampled);
    assert_eq!(observation.layers.len(), 1, "only AD_Miejscowosc is reached");
}
