//! GeoArrow arrays that exist already, converted in their own layout
//! (`docs/pyproj-phase3.md`, GeoArrow): what `xeibe_proj.arrow` runs. With
//! the `arrow` feature, on for the crate itself and off for the
//! workspace's dependents.
//!
//! The layout decides the encoding: lists (and large lists) down to
//! interleaved (`FixedSizeList<double>`) or separated (`Struct<x, y, …>`)
//! coordinates, WKB in `Binary`, `LargeBinary` or `BinaryView`, and boxes
//! (`Struct<xmin, ymin, …, xmax, ymax, …>`). The coordinates are copied and
//! converted; offsets and validity buffers are shared with the input.
//! Coordinates under a null geometry, or outside every geometry of a sliced
//! array, are left as they are: they may be anything. Coordinates are taken
//! as the transform's axis order has them; GeoArrow's are easting first
//! ([`crate::AxisOrder::EastFirst`]).
//!
//! [`convert_chunks`] converts; [`extent`] gives the box the coordinates
//! cover, to load the grid data they need first
//! ([`Transform::grid_requests`]). Rows count across the chunks, so an
//! error names a geometry of the whole input.

use std::sync::Arc;

use crate::{Error, Transform};
use arrow_array::cast::AsArray;
use arrow_array::types::Float64Type;
use arrow_array::{
    Array, ArrayRef, BinaryViewArray, FixedSizeListArray, Float64Array, GenericBinaryArray,
    GenericListArray, OffsetSizeTrait, StructArray,
};
use arrow_buffer::{Buffer, NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow_schema::{DataType, Fields};

/// A row that no geometry reaches: under a null, or outside a sliced
/// array's offsets.
const DEAD: usize = usize::MAX;

/// The points on each edge a box is densified with, as pyproj's
/// `transform_bounds` default.
const DENSIFY: usize = 21;

/// Why an array couldn't be converted.
#[derive(Debug, thiserror::Error)]
pub enum ArrayError {
    /// Not a GeoArrow layout this reads, or malformed WKB. The message
    /// names the geometry where there is one.
    #[error("{0}")]
    Layout(String),
    /// A coordinate of geometry `row` (counted over every chunk) failed.
    #[error("{}", convert_message(*row, error))]
    Convert {
        row: Option<usize>,
        #[source]
        error: Error,
    },
}

fn convert_message(row: Option<usize>, error: &Error) -> String {
    let place = row.map_or(String::new(), |row| format!("geometry {row}: "));
    match error {
        Error::Coordinate { x, y, reason, .. } => format!("{place}coordinate ({x}, {y}): {reason}"),
        error => format!("{place}{error}"),
    }
}

fn layout(message: impl std::fmt::Display) -> ArrayError {
    ArrayError::Layout(message.to_string())
}

/// Every chunk converted by `transform`, in the same layout.
pub fn convert_chunks(
    chunks: &[ArrayRef],
    transform: &Transform,
) -> Result<Vec<ArrayRef>, ArrayError> {
    walk_chunks(chunks, &mut Converting(transform))
}

/// One array converted by `transform`, in the same layout.
pub fn convert_array(array: &ArrayRef, transform: &Transform) -> Result<ArrayRef, ArrayError> {
    let mut out = convert_chunks(std::slice::from_ref(array), transform)?;
    Ok(out.pop().expect("one chunk in, one out"))
}

/// The box `[min_x, min_y, max_x, max_y]` around every finite coordinate
/// [`convert_chunks`] would convert (a box's corners for boxes), or
/// `None` when there is none.
pub fn extent(chunks: &[ArrayRef]) -> Result<Option<[f64; 4]>, ArrayError> {
    let mut extent = Extent::default();
    walk_chunks(chunks, &mut extent)?;
    Ok(extent.bounds())
}

fn walk_chunks(chunks: &[ArrayRef], op: &mut dyn Op) -> Result<Vec<ArrayRef>, ArrayError> {
    let mut base = 0;
    let mut out = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let nulls = chunk.logical_nulls();
        let rows: Vec<usize> = (0..chunk.len())
            .map(|i| match &nulls {
                Some(n) if n.is_null(i) => DEAD,
                _ => base + i,
            })
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
    fn with_capacity(z: bool, n: usize) -> Gathered {
        Gathered {
            x: Vec::with_capacity(n),
            y: Vec::with_capacity(n),
            z: z.then(|| Vec::with_capacity(n)),
            rows: Vec::with_capacity(n),
        }
    }

    fn apply(&mut self, op: &mut dyn Op) -> Result<(), ArrayError> {
        op.points(&mut self.x, &mut self.y, self.z.as_deref_mut(), &self.rows)
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

/// What a walk does with the coordinates it finds, in place. `rows[k]` is
/// the geometry of the k-th coordinate.
trait Op {
    /// Separate x, y and z buffers of one length.
    fn points(
        &mut self,
        x: &mut [f64],
        y: &mut [f64],
        z: Option<&mut [f64]>,
        rows: &[usize],
    ) -> Result<(), ArrayError>;
    /// Interleaved coordinates of `dims.count` values each.
    fn interleaved(
        &mut self,
        coords: &mut [f64],
        dims: Dims,
        rows: &[usize],
    ) -> Result<(), ArrayError>;
    /// Boxes as `[min_x, min_y, max_x, max_y]`, one for each row.
    fn boxes(&mut self, boxes: &mut [[f64; 4]], rows: &[usize]) -> Result<(), ArrayError>;
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
    fn points(
        &mut self,
        x: &mut [f64],
        y: &mut [f64],
        _z: Option<&mut [f64]>,
        _rows: &[usize],
    ) -> Result<(), ArrayError> {
        x.iter().zip(y.iter()).for_each(|(&x, &y)| self.add(x, y));
        Ok(())
    }

    fn interleaved(
        &mut self,
        coords: &mut [f64],
        dims: Dims,
        _rows: &[usize],
    ) -> Result<(), ArrayError> {
        coords
            .chunks_exact(dims.count)
            .for_each(|c| self.add(c[0], c[1]));
        Ok(())
    }

    fn boxes(&mut self, boxes: &mut [[f64; 4]], _rows: &[usize]) -> Result<(), ArrayError> {
        for b in boxes.iter() {
            self.add(b[0], b[1]);
            self.add(b[2], b[3]);
        }
        Ok(())
    }
}

/// Converts what it is given.
struct Converting<'a>(&'a Transform);

/// A conversion's error, with the row of the coordinate that failed.
fn failed(error: Error, rows: &[usize]) -> ArrayError {
    let row = match &error {
        Error::Coordinate { index, .. } => rows.get(*index).copied(),
        _ => None,
    };
    ArrayError::Convert { row, error }
}

impl Op for Converting<'_> {
    fn points(
        &mut self,
        x: &mut [f64],
        y: &mut [f64],
        z: Option<&mut [f64]>,
        rows: &[usize],
    ) -> Result<(), ArrayError> {
        match z {
            Some(z) => self.0.convert_xyz(x, y, z),
            None => self.0.convert_xy(x, y),
        }
        .map_err(|e| failed(e, rows))
    }

    fn interleaved(
        &mut self,
        coords: &mut [f64],
        dims: Dims,
        rows: &[usize],
    ) -> Result<(), ArrayError> {
        // convert_interleaved takes a third value as z: not for XYM.
        if dims.z || dims.count == 2 {
            return self
                .0
                .convert_interleaved(coords, dims.count)
                .map_err(|e| failed(e, rows));
        }
        let n = coords.len() / dims.count;
        let mut g = Gathered::with_capacity(false, n);
        for (c, &row) in coords.chunks_exact(dims.count).zip(rows) {
            g.push(c[0], c[1], None, row);
        }
        g.apply(self)?;
        for (k, c) in coords.chunks_exact_mut(dims.count).enumerate() {
            c[0] = g.x[k];
            c[1] = g.y[k];
        }
        Ok(())
    }

    fn boxes(&mut self, boxes: &mut [[f64; 4]], rows: &[usize]) -> Result<(), ArrayError> {
        for (b, &row) in boxes.iter_mut().zip(rows) {
            *b = self
                .0
                .convert_bounds(*b, DENSIFY)
                .map_err(|error| ArrayError::Convert {
                    row: Some(row),
                    error,
                })?;
        }
        Ok(())
    }
}

// The layouts --------------------------------------------------------------------

/// Which of a coordinate's values are x, y and z: x and y first, then z
/// when there is one; M is left alone.
#[derive(Clone, Copy, PartialEq, Eq)]
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
fn walk(array: &ArrayRef, rows: &[usize], op: &mut dyn Op) -> Result<ArrayRef, ArrayError> {
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
    nulls: Option<&NullBuffer>,
    rows: &[usize],
    child_len: usize,
) -> Vec<usize> {
    let mut out = vec![DEAD; child_len];
    for (i, w) in offsets.windows(2).enumerate() {
        if rows[i] == DEAD || nulls.is_some_and(|n| n.is_null(i)) {
            continue;
        }
        let (start, end) = (w[0].as_usize(), w[1].as_usize());
        if start <= end && end <= child_len {
            out[start..end].fill(rows[i]);
        }
    }
    out
}

fn live(rows: &[usize], nulls: Option<&NullBuffer>, i: usize) -> Option<usize> {
    (rows[i] != DEAD && nulls.is_none_or(|n| n.is_valid(i))).then_some(rows[i])
}

/// Whether every element is a live coordinate: then the buffers are
/// converted as they are, without gathering.
fn all_live(rows: &[usize], nulls: Option<&NullBuffer>) -> bool {
    nulls.is_none_or(|n| n.null_count() == 0) && rows.iter().all(|&r| r != DEAD)
}

fn list<O: OffsetSizeTrait>(
    l: &GenericListArray<O>,
    rows: &[usize],
    op: &mut dyn Op,
) -> Result<ArrayRef, ArrayError> {
    let child = child_rows(l.offsets(), l.nulls(), rows, l.values().len());
    let values = walk(l.values(), &child, op)?;
    let (field, offsets, _, nulls) = l.clone().into_parts();
    Ok(Arc::new(
        GenericListArray::<O>::try_new(field, offsets, values, nulls).map_err(layout)?,
    ))
}

fn doubles<'a>(array: &'a ArrayRef, what: &str) -> Result<&'a Float64Array, ArrayError> {
    array
        .as_primitive_opt::<Float64Type>()
        .ok_or_else(|| layout(format!("{what} are {}, not double", array.data_type())))
}

/// Coordinates `x0 y0 [z0] [m0] x1 …` in a `FixedSizeList<double>`.
fn interleaved(
    l: &FixedSizeListArray,
    rows: &[usize],
    op: &mut dyn Op,
) -> Result<ArrayRef, ArrayError> {
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
    if all_live(rows, l.nulls()) {
        op.interleaved(&mut out[..l.len() * size], dims, rows)?;
    } else {
        let mut g = Gathered::with_capacity(dims.z, l.len());
        let mut at = Vec::with_capacity(l.len());
        for i in 0..l.len() {
            if let Some(row) = live(rows, l.nulls(), i) {
                let c = &out[i * size..i * size + size];
                g.push(c[0], c[1], dims.z.then(|| c[2]), row);
                at.push(i);
            }
        }
        g.apply(op)?;
        for (k, &i) in at.iter().enumerate() {
            out[i * size] = g.x[k];
            out[i * size + 1] = g.y[k];
            if let Some(z) = &g.z {
                out[i * size + 2] = z[k];
            }
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
fn separated(s: &StructArray, rows: &[usize], op: &mut dyn Op) -> Result<ArrayRef, ArrayError> {
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
    if all_live(rows, s.nulls()) {
        let [x, y, rest @ ..] = cols.as_mut_slice() else {
            unreachable!("Dims has x and y");
        };
        let z = if dims.z { rest.first_mut() } else { None };
        op.points(x, y, z.map(Vec::as_mut_slice), rows)?;
    } else {
        let mut g = Gathered::with_capacity(dims.z, s.len());
        let mut at = Vec::with_capacity(s.len());
        for (i, (&x, &y)) in cols[0].iter().zip(&cols[1]).enumerate() {
            if let Some(row) = live(rows, s.nulls(), i) {
                g.push(x, y, dims.z.then(|| cols[2][i]), row);
                at.push(i);
            }
        }
        g.apply(op)?;
        for (k, &i) in at.iter().enumerate() {
            cols[0][i] = g.x[k];
            cols[1][i] = g.y[k];
            if let Some(z) = &g.z {
                cols[2][i] = z[k];
            }
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
fn boxes(s: &StructArray, rows: &[usize], op: &mut dyn Op) -> Result<ArrayRef, ArrayError> {
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
        if let Some(row) = live(rows, s.nulls(), i) {
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
) -> Result<ArrayRef, ArrayError> {
    let (offsets, values, nulls) = b.clone().into_parts();
    let mut buffers = vec![values.as_slice().to_vec()];
    // At least 16 bytes for each coordinate.
    let mut coords = Vec::with_capacity(values.len() / 16);
    for i in 0..b.len() {
        if let Some(row) = live(rows, b.nulls(), i) {
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
fn wkb_view(b: &BinaryViewArray, rows: &[usize], op: &mut dyn Op) -> Result<ArrayRef, ArrayError> {
    let (views, data, nulls) = b.clone().into_parts();
    let mut buffers: Vec<Vec<u8>> = data.iter().map(|d| d.as_slice().to_vec()).collect();
    let mut coords = Vec::with_capacity(buffers.iter().map(Vec::len).sum::<usize>() / 16);
    for i in 0..b.len() {
        let Some(row) = live(rows, b.nulls(), i) else {
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
) -> Result<(), ArrayError> {
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
) -> Result<(), ArrayError> {
    let with_z = coords.iter().filter(|c| c.z).count();
    let mut flat = Gathered::with_capacity(false, coords.len() - with_z);
    let mut deep = Gathered::with_capacity(true, with_z);
    for c in coords {
        let b = &buffers[c.buffer];
        let (x, y) = (read_f64(b, c.pos, c.le), read_f64(b, c.pos + 8, c.le));
        if c.z {
            deep.push(x, y, Some(read_f64(b, c.pos + 16, c.le)), c.row);
        } else {
            flat.push(x, y, None, c.row);
        }
    }
    for g in [&mut flat, &mut deep] {
        if !g.x.is_empty() {
            g.apply(op)?;
        }
    }
    let (mut f, mut d) = (0, 0);
    for c in coords {
        let b = &mut buffers[c.buffer];
        let (g, k) = if c.z {
            d += 1;
            (&deep, d - 1)
        } else {
            f += 1;
            (&flat, f - 1)
        };
        write_f64(b, c.pos, c.le, g.x[k]);
        write_f64(b, c.pos + 8, c.le, g.y[k]);
        if let Some(z) = &g.z {
            write_f64(b, c.pos + 16, c.le, z[k]);
        }
    }
    Ok(())
}
