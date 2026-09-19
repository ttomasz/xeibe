//! Geometry column builders: native GeoArrow types or WKB (with curves).

use arrow_array::ArrayRef;
use geoarrow_array::builder::{
    GeometryBuilder, LineStringBuilder, MultiLineStringBuilder, MultiPointBuilder,
    MultiPolygonBuilder, PointBuilder, PolygonBuilder, WkbBuilder,
};
use xeibe_geom::Geometry;

pub enum GeometryColumnBuilder {
    Point(PointBuilder),
    LineString(LineStringBuilder),
    Polygon(PolygonBuilder),
    MultiPoint(MultiPointBuilder),
    MultiLineString(MultiLineStringBuilder),
    MultiPolygon(MultiPolygonBuilder),
    Geometry(GeometryBuilder),
    Wkb(WkbBuilder<i32>),
}

impl GeometryColumnBuilder {
    /// Chosen from the field's GeoArrow extension type (set by the rule engine).
    pub fn for_field(field: &arrow_schema::Field, capacity: usize) -> crate::Result<Self> {
        todo!()
    }

    pub fn push(&mut self, geometry: Option<&Geometry>) -> crate::Result<()> {
        todo!()
    }

    pub fn finish(self) -> ArrayRef {
        todo!()
    }
}
