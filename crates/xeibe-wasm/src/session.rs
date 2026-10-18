//! What the page does with xeibe, in plain Rust: files held in memory, a
//! scan, and a read whose batches are kept for the table and laid out for
//! the map. `lib.rs` only wraps this for JavaScript, so it is tested
//! natively.

use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::{Array, ArrayRef, RecordBatch, RecordBatchReader};
use arrow_cast::display::{ArrayFormatter, FormatOptions};
use arrow_schema::{DataType, Field, TimeUnit};
use serde_json::{Value, json};
use xeibe_arrow::report::Warning;
use xeibe_arrow::{LayerReader, ReadOptions, ReadReport, ScanResult};
use xeibe_core::archive::{self, Archive};
use xeibe_core::{Location, RandomAccess, RandomAccessSource, Source};
use xeibe_geom::{AxisDecision, AxisKey, SrsName};
use xeibe_reproject::{AxisOrder, Crs, GridRequest, Grids, Transform};
use xeibe_schema::ScanExtent;
use xeibe_schema::rules::meta;

use crate::display::{Drawing, Parts, draw};

/// The longest text a table cell shows.
const MAX_CELL: usize = 300;

pub type Result<T> = std::result::Result<T, String>;

/// The files dropped on the page, as sources (a zip archive is its
/// members), and the last scan of them.
#[derive(Default)]
pub struct Dataset {
    sources: Vec<Source>,
    /// The files' sizes, for progress.
    sizes: u64,
    scan: Option<ScanResult>,
}

impl Dataset {
    /// Add a file held in memory: see [`Dataset::add_data`].
    pub fn add_file(&mut self, name: String, bytes: Vec<u8>) -> Result<()> {
        self.add_data(name, Arc::new(bytes::Bytes::from(bytes)))
    }

    /// Add a file, read in pieces as it is needed. A zip archive adds its
    /// GML members (`name!/member`), as a local zip does in the CLI; one
    /// without any, or with one xeibe can't read, is an error.
    pub fn add_data(&mut self, name: String, data: Arc<dyn RandomAccess>) -> Result<()> {
        self.scan = None;
        self.sizes += data.len();
        if name.to_ascii_lowercase().ends_with(".zip") {
            let archive = Archive::Random { name, data };
            let members = archive::expand(archive, &[]).map_err(|e| e.to_string())?;
            self.sources.extend(members);
        } else {
            self.sources
                .push(Source::Stream(Arc::new(RandomAccessSource { name, data })));
        }
        Ok(())
    }

    /// The total size of the files, for progress.
    pub fn size(&self) -> u64 {
        self.sizes
    }

    fn sources(&self) -> Vec<Source> {
        self.sources.clone()
    }

    /// Source names, by `SourceId`.
    fn names(&self) -> Vec<String> {
        self.sources.iter().map(Source::name).collect()
    }

    /// Scan every file in full. Returns the Overview's JSON.
    pub fn scan(&mut self, options: Option<&str>) -> Result<Value> {
        let options = parse_options(options)?;
        let scan = xeibe_arrow::scan(self.sources(), ScanExtent::Full, &options)
            .map_err(|e| e.to_string())?;
        let json = scan_json(&scan, &self.names());
        self.scan = Some(scan);
        Ok(json)
    }

    /// One layer's columns as the last scan inferred them, with the
    /// reasoning (`xeibe gml scan --explain`).
    pub fn schema(&self, layer: &str) -> Result<Value> {
        let scan = self.scan.as_ref().ok_or("no scan yet")?;
        let schema = scan.arrow_schema(layer).map_err(|e| e.to_string())?;
        let explain = scan.explain(layer).map_err(|e| e.to_string())?;
        Ok(json!({
            "columns": schema.fields().iter().map(|f| column_json(f)).collect::<Vec<_>>(),
            "explain": explain,
        }))
    }

    /// Start reading `layer`, at most `limit` rows.
    pub fn read(&self, layer: &str, options: Option<&str>, limit: Option<u64>) -> Result<Reader> {
        let options = parse_options(options)?;
        let reader =
            xeibe_arrow::read(self.sources(), layer, None, &options).map_err(|e| e.to_string())?;
        Ok(Reader::new(reader, &options, limit, self.names()))
    }
}

fn parse_options(options: Option<&str>) -> Result<ReadOptions> {
    match options {
        None => Ok(ReadOptions::default()),
        Some(json) => serde_json::from_str(json).map_err(|e| format!("options: {e}")),
    }
}

/// The column drawn on the map: the primary geometry column, or the first.
fn drawn_column(schema: &arrow_schema::Schema, options: &ReadOptions) -> Option<usize> {
    let geometry: Vec<usize> = (0..schema.fields().len())
        .filter(|&i| geometry_kind(schema.field(i)).is_some())
        .collect();
    let primary = options.geometry.primary.as_deref();
    geometry
        .iter()
        .copied()
        .find(|&i| Some(schema.field(i).name().as_str()) == primary)
        .or(geometry.first().copied())
}

/// `geoarrow.multipolygon` and the like; `None` for other columns and for
/// boxes, which have nothing to draw.
fn geometry_kind(field: &Field) -> Option<&str> {
    field
        .metadata()
        .get("ARROW:extension:name")
        .map(String::as_str)
        .filter(|name| name.starts_with("geoarrow.") && *name != "geoarrow.box")
}

/// A read in progress: batches pulled one at a time, each laid out for the
/// map once the grids its area needs are loaded.
pub struct Reader {
    reader: LayerReader,
    files: Vec<String>,
    limit: Option<u64>,
    /// The batches read so far, for the table.
    batches: Vec<RecordBatch>,
    rows: u64,
    column: Option<usize>,
    map: MapSetup,
    origin: Option<[f64; 2]>,
    /// A batch read but not yet laid out.
    pending: Option<(RecordBatch, Parts)>,
    not_on_map: u64,
    map_error: Option<String>,
    geometry_errors: Vec<(u64, String)>,
}

impl Reader {
    fn new(
        reader: LayerReader,
        options: &ReadOptions,
        limit: Option<u64>,
        files: Vec<String>,
    ) -> Self {
        let schema = reader.schema();
        let column = drawn_column(&schema, options);
        let srs = column.and_then(|i| schema.field(i).metadata().get(meta::SRS_NAME).cloned());
        let map = match column {
            Some(_) => MapSetup::new(srs.as_deref(), Grids::ProjData),
            None => MapSetup::none("the layer has no geometry column"),
        };
        Reader {
            reader,
            files,
            limit,
            batches: Vec::new(),
            rows: 0,
            column,
            map,
            origin: None,
            pending: None,
            not_on_map: 0,
            map_error: None,
            geometry_errors: Vec::new(),
        }
    }

    /// The columns, the column drawn and how the map shows it.
    pub fn info(&self) -> Value {
        let schema = self.reader.schema();
        json!({
            "columns": schema.fields().iter().map(|f| column_json(f)).collect::<Vec<_>>(),
            "geometry_column": self.column.map(|i| schema.field(i).name().clone()),
            "map": self.map.json(),
        })
    }

    /// Read the next batch. `false` at the end of the layer, or once
    /// `limit` rows are read.
    pub fn next_batch(&mut self) -> Result<bool> {
        if self.pending.is_some() {
            return Err("the batch read last hasn't been taken".into());
        }
        let left = self.limit.map(|limit| limit.saturating_sub(self.rows));
        if left == Some(0) {
            return Ok(false);
        }
        let Some(batch) = self.reader.next_batch() else {
            return Ok(false);
        };
        let mut batch = batch.map_err(|e| e.to_string())?;
        if let Some(left) = left
            && (batch.num_rows() as u64) > left
        {
            batch = batch.slice(0, left as usize);
        }
        let parts = match self.column {
            Some(column) => Parts::decode(&batch, column)?,
            None => Parts {
                rows: vec![Vec::new(); batch.num_rows()],
                errors: Vec::new(),
            },
        };
        if self.origin.is_none() {
            self.origin = parts.first_position();
        }
        self.pending = Some((batch, parts));
        Ok(true)
    }

    /// The grid data the map still needs for the batch just read: fetch each
    /// range, hand it to [`Reader::add_grid_data`], and ask again until this
    /// is empty.
    pub fn grid_requests(&self) -> Vec<GridRequest> {
        match (&self.map.transform, &self.pending) {
            (Some(transform), Some((_, parts))) => match parts.bounds() {
                Some(bounds) => transform.grid_requests(Some(bounds)),
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    pub fn add_grid_data(&mut self, file: &str, offset: u64, data: Vec<u8>) -> Result<()> {
        let transform = self.map.transform.as_ref().ok_or("no map transform")?;
        transform
            .add_grid_data(file, offset, data)
            .map_err(|e| e.to_string())
    }

    /// Give up on grids (one couldn't be fetched): the map converts with
    /// EPSG's operations without grids from now on, and says so.
    pub fn without_grids(&mut self, reason: &str) {
        let srs = self.map.srs_name.clone();
        let mut setup = MapSetup::new(srs.as_deref(), Grids::None);
        setup.notes.push(format!("shown without grids: {reason}"));
        self.map = setup;
    }

    /// Lay out the batch just read, and keep it for the table.
    pub fn take(&mut self) -> Result<Drawing> {
        let (batch, parts) = self.pending.take().ok_or("no batch read")?;
        let first_row = u32::try_from(self.rows).map_err(|_| "more than 2^32 rows")?;
        let drawing = draw(
            &parts,
            first_row,
            self.origin.unwrap_or([0.0, 0.0]),
            self.map.transform.as_ref(),
        );
        self.not_on_map += drawing.not_on_map.len() as u64;
        if self.map_error.is_none() {
            self.map_error = drawing.map_error.clone();
        }
        for (row, error) in parts.errors {
            if self.geometry_errors.len() < 100 {
                self.geometry_errors.push((self.rows + row as u64, error));
            }
        }
        self.rows += batch.num_rows() as u64;
        self.batches.push(batch);
        Ok(drawing)
    }

    /// The source space's origin: subtracted from its coordinates.
    pub fn origin(&self) -> Option<[f64; 2]> {
        self.origin
    }

    pub fn rows_read(&self) -> u64 {
        self.rows
    }

    /// Rows `start..start + count` as text, one list of cells per row.
    pub fn rows(&self, start: u64, count: u64) -> Vec<Vec<String>> {
        let options = FormatOptions::default().with_null("");
        let mut out = Vec::new();
        let mut first = 0u64;
        let end = start + count;
        for batch in &self.batches {
            let n = batch.num_rows() as u64;
            if first + n > start && first < end {
                let from = start.saturating_sub(first) as usize;
                let to = (end - first).min(n) as usize;
                let schema = batch.schema_ref();
                let columns: Vec<Column> = batch
                    .columns()
                    .iter()
                    .zip(schema.fields())
                    .map(|(array, field)| Column::new(array.as_ref(), field, &options))
                    .collect();
                for row in from..to {
                    out.push(columns.iter().map(|c| c.cell(row)).collect());
                }
            }
            first += n;
            if first >= end {
                break;
            }
        }
        out
    }

    /// The read report, and what the map couldn't show.
    pub fn report(&self) -> Value {
        let mut json = report_json(&self.reader.report(), &self.files);
        json["rows"] = json!(self.rows);
        json["not_on_map"] = json!(self.not_on_map);
        json["map_error"] = json!(self.map_error);
        json["geometry_errors"] = self
            .geometry_errors
            .iter()
            .map(|(row, error)| json!({"row": row, "message": error}))
            .collect();
        json
    }
}

/// A column formatted for the table.
enum Column<'a> {
    Geometry(Option<Vec<Option<Vec<u8>>>>),
    Other(Option<ArrayFormatter<'a>>),
    /// A timestamp with a time zone, as a UTC instant (`…Z`): the array
    /// without its zone.
    Utc(Option<ArrayRef>),
}

impl<'a> Column<'a> {
    fn new(array: &'a dyn Array, field: &Field, options: &FormatOptions<'a>) -> Self {
        if geometry_kind(field).is_some() {
            return Column::Geometry(crate::display::column_wkb(array, field).ok());
        }
        if let DataType::Timestamp(_, Some(_)) = array.data_type() {
            return Column::Utc(without_zone(array));
        }
        Column::Other(ArrayFormatter::try_new(array, options).ok())
    }

    fn cell(&self, row: usize) -> String {
        match self {
            Column::Geometry(Some(values)) => match &values[row] {
                None => String::new(),
                Some(bytes) => match xeibe_geom::wkb::read_wkb(bytes) {
                    Ok(geometry) => crate::wkt::short_wkt(&geometry, MAX_CELL),
                    Err(error) => format!("(invalid WKB: {error})"),
                },
            },
            Column::Utc(Some(array)) => {
                let options = FormatOptions::default().with_null("");
                match ArrayFormatter::try_new(array.as_ref(), &options) {
                    Ok(formatter) if array.is_valid(row) => format!("{}Z", formatter.value(row)),
                    Ok(_) => String::new(),
                    Err(_) => "(can't show)".into(),
                }
            }
            Column::Other(Some(formatter)) => {
                let mut text = formatter.value(row).to_string();
                if text.len() > MAX_CELL {
                    let mut end = MAX_CELL;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                    text.push('…');
                }
                text
            }
            _ => "(can't show)".into(),
        }
    }
}

/// A timestamp array with a time zone, without the zone: xeibe's are UTC
/// instants (`timestamptz`), and formatting a named zone would need Arrow's
/// time-zone database in the page.
fn without_zone(array: &dyn Array) -> Option<ArrayRef> {
    use arrow_array::types::{
        TimestampMicrosecondType, TimestampMillisecondType, TimestampNanosecondType,
        TimestampSecondType,
    };
    let naive: ArrayRef = match array.data_type() {
        DataType::Timestamp(TimeUnit::Second, _) => Arc::new(
            array
                .as_primitive::<TimestampSecondType>()
                .clone()
                .with_timezone_opt(None::<String>),
        ),
        DataType::Timestamp(TimeUnit::Millisecond, _) => Arc::new(
            array
                .as_primitive::<TimestampMillisecondType>()
                .clone()
                .with_timezone_opt(None::<String>),
        ),
        DataType::Timestamp(TimeUnit::Microsecond, _) => Arc::new(
            array
                .as_primitive::<TimestampMicrosecondType>()
                .clone()
                .with_timezone_opt(None::<String>),
        ),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => Arc::new(
            array
                .as_primitive::<TimestampNanosecondType>()
                .clone()
                .with_timezone_opt(None::<String>),
        ),
        _ => return None,
    };
    Some(naive)
}

/// A CRS's area of use as a ring: in longitude/latitude, and in the source
/// CRS.
type Area = (Vec<[f64; 2]>, Vec<[f64; 2]>);

/// How the map converts the drawn column's CRS.
struct MapSetup {
    srs_name: Option<String>,
    /// To longitude/latitude; `None` when the CRS can't be converted.
    transform: Option<Transform>,
    /// What the map is in: `EPSG:4326`, or the CRS's own geographic CRS.
    target: Option<String>,
    crs: Value,
    /// The CRS's area of use: in longitude/latitude, and in the source CRS.
    area: Option<Area>,
    notes: Vec<String>,
}

impl MapSetup {
    fn none(note: &str) -> Self {
        MapSetup {
            srs_name: None,
            transform: None,
            target: None,
            crs: Value::Null,
            area: None,
            notes: vec![note.into()],
        }
    }

    fn new(srs_name: Option<&str>, grids: Grids) -> Self {
        let mut setup = MapSetup {
            srs_name: srs_name.map(str::to_string),
            transform: None,
            target: None,
            crs: srs_name.map_or(Value::Null, crs_json),
            area: None,
            notes: Vec::new(),
        };
        let Some(srs_name) = srs_name else {
            setup
                .notes
                .push("no CRS: the map shows the coordinates as read".into());
            return setup;
        };
        let parsed = SrsName::parse(srs_name);
        let Some(crs) = parsed.crs else {
            setup.notes.push(format!(
                "{srs_name} names no known CRS: the map shows the coordinates as read"
            ));
            return setup;
        };
        let horizontal = crs.horizontal();
        let source = if horizontal.is_lon_lat_by_definition() {
            Crs::from_epsg(4326)
        } else {
            match horizontal.projjson() {
                Some(json) => Crs::from_projjson(&json.to_string()),
                None => {
                    setup.notes.push(format!(
                        "{} isn't in the EPSG tables: the map shows the coordinates as read",
                        horizontal.authority_code()
                    ));
                    return setup;
                }
            }
        };
        let source = match source {
            Ok(source) => source,
            Err(error) => {
                setup.notes.push(format!("{srs_name}: {error}"));
                return setup;
            }
        };
        let wgs84 = Crs::from_epsg(4326).expect("EPSG:4326 is in the tables");
        match Transform::with_grids(&source, &wgs84, AxisOrder::EastFirst, grids) {
            Ok(transform) => {
                setup.transform = Some(transform);
                setup.target = Some("EPSG:4326".into());
            }
            Err(to_wgs84) => match base_geographic(horizontal)
                .and_then(|base| Transform::new(&source, &base, AxisOrder::EastFirst).ok())
            {
                Some(transform) => {
                    setup.notes.push(format!(
                        "no transformation to WGS 84 ({to_wgs84}): shown in {}, on the data's own datum",
                        transform.target().name()
                    ));
                    setup.target = Some(transform.target().name().to_string());
                    setup.transform = Some(transform);
                }
                None => setup.notes.push(format!(
                    "can't be converted to longitude/latitude ({to_wgs84}): the map shows the coordinates as read"
                )),
            },
        }
        setup.area = setup.area_of_use(horizontal);
        setup
    }

    /// EPSG's area of use, densified, on the map and in the source CRS
    /// (converted without grids: it only frames the data).
    fn area_of_use(&self, crs: &xeibe_geom::CrsRef) -> Option<Area> {
        let transform = self.transform.as_ref()?;
        let code = epsg_code(crs)?;
        let [south, west, north, east] = xeibe_crs::get(code)?.area_wgs84?.map(f64::from);
        const STEPS: usize = 32;
        let mut ring = Vec::with_capacity(4 * STEPS + 1);
        let edge = |from: [f64; 2], to: [f64; 2], ring: &mut Vec<[f64; 2]>| {
            for k in 0..STEPS {
                let t = k as f64 / STEPS as f64;
                ring.push([
                    from[0] + (to[0] - from[0]) * t,
                    from[1] + (to[1] - from[1]) * t,
                ]);
            }
        };
        edge([west, south], [east, south], &mut ring);
        edge([east, south], [east, north], &mut ring);
        edge([east, north], [west, north], &mut ring);
        edge([west, north], [west, south], &mut ring);
        ring.push([west, south]);
        let wgs84 = Crs::from_epsg(4326).ok()?;
        let back = Transform::with_grids(
            &wgs84,
            transform.source(),
            AxisOrder::EastFirst,
            Grids::None,
        )
        .ok()?;
        let source = ring
            .iter()
            .filter_map(|&[lon, lat]| back.convert(lon, lat, None).ok().map(|(x, y, _)| [x, y]))
            .collect();
        Some((ring, source))
    }

    fn json(&self) -> Value {
        json!({
            "srs_name": self.srs_name,
            "crs": self.crs,
            "target": self.target,
            "notes": self.notes,
            "area_of_use": self.area.as_ref().map(|(map, source)| json!({"map": map, "source": source})),
        })
    }
}

fn epsg_code(crs: &xeibe_geom::CrsRef) -> Option<u32> {
    match crs.horizontal() {
        xeibe_geom::CrsRef::Code { authority, code } if authority.eq_ignore_ascii_case("EPSG") => {
            code.parse().ok()
        }
        _ => None,
    }
}

/// The geographic CRS a projected CRS is based on (itself for a geographic
/// one), from its PROJJSON.
fn base_geographic(crs: &xeibe_geom::CrsRef) -> Option<Crs> {
    let json = crs.projjson()?;
    let base = match json["type"].as_str()? {
        "GeographicCRS" => json,
        "ProjectedCRS" => {
            let mut base = json.get("base_crs")?.clone();
            base.as_object_mut()?
                .entry("type")
                .or_insert_with(|| json!("GeographicCRS"));
            base
        }
        _ => return None,
    };
    Crs::from_projjson(&base.to_string()).ok()
}

/// An srsName with what the EPSG tables say about its CRS.
fn crs_json(srs_name: &str) -> Value {
    let parsed = SrsName::parse(srs_name);
    let code = parsed.crs.as_ref().and_then(epsg_code);
    let record = code.and_then(xeibe_crs::get);
    json!({
        "srs_name": srs_name,
        "crs": parsed.crs.as_ref().map(|c| c.authority_code()),
        "name": record.map(|r| r.name),
        "kind": record.map(|r| format!("{:?}", r.kind)),
        "deprecated": record.map(|r| r.deprecated),
        "area_wgs84": record.and_then(|r| r.area_wgs84),
    })
}

fn column_json(field: &Field) -> Value {
    let metadata = field.metadata();
    json!({
        "name": field.name(),
        "type": geometry_kind(field).map_or_else(|| field.data_type().to_string(), str::to_string),
        "nullable": field.is_nullable(),
        "path": metadata.get(meta::PATH),
        "srs_name": metadata.get(meta::SRS_NAME),
        "axis_decision": metadata.get(meta::AXIS_DECISION),
    })
}

fn scan_json(scan: &ScanResult, files: &[String]) -> Value {
    let layers: Vec<Value> = scan
        .layers()
        .into_iter()
        .map(|layer| {
            json!({
                "name": &*layer.name.local,
                "qname": layer.name.to_clark(),
                "feature_count": layer.feature_count,
                "geometry_columns": layer.geometry_columns,
                "crs": layer.crs.iter().map(|srs| crs_json(srs)).collect::<Vec<_>>(),
                "extent": layer.extent,
            })
        })
        .collect();
    let axis: Vec<Value> = scan
        .axis_decisions()
        .into_iter()
        .map(|(layer, key, decision)| {
            let mut json = axis_json(&key, &decision, files);
            json["layer"] = json!(&*layer.local);
            json
        })
        .collect();
    json!({
        "files": files,
        "complete": scan.is_complete(),
        "skipped_sources": scan.skipped_sources(),
        "extent": scan.extent(),
        "layers": layers,
        "axis_decisions": axis,
    })
}

fn axis_json(key: &AxisKey, decision: &AxisDecision, files: &[String]) -> Value {
    json!({
        "source": files.get(key.source.0 as usize),
        "srs_name": key.srs_name,
        "dialect": format!("{:?}", key.dialect),
        "swap": decision.swap,
        "reason": decision.reason,
        "conflicts": decision.conflicts,
    })
}

fn location_json(location: &Location, files: &[String]) -> Value {
    json!({
        "source": files.get(location.source.0 as usize),
        "byte": location.byte_offset,
        "feature": location.feature_seq,
        "gml_id": location.gml_id,
    })
}

fn report_json(report: &ReadReport, files: &[String]) -> Value {
    let warning = |w: &Warning| {
        json!({
            "kind": format!("{:?}", w.kind),
            "location": w.location.as_ref().map(|l| location_json(l, files)),
            "message": w.message,
        })
    };
    json!({
        "features_per_layer": report.features_per_layer,
        "skipped": report.skipped.iter().map(|(location, message)| json!({
            "location": location_json(location, files),
            "message": message,
        })).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(warning).collect::<Vec<_>>(),
        "axis_decisions": report.axis_decisions.iter()
            .map(|(key, decision)| axis_json(key, decision, files))
            .collect::<Vec<_>>(),
    })
}
