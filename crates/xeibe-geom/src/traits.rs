//! `geo-traits` implementations for the linear subset of the model.
//!
//! Traits are implemented on the model types and on references to them
//! (`LineString` and `&'a LineString`, …), so nested types can be returned
//! without copying. Curve variants of [`Geometry`] are not representable in
//! `geo-traits`: call [`Geometry::simplify_types`] and check
//! [`Geometry::is_simple`] first, and use [`crate::wkb`] for curves.
//! [`GeometryTrait::as_type`] panics on a curve variant.

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
        dimensions(Some(self.dim))
    }

    fn x(&self) -> f64 {
        self.values[0]
    }

    fn y(&self) -> f64 {
        self.values[1]
    }

    fn nth_or_panic(&self, n: usize) -> f64 {
        self.values[n]
    }
}

fn dimensions(dim: Option<Dim>) -> Dimensions {
    match dim {
        Some(Dim::Xyz) => Dimensions::Xyz,
        Some(Dim::Xy) | None => Dimensions::Xy,
    }
}

/// The `geo-traits` view of a geometry, shared by `Geometry` and `&Geometry`.
type GeometryTypeOf<'a> = GeometryType<
    'a,
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    GeometryCollection,
    UnimplementedRect<f64>,
    UnimplementedTriangle<f64>,
    UnimplementedLine<f64>,
>;

fn geometry_as_type(geometry: &Geometry) -> GeometryTypeOf<'_> {
    match geometry {
        Geometry::Point(g) => GeometryType::Point(g),
        Geometry::LineString(g) => GeometryType::LineString(g),
        Geometry::Polygon(g) => GeometryType::Polygon(g),
        Geometry::MultiPoint(g) => GeometryType::MultiPoint(g),
        Geometry::MultiLineString(g) => GeometryType::MultiLineString(g),
        Geometry::MultiPolygon(g) => GeometryType::MultiPolygon(g),
        Geometry::GeometryCollection(g) => GeometryType::GeometryCollection(g),
        curve => panic!(
            "{:?} is a curve type, which geo-traits cannot represent; \
             call Geometry::simplify_types or write it as WKB",
            curve.kind()
        ),
    }
}

/// Shared associated types: every nested geometry is the model type itself,
/// borrowed by `as_type`. `$dim` and `$as_type` are evaluated with `$this`
/// bound to `&Self`.
macro_rules! geometry_trait_body {
    (|$this:ident| dim: $dim:expr, as_type: $as_type:expr) => {
        type T = f64;
        type PointType<'b>
            = Point
        where
            Self: 'b;
        type LineStringType<'b>
            = LineString
        where
            Self: 'b;
        type PolygonType<'b>
            = Polygon
        where
            Self: 'b;
        type MultiPointType<'b>
            = MultiPoint
        where
            Self: 'b;
        type MultiLineStringType<'b>
            = MultiLineString
        where
            Self: 'b;
        type MultiPolygonType<'b>
            = MultiPolygon
        where
            Self: 'b;
        type GeometryCollectionType<'b>
            = GeometryCollection
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
            let $this = self;
            dimensions($dim)
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
            let $this = self;
            $as_type
        }
    };
}

/// `GeometryTrait` for a model type and a reference to it.
macro_rules! simple_geometry_trait {
    ($ty:ident, $variant:ident, |$this:ident| $dim:expr) => {
        impl GeometryTrait for $ty {
            geometry_trait_body!(|this| dim: {
                let $this: &$ty = this;
                $dim
            }, as_type: GeometryType::$variant(this));
        }
        impl GeometryTrait for &$ty {
            geometry_trait_body!(|this| dim: {
                let $this: &$ty = this;
                $dim
            }, as_type: GeometryType::$variant(*this));
        }
    };
}

impl GeometryTrait for Geometry {
    geometry_trait_body!(|this| dim: Geometry::dim(this), as_type: geometry_as_type(this));
}
impl GeometryTrait for &Geometry {
    geometry_trait_body!(|this| dim: Geometry::dim(this), as_type: geometry_as_type(this));
}

simple_geometry_trait!(Point, Point, |g| Point::dim(g));
simple_geometry_trait!(LineString, LineString, |g| g.coords.dim);
simple_geometry_trait!(Polygon, Polygon, |g| Polygon::dim(g));
simple_geometry_trait!(MultiPoint, MultiPoint, |g| crate::model::max_dim(
    g.0.iter().map(Point::dim)
));
simple_geometry_trait!(MultiLineString, MultiLineString, |g| crate::model::max_dim(
    g.0.iter().map(|line| line.coords.dim)
));
simple_geometry_trait!(MultiPolygon, MultiPolygon, |g| crate::model::max_dim(
    g.0.iter().map(Polygon::dim)
));
simple_geometry_trait!(GeometryCollection, GeometryCollection, |g| crate::model::max_dim(
    g.0.iter().map(Geometry::dim)
));

/// Implement a `geo-traits` trait for a model type and a reference to it.
macro_rules! for_both {
    ($trait:ident for $ty:ident { $($body:tt)* }) => {
        impl $trait for $ty {
            $($body)*
        }
        impl $trait for &$ty {
            $($body)*
        }
    };
}

for_both!(PointTrait for Point {
    type CoordType<'b>
        = CoordRef<'b>
    where
        Self: 'b;

    fn coord(&self) -> Option<Self::CoordType<'_>> {
        let values = self.coord.as_deref()?;
        let dim = if values.len() >= 3 { Dim::Xyz } else { Dim::Xy };
        Some(CoordRef { values, dim })
    }
});

for_both!(LineStringTrait for LineString {
    type CoordType<'b>
        = CoordRef<'b>
    where
        Self: 'b;

    fn num_coords(&self) -> usize {
        self.coords.len()
    }

    unsafe fn coord_unchecked(&self, i: usize) -> Self::CoordType<'_> {
        CoordRef {
            values: self.coords.get(i),
            dim: self.coords.dim.unwrap_or(Dim::Xy),
        }
    }
});

for_both!(PolygonTrait for Polygon {
    type RingType<'b>
        = &'b LineString
    where
        Self: 'b;

    fn exterior(&self) -> Option<Self::RingType<'_>> {
        self.exterior.as_ref()
    }

    fn num_interiors(&self) -> usize {
        self.interiors.len()
    }

    unsafe fn interior_unchecked(&self, i: usize) -> Self::RingType<'_> {
        &self.interiors[i]
    }
});

for_both!(MultiPointTrait for MultiPoint {
    type InnerPointType<'b>
        = &'b Point
    where
        Self: 'b;

    fn num_points(&self) -> usize {
        self.0.len()
    }

    unsafe fn point_unchecked(&self, i: usize) -> Self::InnerPointType<'_> {
        &self.0[i]
    }
});

for_both!(MultiLineStringTrait for MultiLineString {
    type InnerLineStringType<'b>
        = &'b LineString
    where
        Self: 'b;

    fn num_line_strings(&self) -> usize {
        self.0.len()
    }

    unsafe fn line_string_unchecked(&self, i: usize) -> Self::InnerLineStringType<'_> {
        &self.0[i]
    }
});

for_both!(MultiPolygonTrait for MultiPolygon {
    type InnerPolygonType<'b>
        = &'b Polygon
    where
        Self: 'b;

    fn num_polygons(&self) -> usize {
        self.0.len()
    }

    unsafe fn polygon_unchecked(&self, i: usize) -> Self::InnerPolygonType<'_> {
        &self.0[i]
    }
});

for_both!(GeometryCollectionTrait for GeometryCollection {
    type GeometryType<'b>
        = &'b Geometry
    where
        Self: 'b;

    fn num_geometries(&self) -> usize {
        self.0.len()
    }

    unsafe fn geometry_unchecked(&self, i: usize) -> Self::GeometryType<'_> {
        &self.0[i]
    }
});
