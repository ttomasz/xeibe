//! WebAssembly bindings for the reproject page (`docs/web-reproject.md`):
//! EPSG CRSs, the conversion of pasted points, and what the operations
//! between datums are.
//!
//! Kept thin, as `xeibe-wasm` is: JSON text and bytes cross, nothing else.
//! The work is in [`page`], which is tested natively. Everything runs on
//! the page's main thread.

pub mod page;

use wasm_bindgen::prelude::*;

fn error(message: impl std::fmt::Display) -> JsError {
    JsError::new(&message.to_string())
}

/// JSON: the card of the CRS `code` names (`2180` or `EPSG:2180`), with
/// `error` when it names none or one that can't be converted.
#[wasm_bindgen(js_name = crsCard)]
pub fn crs_card(code: &str) -> String {
    page::crs_card(code).to_string()
}

/// JSON: west, south, east, north of the points (JSON, as for
/// [`Session::convert`]) in degrees on the CRS `src`'s datum, or `null`.
#[wasm_bindgen(js_name = lonlatBounds)]
pub fn lonlat_bounds(src: &str, east_first: bool, points: &str) -> Result<String, JsError> {
    let src = page::parse_code(src).map_err(error)?;
    let points = page::parse_points(points).map_err(error)?;
    serde_json::to_string(&page::lonlat_bounds(src, east_first, &points)).map_err(error)
}

/// The EPSG version of the tables.
#[wasm_bindgen(js_name = epsgVersion)]
pub fn epsg_version() -> String {
    xeibe_crs::EPSG_VERSION.to_string()
}

/// A conversion from one EPSG CRS to another.
#[wasm_bindgen]
pub struct Session(page::Session);

#[wasm_bindgen]
impl Session {
    /// `options` is JSON: `{"east_first", "area_of_interest", "accuracy",
    /// "operation"}`, any of them left out.
    #[wasm_bindgen(constructor)]
    pub fn new(src: &str, dst: &str, options: &str) -> Result<Session, JsError> {
        let options = page::Options::from_json(options).map_err(error)?;
        Ok(Session(
            page::Session::new(src, dst, &options).map_err(error)?,
        ))
    }

    /// JSON: the candidate operations, best first, with their steps.
    pub fn info(&self) -> String {
        self.0.info().to_string()
    }

    /// JSON: the grid ranges to fetch before converting `points`.
    #[wasm_bindgen(js_name = gridRequests)]
    pub fn grid_requests(&self, points: &str) -> Result<String, JsError> {
        let points = page::parse_points(points).map_err(error)?;
        let requests: Vec<_> = self
            .0
            .grid_requests(&points)
            .iter()
            .map(page::grid_request_json)
            .collect();
        Ok(serde_json::Value::Array(requests).to_string())
    }

    /// Hand over a fetched range of a grid file.
    #[wasm_bindgen(js_name = addGridData)]
    pub fn add_grid_data(&self, file: &str, offset: f64, data: Vec<u8>) -> Result<(), JsError> {
        self.0
            .add_grid_data(file, offset as u64, data)
            .map_err(error)
    }

    /// A grid file couldn't be fetched, for `reason`.
    #[wasm_bindgen(js_name = gridFailed)]
    pub fn grid_failed(&self, file: &str, reason: &str) {
        self.0.grid_failed(file, reason);
    }

    /// JSON: each line's result. `points` is JSON, one entry per line:
    /// `null`, `[x, y]` or `[x, y, z]`.
    pub fn convert(&self, points: &str) -> Result<String, JsError> {
        let points = page::parse_points(points).map_err(error)?;
        Ok(self.0.convert(&points).to_string())
    }

    /// JSON: everything about one point.
    pub fn details(&self, x: f64, y: f64, z: Option<f64>) -> String {
        self.0.details((x, y, z)).to_string()
    }
}
