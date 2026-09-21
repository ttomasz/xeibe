//! Turn CLI/library inputs (paths, globs, `-`, `http(s)://`, `s3://`, …, optionally
//! with `!/member` for local zip archives) into sources. Local inputs are expanded
//! with `xeibe_core::source::expand_sources`; a remote zip is
//! `xeibe_core::Error::RemoteArchive`.

#[cfg(feature = "http")]
use std::sync::Arc;

use url::Url;
use xeibe_core::Source;

use crate::options::IoOptions;

/// Resolve inputs in order:
///
/// - `-` is stdin, a one-shot source;
/// - `http://` and `https://` are one streaming `GET` each (feature `http`);
/// - `file://` URLs are local paths;
/// - other URLs (`s3://`, `gs://`, `az://`, …) are object stores (feature
///   `object-store`): one object, a prefix ending in `/`, or a glob;
/// - everything else is a local path, directory, glob, zip archive or
///   `archive.zip!/member`.
///
/// Remote inputs are not contacted here, except to list an object-store prefix
/// or glob. Object stores take their credentials from the environment
/// (`AWS_ACCESS_KEY_ID`, …). Call this outside async code: it may block on a
/// tokio runtime, and one of its own is started if none is current.
pub fn resolve_sources(inputs: &[String], options: &IoOptions) -> crate::Result<Vec<Source>> {
    #[cfg(feature = "http")]
    let mut client: Option<Arc<crate::http::HttpClient>> = None;
    let mut sources = Vec::new();
    for input in inputs {
        if input == "-" {
            sources.push(Source::reader("stdin", Box::new(std::io::stdin())));
            continue;
        }
        if !input.contains("://") {
            sources.extend(xeibe_core::source::expand_sources(std::slice::from_ref(
                input,
            ))?);
            continue;
        }

        let url =
            Url::parse(input).map_err(|e| crate::Error::InvalidUrl(format!("{input}: {e}")))?;
        if url.scheme() == "file" {
            let path = url
                .to_file_path()
                .map_err(|()| crate::Error::InvalidUrl(format!("{input}: not a local path")))?;
            let path = path.display().to_string();
            sources.extend(xeibe_core::source::expand_sources(&[path])?);
            continue;
        }
        let lower = input.to_ascii_lowercase();
        if lower.contains(".zip!/") || url.path().to_ascii_lowercase().ends_with(".zip") {
            return Err(xeibe_core::Error::RemoteArchive(input.clone()).into());
        }
        match url.scheme() {
            "http" | "https" => {
                #[cfg(feature = "http")]
                {
                    let client = match &client {
                        Some(client) => client.clone(),
                        None => client
                            .insert(Arc::new(crate::http::HttpClient::new(&options.http)?))
                            .clone(),
                    };
                    let source = crate::http::HttpSource::new(client, url);
                    sources.push(Source::Stream(Arc::new(source)));
                }
                #[cfg(not(feature = "http"))]
                return Err(crate::Error::InvalidUrl(format!(
                    "{input}: xeibe-io was built without the `http` feature"
                )));
            }
            _ => sources.extend(object_store_sources(input, &url, options)?),
        }
    }
    Ok(sources)
}

#[cfg(feature = "object-store")]
fn object_store_sources(input: &str, url: &Url, options: &IoOptions) -> crate::Result<Vec<Source>> {
    // The store is made for the bucket alone; the key, which may be a glob, is
    // handled by `store_sources`.
    let mut root = url.clone();
    root.set_path("/");
    root.set_query(None);
    root.set_fragment(None);
    let (store, _) = object_store::parse_url_opts(&root, std::env::vars())
        .map_err(|e| crate::Error::InvalidUrl(format!("{input}: {e}")))?;
    // The key as written, not as `url` parsed it: `?` and `{` are glob characters
    // here, and keys in `s3://` URIs are not percent-encoded.
    let authority_and_key = &input[input.find("://").map_or(0, |i| i + 3)..];
    let key = authority_and_key.split_once('/').map_or("", |(_, key)| key);
    let base = root.as_str().trim_end_matches('/');
    let runtime = crate::object_store::runtime()?;
    crate::object_store::store_sources(store.into(), base, key, &runtime, options)
}

#[cfg(not(feature = "object-store"))]
fn object_store_sources(input: &str, url: &Url, _: &IoOptions) -> crate::Result<Vec<Source>> {
    Err(crate::Error::InvalidUrl(format!(
        "{input}: `{}://` needs xeibe-io's `object-store` feature",
        url.scheme()
    )))
}
