//! Inputs → sources. URLs go through the session's object store registry and
//! are read with one streaming `get` each; plain paths are local files,
//! directories, globs or zip archives, as everywhere else.

use std::sync::Arc;

use datafusion::error::{DataFusionError, Result};
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::execution::runtime_env::RuntimeEnv;
use tokio::runtime::{Handle, RuntimeFlavor};
use xeibe_core::Source;
use xeibe_io::IoOptions;

/// Resolve inputs against `runtime`'s object store registry. Blocking: listing
/// a prefix or glob waits on the store, so call it in `spawn_blocking` (or
/// [`blocking`]), never on a tokio worker thread.
///
/// `file://` URLs use the registry too (DataFusion registers a local store for
/// them); a zip archive must be given as a plain path.
pub fn resolve(runtime: &RuntimeEnv, inputs: &[String], handle: Option<&Handle>) -> Result<Vec<Source>> {
    let options = IoOptions::default();
    let mut sources = Vec::new();
    for input in inputs {
        let Some((base, key)) = split_url(input) else {
            sources.extend(xeibe_core::source::expand_sources(std::slice::from_ref(input)).map_err(external)?);
            continue;
        };
        let handle = handle.ok_or_else(|| {
            DataFusionError::Plan(format!("{input}: object store inputs need a tokio runtime"))
        })?;
        let store = runtime.object_store(ObjectStoreUrl::parse(base)?)?;
        sources.extend(xeibe_io::object_store::store_sources(store, base, key, handle, &options).map_err(external)?);
    }
    if sources.is_empty() {
        return Err(DataFusionError::Plan(format!("no GML sources in {inputs:?}")));
    }
    Ok(sources)
}

/// `scheme://authority/key` → (`scheme://authority`, `key`). The key is taken
/// as written, so glob characters survive.
fn split_url(input: &str) -> Option<(&str, &str)> {
    let scheme_end = input.find("://")?;
    if !input[..scheme_end].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) {
        return None;
    }
    let authority = scheme_end + 3;
    Some(match input[authority..].find('/') {
        Some(slash) => (&input[..authority + slash], &input[authority + slash + 1..]),
        None => (input, ""),
    })
}

/// Whether any input is a URL (read through an object store).
pub(crate) fn has_urls(inputs: &[String]) -> bool {
    inputs.iter().any(|input| split_url(input).is_some())
}

/// Run blocking work from synchronous code that may be on a tokio worker
/// thread (a table function is called while planning): `block_in_place` on a
/// multi-threaded runtime, a separate thread otherwise.
///
/// On a current-thread runtime, the object store bodies could not make
/// progress while that thread waits, so URL inputs are refused there.
pub(crate) fn blocking<T: Send>(inputs: &[String], f: impl FnOnce(Option<&Handle>) -> Result<T> + Send) -> Result<T> {
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| f(Some(&handle)))
        }
        Ok(_) if has_urls(inputs) => Err(DataFusionError::Plan(
            "sampling a layer from an object store while planning needs a multi-threaded tokio runtime; \
             give a schema (settings file) or use GmlTable::try_new"
                .into(),
        )),
        _ => std::thread::scope(|scope| {
            scope
                .spawn(|| f(None))
                .join()
                .unwrap_or_else(|_| Err(DataFusionError::Internal("GML sampling thread panicked".into())))
        }),
    }
}

pub(crate) fn external(error: impl std::error::Error + Send + Sync + 'static) -> DataFusionError {
    DataFusionError::External(Box::new(error))
}

/// Resolve inputs from async code.
pub(crate) async fn resolve_async(runtime: Arc<RuntimeEnv>, inputs: Vec<String>) -> Result<Vec<Source>> {
    let handle = Handle::current();
    tokio::task::spawn_blocking(move || resolve(&runtime, &inputs, Some(&handle)))
        .await
        .map_err(|error| DataFusionError::External(Box::new(error)))?
}

#[cfg(test)]
mod tests {
    use super::split_url;

    #[test]
    fn urls_split_into_store_and_key() {
        assert_eq!(split_url("s3://bucket/a/*.gml"), Some(("s3://bucket", "a/*.gml")));
        assert_eq!(split_url("file:///data/x.gml"), Some(("file://", "data/x.gml")));
        assert_eq!(split_url("s3://bucket"), Some(("s3://bucket", "")));
        assert_eq!(split_url("data/x.gml"), None);
    }
}
