//! `geo-traits` implementations for the linear subset of the model.
//!
//! Traits are implemented on references (`&'a LineString`, …) so nested types
//! can be returned without copying. Curve variants of [`Geometry`] are not
//! representable in `geo-traits`; call [`Geometry::is_simple`] first and use
//! [`crate::wkb`] for curves.

use geo_traits::{
    CoordTrait, Dimensions, GeometryCollectionTrait, GeometryTrait, GeometryType, LineStringTrait,
    MultiLineStringTrait, MultiPointTrait, MultiPolygonTrait, PointTrait, PolygonTrait,
    UnimplementedLine, UnimplementedRect, UnimplementedTriangle,
};

use crate::model::{
    Dim, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon,
    Point, Polygon,
};

/// A borrowed coordinate.
#[derive(Debug, Clone, Copy)]
pub struct CoordRef<'a> {
    pub values: &'a [f64],
    pub dim: Dim,
}

impl CoordTrait for CoordRef<'_> {
    type T = f64;

    fn dim(&self) -> Dimensions {
        todo!()
    }

    fn x(&self) -> f64 {
        todo!()
    }

    fn y(&self) -> f64 {
        todo!()
    }

    fn nth_or_panic(&self, n: usize) -> f64 {
        todo!()
    }
}

/// Shared associated types: every nested geometry is returned by reference.
macro_rules! geometry_trait_body {
    () => {
        type T = f64;
        type PointType<'b>
            = &'b Point
        where
            Self: 'b;
        type LineStringType<'b>
            = &'b LineString
        where
            Self: 'b;
        type PolygonType<'b>
            = &'b Polygon
        where
            Self: 'b;
        type MultiPointType<'b>
            = &'b MultiPoint
        where
            Self: 'b;
        type MultiLineStringType<'b>
            = &'b MultiLineString
        where
            Self: 'b;
        type MultiPolygonType<'b>
            = &'b MultiPolygon
        where
            Self: 'b;
        type GeometryCollectionType<'b>
            = &'b GeometryCollection
        where
            Self: 'b;
        type RectType<'b>
            = UnimplementedRect<f64>
        where
            Self: 'b;
        type TriangleType<'b>
            = UnimplementedTriangle<f64>
        where
            Self: 'b;
        type LineType<'b>
            = UnimplementedLine<f64>
        where
            Self: 'b;

        fn dim(&self) -> Dimensions {
            todo!()
        }

        fn as_type(
            &self,
        ) -> GeometryType<
            '_,
            Self::PointType<'_>,
            Self::LineStringType<'_>,
            Self::PolygonType<'_>,
            Self::MultiPointType<'_>,
            Self::MultiLineStringType<'_>,
            Self::MultiPolygonType<'_>,
            Self::GeometryCollectionType<'_>,
            Self::RectType<'_>,
            Self::TriangleType<'_>,
            Self::LineType<'_>,
        > {
            todo!()
        }
    };
}

impl GeometryTrait for Geometry {
    geometry_trait_body!();
}
impl GeometryTrait for &Geometry {
    geometry_trait_body!();
}
impl GeometryTrait for &Point {
    geometry_trait_body!();
}
impl GeometryTrait for &LineString {
    geometry_trait_body!();
}
impl GeometryTrait for &Polygon {
    geometry_trait_body!();
}
impl GeometryTrait for &MultiPoint {
    geometry_trait_body!();
}
impl GeometryTrait for &MultiLineString {
    geometry_trait_body!();
}
impl GeometryTrait for &MultiPolygon {
    geometry_trait_body!();
}
impl GeometryTrait for &GeometryCollection {
    geometry_trait_body!();
}

impl PointTrait for &Point {
    type CoordType<'b>
        = CoordRef<'b>
    where
        Self: 'b;

    fn coord(&self) -> Option<Self::CoordType<'_>> {
        todo!()
    }
}

impl LineStringTrait for &LineString {
    type CoordType<'b>
        = CoordRef<'b>
    where
        Self: 'b;

    fn num_coords(&self) -> usize {
        todo!()
    }

    unsafe fn coord_unchecked(&self, i: usize) -> Self::CoordType<'_> {
        todo!()
    }
}

impl PolygonTrait for &Polygon {
    type RingType<'b>
        = &'b LineString
    where
        Self: 'b;

    fn exterior(&self) -> Option<Self::RingType<'_>> {
        todo!()
    }

    fn num_interiors(&self) -> usize {
        todo!()
    }

    unsafe fn interior_unchecked(&self, i: usize) -> Self::RingType<'_> {
        todo!()
    }
}

impl MultiPointTrait for &MultiPoint {
    type InnerPointType<'b>
        = &'b Point
    where
        Self: 'b;

    fn num_points(&self) -> usize {
        todo!()
    }

    unsafe fn point_unchecked(&self, i: usize) -> Self::InnerPointType<'_> {
        todo!()
    }
}

impl MultiLineStringTrait for &MultiLineString {
    type InnerLineStringType<'b>
        = &'b LineString
    where
        Self: 'b;

    fn num_line_strings(&self) -> usize {
        todo!()
    }

    unsafe fn line_string_unchecked(&self, i: usize) -> Self::InnerLineStringType<'_> {
        todo!()
    }
}

impl MultiPolygonTrait for &MultiPolygon {
    type InnerPolygonType<'b>
        = &'b Polygon
    where
        Self: 'b;

    fn num_polygons(&self) -> usize {
        todo!()
    }

    unsafe fn polygon_unchecked(&self, i: usize) -> Self::InnerPolygonType<'_> {
        todo!()
    }
}

impl GeometryCollectionTrait for &GeometryCollection {
    type GeometryType<'b>
        = &'b Geometry
    where
        Self: 'b;

    fn num_geometries(&self) -> usize {
        todo!()
    }

    unsafe fn geometry_unchecked(&self, i: usize) -> Self::GeometryType<'_> {
        todo!()
    }
}
