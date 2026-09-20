//! KVP request building (`docs/wfs.md`, "Request encoding").
//!
//! The parameter names differ per version, which is most of what this is about:
//! `TYPENAMES`/`TYPENAME`, `COUNT`/`MAXFEATURES`, `NAMESPACES`/`NAMESPACE`.

use url::Url;
use xeibe_wfs::WfsVersion;
use xeibe_wfs::request::{GetFeature, ResultType, SortOrder, get_capabilities_url};

fn base() -> Url {
    Url::parse("https://example.com/wfs").expect("a URL")
}

fn get_feature(version: WfsVersion) -> GetFeature {
    GetFeature {
        version,
        type_name: "ms:AD.Address".into(),
        namespace: None,
        srs_name: None,
        output_format: None,
        bbox: None,
        filter: None,
        property_names: Vec::new(),
        sort_by: Vec::new(),
        result_type: ResultType::Results,
        count: None,
        start_index: None,
        vendor: Vec::new(),
    }
}

/// Parameters as upper-case keys, the way the spec writes them.
fn params(url: &Url) -> Vec<(String, String)> {
    url.query_pairs()
        .map(|(key, value)| (key.to_uppercase(), value.to_string()))
        .collect()
}

fn value(url: &Url, key: &str) -> Option<String> {
    params(url)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

#[test]
fn a_wfs_20_request_uses_the_plural_type_names() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.count = Some(1000);
    request.start_index = Some(2000);
    let url = request.to_url(&base()).expect("a URL");
    assert_eq!(value(&url, "SERVICE").as_deref(), Some("WFS"));
    assert_eq!(value(&url, "VERSION").as_deref(), Some("2.0.0"));
    assert_eq!(value(&url, "REQUEST").as_deref(), Some("GetFeature"));
    assert_eq!(value(&url, "TYPENAMES").as_deref(), Some("ms:AD.Address"));
    assert_eq!(value(&url, "COUNT").as_deref(), Some("1000"));
    assert_eq!(value(&url, "STARTINDEX").as_deref(), Some("2000"));
    assert_eq!(value(&url, "TYPENAME"), None);
    assert_eq!(value(&url, "MAXFEATURES"), None);
}

#[test]
fn the_older_versions_use_maxfeatures() {
    for version in [WfsVersion::V1_0_0, WfsVersion::V1_1_0] {
        let mut request = get_feature(version);
        request.count = Some(500);
        let url = request.to_url(&base()).expect("a URL");
        assert_eq!(value(&url, "TYPENAME").as_deref(), Some("ms:AD.Address"));
        assert_eq!(value(&url, "MAXFEATURES").as_deref(), Some("500"));
        assert_eq!(value(&url, "COUNT"), None);
    }
}

#[test]
fn namespaces_are_declared_the_way_each_version_wants() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.namespace = Some((
        "ms".into(),
        "http://mapserver.gis.umn.edu/mapserver".into(),
    ));
    let url = request.to_url(&base()).expect("a URL");
    assert_eq!(
        value(&url, "NAMESPACES").as_deref(),
        Some("xmlns(ms,http://mapserver.gis.umn.edu/mapserver)")
    );

    let mut request = get_feature(WfsVersion::V1_1_0);
    request.namespace = Some((
        "ms".into(),
        "http://mapserver.gis.umn.edu/mapserver".into(),
    ));
    let url = request.to_url(&base()).expect("a URL");
    assert_eq!(
        value(&url, "NAMESPACE").as_deref(),
        Some("xmlns(ms=http://mapserver.gis.umn.edu/mapserver)")
    );
}

#[test]
fn a_hits_request_asks_only_for_the_count() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.result_type = ResultType::Hits;
    let url = request.to_url(&base()).expect("a URL");
    assert_eq!(value(&url, "RESULTTYPE").as_deref(), Some("hits"));
}

#[test]
fn the_bbox_carries_its_crs_and_keeps_the_corner_order() {
    // Lower corner then upper corner, in the CRS's axis order (06-121r9 §10.2.3).
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.bbox = Some((
        [49.0, 14.1, 54.9, 24.2],
        Some("urn:ogc:def:crs:EPSG::4258".into()),
    ));
    let url = request.to_url(&base()).expect("a URL");
    assert_eq!(
        value(&url, "BBOX").as_deref(),
        Some("49,14.1,54.9,24.2,urn:ogc:def:crs:EPSG::4258")
    );

    let mut without_crs = get_feature(WfsVersion::V1_1_0);
    without_crs.bbox = Some(([0.0, 1.0, 2.0, 3.0], None));
    let url = without_crs.to_url(&base()).expect("a URL");
    assert_eq!(value(&url, "BBOX").as_deref(), Some("0,1,2,3"));
}

#[test]
fn sorting_uses_each_versions_spelling() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.sort_by = vec![
        ("gml:id".into(), SortOrder::Asc),
        ("name".into(), SortOrder::Desc),
    ];
    assert_eq!(
        value(&request.to_url(&base()).expect("a URL"), "SORTBY").as_deref(),
        Some("gml:id ASC,name DESC")
    );

    let mut request = get_feature(WfsVersion::V1_1_0);
    request.sort_by = vec![("gml:id".into(), SortOrder::Desc)];
    assert_eq!(
        value(&request.to_url(&base()).expect("a URL"), "SORTBY").as_deref(),
        Some("gml:id D")
    );
}

#[test]
fn srs_name_output_format_and_properties_are_passed_through() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.srs_name = Some("urn:ogc:def:crs:EPSG::2180".into());
    request.output_format = Some("application/gml+xml; version=3.2".into());
    request.property_names = vec!["geom".into(), "name".into()];
    let url = request.to_url(&base()).expect("a URL");
    assert_eq!(
        value(&url, "SRSNAME").as_deref(),
        Some("urn:ogc:def:crs:EPSG::2180")
    );
    assert_eq!(
        value(&url, "OUTPUTFORMAT").as_deref(),
        Some("application/gml+xml; version=3.2")
    );
    assert_eq!(value(&url, "PROPERTYNAME").as_deref(), Some("geom,name"));
}

#[test]
fn vendor_parameters_and_filters_are_sent_as_given() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.filter = Some("<fes:Filter/>".into());
    request.vendor = vec![("CQL_FILTER".into(), "area > 10".into())];
    let url = request.to_url(&base()).expect("a URL");
    assert_eq!(value(&url, "FILTER").as_deref(), Some("<fes:Filter/>"));
    assert_eq!(value(&url, "CQL_FILTER").as_deref(), Some("area > 10"));
    // The query string is escaped.
    assert!(url.as_str().contains("CQL_FILTER=area%20%3E%2010") || url.as_str().contains("CQL_FILTER=area+%3E+10"),
        "{url}");
}

#[test]
fn a_base_url_that_already_has_a_query_keeps_it() {
    let base = Url::parse("https://example.com/wfs?map=/data/x.map").expect("a URL");
    let url = get_feature(WfsVersion::V2_0_0).to_url(&base).expect("a URL");
    assert_eq!(value(&url, "MAP").as_deref(), Some("/data/x.map"));
    assert_eq!(value(&url, "REQUEST").as_deref(), Some("GetFeature"));
}

#[test]
fn a_capabilities_url_can_ask_for_a_version_or_not() {
    let url = get_capabilities_url(&base(), Some(WfsVersion::V2_0_2)).expect("a URL");
    assert_eq!(value(&url, "REQUEST").as_deref(), Some("GetCapabilities"));
    assert_eq!(value(&url, "SERVICE").as_deref(), Some("WFS"));
    assert_eq!(value(&url, "VERSION").as_deref(), Some("2.0.2"));

    let negotiated = get_capabilities_url(&base(), None).expect("a URL");
    assert_eq!(value(&negotiated, "VERSION"), None, "let the server choose");
}
