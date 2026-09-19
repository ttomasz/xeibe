//! `ows:ExceptionReport` (1.1/2.0) and `ServiceExceptionReport` (1.0).

#[derive(Debug, Clone)]
pub struct ExceptionReport {
    pub exceptions: Vec<Exception>,
}

#[derive(Debug, Clone)]
pub struct Exception {
    pub code: Option<String>,
    pub locator: Option<String>,
    pub text: Vec<String>,
}

impl ExceptionReport {
    pub fn parse(xml: &[u8]) -> Option<Self> {
        todo!()
    }

    /// `ResponseCacheExpired`: resume with `startIndex`.
    pub fn is_cache_expired(&self) -> bool {
        todo!()
    }
}

impl std::fmt::Display for ExceptionReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}
