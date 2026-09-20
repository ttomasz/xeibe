//! Choosing a paging strategy and planning the next page (`docs/wfs.md`,
//! "Paging").

use xeibe_wfs::paging::{NextPage, choose, next_page};
use xeibe_wfs::response::ResponseInfo;
use xeibe_wfs::{Capabilities, FeatureTypeInfo, PagingStrategy, WfsVersion};

fn feature_type() -> FeatureTypeInfo {
    FeatureTypeInfo {
        name: "ms:AD.Address".into(),
        namespace: None,
        title: None,
        default_crs: None,
        other_crs: Vec::new(),
        output_formats: Vec::new(),
        wgs84_bbox: None,
    }
}

fn capabilities(version: WfsVersion, paging: Option<bool>, count_default: Option<u64>) -> Capabilities {
    Capabilities {
        version,
        service_title: None,
        feature_types: vec![feature_type()],
        get_feature_url: "https://example.com/wfs".into(),
        output_formats: Vec::new(),
        constraints: xeibe_wfs::capabilities::Constraints {
            implements_result_paging: paging,
            count_default,
            ..Default::default()
        },
    }
}

fn response(returned: Option<u64>, matched: Option<u64>, next: Option<&str>) -> ResponseInfo {
    ResponseInfo {
        number_matched: matched,
        number_returned: returned,
        next: next.map(str::to_string),
        complete: true,
        ..ResponseInfo::default()
    }
}

#[test]
fn a_server_that_supports_paging_gets_start_index_paging() {
    let strategy = choose(
        &capabilities(WfsVersion::V2_0_0, Some(true), Some(5000)),
        &feature_type(),
        None,
        None,
    );
    assert_eq!(strategy, PagingStrategy::StartIndex { page_size: 5000 });
}

#[test]
fn the_page_size_never_exceeds_the_servers_limit() {
    let strategy = choose(
        &capabilities(WfsVersion::V2_0_0, Some(true), Some(1000)),
        &feature_type(),
        Some(50_000),
        None,
    );
    assert_eq!(
        strategy,
        PagingStrategy::StartIndex { page_size: 1000 },
        "CountDefault caps the page size"
    );
}

#[test]
fn a_1x_server_is_read_in_one_request_unless_told_otherwise() {
    let strategy = choose(
        &capabilities(WfsVersion::V1_1_0, None, None),
        &feature_type(),
        None,
        None,
    );
    assert_eq!(strategy, PagingStrategy::Single);

    // Vendor paging on 1.x is an explicit opt-in.
    let forced = PagingStrategy::VendorStartIndex { page_size: 1000 };
    assert_eq!(
        choose(
            &capabilities(WfsVersion::V1_1_0, None, None),
            &feature_type(),
            None,
            Some(forced.clone()),
        ),
        forced
    );
}

#[test]
fn a_next_link_is_followed_until_there_is_none() {
    let strategy = PagingStrategy::NextLink;
    assert_eq!(
        next_page(&strategy, 2, &response(Some(2), Some(10), Some("https://example.com/p2"))),
        NextPage::Url("https://example.com/p2".into())
    );
    assert_eq!(
        next_page(&strategy, 10, &response(Some(2), Some(10), None)),
        NextPage::Done
    );
}

#[test]
fn start_index_paging_stops_at_a_short_page() {
    let strategy = PagingStrategy::StartIndex { page_size: 100 };
    assert_eq!(
        next_page(&strategy, 100, &response(Some(100), Some(250), None)),
        NextPage::StartIndex(100)
    );
    assert_eq!(
        next_page(&strategy, 200, &response(Some(100), Some(250), None)),
        NextPage::StartIndex(200)
    );
    assert_eq!(
        next_page(&strategy, 250, &response(Some(50), Some(250), None)),
        NextPage::Done,
        "the total has been reached"
    );
    assert_eq!(
        next_page(&strategy, 40, &response(Some(40), None, None)),
        NextPage::Done,
        "a page shorter than the page size is the last one"
    );
}

#[test]
fn a_single_request_never_asks_for_another_page() {
    assert_eq!(
        next_page(&PagingStrategy::Single, 2, &response(Some(2), Some(2), None)),
        NextPage::Done
    );
}
