//! Paging strategy selection and page planning (`docs/wfs.md` "Paging").

use crate::response::ResponseInfo;
use crate::{Capabilities, FeatureTypeInfo};

#[derive(Debug, Clone, PartialEq)]
pub enum PagingStrategy {
    /// Follow server-generated `next` URIs.
    NextLink,
    StartIndex {
        page_size: u64,
    },
    /// `maxFeatures` + vendor `startIndex` on WFS 1.x (explicit opt-in).
    VendorStartIndex {
        page_size: u64,
    },
    /// Split the bbox until each tile's hit count fits the server limit.
    SpatialTiling {
        max_per_tile: u64,
    },
    Single,
}

/// Page size when neither the user nor the server's `CountDefault` gives one.
pub const DEFAULT_PAGE_SIZE: u64 = 5000;

/// Choose a strategy from the capabilities, before the first request.
///
/// `startIndex` paging on a WFS 2.0 server that implements result paging,
/// otherwise a single request. The `next`-link strategy is taken up later,
/// when the first response of an automatic choice carries a `next` link (see
/// `WfsClient::pages`). A forced strategy is kept, with its page size capped by
/// `CountDefault`.
pub fn choose(
    capabilities: &Capabilities,
    // Not consulted yet: every paging constraint is service-wide.
    _feature_type: &FeatureTypeInfo,
    requested_page_size: Option<u64>,
    forced: Option<PagingStrategy>,
) -> PagingStrategy {
    let count_default = capabilities.constraints.count_default;
    let cap = |page_size: u64| {
        match count_default {
            Some(limit) if limit > 0 => page_size.min(limit),
            _ => page_size,
        }
        .max(1)
    };
    match forced {
        Some(PagingStrategy::StartIndex { page_size }) => PagingStrategy::StartIndex {
            page_size: cap(page_size),
        },
        Some(PagingStrategy::VendorStartIndex { page_size }) => PagingStrategy::VendorStartIndex {
            page_size: cap(page_size),
        },
        Some(forced) => forced,
        None if capabilities.version.is_2()
            && capabilities.constraints.implements_result_paging == Some(true) =>
        {
            PagingStrategy::StartIndex {
                page_size: page_size(capabilities, requested_page_size),
            }
        }
        None => PagingStrategy::Single,
    }
}

/// `min(requested, CountDefault)`, or whichever is given, or
/// [`DEFAULT_PAGE_SIZE`].
pub fn page_size(capabilities: &Capabilities, requested: Option<u64>) -> u64 {
    let count_default = capabilities.constraints.count_default.filter(|&c| c > 0);
    match (requested, count_default) {
        (Some(requested), Some(limit)) => requested.min(limit),
        (Some(requested), None) => requested,
        (None, Some(limit)) => limit,
        (None, None) => DEFAULT_PAGE_SIZE,
    }
    .max(1)
}

/// Where to continue after a page (or after `ResponseCacheExpired`).
#[derive(Debug, Clone, PartialEq)]
pub enum NextPage {
    Url(String),
    StartIndex(u64),
    Done,
}

/// Offset paging stops at a short (or empty) page, or once `numberMatched`
/// features have arrived; `next`-link paging stops at the page without one.
pub fn next_page(strategy: &PagingStrategy, fetched_so_far: u64, last: &ResponseInfo) -> NextPage {
    match strategy {
        PagingStrategy::NextLink => match &last.next {
            Some(next) => NextPage::Url(next.clone()),
            None => NextPage::Done,
        },
        PagingStrategy::StartIndex { page_size }
        | PagingStrategy::VendorStartIndex { page_size } => {
            let returned = last.returned();
            // With a known total, a short page is not the end: the server may
            // cap pages below the page size without advertising it.
            let done = match last.number_matched {
                Some(matched) => returned == 0 || fetched_so_far >= matched,
                None => returned == 0 || returned < *page_size,
            };
            if done {
                NextPage::Done
            } else {
                NextPage::StartIndex(fetched_so_far)
            }
        }
        PagingStrategy::SpatialTiling { .. } | PagingStrategy::Single => NextPage::Done,
    }
}
