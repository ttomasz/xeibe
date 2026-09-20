//! The splitter against the real samples in `tests/data`.
//!
//! GDAL's layer list and feature counts are reliable here (unlike its axis
//! order), so they are used as the expected values: every layer GDAL lists is a
//! feature element, and the number of features must match.

use xeibe_core::{QName, Source, SplitterOptions};
use xeibe_testkit::samples::{sample, samples};

use crate::support::{feature_names, features, one_feature_per_chunk};

fn split_sample(sample: &xeibe_testkit::samples::Sample, options: SplitterOptions) -> Vec<xeibe_core::FeatureChunk> {
    let source = Source::file(sample.path()).expect("a file source");
    let reader = source.open().expect("the sample opens");
    xeibe_core::FeatureSplitter::new(reader, xeibe_core::SourceId(0), options)
        .collect::<xeibe_core::Result<Vec<_>>>()
        .unwrap_or_else(|e| panic!("{}: {e}", sample.name))
}

/// One test per sample would be nicer to read, but the list comes from the
/// manifest; failures name the sample.
#[test]
fn finds_every_feature_of_every_sample() {
    for sample in samples() {
        let chunks = split_sample(&sample, one_feature_per_chunk());
        let names = feature_names(&chunks);
        assert_eq!(
            names.len() as u64,
            sample.total_features(),
            "{}: features found {:?}, GDAL counted {:?} in {:?}",
            sample.name,
            names.len(),
            sample.total_features(),
            sample.gdal.feature_counts
        );

        let mut found: Vec<String> = names.clone();
        found.sort();
        found.dedup();
        let mut expected: Vec<String> = sample
            .gdal_report()
            .layers
            .iter()
            .map(|layer| layer.name.clone())
            .collect();
        expected.sort();
        assert_eq!(found, expected, "{}: layers", sample.name);
    }
}

#[test]
fn chunk_counts_follow_the_target_size() {
    for sample in samples() {
        let whole = split_sample(&sample, SplitterOptions::default());
        assert!(
            whole.len() <= 1,
            "{}: the samples are small, so the default target gives one chunk, got {}",
            sample.name,
            whole.len()
        );
        let per_feature = split_sample(&sample, one_feature_per_chunk());
        assert_eq!(
            per_feature.len() as u64,
            sample.total_features(),
            "{}: one chunk per feature",
            sample.name
        );
    }
}

#[test]
fn prg_stores_its_layers_one_after_another() {
    // The layer filter has to skip the first layers cheaply: PRG keeps
    // AD_Miejscowosc, then AD_UlicaPlac, then AD_PunktAdresowy.
    let sample = sample("pl-prg-address-points");
    let names = feature_names(&split_sample(&sample, one_feature_per_chunk()));
    assert_eq!(
        names,
        [
            "AD_Miejscowosc",
            "AD_Miejscowosc",
            "AD_UlicaPlac",
            "AD_UlicaPlac",
            "AD_PunktAdresowy",
            "AD_PunktAdresowy",
        ]
    );

    let prgad = "https://geoportal.gov.pl/schemas/prgad/1.0";
    let options = SplitterOptions {
        layers: Some(vec![QName::new(Some(prgad), "AD_PunktAdresowy")]),
        ..one_feature_per_chunk()
    };
    let filtered = split_sample(&sample, options);
    assert_eq!(
        features(&filtered),
        vec![QName::new(Some(prgad), "AD_PunktAdresowy"); 2]
    );
}

#[test]
fn reads_the_header_of_a_wfs_20_response() {
    let sample = sample("pl-gugik-mapserver-addresses-wfs200");
    let source = Source::file(sample.path()).unwrap();
    let mut splitter = xeibe_core::FeatureSplitter::new(
        source.open().unwrap(),
        xeibe_core::SourceId(0),
        SplitterOptions::default(),
    );
    let header = splitter.header().expect("a header");
    assert_eq!(&*header.root.local, "FeatureCollection");
    assert_eq!(header.root.ns.as_deref(), Some(xeibe_core::ns::WFS_20));
    let attribute = |local: &str| {
        header
            .wfs_attributes
            .iter()
            .find(|(name, _)| &*name.local == local)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(attribute("numberMatched"), Some("unknown"));
    assert_eq!(attribute("numberReturned"), Some("2"));
    assert!(header.schema_location.is_some());
    assert!(!header.fme_produced);
}

#[test]
fn recognises_fme_produced_data() {
    let sample = sample("ch-geneva-arcbycenterpoint-fme");
    let source = Source::file(sample.path()).unwrap();
    let mut splitter = xeibe_core::FeatureSplitter::new(
        source.open().unwrap(),
        xeibe_core::SourceId(0),
        SplitterOptions::default(),
    );
    assert!(
        splitter.header().expect("a header").fme_produced,
        "the root declares xmlns:fme"
    );
}

#[test]
fn reads_a_nas_document_whose_collection_is_nested() {
    // ALKIS/NAS wraps a wfs:FeatureCollection inside AX_Bestandsdatenauszug.
    let sample = sample("de-hamburg-alkis-nas-arcs");
    let names = feature_names(&split_sample(&sample, one_feature_per_chunk()));
    assert_eq!(names.len(), 4, "{names:?}");
}
