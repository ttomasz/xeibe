//! `geo-traits` for the linear subset of the model (`docs/geometry.md`).
//!
//! These implementations are what feeds the GeoArrow native builders, so they
//! are exercised through the traits, not through the concrete types.

use geo_traits::{
    CoordTrait, Dimensions, GeometryTrait, GeometryType, LineStringTrait, MultiPolygonTrait,
    PointTrait, PolygonTrait,
};
use xeibe_geom::model::Geometry;

use crate::support::geometry;

/// What a GeoArrow builder would do: look at the type, then at the coordinates.
fn coordinate_count(geometry: &impl GeometryTrait<T = f64>) -> usize {
    match geometry.as_type() {
        GeometryType::Point(point) => usize::from(point.coord().is_some()),
        GeometryType::LineString(line) => line.num_coords(),
        GeometryType::Polygon(polygon) => {
            polygon.exterior().map_or(0, |ring| ring.num_coords())
                + polygon.interiors().map(|ring| ring.num_coords()).sum::<usize>()
        }
        GeometryType::MultiPolygon(multi) => multi
            .polygons()
            .map(|polygon| polygon.exterior().map_or(0, |ring| ring.num_coords()))
            .sum(),
        _ => 0,
    }
}

#[test]
fn a_point_exposes_its_coordinate() {
    let point = geometry("<gml:Point><gml:pos>1 2</gml:pos></gml:Point>");
    let Geometry::Point(inner) = &point else {
        panic!("expected a point");
    };
    let coord = (&inner).coord().expect("a coordinate");
    assert_eq!(coord.x(), 1.0);
    assert_eq!(coord.y(), 2.0);
    assert_eq!(coord.dim(), Dimensions::Xy);
    assert_eq!(coordinate_count(&point), 1);
}

#[test]
fn an_empty_point_has_no_coordinate() {
    let point = geometry("<gml:Point/>");
    let Geometry::Point(inner) = &point else {
        panic!("expected a point");
    };
    assert!((&inner).coord().is_none());
}

#[test]
fn three_dimensional_geometry_reports_xyz() {
    let point = geometry("<gml:Point><gml:pos>1 2 3</gml:pos></gml:Point>");
    assert_eq!(GeometryTrait::dim(&point), Dimensions::Xyz);
    let Geometry::Point(inner) = &point else {
        panic!("expected a point");
    };
    let coord = (&inner).coord().expect("a coordinate");
    assert_eq!(coord.nth_or_panic(2), 3.0);
}

#[test]
fn a_line_string_exposes_its_coordinates_in_order() {
    let line = geometry("<gml:LineString><gml:posList>1 2 3 4 5 6</gml:posList></gml:LineString>");
    assert_eq!(coordinate_count(&line), 3);
    let Geometry::LineString(inner) = &line else {
        panic!("expected a line string");
    };
    let coords: Vec<(f64, f64)> = (&inner).coords().map(|c| (c.x(), c.y())).collect();
    assert_eq!(coords, [(1.0, 2.0), (3.0, 4.0), (5.0, 6.0)]);
}

#[test]
fn a_polygon_exposes_its_rings() {
    let polygon = geometry(concat!(
        "<gml:Polygon><gml:exterior><gml:LinearRing>",
        "<gml:posList>0 0 4 0 4 4 0 4 0 0</gml:posList></gml:LinearRing></gml:exterior>",
        "<gml:interior><gml:LinearRing>",
        "<gml:posList>1 1 2 1 2 2 1 2 1 1</gml:posList></gml:LinearRing></gml:interior>",
        "</gml:Polygon>"
    ));
    let Geometry::Polygon(inner) = &polygon else {
        panic!("expected a polygon");
    };
    assert_eq!((&inner).num_interiors(), 1);
    assert_eq!((&inner).exterior().expect("a ring").num_coords(), 5);
    assert_eq!(coordinate_count(&polygon), 10);
}

#[test]
fn aggregates_expose_their_members() {
    let multi = geometry(concat!(
        "<gml:MultiSurface><gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing>",
        "<gml:posList>0 0 1 0 1 1 0 0</gml:posList>",
        "</gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember>",
        "<gml:surfaceMember><gml:Polygon><gml:exterior><gml:LinearRing>",
        "<gml:posList>4 0 5 0 5 1 4 0</gml:posList>",
        "</gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember></gml:MultiSurface>"
    ));
    assert!(matches!(
        multi.as_type(),
        GeometryType::MultiPolygon(_)
    ));
    assert_eq!(coordinate_count(&multi), 8);
}

#[test]
fn a_geometry_reference_also_implements_the_trait() {
    // Builders take geometries by reference, so `&Geometry` must work too.
    let point = geometry("<gml:Point><gml:pos>1 2</gml:pos></gml:Point>");
    assert_eq!(coordinate_count(&&point), 1);
}
