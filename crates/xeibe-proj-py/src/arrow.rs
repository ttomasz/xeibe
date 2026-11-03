//! GeoArrow arrays converted in their own buffers, for `xeibe_proj.arrow`
//! (`docs/pyproj-phase3.md`, GeoArrow).
//!
//! Arrays come in and go out through the Arrow C Data and C Stream
//! Interfaces (`__arrow_c_array__`, `__arrow_c_stream__`), so no Python
//! Arrow library is imported. The layout decides the encoding: lists of
//! interleaved (`FixedSizeList<double>`) or separated (`Struct<x, y, …>`)
//! coordinates, WKB in `Binary`, `LargeBinary` or `BinaryView`, and boxes
//! (`Struct<xmin, ymin, …, xmax, ymax, …>`). The coordinates are copied and
//! converted; offsets and validity buffers are shared with the input.
//! Coordinates under a null geometry, or outside every geometry of a sliced
//! array, are left as they are: they may be anything.

use std::collections::{HashMap, VecDeque};
use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::ffi::{FFI_ArrowArray, FFI_ArrowSchema, from_ffi};
use arrow_array::ffi_stream::FFI_ArrowArrayStream;
use arrow_array::types::Float64Type;
use arrow_array::{
    Array, ArrayRef, BinaryViewArray, FixedSizeListArray, Float64Array, GenericBinaryArray,
    GenericListArray, OffsetSizeTrait, StructArray, make_array,
};
use arrow_buffer::{Buffer, OffsetBuffer, ScalarBuffer};
use arrow_schema::{DataType, Field, FieldRef, Fields};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyCapsule;
use xeibe_reproject::Error;

use crate::convert::{Calls, Convert, Missing};
use crate::{Transform, exception};

/// A row that no geometry reaches: under a null, or outside a sliced
/// array's offsets.
const DEAD: usize = usize::MAX;

/// The box `transform_bounds` densifies a box's edges with, as pyproj's
/// default.
const DENSIFY: usize = 21;

/// A GeoArrow array (or chunked array) and its field, as imported.
#[pyclass(frozen, module = "xeibe_proj._native")]
pub(crate) struct ArrowData {
    field: FieldRef,
    chunks: Vec<ArrayRef>,
}

/// Why a conversion stopped.
enum Failure {
    /// Not a layout this reads, or malformed WKB: a `ValueError`.
    Layout(String),
    /// A coordinate of geometry `row` (in the whole input) failed.
    Convert(Option<usize>, Error),
    /// Grid data is missing for the box.
    Grids(Missing),
}

fn layout(message: impl std::fmt::Display) -> Failure {
    Failure::Layout(message.to_string())
}

impl Failure {
    fn into_err(self, py: Python<'_>) -> PyErr {
        match self {
            Failure::Layout(message) => PyValueError::new_err(message),
            Failure::Grids(missing) => missing.into_err(),
            Failure::Convert(row, error) => {
                let place = row.map_or(String::new(), |row| format!("geometry {row}: "));
                let message = match error {
                    Error::Coordinate { x, y, reason, .. } => {
                        format!("{place}coordinate ({x}, {y}): {reason}")
                    }
                    error => format!("{place}{error}"),
                };
                exception(py, "ProjError", message)
            }
        }
    }
}

fn arrow_error(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(format!("Arrow: {e}"))
}

#[pymethods]
impl ArrowData {
    /// Import an array from the two capsules of `__arrow_c_array__`.
    #[staticmethod]
    fn from_array(schema: &Bound<'_, PyCapsule>, array: &Bound<'_, PyCapsule>) -> PyResult<Self> {
        let schema_ptr = schema.pointer_checked(Some(c"arrow_schema"))?;
        let array_ptr = array.pointer_checked(Some(c"arrow_array"))?;
        // SAFETY: the capsules hold the structs their names say. The schema
        // stays the capsule's to release; the array is moved out, leaving a
        // released one behind, as the PyCapsule interface asks.
        let schema = unsafe { &*(schema_ptr.as_ptr() as *const FFI_ArrowSchema) };
        let array = unsafe { FFI_ArrowArray::from_raw(array_ptr.as_ptr() as *mut FFI_ArrowArray) };
        let field = Field::try_from(schema).map_err(arrow_error)?;
        // SAFETY: the array was made for this schema.
        let data = unsafe { from_ffi(array, schema) }.map_err(arrow_error)?;
        Ok(ArrowData {
            field: Arc::new(field),
            chunks: vec![make_array(data)],
        })
    }

    /// Import every chunk of the capsule of `__arrow_c_stream__`.
    #[staticmethod]
    fn from_stream(stream: &Bound<'_, PyCapsule>) -> PyResult<Self> {
        let ptr = stream.pointer_checked(Some(c"arrow_array_stream"))?;
        // SAFETY: moved out of the capsule, which is left a released stream.
        let mut stream =
            unsafe { FFI_ArrowArrayStream::from_raw(ptr.as_ptr() as *mut FFI_ArrowArrayStream) };
        let (Some(get_schema), Some(get_next)) = (stream.get_schema, stream.get_next) else {
            return Err(arrow_error("the stream is released"));
        };
        let mut schema = FFI_ArrowSchema::empty();
        // SAFETY: the stream's own callbacks, on the stream.
        let rc = unsafe { get_schema(&mut stream, &mut schema) };
        if rc != 0 {
            return Err(arrow_error(stream_error(&mut stream, rc)));
        }
        let field = Field::try_from(&schema).map_err(arrow_error)?;
        let mut chunks = Vec::new();
        loop {
            let mut array = FFI_ArrowArray::empty();
            let rc = unsafe { get_next(&mut stream, &mut array) };
            if rc != 0 {
                return Err(arrow_error(stream_error(&mut stream, rc)));
            }
            if array.is_released() {
                break;
            }
            // SAFETY: the stream's arrays have the stream's schema.
            let data = unsafe { from_ffi(array, &schema) }.map_err(arrow_error)?;
            chunks.push(make_array(data));
        }
        Ok(ArrowData {
            field: Arc::new(field),
            chunks,
        })
    }

    /// The field's metadata (`ARROW:extension:name` and
    /// `ARROW:extension:metadata` among them).
    #[getter]
    fn metadata(&self) -> HashMap<String, String> {
        self.field.metadata().clone()
    }

    /// The number of chunks.
    #[getter]
    fn num_chunks(&self) -> usize {
        self.chunks.len()
    }

    fn __len__(&self) -> usize {
        self.chunks.iter().map(|c| c.len()).sum()
    }

    /// The same data with the field's metadata replaced.
    fn with_metadata(&self, metadata: HashMap<String, String>) -> ArrowData {
        ArrowData {
            field: Arc::new(self.field.as_ref().clone().with_metadata(metadata)),
            chunks: self.chunks.clone(),
        }
    }

    fn __arrow_c_schema__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyCapsule>> {
        let schema = FFI_ArrowSchema::try_from(self.field.as_ref()).map_err(arrow_error)?;
        PyCapsule::new_with_value(py, schema, c"arrow_schema")
    }

    /// The one chunk; a `TypeError` for chunked data. `requested_schema`
    /// is ignored, as the interface allows.
    #[pyo3(signature = (requested_schema = None))]
    fn __arrow_c_array__<'py>(
        &self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyAny>>,
    ) -> PyResult<(Bound<'py, PyCapsule>, Bound<'py, PyCapsule>)> {
        let _ = requested_schema;
        let [chunk] = self.chunks.as_slice() else {
            return Err(PyTypeError::new_err("chunked data: use __arrow_c_stream__"));
        };
        let schema = self.__arrow_c_schema__(py)?;
        let array = FFI_ArrowArray::new(&chunk.to_data());
        Ok((
            schema,
            PyCapsule::new_with_value(py, array, c"arrow_array")?,
        ))
    }

    /// The chunks as a stream. `requested_schema` is ignored.
    #[pyo3(signature = (requested_schema = None))]
    fn __arrow_c_stream__<'py>(
        &self,
        py: Python<'py>,
        requested_schema: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        let _ = requested_schema;
        let stream = export_stream(self.field.clone(), self.chunks.clone());
        PyCapsule::new_with_value(py, stream, c"arrow_array_stream")
    }

    /// The data converted by `native` (a `Transform`), easting first. With
    /// `check_grids`, `GridsNeeded` when grid data for its extent is
    /// missing; a `ProjError` naming the geometry when a coordinate fails.
    #[pyo3(signature = (native, check_grids = true))]
    fn convert(
        &self,
        py: Python<'_>,
        native: &Bound<'_, Transform>,
        check_grids: bool,
    ) -> PyResult<ArrowData> {
        let native = native.get();
        let chunks = py
            .detach(|| convert_chunks(&self.chunks, &native.inner, &native.calls, check_grids))
            .map_err(|f| f.into_err(py))?;
        Ok(ArrowData {
            field: self.field.clone(),
            chunks,
        })
    }
}

/// Every chunk converted, after checking the grids for their extent.
fn convert_chunks<C: Convert>(
    chunks: &[ArrayRef],
    c: &C,
    calls: &Calls,
    check_grids: bool,
) -> Result<Vec<ArrayRef>, Failure> {
    if check_grids {
        let mut extent = Extent::default();
        walk_chunks(chunks, &mut extent)?;
        calls
            .check_grids(c, extent.bounds())
            .map_err(Failure::Grids)?;
    }
    walk_chunks(chunks, &mut Converting(c))
}

fn walk_chunks(chunks: &[ArrayRef], op: &mut dyn Op) -> Result<Vec<ArrayRef>, Failure> {
    let mut base = 0;
    let mut out = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let rows: Vec<usize> = (0..chunk.len())
            .map(|i| if chunk.is_valid(i) { base + i } else { DEAD })
            .collect();
        out.push(walk(chunk, &rows, op)?);
        base += chunk.len();
    }
    Ok(out)
}

// The conversion and the extent -----------------------------------------------

/// The coordinates of one layer, taken out of their buffers, with the row
/// of the geometry each belongs to.
#[derive(Default)]
struct Gathered {
    x: Vec<f64>,
    y: Vec<f64>,
    z: Option<Vec<f64>>,
    rows: Vec<usize>,
}

impl Gathered {
    fn new(z: bool) -> Gathered {
        Gathered {
            z: z.then(Vec::new),
            ..Gathered::default()
        }
    }

    fn push(&mut self, x: f64, y: f64, z: Option<f64>, row: usize) {
        self.x.push(x);
        self.y.push(y);
        if let (Some(zs), Some(z)) = (self.z.as_mut(), z) {
            zs.push(z);
        }
        self.rows.push(row);
    }
}

/// What a walk does with the coordinates it gathers.
trait Op {
    fn points(&mut self, g: &mut Gathered) -> Result<(), Failure>;
    /// Boxes as `[min_x, min_y, max_x, max_y]`, one for each row.
    fn boxes(&mut self, boxes: &mut [[f64; 4]], rows: &[usize]) -> Result<(), Failure>;
}

/// The box around every finite coordinate.
struct Extent([f64; 4]);

impl Default for Extent {
    fn default() -> Self {
        Extent([
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ])
    }
}

impl Extent {
    fn add(&mut self, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() {
            let b = &mut self.0;
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
    }

    fn bounds(&self) -> Option<[f64; 4]> {
        self.0[0].is_finite().then_some(self.0)
    }
}

impl Op for Extent {
    fn points(&mut self, g: &mut Gathered) -> Result<(), Failure> {
        g.x.iter().zip(&g.y).for_each(|(&x, &y)| self.add(x, y));
        Ok(())
    }

    fn boxes(&mut self, boxes: &mut [[f64; 4]], _rows: &[usize]) -> Result<(), Failure> {
        for b in boxes.iter() {
            self.add(b[0], b[1]);
            self.add(b[2], b[3]);
        }
        Ok(())
    }
}

/// Converts what it is given.
struct Converting<'a, C: Convert>(&'a C);

impl<C: Convert> Op for Converting<'_, C> {
    fn points(&mut self, g: &mut Gathered) -> Result<(), Failure> {
        match g.z.as_mut() {
            Some(z) => self.0.convert_xyz(&mut g.x, &mut g.y, z),
            None => self.0.convert_xy(&mut g.x, &mut g.y),
        }
        .map_err(|e| {
            let row = match &e {
                Error::Coordinate { index, .. } => g.rows.get(*index).copied(),
                _ => None,
            };
            Failure::Convert(row, e)
        })
    }

    fn boxes(&mut self, boxes: &mut [[f64; 4]], rows: &[usize]) -> Result<(), Failure> {
        for (b, &row) in boxes.iter_mut().zip(rows) {
            *b = self
                .0
                .convert_bounds(*b, DENSIFY)
                .map_err(|e| Failure::Convert(Some(row), e))?;
        }
        Ok(())
    }
}

// The layouts --------------------------------------------------------------------

/// Which of a coordinate's values are x, y and z: x and y first, then z
/// when there is one; M is left alone.
#[derive(Clone, Copy)]
struct Dims {
    count: usize,
    z: bool,
}

impl Dims {
    /// From GeoArrow's names: `xy`, `xyz`, `xym`, `xyzm` (an interleaved
    /// child's name, or a struct's field names joined).
    fn named(name: &str) -> Option<Dims> {
        match name {
            "xy" => Some(Dims { count: 2, z: false }),
            "xyz" => Some(Dims { count: 3, z: true }),
            "xym" => Some(Dims { count: 3, z: false }),
            "xyzm" => Some(Dims { count: 4, z: true }),
            _ => None,
        }
    }
}

/// A new array with the same layout and the coordinates converted.
fn walk(array: &ArrayRef, rows: &[usize], op: &mut dyn Op) -> Result<ArrayRef, Failure> {
    match array.data_type() {
        DataType::List(_) => list(array.as_list::<i32>(), rows, op),
        DataType::LargeList(_) => list(array.as_list::<i64>(), rows, op),
        DataType::FixedSizeList(..) => interleaved(array.as_fixed_size_list(), rows, op),
        DataType::Struct(fields) if is_box(fields) => boxes(array.as_struct(), rows, op),
        DataType::Struct(_) => separated(array.as_struct(), rows, op),
        DataType::Binary => wkb(array.as_binary::<i32>(), rows, op),
        DataType::LargeBinary => wkb(array.as_binary::<i64>(), rows, op),
        DataType::BinaryView => wkb_view(array.as_binary_view(), rows, op),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => Err(layout(
            "WKT isn't supported: only GeoArrow's native encodings and WKB are",
        )),
        other => Err(layout(format!("{other} is not a GeoArrow layout"))),
    }
}

/// The rows of a list's values: each value takes its list's row, and
/// values under a null list or outside every list are dead.
fn child_rows<O: OffsetSizeTrait>(
    offsets: &OffsetBuffer<O>,
    array: &dyn Array,
    rows: &[usize],
    child_len: usize,
) -> Vec<usize> {
    let mut out = vec![DEAD; child_len];
    for (i, w) in offsets.windows(2).enumerate() {
        if rows[i] == DEAD || array.is_null(i) {
            continue;
        }
        let (start, end) = (w[0].as_usize(), w[1].as_usize());
        if start <= end && end <= child_len {
            out[start..end].fill(rows[i]);
        }
    }
    out
}

fn live(rows: &[usize], array: &dyn Array, i: usize) -> Option<usize> {
    (rows[i] != DEAD && array.is_valid(i)).then_some(rows[i])
}

fn list<O: OffsetSizeTrait>(
    l: &GenericListArray<O>,
    rows: &[usize],
    op: &mut dyn Op,
) -> Result<ArrayRef, Failure> {
    let child = child_rows(l.offsets(), l, rows, l.values().len());
    let values = walk(l.values(), &child, op)?;
    let (field, offsets, _, nulls) = l.clone().into_parts();
    Ok(Arc::new(
        GenericListArray::<O>::try_new(field, offsets, values, nulls).map_err(layout)?,
    ))
}

fn doubles<'a>(array: &'a ArrayRef, what: &str) -> Result<&'a Float64Array, Failure> {
    array
        .as_primitive_opt::<Float64Type>()
        .ok_or_else(|| layout(format!("{what} are {}, not double", array.data_type())))
}

/// Coordinates `x0 y0 [z0] [m0] x1 …` in a `FixedSizeList<double>`.
fn interleaved(
    l: &FixedSizeListArray,
    rows: &[usize],
    op: &mut dyn Op,
) -> Result<ArrayRef, Failure> {
    let size = l.value_length() as usize;
    let DataType::FixedSizeList(field, _) = l.data_type() else {
        unreachable!("an interleaved array is a fixed-size list");
    };
    let dims = Dims::named(field.name())
        .filter(|d| d.count == size)
        .or(match size {
            2 => Dims::named("xy"),
            3 => Dims::named("xyz"),
            4 => Dims::named("xyzm"),
            _ => None,
        })
        .ok_or_else(|| layout(format!("coordinates of {size} values")))?;
    let values = doubles(l.values(), "coordinates")?;
    let mut out = values.values().to_vec();
    let mut g = Gathered::new(dims.z);
    let mut at = Vec::new();
    for i in 0..l.len() {
        if let Some(row) = live(rows, l, i) {
            let c = &out[i * size..i * size + size];
            g.push(c[0], c[1], dims.z.then(|| c[2]), row);
            at.push(i);
        }
    }
    op.points(&mut g)?;
    for (k, &i) in at.iter().enumerate() {
        out[i * size] = g.x[k];
        out[i * size + 1] = g.y[k];
        if let Some(z) = &g.z {
            out[i * size + 2] = z[k];
        }
    }
    let values = Float64Array::new(ScalarBuffer::from(out), values.nulls().cloned());
    Ok(Arc::new(
        FixedSizeListArray::try_new(
            field.clone(),
            size as i32,
            Arc::new(values),
            l.nulls().cloned(),
        )
        .map_err(layout)?,
    ))
}

/// Coordinates as a `Struct<x: double, y: double, [z], [m]>`.
fn separated(s: &StructArray, rows: &[usize], op: &mut dyn Op) -> Result<ArrayRef, Failure> {
    let (fields, columns, nulls) = s.clone().into_parts();
    let names: String = fields.iter().map(|f| f.name().as_str()).collect();
    let dims = Dims::named(&names)
        .filter(|d| d.count == fields.len())
        .ok_or_else(|| {
            layout(format!(
                "a struct of {} is neither coordinates (x, y, [z], [m]) nor a box",
                fields
                    .iter()
                    .map(|f| f.name().as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
    let mut cols = Vec::with_capacity(columns.len());
    for c in &columns {
        cols.push(doubles(c, "coordinates")?.values().to_vec());
    }
    let mut g = Gathered::new(dims.z);
    let mut at = Vec::new();
    for (i, (&x, &y)) in cols[0].iter().zip(&cols[1]).enumerate() {
        if let Some(row) = live(rows, s, i) {
            g.push(x, y, dims.z.then(|| cols[2][i]), row);
            at.push(i);
        }
    }
    op.points(&mut g)?;
    for (k, &i) in at.iter().enumerate() {
        cols[0][i] = g.x[k];
        cols[1][i] = g.y[k];
        if let Some(z) = &g.z {
            cols[2][i] = z[k];
        }
    }
    let arrays = cols
        .into_iter()
        .zip(&columns)
        .map(|(values, old)| {
            Arc::new(Float64Array::new(
                ScalarBuffer::from(values),
                old.nulls().cloned(),
            )) as ArrayRef
        })
        .collect();
    Ok(Arc::new(
        StructArray::try_new(fields, arrays, nulls).map_err(layout)?,
    ))
}

const BOX_FIELDS: [&str; 4] = ["xmin", "ymin", "xmax", "ymax"];

/// A GeoArrow box: `xmin`, `ymin`, `[zmin]`, `[mmin]`, `xmax`, `ymax`, …
fn is_box(fields: &Fields) -> bool {
    BOX_FIELDS
        .iter()
        .all(|name| fields.iter().any(|f| f.name() == name))
}

/// Boxes, each converted as `transform_bounds` converts one (21 points on
/// each edge). A geographic result with `xmin > xmax` crosses the
/// antimeridian, as GeoArrow's boxes may. Z and M bounds stay as they are.
fn boxes(s: &StructArray, rows: &[usize], op: &mut dyn Op) -> Result<ArrayRef, Failure> {
    let (fields, columns, nulls) = s.clone().into_parts();
    let index = BOX_FIELDS.map(|name| {
        fields
            .iter()
            .position(|f| f.name() == name)
            .expect("is_box found it")
    });
    let mut cols: Vec<Option<Vec<f64>>> = vec![None; columns.len()];
    for &k in &index {
        cols[k] = Some(doubles(&columns[k], "box bounds")?.values().to_vec());
    }
    let value = |cols: &[Option<Vec<f64>>], k: usize, i: usize| cols[k].as_ref().unwrap()[i];
    let mut found = Vec::new();
    let mut live_rows = Vec::new();
    let mut at = Vec::new();
    for i in 0..s.len() {
        if let Some(row) = live(rows, s, i) {
            found.push(index.map(|k| value(&cols, k, i)));
            live_rows.push(row);
            at.push(i);
        }
    }
    op.boxes(&mut found, &live_rows)?;
    for (b, &i) in found.iter().zip(&at) {
        for (v, &k) in b.iter().zip(&index) {
            cols[k].as_mut().unwrap()[i] = *v;
        }
    }
    let arrays = cols
        .into_iter()
        .zip(&columns)
        .map(|(values, old)| match values {
            Some(values) => Arc::new(Float64Array::new(
                ScalarBuffer::from(values),
                old.nulls().cloned(),
            )) as ArrayRef,
            None => old.clone(),
        })
        .collect();
    Ok(Arc::new(
        StructArray::try_new(fields, arrays, nulls).map_err(layout)?,
    ))
}

// WKB ----------------------------------------------------------------------------

/// Where a coordinate is: buffer, byte offset of x, byte order, and whether
/// a z follows y.
struct WkbCoord {
    buffer: usize,
    pos: usize,
    le: bool,
    z: bool,
    row: usize,
}

fn wkb<O: OffsetSizeTrait>(
    b: &GenericBinaryArray<O>,
    rows: &[usize],
    op: &mut dyn Op,
) -> Result<ArrayRef, Failure> {
    let (offsets, values, nulls) = b.clone().into_parts();
    let mut buffers = vec![values.as_slice().to_vec()];
    let mut coords = Vec::new();
    for i in 0..b.len() {
        if let Some(row) = live(rows, b, i) {
            let (start, end) = (offsets[i].as_usize(), offsets[i + 1].as_usize());
            parse_wkb(&buffers[0], 0, start, end, row, &mut coords)?;
        }
    }
    convert_wkb(&mut buffers, &coords, op)?;
    let values = Buffer::from(buffers.pop().expect("one buffer"));
    Ok(Arc::new(
        GenericBinaryArray::<O>::try_new(offsets, values, nulls).map_err(layout)?,
    ))
}

/// WKB in a `BinaryView`. Values of 12 bytes or fewer are inline in their
/// view, and are too short to hold a coordinate. The views keep the first
/// four bytes of each value, the byte order and type, which don't change.
fn wkb_view(b: &BinaryViewArray, rows: &[usize], op: &mut dyn Op) -> Result<ArrayRef, Failure> {
    let (views, data, nulls) = b.clone().into_parts();
    let mut buffers: Vec<Vec<u8>> = data.iter().map(|d| d.as_slice().to_vec()).collect();
    let mut coords = Vec::new();
    for i in 0..b.len() {
        let Some(row) = live(rows, b, i) else {
            continue;
        };
        let view = views[i];
        let len = view as u32 as usize;
        if len <= 12 {
            continue;
        }
        let buffer = (view >> 64) as u32 as usize;
        let start = (view >> 96) as u32 as usize;
        let bytes = buffers
            .get(buffer)
            .ok_or_else(|| layout(format!("geometry {row}: no data buffer {buffer}")))?;
        parse_wkb(bytes, buffer, start, start + len, row, &mut coords)?;
    }
    convert_wkb(&mut buffers, &coords, op)?;
    let data: Vec<Buffer> = buffers.into_iter().map(Buffer::from).collect();
    Ok(Arc::new(
        BinaryViewArray::try_new(views, data, nulls).map_err(layout)?,
    ))
}

/// Gather the coordinates of the WKB value `bytes[start..end]`.
fn parse_wkb(
    bytes: &[u8],
    buffer: usize,
    start: usize,
    end: usize,
    row: usize,
    out: &mut Vec<WkbCoord>,
) -> Result<(), Failure> {
    let bad = |m: String| layout(format!("geometry {row}: WKB: {m}"));
    if end > bytes.len() || start > end {
        return Err(bad("the value is outside its buffer".into()));
    }
    let mut reader = Wkb {
        bytes: &bytes[..end],
        pos: start,
        buffer,
        row,
        out,
    };
    reader.geometry(0).map_err(bad)?;
    if reader.pos != end {
        return Err(bad(format!(
            "{} bytes left over after the geometry",
            end - reader.pos
        )));
    }
    Ok(())
}

struct Wkb<'a, 'b> {
    bytes: &'a [u8],
    pos: usize,
    buffer: usize,
    row: usize,
    out: &'b mut Vec<WkbCoord>,
}

impl Wkb<'_, '_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len());
        let end = end.ok_or("the value ends inside a geometry")?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn u32(&mut self, le: bool) -> Result<usize, String> {
        let b: [u8; 4] = self.take(4)?.try_into().expect("four bytes");
        Ok(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        } as usize)
    }

    fn points(&mut self, n: usize, dims: usize, le: bool, z: bool) -> Result<(), String> {
        let size = dims * 8;
        let start = self.pos;
        let total = n.checked_mul(size).ok_or("too many points")?;
        self.take(total)?;
        self.out.extend((0..n).map(|k| WkbCoord {
            buffer: self.buffer,
            pos: start + k * size,
            le,
            z,
            row: self.row,
        }));
        Ok(())
    }

    /// One geometry, with its own byte order and type. ISO and EWKB types
    /// (Z, M, SRID flags) are read; curves are not.
    fn geometry(&mut self, depth: usize) -> Result<(), String> {
        if depth > 64 {
            return Err("geometries nested more than 64 deep".into());
        }
        let le = match self.take(1)?[0] {
            0 => false,
            1 => true,
            b => return Err(format!("byte order {b}")),
        };
        let raw = self.u32(le)?;
        let mut z = raw & 0x8000_0000 != 0;
        let mut m = raw & 0x4000_0000 != 0;
        if raw & 0x2000_0000 != 0 {
            self.take(4)?;
        }
        let code = raw & 0x1fff_ffff;
        match code / 1000 {
            0 => {}
            1 => z = true,
            2 => m = true,
            3 => (z, m) = (true, true),
            _ => return Err(format!("geometry type {raw}")),
        }
        let dims = 2 + z as usize + m as usize;
        match code % 1000 {
            1 => self.points(1, dims, le, z),
            2 => {
                let n = self.u32(le)?;
                self.points(n, dims, le, z)
            }
            3 | 17 => {
                for _ in 0..self.u32(le)? {
                    let n = self.u32(le)?;
                    self.points(n, dims, le, z)?;
                }
                Ok(())
            }
            4..=7 | 15 | 16 => {
                for _ in 0..self.u32(le)? {
                    self.geometry(depth + 1)?;
                }
                Ok(())
            }
            8..=14 => Err("curved geometries aren't supported".into()),
            _ => Err(format!("geometry type {raw}")),
        }
    }
}

fn read_f64(bytes: &[u8], pos: usize, le: bool) -> f64 {
    let b: [u8; 8] = bytes[pos..pos + 8].try_into().expect("eight bytes");
    if le {
        f64::from_le_bytes(b)
    } else {
        f64::from_be_bytes(b)
    }
}

fn write_f64(bytes: &mut [u8], pos: usize, le: bool, value: f64) {
    let b = if le {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    };
    bytes[pos..pos + 8].copy_from_slice(&b);
}

/// Convert the coordinates found, those with z and those without apart.
fn convert_wkb(
    buffers: &mut [Vec<u8>],
    coords: &[WkbCoord],
    op: &mut dyn Op,
) -> Result<(), Failure> {
    for with_z in [false, true] {
        let mine: Vec<&WkbCoord> = coords.iter().filter(|c| c.z == with_z).collect();
        if mine.is_empty() {
            continue;
        }
        let mut g = Gathered::new(with_z);
        for c in &mine {
            let b = &buffers[c.buffer];
            let z = with_z.then(|| read_f64(b, c.pos + 16, c.le));
            g.push(
                read_f64(b, c.pos, c.le),
                read_f64(b, c.pos + 8, c.le),
                z,
                c.row,
            );
        }
        op.points(&mut g)?;
        for (k, c) in mine.iter().enumerate() {
            let b = &mut buffers[c.buffer];
            write_f64(b, c.pos, c.le, g.x[k]);
            write_f64(b, c.pos + 8, c.le, g.y[k]);
            if let Some(z) = &g.z {
                write_f64(b, c.pos + 16, c.le, z[k]);
            }
        }
    }
    Ok(())
}

// The C Stream Interface, for arrays -------------------------------------------
//
// arrow-rs exports streams of record batches only; a chunked geometry
// column is a stream of plain arrays.

struct Exported {
    field: FieldRef,
    chunks: VecDeque<ArrayRef>,
    error: Option<CString>,
}

const EIO: c_int = 5;

fn export_stream(field: FieldRef, chunks: Vec<ArrayRef>) -> FFI_ArrowArrayStream {
    let private = Box::new(Exported {
        field,
        chunks: chunks.into(),
        error: None,
    });
    FFI_ArrowArrayStream {
        get_schema: Some(stream_get_schema),
        get_next: Some(stream_get_next),
        get_last_error: Some(stream_get_last_error),
        release: Some(stream_release),
        private_data: Box::into_raw(private) as *mut c_void,
    }
}

/// # Safety
/// `stream` is a live stream made by `export_stream`.
unsafe fn exported<'a>(stream: *mut FFI_ArrowArrayStream) -> &'a mut Exported {
    unsafe { &mut *((*stream).private_data as *mut Exported) }
}

unsafe extern "C" fn stream_get_schema(
    stream: *mut FFI_ArrowArrayStream,
    out: *mut FFI_ArrowSchema,
) -> c_int {
    let private = unsafe { exported(stream) };
    match FFI_ArrowSchema::try_from(private.field.as_ref()) {
        Ok(schema) => {
            unsafe { std::ptr::write(out, schema) };
            0
        }
        Err(e) => {
            private.error = CString::new(e.to_string()).ok();
            EIO
        }
    }
}

unsafe extern "C" fn stream_get_next(
    stream: *mut FFI_ArrowArrayStream,
    out: *mut FFI_ArrowArray,
) -> c_int {
    let private = unsafe { exported(stream) };
    let array = match private.chunks.pop_front() {
        Some(chunk) => FFI_ArrowArray::new(&chunk.to_data()),
        None => FFI_ArrowArray::empty(),
    };
    unsafe { std::ptr::write(out, array) };
    0
}

unsafe extern "C" fn stream_get_last_error(stream: *mut FFI_ArrowArrayStream) -> *const c_char {
    let private = unsafe { exported(stream) };
    private
        .error
        .as_ref()
        .map_or(std::ptr::null(), |e| e.as_ptr())
}

unsafe extern "C" fn stream_release(stream: *mut FFI_ArrowArrayStream) {
    if stream.is_null() {
        return;
    }
    let stream = unsafe { &mut *stream };
    if !stream.private_data.is_null() {
        drop(unsafe { Box::from_raw(stream.private_data as *mut Exported) });
    }
    stream.private_data = std::ptr::null_mut();
    stream.get_schema = None;
    stream.get_next = None;
    stream.get_last_error = None;
    stream.release = None;
}

/// The message of a stream's failed call.
fn stream_error(stream: &mut FFI_ArrowArrayStream, rc: c_int) -> String {
    let message = stream.get_last_error.and_then(|get| {
        // SAFETY: the stream's own callback; the string lives until the
        // next call on the stream.
        let ptr = unsafe { get(stream) };
        (!ptr.is_null()).then(|| {
            unsafe { std::ffi::CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned()
        })
    });
    message.unwrap_or_else(|| format!("the stream failed with error {rc}"))
}
