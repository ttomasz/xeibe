//! Grids for an export to another CRS, fetched from `cdn.proj.org` while the
//! read asks for them. The read's loader is synchronous, so the fetch is a
//! synchronous `XMLHttpRequest`, which only a worker may make; the page runs
//! the module in one.

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{XmlHttpRequest, XmlHttpRequestResponseType};
use xeibe_arrow::GridLoader;
use xeibe_reproject::GridRequest;

/// One range of a grid file a request at a time, as `fetchRange` in the
/// worker fetches the map's.
pub fn cdn() -> GridLoader {
    GridLoader::new(|request| fetch(request).map_err(|e| format!("{}: {e}", request.file)))
}

fn fetch(request: &GridRequest) -> Result<Vec<u8>, String> {
    let js = |e: JsValue| {
        e.dyn_ref::<js_sys::Error>()
            .map_or_else(|| format!("{e:?}"), |e| String::from(e.message()))
    };
    let xhr = XmlHttpRequest::new().map_err(js)?;
    xhr.open_with_async("GET", &request.url(), false)
        .map_err(js)?;
    let end = request.length.map_or(String::new(), |length| {
        (request.offset + length - 1).to_string()
    });
    xhr.set_request_header("Range", &format!("bytes={}-{end}", request.offset))
        .map_err(js)?;
    xhr.set_response_type(XmlHttpRequestResponseType::Arraybuffer);
    xhr.send().map_err(js)?;
    let status = xhr.status().map_err(js)?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    let mut bytes = js_sys::Uint8Array::new(&xhr.response().map_err(js)?).to_vec();
    // A server that ignores Range sends the whole file.
    if status == 200 && request.offset > 0 {
        let start = (request.offset as usize).min(bytes.len());
        let end = request.length.map_or(bytes.len(), |length| {
            (start + length as usize).min(bytes.len())
        });
        bytes = bytes[start..end].to_vec();
    }
    Ok(bytes)
}
