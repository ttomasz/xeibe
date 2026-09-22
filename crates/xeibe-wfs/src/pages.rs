//! Pages of one feature type streamed into a read: capabilities → hits → pages,
//! each page fetched whole into memory, checked for exceptions and truncation,
//! and handed on as a source. Nothing is saved (see `docs/wfs.md`).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use url::Url;
use xeibe_core::{BytesSource, Source, Sources};

use crate::exception::ExceptionReport;
use crate::paging::{self, NextPage, PagingStrategy};
use crate::request::{GetFeature, ResultType, SortOrder, get_capabilities_url};
use crate::response::{self, Response, ResponseInfo};
use crate::{Capabilities, Error, FeatureTypeInfo, WfsOptions, WfsVersion};

/// Progress callback payload, sent after every page.
#[derive(Debug, Clone)]
pub struct Progress {
    pub type_name: String,
    pub pages: u64,
    pub features: u64,
    pub number_matched: Option<u64>,
    /// Raised since the previous page: a changed `numberMatched`, repeated
    /// `gml:id`s, a retried truncated page, paging that is not transaction-safe, …
    pub warnings: Vec<String>,
}

pub struct WfsClient {
    endpoint: Url,
    options: WfsOptions,
    http: Arc<crate::http::HttpClient>,
    /// Fetched once per client; `count` and `pages` need it too.
    capabilities: Mutex<Option<Arc<Capabilities>>>,
}

impl WfsClient {
    /// `endpoint` is the service URL, with or without `SERVICE`/`REQUEST`
    /// parameters (they are replaced per request). Nothing is requested yet.
    pub fn new(endpoint: &str, options: WfsOptions) -> crate::Result<Self> {
        let endpoint = Url::parse(endpoint)?;
        let http = Arc::new(crate::http::HttpClient::new(&options)?);
        Ok(Self {
            endpoint,
            options,
            http,
            capabilities: Mutex::new(None),
        })
    }

    /// `GetCapabilities` for `options.version`, or the server's own choice.
    pub fn capabilities(&self) -> crate::Result<Capabilities> {
        Ok(self.cached_capabilities()?.as_ref().clone())
    }

    /// `resultType=hits`: `numberMatched` (2.0) or `numberOfFeatures` (1.1).
    /// `None` when the server says "unknown", and for WFS 1.0, which has no hits
    /// request (nothing is requested then).
    pub fn count(&self, type_name: &str) -> crate::Result<Option<u64>> {
        let capabilities = self.cached_capabilities()?;
        let feature_type = find_type(&capabilities, type_name)?;
        let version = self.version(&capabilities);
        if version == WfsVersion::V1_0_0 {
            return Ok(None);
        }
        let request = self.request(&capabilities, feature_type, ResultType::Hits);
        let body = self.http.get(&request.to_url(&self.endpoint)?)?;
        match response::inspect(&body)? {
            Response::Exception(report) => Err(Error::Exception(report)),
            Response::Features(info) if version.is_2() => Ok(info.number_matched),
            Response::Features(info) => Ok(info.number_returned),
        }
    }

    /// Every page of `type_name`, lazily: the next page is requested when the read
    /// needs more features (up to `options.concurrency` pages ahead, kept in order).
    /// A page that fails after retries ends the sequence with an error.
    ///
    /// Capabilities are fetched (once per client) before this returns, so an
    /// unknown type fails here; no page is requested until the first is pulled.
    pub fn pages(
        &self,
        type_name: &str,
        progress: Box<dyn FnMut(&Progress) + Send>,
    ) -> crate::Result<Sources> {
        if self.options.dedupe {
            return Err(Error::Unsupported(
                "removing duplicate features across pages is not implemented; they are reported"
                    .into(),
            ));
        }
        let capabilities = self.cached_capabilities()?;
        let feature_type = find_type(&capabilities, type_name)?;
        let version = self.version(&capabilities);
        let strategy = paging::choose(
            &capabilities,
            feature_type,
            self.options.page_size,
            self.options.strategy.clone(),
        );
        if matches!(strategy, PagingStrategy::SpatialTiling { .. }) {
            return Err(Error::Unsupported(
                "spatial tiling is not implemented".into(),
            ));
        }
        let mut template = self.request(&capabilities, feature_type, ResultType::Results);
        // A forced next-link strategy asks the server to page by setting `count`.
        if strategy == PagingStrategy::NextLink {
            template.count = self
                .options
                .page_size
                .map(|size| paging::page_size(&capabilities, Some(size)));
        }
        let next = match &strategy {
            PagingStrategy::StartIndex { .. } | PagingStrategy::VendorStartIndex { .. } => {
                NextPage::StartIndex(0)
            }
            _ => NextPage::Url(template.to_url(&self.endpoint)?.to_string()),
        };
        let constraints = &capabilities.constraints;
        let pager = Pager {
            http: self.http.clone(),
            base: self.endpoint.clone(),
            template,
            sort_by: self.options.sort_by.clone(),
            version,
            automatic: self.options.strategy.is_none(),
            strategy,
            next,
            // After `ResponseCacheExpired` only a server that implements paging
            // can continue at an offset.
            resume_page_size: (version.is_2()
                && constraints.implements_result_paging == Some(true))
            .then(|| paging::page_size(&capabilities, self.options.page_size)),
            paging_is_transaction_safe: constraints.paging_is_transaction_safe,
            max_retries: self.options.max_retries,
            attempts: 0,
            fetched: 0,
            pages: 0,
            first_matched: None,
            matched_warned: false,
            unsafe_warned: false,
            previous_ids: HashSet::new(),
            warnings: Vec::new(),
            progress: Progress {
                type_name: feature_type.name.clone(),
                pages: 0,
                features: 0,
                number_matched: None,
                warnings: Vec::new(),
            },
            callback: progress,
            done: false,
        };
        Ok(read_ahead(pager, self.options.concurrency))
    }

    fn cached_capabilities(&self) -> crate::Result<Arc<Capabilities>> {
        let mut cached = self
            .capabilities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(capabilities) = cached.as_ref() {
            return Ok(capabilities.clone());
        }
        let url = get_capabilities_url(&self.endpoint, self.options.version)?;
        let capabilities = Arc::new(Capabilities::parse(&self.http.get(&url)?)?);
        *cached = Some(capabilities.clone());
        Ok(capabilities)
    }

    fn version(&self, capabilities: &Capabilities) -> WfsVersion {
        self.options.version.unwrap_or(capabilities.version)
    }

    /// The query shared by hits and pages. Requests go to the endpoint the user
    /// gave, not the advertised `GetFeature` URL: servers often advertise an
    /// internal host name, or drop parameters such as MapServer's `map=`.
    fn request(
        &self,
        capabilities: &Capabilities,
        feature_type: &FeatureTypeInfo,
        result_type: ResultType,
    ) -> GetFeature {
        let namespace = match (feature_type.name.split_once(':'), &feature_type.namespace) {
            (Some((prefix, _)), Some(uri)) => Some((prefix.to_string(), uri.clone())),
            _ => None,
        };
        GetFeature {
            version: self.version(capabilities),
            type_name: feature_type.name.clone(),
            namespace,
            srs_name: self.options.srs_name.clone(),
            output_format: capabilities.preferred_output_format(feature_type),
            bbox: self.options.bbox.clone(),
            filter: self.options.filter.clone(),
            property_names: Vec::new(),
            sort_by: Vec::new(),
            result_type,
            count: None,
            start_index: None,
            vendor: self.options.vendor_params.clone(),
        }
    }
}

fn find_type<'a>(capabilities: &'a Capabilities, name: &str) -> crate::Result<&'a FeatureTypeInfo> {
    capabilities
        .feature_type(name)
        .ok_or_else(|| Error::UnknownFeatureType(name.to_string()))
}

/// The pager itself when `concurrency` is 1. Otherwise a thread runs it and
/// hands pages over a channel: `concurrency - 1` pages wait for the read
/// (one of them held by the thread until the read takes the one before it).
fn read_ahead(pager: Pager, concurrency: usize) -> Sources {
    if concurrency <= 1 {
        return Sources::lazy(pager);
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(concurrency - 2);
    std::thread::spawn(move || {
        for page in pager {
            // The read was dropped: stop fetching.
            if sender.send(page).is_err() {
                break;
            }
        }
    });
    Sources::lazy(receiver.into_iter())
}

/// The paging loop, one page per `next`.
struct Pager {
    http: Arc<crate::http::HttpClient>,
    base: Url,
    template: GetFeature,
    sort_by: Option<String>,
    version: WfsVersion,
    /// The strategy was chosen, not forced: a `next` link may take over.
    automatic: bool,
    strategy: PagingStrategy,
    next: NextPage,
    resume_page_size: Option<u64>,
    paging_is_transaction_safe: Option<bool>,
    max_retries: u32,
    /// Truncated responses of the current page so far.
    attempts: u32,
    fetched: u64,
    pages: u64,
    first_matched: Option<u64>,
    matched_warned: bool,
    unsafe_warned: bool,
    /// Ids of the previous page. Offset paging over changing data shifts
    /// features across a page boundary, so a repeat shows up in the next page;
    /// comparing with one page keeps memory bounded by the page size.
    previous_ids: HashSet<String>,
    /// Warnings for the next progress report.
    warnings: Vec<String>,
    progress: Progress,
    callback: Box<dyn FnMut(&Progress) + Send>,
    done: bool,
}

impl Iterator for Pager {
    type Item = xeibe_core::Result<Source>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.next_page() {
            Ok(Some(source)) => Some(Ok(source)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(error) => {
                self.done = true;
                Some(Err(error.into()))
            }
        }
    }
}

impl Pager {
    /// Fetch one page completely; retried whole on a truncated response, with
    /// half the page size under offset paging.
    fn next_page(&mut self) -> crate::Result<Option<Source>> {
        loop {
            let url = match &self.next {
                NextPage::Done => return Ok(None),
                NextPage::Url(url) => Url::parse(url)?,
                NextPage::StartIndex(start) => self.offset_url(*start)?,
            };
            let page = self.pages + 1;
            let body = match self.http.get(&url) {
                Ok(body) => body,
                Err(Error::Exception(report)) => {
                    self.resume_after(report)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let info = match response::inspect(&body)? {
                Response::Features(info) => info,
                Response::Exception(report) => {
                    self.resume_after(report)?;
                    continue;
                }
            };
            if !info.complete || info.truncated {
                self.attempts += 1;
                if self.attempts > self.max_retries {
                    return Err(Error::Truncated { page });
                }
                let halved = self.halve_page_size();
                self.warnings.push(match halved {
                    Some(size) => {
                        format!("page {page} was truncated; retrying with {size} features per page")
                    }
                    None => format!("page {page} was truncated; retrying"),
                });
                continue;
            }
            self.attempts = 0;
            self.after_page(page, &info);
            let source = BytesSource {
                name: url.to_string(),
                bytes: body,
            };
            return Ok(Some(Source::Stream(Arc::new(source))));
        }
    }

    /// `ResponseCacheExpired` while following `next` links continues at the
    /// current offset if the server implements paging; any other exception
    /// ends the read.
    fn resume_after(&mut self, report: ExceptionReport) -> crate::Result<()> {
        match self.resume_page_size {
            Some(page_size)
                if report.is_cache_expired() && self.strategy == PagingStrategy::NextLink =>
            {
                self.strategy = PagingStrategy::StartIndex { page_size };
                self.next = NextPage::StartIndex(self.fetched);
                self.warnings.push(format!(
                    "the server's result cache expired; continuing with startIndex={}",
                    self.fetched
                ));
                Ok(())
            }
            _ => Err(Error::Exception(report)),
        }
    }

    fn offset_url(&self, start: u64) -> crate::Result<Url> {
        let page_size = match self.strategy {
            PagingStrategy::StartIndex { page_size }
            | PagingStrategy::VendorStartIndex { page_size } => page_size,
            _ => paging::DEFAULT_PAGE_SIZE,
        };
        let mut request = self.template.clone();
        request.count = Some(page_size);
        request.start_index = Some(start);
        if let Some(property) = &self.sort_by {
            request.sort_by = vec![(property.clone(), SortOrder::Asc)];
        }
        request.to_url(&self.base)
    }

    /// The new page size under offset paging; `None` for other strategies.
    fn halve_page_size(&mut self) -> Option<u64> {
        match &mut self.strategy {
            PagingStrategy::StartIndex { page_size }
            | PagingStrategy::VendorStartIndex { page_size } => {
                *page_size = (*page_size / 2).max(1);
                Some(*page_size)
            }
            _ => None,
        }
    }

    /// Book-keeping, checks and the progress report for a complete page.
    fn after_page(&mut self, page: u64, info: &ResponseInfo) {
        // An automatic choice defers to server-generated `next` links (2.0).
        if self.automatic
            && page == 1
            && self.version.is_2()
            && info.next.is_some()
            && self.strategy != PagingStrategy::NextLink
        {
            self.strategy = PagingStrategy::NextLink;
        }
        let returned = info.returned();
        self.fetched += returned;
        self.pages = page;
        self.next = paging::next_page(&self.strategy, self.fetched, info);

        if page == 1 {
            self.first_matched = info.number_matched;
        } else if let (Some(first), Some(now)) = (self.first_matched, info.number_matched)
            && first != now
            && !self.matched_warned
        {
            self.matched_warned = true;
            self.warnings.push(format!(
                "numberMatched changed from {first} to {now}: the data changed during the read"
            ));
        }
        if self.strategy == PagingStrategy::Single
            && let Some(matched) = info.number_matched
            && returned < matched
        {
            self.warnings.push(format!(
                "a single request returned {returned} of {matched} features; the server does not page this layer"
            ));
        }
        if self.version.is_2()
            && self.next != NextPage::Done
            && self.paging_is_transaction_safe != Some(true)
            && !self.unsafe_warned
        {
            self.unsafe_warned = true;
            self.warnings.push(
                "the server does not declare PagingIsTransactionSafe: features may be skipped or repeated if the data changes during the read"
                    .into(),
            );
        }
        let mut ids = HashSet::with_capacity(info.feature_ids.len());
        let mut repeated = 0u64;
        for id in &info.feature_ids {
            if self.previous_ids.contains(id) || !ids.insert(id.clone()) {
                repeated += 1;
            }
        }
        if repeated > 0 {
            self.warnings.push(format!(
                "{repeated} features on page {page} repeat a gml:id of this or the previous page"
            ));
        }
        self.previous_ids = ids;

        self.progress.pages = self.pages;
        self.progress.features = self.fetched;
        self.progress.number_matched = info.number_matched.or(self.first_matched);
        self.progress.warnings = std::mem::take(&mut self.warnings);
        (self.callback)(&self.progress);
    }
}
