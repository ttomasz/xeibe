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

/// Choose a strategy from capabilities and the first response.
pub fn choose(
    capabilities: &Capabilities,
    feature_type: &FeatureTypeInfo,
    requested_page_size: Option<u64>,
    forced: Option<PagingStrategy>,
) -> PagingStrategy {
    todo!()
}

/// Where to continue after a page (or after `ResponseCacheExpired`).
#[derive(Debug, Clone, PartialEq)]
pub enum NextPage {
    Url(String),
    StartIndex(u64),
    Done,
}

pub fn next_page(strategy: &PagingStrategy, fetched_so_far: u64, last: &ResponseInfo) -> NextPage {
    todo!()
}
