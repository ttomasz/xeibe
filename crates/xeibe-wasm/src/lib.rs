//! WebAssembly bindings for the web page (`docs/web.md`): files held in
//! memory, a scan, and a read whose batches go to the table as text and to
//! the map as flat buffers.
//!
//! Kept thin so that the unstable Rust API doesn't spread into JavaScript:
//! JSON text and typed arrays cross, nothing else. The work is in
//! [`session`] and [`display`], which are tested natively.
//!
//! Everything runs on the calling thread (`threads = 1`), in the page's Web
//! Worker.

pub mod display;
pub mod session;
mod wkt;

use wasm_bindgen::prelude::*;

fn error(message: impl std::fmt::Display) -> JsError {
    JsError::new(&message.to_string())
}

/// The files dropped on the page.
#[wasm_bindgen]
#[derive(Default)]
pub struct Dataset(session::Dataset);

#[wasm_bindgen]
impl Dataset {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Dataset {
        Dataset::default()
    }

    /// Add a file (`.gml`, `.xml`, `.gz`, `.zst`, or a `.zip` of them) by
    /// name and content.
    #[wasm_bindgen(js_name = addFile)]
    pub fn add_file(&mut self, name: String, bytes: Vec<u8>) -> Result<(), JsError> {
        self.0.add_file(name, bytes).map_err(error)
    }

    /// Scan every file in full; `options` is the JSON of the read options.
    /// Returns JSON: layers, axis decisions, skipped sources, extent.
    pub fn scan(&mut self, options: Option<String>) -> Result<String, JsError> {
        Ok(self.0.scan(options.as_deref()).map_err(error)?.to_string())
    }

    /// JSON: the columns the scan inferred for `layer`, and its reasoning.
    pub fn schema(&self, layer: &str) -> Result<String, JsError> {
        Ok(self.0.schema(layer).map_err(error)?.to_string())
    }

    /// Start reading `layer`, at most `limit` rows.
    pub fn read(
        &self,
        layer: &str,
        options: Option<String>,
        limit: Option<f64>,
    ) -> Result<Reader, JsError> {
        let limit = limit.map(|n| n.max(0.0) as u64);
        Ok(Reader(
            self.0
                .read(layer, options.as_deref(), limit)
                .map_err(error)?,
        ))
    }
}

/// A read in progress. For each batch: `next()`, then the grid loop
/// (`gridRequests()`, `fetch`, `addGridData()`, until no request is left),
/// then `take()`.
#[wasm_bindgen]
pub struct Reader(session::Reader);

#[wasm_bindgen]
impl Reader {
    /// JSON: the columns, the column drawn and how the map shows it.
    pub fn info(&self) -> String {
        self.0.info().to_string()
    }

    /// Read the next batch; `false` at the end.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<bool, JsError> {
        self.0.next_batch().map_err(error)
    }

    /// JSON: `[{file, url, offset, length}]`, the grid byte ranges the map
    /// still needs for the batch just read. `length` is `null` for the rest
    /// of the file.
    #[wasm_bindgen(js_name = gridRequests)]
    pub fn grid_requests(&self) -> String {
        let requests: Vec<serde_json::Value> = self
            .0
            .grid_requests()
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "url": r.url(),
                    "file": r.file,
                    "offset": r.offset,
                    "length": r.length,
                })
            })
            .collect();
        serde_json::Value::from(requests).to_string()
    }

    #[wasm_bindgen(js_name = addGridData)]
    pub fn add_grid_data(&mut self, file: &str, offset: f64, data: Vec<u8>) -> Result<(), JsError> {
        self.0
            .add_grid_data(file, offset as u64, data)
            .map_err(error)
    }

    /// A grid couldn't be fetched: draw without grids from now on.
    #[wasm_bindgen(js_name = withoutGrids)]
    pub fn without_grids(&mut self, reason: &str) {
        self.0.without_grids(reason);
    }

    /// Lay out the batch just read for the map, and keep it for the table.
    pub fn take(&mut self) -> Result<Drawing, JsError> {
        Ok(Drawing(self.0.take().map_err(error)?))
    }

    /// `[x, y]` subtracted from the source space's coordinates.
    pub fn origin(&self) -> Option<Vec<f64>> {
        self.0.origin().map(|o| o.to_vec())
    }

    /// JSON: rows `start..start + count` as lists of cell texts.
    pub fn rows(&self, start: f64, count: f64) -> String {
        serde_json::to_string(&self.0.rows(start as u64, count as u64)).expect("strings serialize")
    }

    /// JSON: the read report so far.
    pub fn report(&self) -> String {
        self.0.report().to_string()
    }
}

/// One batch laid out for the map: see [`display::Buffers`].
#[wasm_bindgen]
pub struct Drawing(display::Drawing);

#[wasm_bindgen]
impl Drawing {
    #[wasm_bindgen(js_name = hasMap)]
    pub fn has_map(&self) -> bool {
        self.0.map.is_some()
    }

    /// Positions and bounds: `points`, `lines`, `polygons`, `bounds`, in the
    /// map's space (`map = true`) or the source's.
    pub fn floats(&self, map: bool, name: &str) -> Result<Vec<f64>, JsError> {
        let b = self.buffers(map)?;
        Ok(match name {
            "points" => b.points.clone(),
            "lines" => b.lines.clone(),
            "polygons" => b.polygons.clone(),
            "bounds" => b.bounds.clone(),
            _ => return Err(error(format!("no float buffer {name}"))),
        })
    }

    /// Offsets and rows: `point_rows`, `line_starts`, `line_rows`,
    /// `polygon_starts`, `polygon_rows`, `triangles`, `ring_starts`,
    /// `ring_rows`.
    pub fn ints(&self, map: bool, name: &str) -> Result<Vec<u32>, JsError> {
        let b = self.buffers(map)?;
        Ok(match name {
            "point_rows" => b.point_rows.clone(),
            "line_starts" => b.line_starts.clone(),
            "line_rows" => b.line_rows.clone(),
            "polygon_starts" => b.polygon_starts.clone(),
            "polygon_rows" => b.polygon_rows.clone(),
            "triangles" => b.triangles.clone(),
            "ring_starts" => b.ring_starts.clone(),
            "ring_rows" => b.ring_rows.clone(),
            _ => return Err(error(format!("no index buffer {name}"))),
        })
    }

    /// Rows left off the map: a coordinate didn't convert.
    #[wasm_bindgen(js_name = notOnMap)]
    pub fn not_on_map(&self) -> Vec<u32> {
        self.0.not_on_map.clone()
    }
}

impl Drawing {
    fn buffers(&self, map: bool) -> Result<&display::Buffers, JsError> {
        if map {
            self.0
                .map
                .as_ref()
                .ok_or_else(|| error("no map conversion"))
        } else {
            Ok(&self.0.source)
        }
    }
}
