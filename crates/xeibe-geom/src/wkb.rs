//! ISO WKB writer, including curve types (CircularString, CompoundCurve,
//! CurvePolygon, MultiCurve, MultiSurface) that `geo-traits` cannot express.
//!
//! Type codes are ISO: `base + 1000` for Z. The whole geometry is written in
//! one dimension, the largest of its parts ([`Geometry::dim`]); 2D positions
//! inside a 3D geometry get a NaN Z. An empty point is written with NaN
//! ordinates, as GEOS and GDAL do. Types are written as they are in the model;
//! call [`Geometry::simplify_types`] first for the simplest type.

use crate::model::{
    CircularString, CompoundCurve, Coords, Curve, CurvePart, CurvePolygon, Dim, Geometry,
    LineString, Point, Polygon, Surface,
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
    Writer { out, endianness, dim }.geometry(geometry);
}

pub fn wkb_size(geometry: &Geometry) -> usize {
    let dim = geometry.dim().unwrap_or(Dim::Xy);
    Sizer { position: 8 * dim.size() }.geometry(geometry)
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
        HEADER + COUNT + polygon.rings().map(|ring| self.coords(&ring.coords)).sum::<usize>()
    }

    fn compound_curve(&self, curve: &CompoundCurve) -> usize {
        HEADER + COUNT + curve.parts.iter().map(|part| HEADER + self.coords(part.coords())).sum::<usize>()
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
