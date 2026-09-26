//! Geometry columns for Parquet output (`docs/geometry.md`, "Parquet and
//! GeoParquet output"): every top-level geometry column is written as WKB with
//! the native `GEOMETRY` logical type (CRS as `authority:code`, `srid:0` when
//! unknown), plus GeoParquet 1.1 `geo` metadata with the CRS as PROJJSON or
//! `null`. Curves and other non-simple types are an error: neither standard
//! has them. A geometry column can get a GeoParquet 1.1 `bbox` covering column
//! (`--bbox-column`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use arrow_array::{Array, ArrayRef, BinaryArray, Float64Array, RecordBatch, StructArray, cast::AsArray};
use arrow_buffer::NullBuffer;
use arrow_schema::{DataType, Field, Fields, Schema, SchemaRef};
use geoarrow_array::GeoArrowArray;
use geoarrow_schema::{CrsType, GeoArrowType};
use serde_json::{Value, json};
use xeibe_geom::{CrsRef, SrsName};

use crate::args::BboxColumn;

/// Top-level geometry columns of a layer and what has been written of them.
pub(crate) struct GeoColumns {
    layer: String,
    columns: Vec<GeoColumn>,
    primary: Option<String>,
    /// The schema handed to the Parquet writer: geometry columns as WKB.
    output: SchemaRef,
}

struct GeoColumn {
    index: usize,
    name: String,
    /// GeoParquet `crs`: PROJJSON, or `null` when unknown or not expressible.
    projjson: Value,
    types: BTreeSet<&'static str>,
    bbox: Option<[f64; 4]>,
    /// The name of its `bbox` covering column, which follows it.
    covering: Option<String>,
}

/// The fields of a `bbox` covering column, in the order GeoParquet requires.
const BBOX_FIELDS: [&str; 4] = ["xmin", "ymin", "xmax", "ymax"];

fn bbox_fields() -> Fields {
    BBOX_FIELDS.iter().map(|name| Field::new(*name, DataType::Float64, false)).collect()
}

impl GeoColumns {
    /// `primary` is the `primary` geometry option: a column name; else the first
    /// geometry column is the primary one. `first`: the first batch, which
    /// tells `BboxColumn::Auto` whether a WKB column holds points.
    pub(crate) fn new(
        layer: &str,
        schema: &Schema,
        primary: Option<&str>,
        bbox: BboxColumn,
        first: Option<&RecordBatch>,
    ) -> super::Result<Self> {
        let mut columns = Vec::new();
        let mut fields = Vec::new();
        let mut names: HashSet<String> = schema.fields().iter().map(|field| field.name().clone()).collect();
        for (index, field) in schema.fields().iter().enumerate() {
            let Some(typ) = GeoArrowType::from_extension_field(field)? else {
                fields.push(field.clone());
                continue;
            };
            let crs = typ.metadata().crs();
            let (projjson, parquet_crs) = crs_forms(crs.crs_type(), crs.crs_value());
            if projjson.is_null() && crs.crs_value().is_some() {
                eprintln!(
                    "warning: column {}: CRS {} has no PROJJSON form; GeoParquet crs is written as null",
                    field.name(),
                    crs.crs_value().map(Value::to_string).unwrap_or_default()
                );
            }
            // Other metadata (`gml:srs_name`, `gml:axis_swapped`, …) is kept.
            let mut metadata: HashMap<String, String> = field
                .metadata()
                .iter()
                .filter(|(key, _)| !key.starts_with("ARROW:extension:"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let extension = match &parquet_crs {
                Some(crs) => json!({ "crs": crs, "crs_type": "authority_code" }),
                None => json!({}),
            };
            metadata.insert("ARROW:extension:name".into(), "geoarrow.wkb".into());
            metadata.insert("ARROW:extension:metadata".into(), extension.to_string());
            fields.push(Arc::new(
                Field::new(field.name(), DataType::Binary, true).with_metadata(metadata),
            ));
            let covered = match bbox {
                BboxColumn::Always => true,
                BboxColumn::Never => false,
                BboxColumn::Auto => !holds_points(&typ, field, first.map(|batch| batch.column(index)))?,
            };
            let covering = covered.then(|| {
                let name = unique_name(&format!("{}_bbox", field.name()), &names);
                names.insert(name.clone());
                fields.push(Arc::new(Field::new(&name, DataType::Struct(bbox_fields()), true)));
                name
            });
            columns.push(GeoColumn {
                index,
                name: field.name().clone(),
                projjson,
                types: BTreeSet::new(),
                bbox: None,
                covering,
            });
        }
        let primary = primary
            .filter(|name| columns.iter().any(|column| column.name == *name))
            .map(str::to_string)
            .or_else(|| columns.first().map(|column| column.name.clone()));
        let output = Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));
        Ok(GeoColumns { layer: layer.to_string(), columns, primary, output })
    }

    pub(crate) fn schema(&self) -> SchemaRef {
        self.output.clone()
    }

    /// The batch with its geometry columns as checked WKB, each followed by
    /// its covering column if it has one; bbox and geometry types are
    /// collected on the way.
    pub(crate) fn convert(&mut self, batch: &RecordBatch) -> super::Result<RecordBatch> {
        let mut arrays: Vec<ArrayRef> = batch.columns().to_vec();
        let mut coverings = Vec::new();
        let schema = batch.schema();
        for column in &mut self.columns {
            let wkb = to_wkb(batch.column(column.index), schema.field(column.index))?;
            let mut boxes = column.covering.as_ref().map(|_| Boxes::with_capacity(wkb.len()));
            for (row, value) in wkb.as_binary::<i32>().iter().enumerate() {
                let Some(value) = value else {
                    if let Some(boxes) = &mut boxes {
                        boxes.push(None);
                    }
                    continue;
                };
                let mut row_bbox = None;
                let mut reader = WkbReader { buf: value, pos: 0, bbox: &mut row_bbox };
                let result = reader.geometry(true);
                column.bbox = union(column.bbox, row_bbox);
                if let Some(boxes) = &mut boxes {
                    // An empty geometry has a bbox of NaNs, as GeoPandas writes.
                    boxes.push(Some(row_bbox.unwrap_or([f64::NAN; 4])));
                }
                match result {
                    Ok(name) => {
                        column.types.insert(name);
                    }
                    Err(WkbIssue::Curve(kind)) => {
                        return Err(format!(
                            "layer {}, column {}: a {kind} (curve) in batch row {row}; Parquet and GeoParquet have \
                             only the seven simple geometry types. Add --linearize to convert arcs to line \
                             segments (lossy), or use --format ipc to keep the curves",
                            self.layer, column.name
                        )
                        .into());
                    }
                    Err(WkbIssue::Unsupported(kind)) => {
                        return Err(format!(
                            "layer {}, column {}: geometry type {kind} can't be written to Parquet, which has \
                             only the seven simple geometry types; use --format ipc",
                            self.layer, column.name
                        )
                        .into());
                    }
                    Err(WkbIssue::Malformed) => {
                        return Err(format!("layer {}, column {}: malformed WKB in batch row {row}", self.layer, column.name).into());
                    }
                }
            }
            arrays[column.index] = wkb;
            if let Some(boxes) = boxes {
                coverings.push((column.index, boxes.finish()?));
            }
        }
        // Each covering column after its geometry column, the last first so
        // the indices of the earlier ones stay valid.
        for (index, covering) in coverings.into_iter().rev() {
            arrays.insert(index + 1, covering);
        }
        Ok(RecordBatch::try_new(self.output.clone(), arrays)?)
    }

    /// The `geo` key-value metadata (GeoParquet 1.1), `None` without geometry.
    pub(crate) fn metadata(&self) -> Option<String> {
        let primary = self.primary.as_ref()?;
        let columns: serde_json::Map<String, Value> = self
            .columns
            .iter()
            .map(|column| {
                let mut entry = json!({
                    "encoding": "WKB",
                    "geometry_types": column.types.iter().collect::<Vec<_>>(),
                    // Explicit null: a missing `crs` would mean OGC:CRS84.
                    "crs": column.projjson,
                });
                if let Some(bbox) = column.bbox {
                    entry["bbox"] = json!(bbox);
                }
                if let Some(covering) = &column.covering {
                    let paths: serde_json::Map<String, Value> =
                        BBOX_FIELDS.iter().map(|field| (field.to_string(), json!([covering, field]))).collect();
                    entry["covering"] = json!({ "bbox": paths });
                }
                (column.name.clone(), entry)
            })
            .collect();
        let geo = json!({ "version": "1.1.0", "primary_column": primary, "columns": columns });
        Some(geo.to_string())
    }
}

/// A geometry column as WKB.
fn to_wkb(array: &ArrayRef, field: &Field) -> super::Result<ArrayRef> {
    let array = geoarrow_array::array::from_arrow_array(array.as_ref(), field)?;
    Ok(geoarrow_array::cast::to_wkb::<i32>(array.as_ref())?.to_array_ref())
}

/// A point column, which `BboxColumn::Auto` leaves without a covering:
/// `geoarrow.point`, or a WKB (or WKT, or mixed) column whose values in the
/// first batch are all points. A column that holds no value there counts as
/// not points.
fn holds_points(typ: &GeoArrowType, field: &Field, first: Option<&ArrayRef>) -> super::Result<bool> {
    use GeoArrowType::*;
    match typ {
        Point(_) => Ok(true),
        Wkb(_) | LargeWkb(_) | WkbView(_) | Wkt(_) | LargeWkt(_) | WktView(_) | Geometry(_) => {
            let Some(array) = first else { return Ok(false) };
            let wkb = to_wkb(array, field)?;
            let wkb: &BinaryArray = wkb.as_binary::<i32>();
            let mut values = wkb.iter().flatten().peekable();
            Ok(values.peek().is_some() && values.all(is_wkb_point))
        }
        _ => Ok(false),
    }
}

/// The WKB value is a Point (ISO or EWKB, any dimension).
fn is_wkb_point(value: &[u8]) -> bool {
    let Some(code) = value.get(1..5) else { return false };
    let code: [u8; 4] = code.try_into().expect("4 bytes");
    let code = match value[0] {
        0 => u32::from_be_bytes(code),
        _ => u32::from_le_bytes(code),
    };
    (code & 0x0FFF_FFFF) % 1000 == 1
}

/// `name`, or `name_2`, `name_3`, … if a column already has it.
fn unique_name(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(name) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name}_{n}")).find(|candidate| !taken.contains(candidate)).expect("a free name")
}

fn union(a: Option<[f64; 4]>, b: Option<[f64; 4]>) -> Option<[f64; 4]> {
    match (a, b) {
        (Some([x0, y0, x1, y1]), Some([u0, v0, u1, v1])) => Some([x0.min(u0), y0.min(v0), x1.max(u1), y1.max(v1)]),
        (a, b) => a.or(b),
    }
}

/// The values of one covering column: a bbox per row, null where the
/// geometry is null.
struct Boxes {
    ordinates: [Vec<f64>; 4],
    valid: Vec<bool>,
}

impl Boxes {
    fn with_capacity(rows: usize) -> Self {
        Boxes { ordinates: std::array::from_fn(|_| Vec::with_capacity(rows)), valid: Vec::with_capacity(rows) }
    }

    fn push(&mut self, bbox: Option<[f64; 4]>) {
        // A null row's fields hold NaN; they are not written.
        let values = bbox.unwrap_or([f64::NAN; 4]);
        for (ordinates, value) in self.ordinates.iter_mut().zip(values) {
            ordinates.push(value);
        }
        self.valid.push(bbox.is_some());
    }

    fn finish(self) -> super::Result<ArrayRef> {
        let nulls = self.valid.contains(&false).then(|| NullBuffer::from(self.valid));
        let arrays = self.ordinates.map(|ordinates| Arc::new(Float64Array::from(ordinates)) as ArrayRef);
        Ok(Arc::new(StructArray::try_new(bbox_fields(), arrays.to_vec(), nulls)?))
    }
}

/// `(GeoParquet crs, Parquet GEOMETRY crs)` from GeoArrow CRS metadata.
fn crs_forms(crs_type: Option<CrsType>, value: Option<&Value>) -> (Value, Option<String>) {
    let Some(value) = value else {
        return (Value::Null, None);
    };
    if crs_type == Some(CrsType::Projjson) || value.is_object() {
        let parquet = projjson_ref(value).map(|crs| crs.authority_code());
        return (value.clone(), parquet);
    }
    // `authority:code` or an srsName as written: PROJJSON from the EPSG tables.
    let Some(text) = value.as_str() else {
        return (Value::Null, None);
    };
    // `SrsName::parse` also reads the compound `EPSG:25832+7837`.
    let mut crs = SrsName::parse(text).crs;
    if crs.is_none() && crs_type == Some(CrsType::AuthorityCode) {
        crs = text.split_once(':').map(|(authority, code)| CrsRef::Code { authority: authority.to_string(), code: code.to_string() });
    }
    let Some(crs) = crs else {
        return (Value::Null, None);
    };
    // Both name the same CRS: without the parts PROJJSON leaves out.
    match crs.projjson() {
        Some(projjson) => {
            let parquet = projjson_ref(&projjson).map(|crs| crs.authority_code());
            (projjson, parquet)
        }
        None => (Value::Null, Some(crs.authority_code())),
    }
}

/// The CRS a PROJJSON document identifies: its `id`, or for a compound CRS
/// without one (as [`CrsRef::projjson`] builds them), its components' ids.
fn projjson_ref(value: &Value) -> Option<CrsRef> {
    let id = &value["id"];
    let code = match &id["code"] {
        Value::String(code) => Some(code.clone()),
        Value::Number(code) => Some(code.to_string()),
        _ => None,
    };
    match (id["authority"].as_str(), code) {
        (Some(authority), Some(code)) => Some(CrsRef::Code { authority: authority.to_string(), code }),
        _ if value["type"] == "CompoundCRS" => {
            value["components"].as_array()?.iter().map(projjson_ref).collect::<Option<_>>().map(CrsRef::Compound)
        }
        _ => None,
    }
}

enum WkbIssue {
    Curve(&'static str),
    Unsupported(String),
    Malformed,
}

/// Walks one ISO (or EWKB) WKB value: its type, the x/y bounding box, and
/// whether it holds curves anywhere.
struct WkbReader<'a> {
    buf: &'a [u8],
    pos: usize,
    bbox: &'a mut Option<[f64; 4]>,
}

impl WkbReader<'_> {
    /// The GeoParquet type name of the geometry (`Point`, `Polygon Z`, …).
    fn geometry(&mut self, top: bool) -> Result<&'static str, WkbIssue> {
        let little = match self.byte()? {
            0 => false,
            1 => true,
            _ => return Err(WkbIssue::Malformed),
        };
        let code = self.u32(little)?;
        // EWKB flags, then ISO thousands.
        let mut z = code & 0x8000_0000 != 0;
        let mut m = code & 0x4000_0000 != 0;
        if code & 0x2000_0000 != 0 {
            self.u32(little)?;
        }
        let iso = code & 0x0FFF_FFFF;
        match iso / 1000 {
            1 => z = true,
            2 => m = true,
            3 => (z, m) = (true, true),
            _ => {}
        }
        let dims = 2 + usize::from(z) + usize::from(m);
        let base = iso % 1000;
        match base {
            1 => self.points(1, dims, little)?,
            2 => {
                let n = self.u32(little)? as usize;
                self.points(n, dims, little)?;
            }
            3 => {
                for _ in 0..self.u32(little)? {
                    let n = self.u32(little)? as usize;
                    self.points(n, dims, little)?;
                }
            }
            4..=7 => {
                for _ in 0..self.u32(little)? {
                    self.geometry(false)?;
                }
            }
            8 => return Err(WkbIssue::Curve("CircularString")),
            9 => return Err(WkbIssue::Curve("CompoundCurve")),
            10 => return Err(WkbIssue::Curve("CurvePolygon")),
            11 => return Err(WkbIssue::Curve("MultiCurve")),
            12 => return Err(WkbIssue::Curve("MultiSurface")),
            13 => return Err(WkbIssue::Curve("Curve")),
            14 => return Err(WkbIssue::Curve("Surface")),
            15 => return Err(WkbIssue::Unsupported("PolyhedralSurface".into())),
            16 => return Err(WkbIssue::Unsupported("TIN".into())),
            17 => return Err(WkbIssue::Unsupported("Triangle".into())),
            other => return Err(WkbIssue::Unsupported(format!("WKB type {other}"))),
        }
        if !top {
            return Ok("");
        }
        const NAMES: [&str; 7] = ["Point", "LineString", "Polygon", "MultiPoint", "MultiLineString", "MultiPolygon", "GeometryCollection"];
        const NAMES_Z: [&str; 7] =
            ["Point Z", "LineString Z", "Polygon Z", "MultiPoint Z", "MultiLineString Z", "MultiPolygon Z", "GeometryCollection Z"];
        let index = base as usize - 1;
        Ok(if z { NAMES_Z[index] } else { NAMES[index] })
    }

    fn points(&mut self, n: usize, dims: usize, little: bool) -> Result<(), WkbIssue> {
        for _ in 0..n {
            let x = self.f64(little)?;
            let y = self.f64(little)?;
            for _ in 2..dims {
                self.f64(little)?;
            }
            // An empty point is written as NaN coordinates.
            if x.is_nan() || y.is_nan() {
                continue;
            }
            *self.bbox = Some(match *self.bbox {
                None => [x, y, x, y],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
            });
        }
        Ok(())
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], WkbIssue> {
        let bytes = self.buf.get(self.pos..self.pos + N).ok_or(WkbIssue::Malformed)?;
        self.pos += N;
        Ok(bytes.try_into().expect("N bytes"))
    }

    fn byte(&mut self) -> Result<u8, WkbIssue> {
        Ok(self.take::<1>()?[0])
    }

    fn u32(&mut self, little: bool) -> Result<u32, WkbIssue> {
        let bytes = self.take::<4>()?;
        Ok(if little { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
    }

    fn f64(&mut self, little: bool) -> Result<f64, WkbIssue> {
        let bytes = self.take::<8>()?;
        Ok(if little { f64::from_le_bytes(bytes) } else { f64::from_be_bytes(bytes) })
    }
}
