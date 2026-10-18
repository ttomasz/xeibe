//! Geometry as WKT for the table, cut short: a cell shows the type and the
//! first coordinates, not a whole parcel boundary.

use std::fmt::Write;

use xeibe_geom::Geometry;
use xeibe_geom::model::{Coords, Curve, CurvePart, Polygon, Surface};

/// WKT of `geometry`, cut at about `max` characters with `…`. Curves keep
/// their types (`CIRCULARSTRING`, …); a Z ordinate shows as `Z`.
pub fn short_wkt(geometry: &Geometry, max: usize) -> String {
    let mut w = Wkt {
        out: String::new(),
        max,
    };
    let z = if geometry.dim() == Some(xeibe_geom::Dim::Xyz) {
        " Z"
    } else {
        ""
    };
    w.geometry(geometry, z);
    if w.out.len() > max {
        let mut end = max;
        while !w.out.is_char_boundary(end) {
            end -= 1;
        }
        w.out.truncate(end);
        w.out.push('…');
    }
    w.out
}

struct Wkt {
    out: String,
    max: usize,
}

impl Wkt {
    fn full(&self) -> bool {
        self.out.len() > self.max
    }

    fn tagged(&mut self, tag: &str, z: &str, body: impl FnOnce(&mut Self)) {
        self.out.push_str(tag);
        self.out.push_str(z);
        self.out.push(' ');
        body(self);
    }

    fn list<T>(&mut self, items: &[T], mut item: impl FnMut(&mut Self, &T)) {
        if items.is_empty() {
            self.out.push_str("EMPTY");
            return;
        }
        self.out.push('(');
        for (i, value) in items.iter().enumerate() {
            if self.full() {
                return;
            }
            if i > 0 {
                self.out.push_str(", ");
            }
            item(self, value);
        }
        self.out.push(')');
    }

    fn coords(&mut self, coords: &Coords) {
        let positions: Vec<&[f64]> = coords.positions().collect();
        self.list(&positions, |w, p| w.position(p));
    }

    fn position(&mut self, p: &[f64]) {
        for (k, value) in p.iter().enumerate() {
            if k > 0 {
                self.out.push(' ');
            }
            let _ = write!(self.out, "{value}");
        }
    }

    fn polygon(&mut self, polygon: &Polygon) {
        let rings: Vec<_> = polygon.rings().collect();
        self.list(&rings, |w, ring| w.coords(&ring.coords));
    }

    fn curve(&mut self, curve: &Curve, z: &str) {
        match curve {
            Curve::Linear(line) => self.coords(&line.coords),
            Curve::Circular(arc) => self.tagged("CIRCULARSTRING", z, |w| w.coords(&arc.coords)),
            Curve::Compound(compound) => self.tagged("COMPOUNDCURVE", z, |w| {
                w.list(&compound.parts, |w, part| match part {
                    CurvePart::Linear(line) => w.coords(&line.coords),
                    CurvePart::Circular(arc) => {
                        w.tagged("CIRCULARSTRING", z, |w| w.coords(&arc.coords))
                    }
                })
            }),
        }
    }

    fn geometry(&mut self, geometry: &Geometry, z: &str) {
        match geometry {
            Geometry::Point(point) => self.tagged("POINT", z, |w| match &point.coord {
                Some(p) => {
                    w.out.push('(');
                    w.position(p);
                    w.out.push(')');
                }
                None => w.out.push_str("EMPTY"),
            }),
            Geometry::LineString(line) => self.tagged("LINESTRING", z, |w| w.coords(&line.coords)),
            Geometry::Polygon(polygon) => self.tagged("POLYGON", z, |w| w.polygon(polygon)),
            Geometry::MultiPoint(points) => self.tagged("MULTIPOINT", z, |w| {
                w.list(&points.0, |w, point| match &point.coord {
                    Some(p) => {
                        w.out.push('(');
                        w.position(p);
                        w.out.push(')');
                    }
                    None => w.out.push_str("EMPTY"),
                })
            }),
            Geometry::MultiLineString(lines) => self.tagged("MULTILINESTRING", z, |w| {
                w.list(&lines.0, |w, line| w.coords(&line.coords))
            }),
            Geometry::MultiPolygon(polygons) => self.tagged("MULTIPOLYGON", z, |w| {
                w.list(&polygons.0, |w, polygon| w.polygon(polygon))
            }),
            Geometry::GeometryCollection(members) => self.tagged("GEOMETRYCOLLECTION", z, |w| {
                w.list(&members.0, |w, member| w.geometry(member, z))
            }),
            Geometry::CircularString(arc) => {
                self.tagged("CIRCULARSTRING", z, |w| w.coords(&arc.coords))
            }
            Geometry::CompoundCurve(compound) => self.curve(&Curve::Compound(compound.clone()), z),
            Geometry::CurvePolygon(polygon) => self.tagged("CURVEPOLYGON", z, |w| {
                let rings: Vec<_> = polygon.rings().collect();
                w.list(&rings, |w, ring| w.curve(ring, z))
            }),
            Geometry::MultiCurve(curves) => self.tagged("MULTICURVE", z, |w| {
                w.list(&curves.0, |w, curve| w.curve(curve, z))
            }),
            Geometry::MultiSurface(surfaces) => self.tagged("MULTISURFACE", z, |w| {
                w.list(&surfaces.0, |w, surface| match surface {
                    Surface::Polygon(polygon) => w.polygon(polygon),
                    Surface::CurvePolygon(polygon) => w.tagged("CURVEPOLYGON", z, |w| {
                        let rings: Vec<_> = polygon.rings().collect();
                        w.list(&rings, |w, ring| w.curve(ring, z))
                    }),
                })
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xeibe_geom::model::{CircularString, LineString, Point};

    fn coords(values: &[f64]) -> Coords {
        Coords {
            dim: Some(xeibe_geom::Dim::Xy),
            values: values.to_vec(),
        }
    }

    #[test]
    fn writes_wkt_and_cuts_it_short() {
        let point = Geometry::Point(Point {
            coord: Some(vec![636640.5, 483432.25]),
        });
        assert_eq!(short_wkt(&point, 100), "POINT (636640.5 483432.25)");
        let line = Geometry::LineString(LineString {
            coords: coords(&[0.0, 0.0, 1.0, 1.0, 2.0, 2.0]),
        });
        assert_eq!(short_wkt(&line, 100), "LINESTRING (0 0, 1 1, 2 2)");
        assert_eq!(short_wkt(&line, 14), "LINESTRING (0 …");
        let arc = Geometry::CircularString(CircularString {
            coords: coords(&[0.0, 0.0, 1.0, 1.0, 2.0, 0.0]),
            computed: Vec::new(),
        });
        assert_eq!(short_wkt(&arc, 100), "CIRCULARSTRING (0 0, 1 1, 2 0)");
        let empty = Geometry::Point(Point { coord: None });
        assert_eq!(short_wkt(&empty, 100), "POINT EMPTY");
    }
}
