//! XML request building for `POST` (`docs/wfs.md`, "Request encoding (XML
//! POST)"): the `GetFeature` document per version, the filter put into it, and
//! the URL it goes to.
//!
//! The element and attribute names follow the schemas (`wfs/2.0/wfs.xsd`,
//! `wfs/1.1.0/wfs.xsd`, `filter/*/sort.xsd`); the documents below validate
//! against them.

use url::Url;
use xeibe_wfs::request::{GetFeature, ResultType, SortOrder, post_url};
use xeibe_wfs::{Error, WfsVersion};

const FILTER_20: &str = r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0"><fes:PropertyIsEqualTo><fes:ValueReference>app:n</fes:ValueReference><fes:Literal>7</fes:Literal></fes:PropertyIsEqualTo></fes:Filter>"#;

fn get_feature(version: WfsVersion) -> GetFeature {
    GetFeature {
        version,
        type_name: "app:Parcel".into(),
        namespace: Some(("app".into(), "http://example.com/app".into())),
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

#[test]
fn a_wfs_20_request_carries_the_query_and_the_paging() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.srs_name = Some("urn:ogc:def:crs:EPSG::2180".into());
    request.output_format = Some("application/gml+xml; version=3.2".into());
    request.property_names = vec!["app:n".into(), "app:geom".into()];
    request.filter = Some(FILTER_20.into());
    request.sort_by = vec![
        ("app:n".into(), SortOrder::Desc),
        ("app:id".into(), SortOrder::Asc),
    ];
    request.count = Some(1000);
    request.start_index = Some(2000);
    let xml = request.to_xml().expect("a document");
    assert_eq!(
        xml,
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:fes="http://www.opengis.net/fes/2.0" service="WFS" version="2.0.0" count="1000" startIndex="2000" outputFormat="application/gml+xml; version=3.2">"#,
            r#"<wfs:Query typeNames="app:Parcel" srsName="urn:ogc:def:crs:EPSG::2180" xmlns:app="http://example.com/app">"#,
            r#"<wfs:PropertyName>app:n</wfs:PropertyName><wfs:PropertyName>app:geom</wfs:PropertyName>"#,
            r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0"><fes:PropertyIsEqualTo><fes:ValueReference>app:n</fes:ValueReference><fes:Literal>7</fes:Literal></fes:PropertyIsEqualTo></fes:Filter>"#,
            r#"<fes:SortBy><fes:SortProperty><fes:ValueReference>app:n</fes:ValueReference><fes:SortOrder>DESC</fes:SortOrder></fes:SortProperty>"#,
            r#"<fes:SortProperty><fes:ValueReference>app:id</fes:ValueReference><fes:SortOrder>ASC</fes:SortOrder></fes:SortProperty></fes:SortBy>"#,
            r#"</wfs:Query></wfs:GetFeature>"#,
        )
    );
}

#[test]
fn a_wfs_11_request_uses_its_own_names_and_full_sort_orders() {
    let mut request = get_feature(WfsVersion::V1_1_0);
    request.property_names = vec!["app:n".into()];
    request.sort_by = vec![("app:n".into(), SortOrder::Desc)];
    request.count = Some(500);
    request.start_index = Some(500);
    let xml = request.to_xml().expect("a document");
    assert_eq!(
        xml,
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<wfs:GetFeature xmlns:wfs="http://www.opengis.net/wfs" xmlns:ogc="http://www.opengis.net/ogc" service="WFS" version="1.1.0" maxFeatures="500" startIndex="500">"#,
            r#"<wfs:Query typeName="app:Parcel" xmlns:app="http://example.com/app">"#,
            // `wfs:PropertyName` in 1.1 too, and DESC, not KVP's D.
            r#"<wfs:PropertyName>app:n</wfs:PropertyName>"#,
            r#"<ogc:SortBy><ogc:SortProperty><ogc:PropertyName>app:n</ogc:PropertyName><ogc:SortOrder>DESC</ogc:SortOrder></ogc:SortProperty></ogc:SortBy>"#,
            r#"</wfs:Query></wfs:GetFeature>"#,
        )
    );
}

#[test]
fn a_hits_request_asks_only_for_the_count() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.result_type = ResultType::Hits;
    let xml = request.to_xml().expect("a document");
    assert!(
        xml.contains(r#" version="2.0.0" resultType="hits">"#),
        "{xml}"
    );
}

#[test]
fn attribute_values_and_names_are_escaped() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.output_format = Some(r#"text/xml; subtype="gml/3.1.1""#.into());
    request.sort_by = vec![("app:a&b".into(), SortOrder::Asc)];
    let xml = request.to_xml().expect("a document");
    assert!(
        xml.contains(r#"outputFormat="text/xml; subtype=&quot;gml/3.1.1&quot;""#),
        "{xml}"
    );
    assert!(
        xml.contains("<fes:ValueReference>app:a&amp;b</fes:ValueReference>"),
        "{xml}"
    );
}

#[test]
fn the_filter_goes_in_without_its_declaration_and_comments() {
    let mut request = get_feature(WfsVersion::V2_0_0);
    request.filter = Some(format!(
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- parcel 7 -->\n{FILTER_20}\n"
    ));
    let xml = request.to_xml().expect("a document");
    assert!(
        xml.contains(&format!(
            r#"xmlns:app="http://example.com/app">{FILTER_20}</wfs:Query>"#
        )),
        "{xml}"
    );
    // A self-closing filter is one element too.
    request.filter = Some(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0"/>"#.into());
    let xml = request.to_xml().expect("a document");
    assert!(
        xml.contains(r#"<fes:Filter xmlns:fes="http://www.opengis.net/fes/2.0"/></wfs:Query>"#),
        "{xml}"
    );
}

#[test]
fn a_filter_that_is_not_one_element_is_rejected() {
    let cases = [
        ("<fes:Filter><fes:And></fes:Filter>", "well-formed"),
        ("<fes:Filter>", "not closed"),
        ("<fes:Filter/><fes:Filter/>", "more than one element"),
        ("text <fes:Filter/>", "text outside"),
        ("<fes:Filter/> trailing", "text outside"),
        ("<!DOCTYPE x [<!ENTITY e \"e\">]><fes:Filter/>", "DTD"),
        ("", "no XML element"),
        ("<!-- only a comment -->", "no XML element"),
    ];
    for (filter, expected) in cases {
        let mut request = get_feature(WfsVersion::V2_0_0);
        request.filter = Some(filter.into());
        match request.to_xml() {
            Err(Error::Filter(message)) => {
                assert!(message.contains(expected), "{filter:?}: {message}")
            }
            other => panic!("{filter:?}: expected a filter error, got {other:?}"),
        }
    }
}

#[test]
fn what_xml_cannot_carry_is_an_error() {
    let mut wfs10 = get_feature(WfsVersion::V1_0_0);
    wfs10.filter = Some(FILTER_20.into());
    let error = wfs10.to_xml().expect_err("WFS 1.0");
    assert!(error.to_string().contains("WFS 1.0"), "{error}");

    let mut bbox = get_feature(WfsVersion::V2_0_0);
    bbox.bbox = Some(([1.0, 2.0, 3.0, 4.0], None));
    let error = bbox.to_xml().expect_err("a bbox");
    assert!(error.to_string().contains("bbox"), "{error}");

    let mut vendor = get_feature(WfsVersion::V2_0_0);
    vendor.vendor = vec![("CQL_FILTER".into(), "n=7".into())];
    let error = vendor.to_xml().expect_err("vendor parameters");
    assert!(error.to_string().contains("CQL_FILTER"), "{error}");
}

#[test]
fn a_post_goes_to_the_service_url_without_wfs_parameters() {
    let url = |text: &str| Url::parse(text).expect("a URL");
    assert_eq!(
        post_url(&url(
            "https://example.com/wfs?service=WFS&Request=GetCapabilities&VERSION=2.0.0"
        )),
        url("https://example.com/wfs"),
        "no empty query left"
    );
    // MapServer's map= stays; a request parameter would make it ignore the body.
    assert_eq!(
        post_url(&url(
            "https://example.com/cgi-bin/mapserv?map=/data/x.map&SERVICE=WFS&typeNames=ms:x&FILTER=y"
        )),
        url("https://example.com/cgi-bin/mapserv?map=%2Fdata%2Fx.map")
    );
}
