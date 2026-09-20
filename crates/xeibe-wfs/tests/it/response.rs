//! Reading the facts a paged download needs out of a response, and OWS
//! exceptions (`docs/wfs.md`, "Response structure", "Errors and robustness").

use xeibe_wfs::exception::ExceptionReport;
use xeibe_wfs::response::{Response, inspect};
use xeibe_testkit::samples::sample;

fn info(xml: &str) -> xeibe_wfs::response::ResponseInfo {
    match inspect(xml.as_bytes()).expect("the response is inspected") {
        Response::Features(info) => info,
        Response::Exception(report) => panic!("unexpected exception: {report}"),
    }
}

#[test]
fn reads_the_wfs_20_paging_attributes() {
    let info = info(concat!(
        r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" "#,
        r#"numberMatched="1000" numberReturned="2" next="https://example.com/wfs?page=2" "#,
        r#"timeStamp="2026-09-19T15:08:29.231Z">"#,
        "<wfs:member/><wfs:member/></wfs:FeatureCollection>"
    ));
    assert_eq!(info.number_matched, Some(1000));
    assert_eq!(info.number_returned, Some(2));
    assert_eq!(info.next.as_deref(), Some("https://example.com/wfs?page=2"));
    assert_eq!(info.time_stamp.as_deref(), Some("2026-09-19T15:08:29.231Z"));
    assert!(info.complete, "the closing element is there");
    assert!(!info.truncated);
}

#[test]
fn an_unknown_number_matched_is_not_a_number() {
    // §7.7.4.2 allows numberMatched="unknown".
    let info = info(concat!(
        r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" "#,
        r#"numberMatched="unknown" numberReturned="2"></wfs:FeatureCollection>"#
    ));
    assert_eq!(info.number_matched, None);
    assert_eq!(info.number_returned, Some(2));
}

#[test]
fn wfs_11_counts_the_features_in_this_response() {
    // 1.1 has no total in a results response (04-094 §9.3).
    let info = info(concat!(
        r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs" "#,
        r#"numberOfFeatures="2"></wfs:FeatureCollection>"#
    ));
    assert_eq!(info.number_returned, Some(2));
    assert_eq!(info.number_matched, None);
}

#[test]
fn a_truncated_response_is_recognised() {
    // No closing element: the page has to be fetched again.
    let cut_off = concat!(
        r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" numberReturned="2">"#,
        "<wfs:member><app:Parcel"
    );
    assert!(!info(cut_off).complete);

    // The explicit marker (2.0).
    let marked = concat!(
        r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0">"#,
        "<wfs:truncatedResponse/></wfs:FeatureCollection>"
    );
    assert!(info(marked).truncated);
}

#[test]
fn nested_collections_and_referenced_members_are_counted() {
    // A multi-query response (§11.3.3.5): a feature that matches several
    // queries appears once and is referenced from the others.
    let xml = concat!(
        r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" "#,
        r#"xmlns:xlink="http://www.w3.org/1999/xlink" numberReturned="2">"#,
        r#"<wfs:member><wfs:FeatureCollection/></wfs:member>"#,
        r##"<wfs:member><wfs:FeatureCollection><wfs:member xlink:href="#f1"/>"##,
        "</wfs:FeatureCollection></wfs:member></wfs:FeatureCollection>"
    );
    let info = info(xml);
    assert_eq!(info.nested_collections, 2);
    assert_eq!(info.referenced_members, 1, "reported, never fetched");
}

#[test]
fn an_exception_report_inside_a_200_response_is_detected() {
    let xml = concat!(
        r#"<ows:ExceptionReport xmlns:ows="http://www.opengis.net/ows/1.1" version="2.0.0">"#,
        r#"<ows:Exception exceptionCode="InvalidParameterValue" locator="typeNames">"#,
        "<ows:ExceptionText>Unknown type</ows:ExceptionText>",
        "</ows:Exception></ows:ExceptionReport>"
    );
    match inspect(xml.as_bytes()).expect("inspected") {
        Response::Exception(report) => {
            assert_eq!(report.exceptions.len(), 1);
            let exception = &report.exceptions[0];
            assert_eq!(exception.code.as_deref(), Some("InvalidParameterValue"));
            assert_eq!(exception.locator.as_deref(), Some("typeNames"));
            assert_eq!(exception.text, ["Unknown type"]);
            assert!(report.to_string().contains("InvalidParameterValue"));
        }
        Response::Features(info) => panic!("expected an exception, got {info:?}"),
    }
}

#[test]
fn the_wfs_10_exception_spelling_is_understood() {
    let xml = concat!(
        "<ServiceExceptionReport version=\"1.2.0\">",
        "<ServiceException code=\"InvalidParameterValue\">msWFSGetFeature(): no such type</ServiceException>",
        "</ServiceExceptionReport>"
    );
    let report = ExceptionReport::parse(xml.as_bytes()).expect("an exception report");
    assert_eq!(report.exceptions.len(), 1);
    assert_eq!(
        report.exceptions[0].code.as_deref(),
        Some("InvalidParameterValue")
    );
}

#[test]
fn a_cache_expired_exception_is_recognised_so_paging_can_resume() {
    let xml = concat!(
        r#"<ows:ExceptionReport xmlns:ows="http://www.opengis.net/ows/1.1">"#,
        r#"<ows:Exception exceptionCode="ResponseCacheExpired"/></ows:ExceptionReport>"#
    );
    let report = ExceptionReport::parse(xml.as_bytes()).expect("an exception report");
    assert!(report.is_cache_expired());

    let other = concat!(
        r#"<ows:ExceptionReport xmlns:ows="http://www.opengis.net/ows/1.1">"#,
        r#"<ows:Exception exceptionCode="OperationProcessingFailed"/></ows:ExceptionReport>"#
    );
    assert!(!ExceptionReport::parse(other.as_bytes()).unwrap().is_cache_expired());
}

#[test]
fn a_normal_feature_collection_is_not_an_exception() {
    assert!(ExceptionReport::parse(b"<wfs:FeatureCollection/>").is_none());
}

#[test]
fn the_saved_wfs_responses_in_tests_data_are_inspected_correctly() {
    let cases = [
        ("pl-gugik-mapserver-addresses-wfs200", Some(2), None),
        ("pl-gugik-inspire-au-wfs200-latlon", Some(1), None),
        ("ee-geoserver-af-wfs110-urnx-yx", Some(2), None),
    ];
    for (name, returned, matched) in cases {
        let sample = sample(name);
        match inspect(&sample.bytes()).expect("inspected") {
            Response::Features(info) => {
                assert_eq!(info.number_returned, returned, "{name}: numberReturned");
                assert_eq!(info.number_matched, matched, "{name}: numberMatched");
                assert!(info.complete, "{name}: a complete page");
            }
            Response::Exception(report) => panic!("{name}: {report}"),
        }
    }
}
