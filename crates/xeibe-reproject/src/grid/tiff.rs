//! GeoTIFF grids as PROJ-data publishes them: classic TIFF, tiled or
//! striped, uncompressed, DEFLATE or LZW, with the horizontal or
//! floating-point predictor, one or several samples (contiguous or in
//! separate planes), and one image per grid or subgrid.
//!
//! The reading semantics are PROJ 9.8.1's `GTiffDataset::nextGrid` and
//! `GTiffGrid` (`src/grids.cpp`, MIT, see PROJ-NOTICE.md): the extent and
//! pixel convention, rows counted from the south, offset and scale from
//! GDAL's metadata applied in single precision, and the nodata value.
//!
//! Parsing works on whatever byte ranges of the file have been loaded
//! ([`Ranges`]), and asks for the range it is missing.

use std::collections::{BTreeMap, HashMap};
use std::f64::consts::PI;
use std::io::Read;
use std::sync::{Arc, OnceLock};

use super::set::ExtentAndRes;

/// Why a file can't be read (yet).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TiffError {
    /// These bytes are needed first.
    Need { offset: u64, length: u64 },
    /// The file is not a grid PROJ could read.
    Invalid(String),
}

fn invalid<T>(message: impl Into<String>) -> Result<T, TiffError> {
    Err(TiffError::Invalid(message.into()))
}

/// The loaded byte ranges of one file, merged where they touch.
#[derive(Debug, Default)]
pub(crate) struct Ranges {
    chunks: BTreeMap<u64, Vec<u8>>,
}

impl Ranges {
    pub(crate) fn insert(&mut self, offset: u64, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        let mut start = offset;
        let mut bytes = data;
        // Merge with every chunk that overlaps or touches [start, end].
        loop {
            let end = start + bytes.len() as u64;
            let touching = self
                .chunks
                .range(..=end)
                .rev()
                .find(|(s, c)| **s + c.len() as u64 >= start)
                .map(|(s, _)| *s);
            let Some(s) = touching else { break };
            let chunk = self.chunks.remove(&s).unwrap_or_default();
            let chunk_end = s + chunk.len() as u64;
            let new_start = start.min(s);
            let new_end = end.max(chunk_end);
            let mut merged = vec![0u8; (new_end - new_start) as usize];
            merged[(s - new_start) as usize..(chunk_end - new_start) as usize]
                .copy_from_slice(&chunk);
            merged[(start - new_start) as usize..(end - new_start) as usize]
                .copy_from_slice(&bytes);
            start = new_start;
            bytes = merged;
        }
        self.chunks.insert(start, bytes);
    }

    /// The bytes at `offset..offset + length`, if loaded.
    pub(crate) fn get(&self, offset: u64, length: u64) -> Result<&[u8], TiffError> {
        if let Some((s, c)) = self.chunks.range(..=offset).next_back()
            && offset + length <= s + c.len() as u64
        {
            let from = (offset - s) as usize;
            return Ok(&c[from..from + length as usize]);
        }
        Err(TiffError::Need { offset, length })
    }

    /// Whether anything at all was loaded.
    pub(crate) fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteOrder {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataType {
    Int16,
    UInt16,
    Int32,
    UInt32,
    Float32,
    Float64,
}

impl DataType {
    fn size(self) -> usize {
        match self {
            DataType::Int16 | DataType::UInt16 => 2,
            DataType::Int32 | DataType::UInt32 | DataType::Float32 => 4,
            DataType::Float64 => 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compression {
    None,
    Lzw,
    Deflate,
}

/// One image (IFD) of the file: a grid or subgrid.
#[derive(Debug)]
pub(crate) struct Image {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) extent: ExtentAndRes,
    pub(crate) samples: usize,
    pub(crate) subfile_type: u32,
    /// GDAL metadata items by (sample or -1, name).
    metadata: HashMap<(i32, String), String>,
    bottom_up: bool,
    separate_planes: bool,
    data_type: DataType,
    order: ByteOrder,
    compression: Compression,
    predictor: u16,
    block_width: usize,
    block_height: usize,
    blocks_per_row: usize,
    /// Blocks in one plane.
    blocks: usize,
    offsets: Vec<u64>,
    counts: Vec<u64>,
    nodata: Option<f32>,
    /// Offset and scale per sample, when GDAL's metadata gives one.
    offset_scale: Vec<(f64, f64)>,
    decoded: Vec<OnceLock<Arc<[f32]>>>,
}

/// The value at a node couldn't be read: its block isn't loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NotLoaded;

impl Image {
    /// GDAL metadata item `name` of the image (`sample` -1) or of a sample.
    pub(crate) fn metadata(&self, name: &str, sample: i32) -> Option<&str> {
        self.metadata
            .get(&(sample, name.to_string()))
            .map(String::as_str)
    }

    /// `GTiffGrid::valueAt`: the value of `sample` at column `x` and row
    /// `y` counted from the south.
    pub(crate) fn value(&self, sample: usize, x: usize, y: usize) -> Result<f32, NotLoaded> {
        let y_tiff = if self.bottom_up {
            y
        } else {
            self.height - 1 - y
        };
        let (block_x, x_off) = (x / self.block_width, x % self.block_width);
        let (block_y, y_off) = (y_tiff / self.block_height, y_tiff % self.block_height);
        let mut block = block_y * self.blocks_per_row + block_x;
        if self.separate_planes {
            block += sample * self.blocks;
        }
        let data = self.decoded[block].get().ok_or(NotLoaded)?;
        let mut at = x_off + y_off * self.block_width;
        if !self.separate_planes {
            at = at * self.samples + sample;
        }
        Ok(data[at])
    }

    /// The blocks holding columns `x0..=x1` and rows `y0..=y1` (from the
    /// south) of every sample.
    pub(crate) fn blocks_for(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<usize> {
        let row = |y: usize| {
            if self.bottom_up {
                y
            } else {
                self.height - 1 - y
            }
        };
        let (r0, r1) = (row(y0).min(row(y1)), row(y0).max(row(y1)));
        let mut out = Vec::new();
        let planes = if self.separate_planes {
            self.samples
        } else {
            1
        };
        for plane in 0..planes {
            for by in r0 / self.block_height..=r1 / self.block_height {
                for bx in x0 / self.block_width..=x1 / self.block_width {
                    out.push(plane * self.blocks + by * self.blocks_per_row + bx);
                }
            }
        }
        out
    }

    pub(crate) fn block_count(&self) -> usize {
        self.decoded.len()
    }

    pub(crate) fn is_decoded(&self, block: usize) -> bool {
        self.decoded[block].get().is_some()
    }

    /// Where the block's bytes are in the file.
    pub(crate) fn block_range(&self, block: usize) -> (u64, u64) {
        (self.offsets[block], self.counts[block])
    }

    /// Decode the block from the file's loaded bytes, if they are there.
    pub(crate) fn decode(&self, block: usize, ranges: &Ranges) -> Result<(), TiffError> {
        if self.is_decoded(block) {
            return Ok(());
        }
        let (offset, count) = self.block_range(block);
        if count == 0 {
            return invalid("empty block");
        }
        let raw = ranges.get(offset, count)?;
        let values = self.decode_block(block, raw)?;
        let _ = self.decoded[block].set(values.into());
        Ok(())
    }

    fn decode_block(&self, block: usize, raw: &[u8]) -> Result<Vec<f32>, TiffError> {
        let bytes = match self.compression {
            Compression::None => raw.to_vec(),
            Compression::Deflate => {
                let mut out = Vec::new();
                flate2::read::ZlibDecoder::new(raw)
                    .read_to_end(&mut out)
                    .map_err(|e| TiffError::Invalid(format!("DEFLATE: {e}")))?;
                out
            }
            Compression::Lzw => super::lzw::decode(raw).map_err(TiffError::Invalid)?,
        };
        let per_pixel = if self.separate_planes {
            1
        } else {
            self.samples
        };
        let row_values = self.block_width * per_pixel;
        let size = self.data_type.size();
        let row_bytes = row_values * size;
        let rows = bytes.len() / row_bytes;
        if rows == 0 {
            return invalid("block shorter than one row");
        }
        let mut bytes = bytes;
        bytes.truncate(rows * row_bytes);
        let plane = if self.separate_planes {
            block / self.blocks
        } else {
            0
        };
        let mut out = Vec::with_capacity(rows * row_values);
        for row in bytes.chunks_exact_mut(row_bytes) {
            match self.predictor {
                1 => {}
                2 => horizontal_predictor(row, size, per_pixel, self.order),
                3 => floating_point_predictor(row, size, per_pixel, self.order),
                p => return invalid(format!("predictor {p}")),
            }
            for (i, v) in row.chunks_exact(size).enumerate() {
                let sample = if self.separate_planes {
                    plane
                } else {
                    i % self.samples
                };
                out.push(self.read_value(v, sample));
            }
        }
        Ok(out)
    }

    /// `GTiffGrid::readValue`: the offset and scale apply unless the value
    /// is nodata, and the result is a float.
    fn read_value(&self, v: &[u8], sample: usize) -> f32 {
        let raw: f64 = match (self.data_type, self.order) {
            (DataType::Int16, ByteOrder::Little) => i16::from_le_bytes([v[0], v[1]]).into(),
            (DataType::Int16, ByteOrder::Big) => i16::from_be_bytes([v[0], v[1]]).into(),
            (DataType::UInt16, ByteOrder::Little) => u16::from_le_bytes([v[0], v[1]]).into(),
            (DataType::UInt16, ByteOrder::Big) => u16::from_be_bytes([v[0], v[1]]).into(),
            (DataType::Int32, o) => i32_bytes(v, o).into(),
            (DataType::UInt32, o) => (i32_bytes(v, o) as u32).into(),
            (DataType::Float32, o) => f32::from_bits(i32_bytes(v, o) as u32).into(),
            (DataType::Float64, ByteOrder::Little) => {
                f64::from_le_bytes(v.try_into().expect("8 bytes"))
            }
            (DataType::Float64, ByteOrder::Big) => {
                f64::from_be_bytes(v.try_into().expect("8 bytes"))
            }
        };
        let is_nodata = self.nodata.is_some_and(|n| raw as f32 == n);
        match self.offset_scale.get(sample) {
            Some((offset, scale)) if !is_nodata => (raw * scale + offset) as f32,
            _ => raw as f32,
        }
    }

    /// `GTiffGrid::isNodata`.
    pub(crate) fn is_nodata(&self, v: f32) -> bool {
        self.nodata.is_some_and(|n| v == n) || v.is_nan()
    }
}

fn i32_bytes(v: &[u8], order: ByteOrder) -> i32 {
    let b = [v[0], v[1], v[2], v[3]];
    match order {
        ByteOrder::Little => i32::from_le_bytes(b),
        ByteOrder::Big => i32::from_be_bytes(b),
    }
}

/// Undo TIFF's horizontal differencing on one row, in the file's byte order.
fn horizontal_predictor(row: &mut [u8], size: usize, stride: usize, order: ByteOrder) {
    let n = row.len() / size;
    let get = |row: &[u8], i: usize| -> u64 {
        let b = &row[i * size..(i + 1) * size];
        let mut v = 0u64;
        match order {
            ByteOrder::Little => {
                for (k, byte) in b.iter().enumerate() {
                    v |= u64::from(*byte) << (8 * k);
                }
            }
            ByteOrder::Big => {
                for byte in b {
                    v = (v << 8) | u64::from(*byte);
                }
            }
        }
        v
    };
    let mask = if size == 8 {
        u64::MAX
    } else {
        (1u64 << (8 * size)) - 1
    };
    for i in stride..n {
        let v = get(row, i).wrapping_add(get(row, i - stride)) & mask;
        let b = &mut row[i * size..(i + 1) * size];
        match order {
            ByteOrder::Little => {
                for (k, byte) in b.iter_mut().enumerate() {
                    *byte = (v >> (8 * k)) as u8;
                }
            }
            ByteOrder::Big => {
                for (k, byte) in b.iter_mut().enumerate() {
                    *byte = (v >> (8 * (size - 1 - k))) as u8;
                }
            }
        }
    }
}

/// Undo TIFF's floating-point predictor (libtiff `fpAcc`) on one row: byte
/// differencing, then byte planes, most significant first, back into
/// values in the file's byte order.
fn floating_point_predictor(row: &mut [u8], size: usize, stride: usize, order: ByteOrder) {
    let byte_stride = stride;
    for i in byte_stride..row.len() {
        row[i] = row[i].wrapping_add(row[i - byte_stride]);
    }
    let count = row.len() / size;
    let planes = row.to_vec();
    for i in 0..count {
        for b in 0..size {
            // Plane 0 holds the most significant bytes.
            let byte = planes[b * count + i];
            let at = match order {
                ByteOrder::Big => b,
                ByteOrder::Little => size - 1 - b,
            };
            row[i * size + at] = byte;
        }
    }
}

struct Reader<'a> {
    ranges: &'a Ranges,
    order: ByteOrder,
}

impl Reader<'_> {
    fn bytes(&self, offset: u64, length: u64) -> Result<&[u8], TiffError> {
        self.ranges.get(offset, length)
    }

    fn u16(&self, offset: u64) -> Result<u16, TiffError> {
        let b = self.bytes(offset, 2)?;
        Ok(match self.order {
            ByteOrder::Little => u16::from_le_bytes([b[0], b[1]]),
            ByteOrder::Big => u16::from_be_bytes([b[0], b[1]]),
        })
    }

    fn u32(&self, offset: u64) -> Result<u32, TiffError> {
        Ok(i32_bytes(self.bytes(offset, 4)?, self.order) as u32)
    }
}

/// One IFD entry.
#[derive(Debug, Clone, Copy)]
struct Entry {
    field_type: u16,
    count: u64,
    /// Where the values are: inline in the entry or elsewhere.
    at: u64,
}

fn type_size(field_type: u16) -> Option<u64> {
    Some(match field_type {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

impl Reader<'_> {
    fn entry(&self, at: u64) -> Result<(u16, Entry), TiffError> {
        let tag = self.u16(at)?;
        let field_type = self.u16(at + 2)?;
        let count = u64::from(self.u32(at + 4)?);
        let size = type_size(field_type).unwrap_or(1) * count;
        let value_at = if size <= 4 {
            at + 8
        } else {
            u64::from(self.u32(at + 8)?)
        };
        Ok((
            tag,
            Entry {
                field_type,
                count,
                at: value_at,
            },
        ))
    }

    /// The values as unsigned integers (BYTE, SHORT, LONG).
    fn uints(&self, e: Entry) -> Result<Vec<u64>, TiffError> {
        let size = type_size(e.field_type).ok_or_else(|| TiffError::Invalid("tag type".into()))?;
        let b = self.bytes(e.at, size * e.count)?;
        Ok(b.chunks_exact(size as usize)
            .map(|c| match (size, self.order) {
                (1, _) => u64::from(c[0]),
                (2, ByteOrder::Little) => u64::from(u16::from_le_bytes([c[0], c[1]])),
                (2, ByteOrder::Big) => u64::from(u16::from_be_bytes([c[0], c[1]])),
                (4, o) => u64::from(i32_bytes(c, o) as u32),
                _ => 0,
            })
            .collect())
    }

    fn uint(&self, e: Entry) -> Result<u64, TiffError> {
        self.uints(e)?
            .first()
            .copied()
            .ok_or_else(|| TiffError::Invalid("empty tag".into()))
    }

    fn doubles(&self, e: Entry) -> Result<Vec<f64>, TiffError> {
        if e.field_type != 12 {
            return invalid("expected DOUBLE values");
        }
        let b = self.bytes(e.at, 8 * e.count)?;
        Ok(b.chunks_exact(8)
            .map(|c| {
                let a: [u8; 8] = c.try_into().expect("8 bytes");
                match self.order {
                    ByteOrder::Little => f64::from_le_bytes(a),
                    ByteOrder::Big => f64::from_be_bytes(a),
                }
            })
            .collect())
    }

    fn ascii(&self, e: Entry) -> Result<String, TiffError> {
        let b = self.bytes(e.at, e.count)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
}

/// Read every image of the file, as far as the loaded bytes allow.
pub(crate) fn parse(ranges: &Ranges) -> Result<Vec<Image>, TiffError> {
    let head = ranges.get(0, 8)?;
    let order = match &head[..4] {
        b"II*\0" => ByteOrder::Little,
        b"MM\0*" => ByteOrder::Big,
        [b'I', b'I', 43, 0] | [b'M', b'M', 0, 43] => return invalid("BigTIFF grids"),
        _ => return invalid("not a TIFF file"),
    };
    let r = Reader { ranges, order };
    let mut next = u64::from(r.u32(4)?);
    let mut images = Vec::new();
    while next != 0 {
        let count = u64::from(r.u16(next)?);
        // The whole directory and the next offset at once.
        r.bytes(next, 2 + 12 * count + 4)?;
        let mut tags = HashMap::new();
        for i in 0..count {
            let (tag, e) = r.entry(next + 2 + 12 * i)?;
            tags.insert(tag, e);
        }
        images.push(image(&r, &tags)?);
        next = u64::from(r.u32(next + 2 + 12 * count)?);
        if images.len() > 10_000 {
            return invalid("too many images");
        }
    }
    if images.is_empty() {
        return invalid("no image");
    }
    Ok(images)
}

const IMAGE_WIDTH: u16 = 256;
const IMAGE_LENGTH: u16 = 257;
const BITS_PER_SAMPLE: u16 = 258;
const COMPRESSION: u16 = 259;
const PHOTOMETRIC: u16 = 262;
const STRIP_OFFSETS: u16 = 273;
const SAMPLES_PER_PIXEL: u16 = 277;
const ROWS_PER_STRIP: u16 = 278;
const STRIP_BYTE_COUNTS: u16 = 279;
const PLANAR_CONFIG: u16 = 284;
const PREDICTOR: u16 = 317;
const TILE_WIDTH: u16 = 322;
const TILE_LENGTH: u16 = 323;
const TILE_OFFSETS: u16 = 324;
const TILE_BYTE_COUNTS: u16 = 325;
const SAMPLE_FORMAT: u16 = 339;
const NEW_SUBFILE_TYPE: u16 = 254;
const MODEL_PIXEL_SCALE: u16 = 33550;
const MODEL_TIEPOINT: u16 = 33922;
const MODEL_TRANSFORMATION: u16 = 34264;
const GEO_KEY_DIRECTORY: u16 = 34735;
const GDAL_METADATA: u16 = 42112;
const GDAL_NODATA: u16 = 42113;

fn image(r: &Reader, tags: &HashMap<u16, Entry>) -> Result<Image, TiffError> {
    let get = |tag: u16| tags.get(&tag).copied();
    let uint_or = |tag: u16, default: u64| -> Result<u64, TiffError> {
        match get(tag) {
            Some(e) => r.uint(e),
            None => Ok(default),
        }
    };
    let width = uint_or(IMAGE_WIDTH, 0)? as usize;
    let height = uint_or(IMAGE_LENGTH, 0)? as usize;
    if width == 0 || height == 0 || width > i32::MAX as usize || height > i32::MAX as usize {
        return invalid("invalid image size");
    }
    let Some(samples) = get(SAMPLES_PER_PIXEL) else {
        return invalid("missing SamplesPerPixel");
    };
    let samples = r.uint(samples)? as usize;
    if samples == 0 {
        return invalid("invalid SamplesPerPixel");
    }
    let Some(bits) = get(BITS_PER_SAMPLE) else {
        return invalid("missing BitsPerSample");
    };
    let bits = r.uint(bits)?;
    let Some(planar) = get(PLANAR_CONFIG) else {
        return invalid("missing PlanarConfig");
    };
    let planar = r.uint(planar)?;
    let Some(format) = get(SAMPLE_FORMAT) else {
        return invalid("missing SampleFormat");
    };
    let data_type = match (r.uint(format)?, bits) {
        (2, 16) => DataType::Int16,
        (1, 16) => DataType::UInt16,
        (2, 32) => DataType::Int32,
        (1, 32) => DataType::UInt32,
        (3, 32) => DataType::Float32,
        (3, 64) => DataType::Float64,
        _ => return invalid("unsupported SampleFormat and BitsPerSample"),
    };
    if uint_or(PHOTOMETRIC, 1)? != 1 {
        return invalid("unsupported Photometric");
    }
    let compression = match uint_or(COMPRESSION, 1)? {
        1 => Compression::None,
        5 => Compression::Lzw,
        8 | 32946 => Compression::Deflate,
        c => return invalid(format!("compression {c}")),
    };
    let predictor = uint_or(PREDICTOR, 1)? as u16;
    let subfile_type = uint_or(NEW_SUBFILE_TYPE, 0)? as u32;

    let tiled = get(TILE_WIDTH).is_some();
    let (block_width, block_height, offsets, counts) = if tiled {
        let (Some(w), Some(h), Some(o), Some(c)) = (
            get(TILE_WIDTH),
            get(TILE_LENGTH),
            get(TILE_OFFSETS),
            get(TILE_BYTE_COUNTS),
        ) else {
            return invalid("incomplete tile tags");
        };
        (
            r.uint(w)? as usize,
            r.uint(h)? as usize,
            r.uints(o)?,
            r.uints(c)?,
        )
    } else {
        let (Some(o), Some(c)) = (get(STRIP_OFFSETS), get(STRIP_BYTE_COUNTS)) else {
            return invalid("incomplete strip tags");
        };
        let rows = uint_or(ROWS_PER_STRIP, height as u64)? as usize;
        (width, rows.min(height), r.uints(o)?, r.uints(c)?)
    };
    if block_width == 0 || block_height == 0 {
        return invalid("invalid block size");
    }
    if (block_width * block_height * data_type.size() * samples) as u64 > 64 * 1024 * 2014 {
        return invalid("unsupported block size");
    }
    let blocks_per_row = width.div_ceil(block_width);
    let blocks = blocks_per_row * height.div_ceil(block_height);
    let separate_planes = samples > 1 && planar == 2;
    let total = if separate_planes {
        blocks * samples
    } else {
        blocks
    };
    if offsets.len() < total || counts.len() < total {
        return invalid("missing block offsets");
    }

    // GeoKeys: model type and raster type.
    let mut geographic = true;
    let mut pixel_is_area = false;
    if let Some(e) = get(GEO_KEY_DIRECTORY) {
        let keys = r.uints(e)?;
        if keys.len() < 4 || keys.len() % 4 != 0 {
            return invalid("wrong number of values in GeoKeys");
        }
        if keys[0] != 1 {
            return invalid("unsupported GeoTIFF major version");
        }
        for k in keys[4..].chunks_exact(4) {
            match k[0] {
                1024 => match k[3] {
                    1 => geographic = false,
                    2 => {}
                    _ => return invalid("unsupported GTModelTypeGeoKey"),
                },
                1025 => pixel_is_area = k[3] == 1,
                _ => {}
            }
        }
    }

    let (mut west, mut north, h_res, v_res);
    match get(MODEL_TRANSFORMATION)
        .map(|e| r.doubles(e))
        .transpose()?
    {
        Some(m) if m.len() == 16 => {
            if m[1] != 0.0 || m[4] != 0.0 {
                return invalid("rotational terms in GeoTransformationMatrix");
            }
            west = m[3];
            h_res = m[0];
            north = m[7];
            v_res = -m[5];
        }
        _ => {
            let Some(scale) = get(MODEL_PIXEL_SCALE) else {
                return invalid("no GeoPixelScale");
            };
            let scale = r.doubles(scale)?;
            if scale.len() != 3 {
                return invalid("wrong number of values in GeoPixelScale");
            }
            h_res = scale[0];
            v_res = scale[1];
            let Some(tie) = get(MODEL_TIEPOINT) else {
                return invalid("no GeoTiePoints");
            };
            let tie = r.doubles(tie)?;
            if tie.len() != 6 {
                return invalid("wrong number of values in GeoTiePoints");
            }
            west = tie[3] - tie[0] * h_res;
            north = tie[4] + tie[1] * v_res;
        }
    }
    if pixel_is_area {
        west += 0.5 * h_res;
        north -= 0.5 * v_res;
    }
    let factor = if geographic { PI / 180.0 } else { 1.0 };
    let mut extent = ExtentAndRes {
        is_geographic: geographic,
        west: west * factor,
        north: north * factor,
        res_x: h_res * factor,
        res_y: v_res.abs() * factor,
        east: (west + h_res * (width - 1) as f64) * factor,
        south: (north - v_res * (height - 1) as f64) * factor,
        inv_res_x: 0.0,
        inv_res_y: 0.0,
    };
    extent.inv_res_x = 1.0 / extent.res_x;
    extent.inv_res_y = 1.0 / extent.res_y;
    if v_res < 0.0 {
        std::mem::swap(&mut extent.north, &mut extent.south);
    }
    let plausible = (!extent.is_geographic
        || (extent.west.abs() <= 4.0 * PI
            && extent.east.abs() <= 4.0 * PI
            && extent.north.abs() <= PI + 1e-5
            && extent.south.abs() <= PI + 1e-5))
        && extent.west < extent.east
        && extent.south < extent.north
        && extent.res_x > 1e-10
        && extent.res_y > 1e-10;
    if !plausible {
        return invalid("inconsistent georeferencing");
    }

    let mut metadata = HashMap::new();
    let mut offset_scale: Vec<(f64, f64)> = Vec::new();
    if let Some(e) = get(GDAL_METADATA) {
        let text = r.ascii(e)?;
        for (sample, name, role, value) in gdal_metadata(&text) {
            if let Some(role) = role
                && sample >= 0
                && sample as usize <= samples
            {
                let v: Option<f64> = value.trim().parse().ok();
                if let Some(v) = v
                    && (role == "offset" || role == "scale")
                {
                    if offset_scale.is_empty() {
                        offset_scale = vec![(0.0, 1.0); samples];
                    }
                    if let Some(slot) = offset_scale.get_mut(sample as usize) {
                        if role == "offset" {
                            slot.0 = v;
                        } else {
                            slot.1 = v;
                        }
                    }
                }
            }
            metadata.insert((sample, name), value);
        }
    }
    let nodata = match get(GDAL_NODATA) {
        Some(e) => r.ascii(e)?.trim().parse::<f64>().ok().map(|v| v as f32),
        None => None,
    };

    Ok(Image {
        width,
        height,
        extent,
        samples,
        subfile_type,
        metadata,
        bottom_up: v_res < 0.0,
        separate_planes,
        data_type,
        order: r.order,
        compression,
        predictor,
        block_width,
        block_height,
        blocks_per_row,
        blocks,
        offsets,
        counts,
        nodata,
        offset_scale,
        decoded: (0..total).map(|_| OnceLock::new()).collect(),
    })
}

/// PROJ's "poor-man XML parsing" of GDAL's metadata tag: every `<Item>`'s
/// sample (-1 when none), name, role and text.
fn gdal_metadata(text: &str) -> Vec<(i32, String, Option<String>, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<Item ") {
        let item = &rest[start..];
        let Some(end_tag) = item.find('>') else { break };
        let Some(end_value) = item[end_tag..].find('<').map(|i| i + end_tag) else {
            break;
        };
        let tag = &item[..end_tag];
        let value = &item[end_tag + 1..end_value];
        let attr = |name: &str| -> Option<&str> {
            let key = format!("{name}=\"");
            let from = tag.find(&key)? + key.len();
            let to = tag[from..].find('"')? + from;
            Some(&tag[from..to])
        };
        let Some(name) = attr("name") else { break };
        let sample = attr("sample")
            .map(|s| {
                let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse().unwrap_or(0)
            })
            .unwrap_or(-1);
        out.push((
            sample,
            name.to_string(),
            attr("role").map(str::to_string),
            value.to_string(),
        ));
        rest = &item[end_value + 1..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_and_serve_subranges() {
        let mut r = Ranges::default();
        r.insert(10, vec![1, 2, 3]);
        r.insert(0, vec![0; 10]);
        r.insert(13, vec![4, 5]);
        assert_eq!(r.get(9, 6).unwrap(), &[0, 1, 2, 3, 4, 5]);
        assert_eq!(
            r.get(14, 2),
            Err(TiffError::Need {
                offset: 14,
                length: 2
            })
        );
        r.insert(12, vec![9, 9, 9, 9, 9]);
        assert_eq!(r.get(10, 7).unwrap(), &[1, 2, 9, 9, 9, 9, 9]);
    }

    #[test]
    fn metadata_items() {
        let xml = r#"<GDALMetadata>
  <Item name="TYPE">HORIZONTAL_OFFSET</Item>
  <Item name="DESCRIPTION" sample="0" role="description">latitude_offset</Item>
  <Item name="OFFSET" sample="1" role="offset">-1.5</Item>
</GDALMetadata>"#;
        let items = gdal_metadata(xml);
        assert_eq!(
            items[0],
            (-1, "TYPE".into(), None, "HORIZONTAL_OFFSET".into())
        );
        assert_eq!(
            items[1],
            (
                0,
                "DESCRIPTION".into(),
                Some("description".into()),
                "latitude_offset".into()
            )
        );
        assert_eq!(items[2].2.as_deref(), Some("offset"));
    }

    #[test]
    fn floating_point_predictor_restores_values() {
        // Two big-endian floats, 1.0 and 2.0, as byte planes with byte
        // differencing (stride 1).
        let values = [1.0f32, 2.0];
        let be: Vec<[u8; 4]> = values.iter().map(|v| v.to_be_bytes()).collect();
        let mut planes = Vec::new();
        for b in 0..4 {
            for v in &be {
                planes.push(v[b]);
            }
        }
        let mut diffed = planes.clone();
        for i in (1..diffed.len()).rev() {
            diffed[i] = diffed[i].wrapping_sub(diffed[i - 1]);
        }
        floating_point_predictor(&mut diffed, 4, 1, ByteOrder::Little);
        let got: Vec<f32> = diffed
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(got, values);
    }
}
