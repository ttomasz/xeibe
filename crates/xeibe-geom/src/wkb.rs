//! ISO WKB writer and reader, including curve types (CircularString,
//! CompoundCurve, CurvePolygon, MultiCurve, MultiSurface) that `geo-traits`
//! cannot express.
//!
//! Type codes are ISO: `base + 1000` for Z. The whole geometry is written in
//! one dimension, the largest of its parts ([`Geometry::dim`]); 2D positions
//! inside a 3D geometry get a NaN Z. An empty point is written with NaN
//! ordinates, as GEOS and GDAL do. Types are written as they are in the model;
//! call [`Geometry::simplify_types`] first for the simplest type.

use crate::model::{
    CircularString, CompoundCurve, Coords, Curve, CurvePart, CurvePolygon, Dim, Geometry,
    GeometryCollection, LineString, MultiCurve, MultiLineString, MultiPoint, MultiPolygon,
    MultiSurface, Point, Polygon, Surface,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Endianness {
    #[default]
    Little,
    Big,
}

pub fn write_wkb(geometry: &Geometry, endianness: Endianness, out: &mut Vec<u8>) {
    out.reserve(wkb_size(geometry));
    let dim = geometry.dim().unwrap_or(Dim::Xy);
    Writer {
        out,
        endianness,
        dim,
    }
    .geometry(geometry);
}

pub fn wkb_size(geometry: &Geometry) -> usize {
    let dim = geometry.dim().unwrap_or(Dim::Xy);
    Sizer {
        position: 8 * dim.size(),
    }
    .geometry(geometry)
}

// ISO base type codes.
const POINT: u32 = 1;
const LINE_STRING: u32 = 2;
const POLYGON: u32 = 3;
const MULTI_POINT: u32 = 4;
const MULTI_LINE_STRING: u32 = 5;
const MULTI_POLYGON: u32 = 6;
const GEOMETRY_COLLECTION: u32 = 7;
const CIRCULAR_STRING: u32 = 8;
const COMPOUND_CURVE: u32 = 9;
const CURVE_POLYGON: u32 = 10;
const MULTI_CURVE: u32 = 11;
const MULTI_SURFACE: u32 = 12;

/// Byte order + type code.
const HEADER: usize = 1 + 4;
const COUNT: usize = 4;

struct Writer<'a> {
    out: &'a mut Vec<u8>,
    endianness: Endianness,
    dim: Dim,
}

impl Writer<'_> {
    fn u32(&mut self, value: u32) {
        match self.endianness {
            Endianness::Little => self.out.extend_from_slice(&value.to_le_bytes()),
            Endianness::Big => self.out.extend_from_slice(&value.to_be_bytes()),
        }
    }

    fn f64(&mut self, value: f64) {
        match self.endianness {
            Endianness::Little => self.out.extend_from_slice(&value.to_le_bytes()),
            Endianness::Big => self.out.extend_from_slice(&value.to_be_bytes()),
        }
    }

    fn count(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("WKB counts are 32-bit"));
    }

    fn header(&mut self, base: u32) {
        self.out.push(match self.endianness {
            Endianness::Little => 1,
            Endianness::Big => 0,
        });
        let code = match self.dim {
            Dim::Xy => base,
            Dim::Xyz => base + 1000,
        };
        self.u32(code);
    }

    /// One position, padded with NaN or truncated to our dimension.
    fn position(&mut self, values: &[f64]) {
        for k in 0..self.dim.size() {
            self.f64(values.get(k).copied().unwrap_or(f64::NAN));
        }
    }

    fn coords(&mut self, coords: &Coords) {
        self.count(coords.len());
        if coords.size() == self.dim.size() {
            let end = coords.len() * coords.size();
            for &value in &coords.values[..end] {
                self.f64(value);
            }
        } else {
            for position in coords.positions() {
                self.position(position);
            }
        }
    }

    fn geometry(&mut self, geometry: &Geometry) {
        match geometry {
            Geometry::Point(point) => self.point(point),
            Geometry::LineString(line) => self.line_string(line),
            Geometry::Polygon(polygon) => self.polygon(polygon),
            Geometry::MultiPoint(points) => {
                self.header(MULTI_POINT);
                self.count(points.0.len());
                points.0.iter().for_each(|p| self.point(p));
            }
            Geometry::MultiLineString(lines) => {
                self.header(MULTI_LINE_STRING);
                self.count(lines.0.len());
                lines.0.iter().for_each(|l| self.line_string(l));
            }
            Geometry::MultiPolygon(polygons) => {
                self.header(MULTI_POLYGON);
                self.count(polygons.0.len());
                polygons.0.iter().for_each(|p| self.polygon(p));
            }
            Geometry::GeometryCollection(members) => {
                self.header(GEOMETRY_COLLECTION);
                self.count(members.0.len());
                members.0.iter().for_each(|g| self.geometry(g));
            }
            Geometry::CircularString(arc) => self.circular_string(arc),
            Geometry::CompoundCurve(curve) => self.compound_curve(curve),
            Geometry::CurvePolygon(polygon) => self.curve_polygon(polygon),
            Geometry::MultiCurve(curves) => {
                self.header(MULTI_CURVE);
                self.count(curves.0.len());
                curves.0.iter().for_each(|c| self.curve(c));
            }
            Geometry::MultiSurface(surfaces) => {
                self.header(MULTI_SURFACE);
                self.count(surfaces.0.len());
                for surface in &surfaces.0 {
                    match surface {
                        Surface::Polygon(polygon) => self.polygon(polygon),
                        Surface::CurvePolygon(polygon) => self.curve_polygon(polygon),
                    }
                }
            }
        }
    }

    fn point(&mut self, point: &Point) {
        self.header(POINT);
        match &point.coord {
            Some(values) => self.position(values),
            None => self.position(&[]),
        }
    }

    fn line_string(&mut self, line: &LineString) {
        self.header(LINE_STRING);
        self.coords(&line.coords);
    }

    fn circular_string(&mut self, arc: &CircularString) {
        self.header(CIRCULAR_STRING);
        self.coords(&arc.coords);
    }

    fn polygon(&mut self, polygon: &Polygon) {
        self.header(POLYGON);
        self.count(polygon.rings().count());
        for ring in polygon.rings() {
            self.coords(&ring.coords);
        }
    }

    fn compound_curve(&mut self, curve: &CompoundCurve) {
        self.header(COMPOUND_CURVE);
        self.count(curve.parts.len());
        for part in &curve.parts {
            match part {
                CurvePart::Linear(line) => self.line_string(line),
                CurvePart::Circular(arc) => self.circular_string(arc),
            }
        }
    }

    fn curve(&mut self, curve: &Curve) {
        match curve {
            Curve::Linear(line) => self.line_string(line),
            Curve::Circular(arc) => self.circular_string(arc),
            Curve::Compound(compound) => self.compound_curve(compound),
        }
    }

    fn curve_polygon(&mut self, polygon: &CurvePolygon) {
        self.header(CURVE_POLYGON);
        self.count(polygon.rings().count());
        polygon.rings().for_each(|ring| self.curve(ring));
    }
}

/// Mirrors [`Writer`], counting bytes instead of writing them.
struct Sizer {
    /// Bytes per position.
    position: usize,
}

impl Sizer {
    fn coords(&self, coords: &Coords) -> usize {
        COUNT + coords.len() * self.position
    }

    fn line_string(&self, line: &LineString) -> usize {
        HEADER + self.coords(&line.coords)
    }

    fn polygon(&self, polygon: &Polygon) -> usize {
        HEADER
            + COUNT
            + polygon
                .rings()
                .map(|ring| self.coords(&ring.coords))
                .sum::<usize>()
    }

    fn compound_curve(&self, curve: &CompoundCurve) -> usize {
        HEADER
            + COUNT
            + curve
                .parts
                .iter()
                .map(|part| HEADER + self.coords(part.coords()))
                .sum::<usize>()
    }

    fn curve(&self, curve: &Curve) -> usize {
        match curve {
            Curve::Linear(line) => self.line_string(line),
            Curve::Circular(arc) => HEADER + self.coords(&arc.coords),
            Curve::Compound(compound) => self.compound_curve(compound),
        }
    }

    fn curve_polygon(&self, polygon: &CurvePolygon) -> usize {
        HEADER + COUNT + polygon.rings().map(|ring| self.curve(ring)).sum::<usize>()
    }

    fn geometry(&self, geometry: &Geometry) -> usize {
        let point = HEADER + self.position;
        match geometry {
            Geometry::Point(_) => point,
            Geometry::LineString(line) => self.line_string(line),
            Geometry::Polygon(polygon) => self.polygon(polygon),
            Geometry::MultiPoint(points) => HEADER + COUNT + points.0.len() * point,
            Geometry::MultiLineString(lines) => {
                HEADER + COUNT + lines.0.iter().map(|l| self.line_string(l)).sum::<usize>()
            }
            Geometry::MultiPolygon(polygons) => {
                HEADER + COUNT + polygons.0.iter().map(|p| self.polygon(p)).sum::<usize>()
            }
            Geometry::GeometryCollection(members) => {
                HEADER + COUNT + members.0.iter().map(|g| self.geometry(g)).sum::<usize>()
            }
            Geometry::CircularString(arc) => HEADER + self.coords(&arc.coords),
            Geometry::CompoundCurve(curve) => self.compound_curve(curve),
            Geometry::CurvePolygon(polygon) => self.curve_polygon(polygon),
            Geometry::MultiCurve(curves) => {
                HEADER + COUNT + curves.0.iter().map(|c| self.curve(c)).sum::<usize>()
            }
            Geometry::MultiSurface(surfaces) => {
                HEADER
                    + COUNT
                    + surfaces
                        .0
                        .iter()
                        .map(|surface| match surface {
                            Surface::Polygon(polygon) => self.polygon(polygon),
                            Surface::CurvePolygon(polygon) => self.curve_polygon(polygon),
                        })
                        .sum::<usize>()
            }
        }
    }
}

/// Read one WKB geometry: what [`write_wkb`] writes, and also OGC/EWKB type
/// codes (Z, M and SRID flags), either byte order per geometry, and M
/// ordinates, which are dropped (the model has none). An empty point is one
/// with NaN ordinates. The bytes must hold exactly one geometry.
pub fn read_wkb(bytes: &[u8]) -> Result<Geometry, String> {
    let mut reader = Reader { bytes, at: 0 };
    let geometry = reader.geometry()?;
    if reader.at != bytes.len() {
        return Err(format!(
            "{} bytes after the geometry",
            bytes.len() - reader.at
        ));
    }
    Ok(geometry)
}

/// EWKB flags in the type code.
const EWKB_Z: u32 = 0x8000_0000;
const EWKB_M: u32 = 0x4000_0000;
const EWKB_SRID: u32 = 0x2000_0000;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

/// One geometry's header: its byte order, base type and ordinates.
#[derive(Clone, Copy)]
struct Header {
    little: bool,
    base: u32,
    /// Ordinates per position as written (2 to 4).
    size: usize,
    dim: Dim,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let bytes = self
            .bytes
            .get(self.at..self.at + N)
            .ok_or_else(|| format!("truncated at byte {}", self.at))?;
        self.at += N;
        Ok(bytes.try_into().expect("N bytes"))
    }

    fn u32(&mut self, little: bool) -> Result<u32, String> {
        let bytes = self.take::<4>()?;
        Ok(if little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn f64(&mut self, little: bool) -> Result<f64, String> {
        let bytes = self.take::<8>()?;
        Ok(if little {
            f64::from_le_bytes(bytes)
        } else {
            f64::from_be_bytes(bytes)
        })
    }

    /// A count of items at least `min_bytes` long each, checked against the
    /// bytes left so that a corrupt count can't allocate.
    fn count(&mut self, little: bool, min_bytes: usize) -> Result<usize, String> {
        let n = self.u32(little)? as usize;
        let left = self.bytes.len() - self.at;
        if n.saturating_mul(min_bytes) > left {
            return Err(format!(
                "count {n} at byte {} exceeds the data",
                self.at - 4
            ));
        }
        Ok(n)
    }

    fn header(&mut self) -> Result<Header, String> {
        let little = match self.take::<1>()?[0] {
            0 => false,
            1 => true,
            other => return Err(format!("byte order {other} at byte {}", self.at - 1)),
        };
        let code = self.u32(little)?;
        let (mut z, mut m) = (code & EWKB_Z != 0, code & EWKB_M != 0);
        if code & EWKB_SRID != 0 {
            self.u32(little)?;
        }
        let iso = code & !(EWKB_Z | EWKB_M | EWKB_SRID);
        let base = iso % 1000;
        match iso / 1000 {
            0 => {}
            1 => z = true,
            2 => m = true,
            3 => (z, m) = (true, true),
            _ => return Err(format!("geometry type {code}")),
        }
        if !(POINT..=MULTI_SURFACE).contains(&base) {
            return Err(format!("geometry type {code}"));
        }
        Ok(Header {
            little,
            base,
            size: 2 + usize::from(z) + usize::from(m),
            dim: if z { Dim::Xyz } else { Dim::Xy },
        })
    }

    /// A header whose type must be one of `bases`.
    fn header_of(&mut self, bases: &[u32], within: &str) -> Result<Header, String> {
        let at = self.at;
        let header = self.header()?;
        if !bases.contains(&header.base) {
            return Err(format!(
                "geometry type {} in a {within} at byte {at}",
                header.base
            ));
        }
        Ok(header)
    }

    fn position(&mut self, header: Header) -> Result<Vec<f64>, String> {
        let mut position = Vec::with_capacity(header.dim.size());
        for k in 0..header.size {
            let value = self.f64(header.little)?;
            if k < header.dim.size() {
                position.push(value);
            }
        }
        Ok(position)
    }

    fn coords(&mut self, header: Header) -> Result<Coords, String> {
        let n = self.count(header.little, header.size * 8)?;
        let mut coords = Coords::new(header.dim);
        coords.values.reserve(n * header.dim.size());
        for _ in 0..n {
            let position = self.position(header)?;
            coords.values.extend(position);
        }
        Ok(coords)
    }

    fn geometry(&mut self) -> Result<Geometry, String> {
        let header = self.header()?;
        self.body(header)
    }

    fn body(&mut self, header: Header) -> Result<Geometry, String> {
        let little = header.little;
        Ok(match header.base {
            POINT => Geometry::Point(self.point(header)?),
            LINE_STRING => Geometry::LineString(LineString {
                coords: self.coords(header)?,
            }),
            POLYGON => Geometry::Polygon(self.polygon(header)?),
            MULTI_POINT => {
                let n = self.count(little, HEADER)?;
                let points = (0..n)
                    .map(|_| {
                        let header = self.header_of(&[POINT], "MultiPoint")?;
                        self.point(header)
                    })
                    .collect::<Result<_, String>>()?;
                Geometry::MultiPoint(MultiPoint(points))
            }
            MULTI_LINE_STRING => {
                let n = self.count(little, HEADER)?;
                let lines = (0..n)
                    .map(|_| {
                        let header = self.header_of(&[LINE_STRING], "MultiLineString")?;
                        Ok(LineString {
                            coords: self.coords(header)?,
                        })
                    })
                    .collect::<Result<_, String>>()?;
                Geometry::MultiLineString(MultiLineString(lines))
            }
            MULTI_POLYGON => {
                let n = self.count(little, HEADER)?;
                let polygons = (0..n)
                    .map(|_| {
                        let header = self.header_of(&[POLYGON], "MultiPolygon")?;
                        self.polygon(header)
                    })
                    .collect::<Result<_, String>>()?;
                Geometry::MultiPolygon(MultiPolygon(polygons))
            }
            GEOMETRY_COLLECTION => {
                let n = self.count(little, HEADER)?;
                let members = (0..n)
                    .map(|_| self.geometry())
                    .collect::<Result<_, String>>()?;
                Geometry::GeometryCollection(GeometryCollection(members))
            }
            CIRCULAR_STRING => Geometry::CircularString(CircularString {
                coords: self.coords(header)?,
                computed: Vec::new(),
            }),
            COMPOUND_CURVE => Geometry::CompoundCurve(self.compound_curve(header)?),
            CURVE_POLYGON => Geometry::CurvePolygon(self.curve_polygon(header)?),
            MULTI_CURVE => {
                let n = self.count(little, HEADER)?;
                let curves = (0..n)
                    .map(|_| self.curve("MultiCurve"))
                    .collect::<Result<_, String>>()?;
                Geometry::MultiCurve(MultiCurve(curves))
            }
            MULTI_SURFACE => {
                let n = self.count(little, HEADER)?;
                let surfaces = (0..n)
                    .map(|_| {
                        let header = self.header_of(&[POLYGON, CURVE_POLYGON], "MultiSurface")?;
                        Ok(match header.base {
                            POLYGON => Surface::Polygon(self.polygon(header)?),
                            _ => Surface::CurvePolygon(self.curve_polygon(header)?),
                        })
                    })
                    .collect::<Result<_, String>>()?;
                Geometry::MultiSurface(MultiSurface(surfaces))
            }
            _ => unreachable!("checked in header()"),
        })
    }

    fn point(&mut self, header: Header) -> Result<Point, String> {
        let position = self.position(header)?;
        let coord = (!position.iter().all(|v| v.is_nan())).then_some(position);
        Ok(Point { coord })
    }

    fn polygon(&mut self, header: Header) -> Result<Polygon, String> {
        let n = self.count(header.little, COUNT)?;
        let mut rings = (0..n).map(|_| {
            Ok(LineString {
                coords: self.coords(header)?,
            })
        });
        let exterior = rings.next().transpose()?;
        let interiors = rings.collect::<Result<_, String>>()?;
        Ok(Polygon {
            exterior,
            interiors,
        })
    }

    /// A LineString, CircularString or CompoundCurve.
    fn curve(&mut self, within: &str) -> Result<Curve, String> {
        let header = self.header_of(&[LINE_STRING, CIRCULAR_STRING, COMPOUND_CURVE], within)?;
        let coords = |reader: &mut Self| reader.coords(header);
        Ok(match header.base {
            LINE_STRING => Curve::Linear(LineString {
                coords: coords(self)?,
            }),
            CIRCULAR_STRING => Curve::Circular(CircularString {
                coords: coords(self)?,
                computed: Vec::new(),
            }),
            _ => Curve::Compound(self.compound_curve(header)?),
        })
    }

    fn compound_curve(&mut self, header: Header) -> Result<CompoundCurve, String> {
        let n = self.count(header.little, HEADER)?;
        let parts = (0..n)
            .map(|_| {
                let header = self.header_of(&[LINE_STRING, CIRCULAR_STRING], "CompoundCurve")?;
                let coords = self.coords(header)?;
                Ok(match header.base {
                    LINE_STRING => CurvePart::Linear(LineString { coords }),
                    _ => CurvePart::Circular(CircularString {
                        coords,
                        computed: Vec::new(),
                    }),
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(CompoundCurve { parts })
    }

    fn curve_polygon(&mut self, header: Header) -> Result<CurvePolygon, String> {
        let n = self.count(header.little, HEADER)?;
        let mut rings = (0..n).map(|_| self.curve("CurvePolygon"));
        let exterior = rings.next().transpose()?;
        let interiors = rings.collect::<Result<_, String>>()?;
        Ok(CurvePolygon {
            exterior,
            interiors,
        })
    }
}
