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
    /// `None` unless the document's root is an exception report. Only the root's
    /// start tag is read for any other document, so this is cheap on a page.
    pub fn parse(xml: &[u8]) -> Option<Self> {
        let xml = crate::xml::utf8(xml).ok()?;
        let (_, root) = crate::xml::root_name(&xml)?;
        if !is_report(&root) {
            return None;
        }
        let root = crate::xml::parse(&xml).ok()?;
        let exceptions = root
            .children
            .iter()
            .filter(|c| c.is("Exception") || c.is("ServiceException"))
            .map(|element| {
                // 1.1/2.0: `ExceptionText` children; 1.0: the element's own text.
                let mut text: Vec<String> = element
                    .children_named("ExceptionText")
                    .map(|t| t.text.clone())
                    .filter(|t| !t.is_empty())
                    .collect();
                if text.is_empty() && !element.text.is_empty() {
                    text.push(element.text.clone());
                }
                Exception {
                    code: element
                        .attr("exceptionCode")
                        .or_else(|| element.attr("code"))
                        .map(str::to_string),
                    locator: element.attr("locator").map(str::to_string),
                    text,
                }
            })
            .collect();
        Some(Self { exceptions })
    }

    /// `ResponseCacheExpired`: resume with `startIndex`.
    pub fn is_cache_expired(&self) -> bool {
        self.has_code("ResponseCacheExpired")
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.exceptions
            .iter()
            .any(|e| e.code.as_deref() == Some(code))
    }
}

/// Root element names of exception reports, in any namespace (1.0 reports
/// often have none, or the OGC one).
fn is_report(local: &str) -> bool {
    local == "ExceptionReport" || local == "ServiceExceptionReport"
}

/// `code (locator): text; …`.
impl std::fmt::Display for ExceptionReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.exceptions.is_empty() {
            return f.write_str("empty exception report");
        }
        for (i, exception) in self.exceptions.iter().enumerate() {
            if i > 0 {
                f.write_str("; ")?;
            }
            f.write_str(exception.code.as_deref().unwrap_or("exception"))?;
            if let Some(locator) = &exception.locator {
                write!(f, " (locator {locator})")?;
            }
            if !exception.text.is_empty() {
                write!(f, ": {}", exception.text.join(" "))?;
            }
        }
        Ok(())
    }
}
