//! The geometry model itself: coordinate sequences, joining, and the type
//! simplification the output encoding relies on (`docs/geometry.md`).

use xeibe_geom::model::{
    CircularString, CompoundCurve, Coords, Curve, CurvePart, CurvePolygon, Dim, Geometry,
    GeometryCollection, JoinResult, LineString, MultiCurve, MultiPolygon, MultiSurface, OutputKind,
    Point, Polygon, Surface,
};

fn coords(dim: Dim, values: &[f64]) -> Coords {
    Coords {
        dim: Some(dim),
        values: values.to_vec(),
    }
}

fn line(values: &[f64]) -> LineString {
    LineString {
        coords: coords(Dim::Xy, values),
    }
}

fn ring() -> LineString {
    line(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0])
}

fn arc() -> CircularString {
    CircularString {
        coords: coords(Dim::Xy, &[0.0, 0.0, 1.0, 1.0, 2.0, 0.0]),
        computed: Vec::new(),
    }
}

#[test]
fn dimensions_have_a_size() {
    assert_eq!(Dim::Xy.size(), 2);
    assert_eq!(Dim::Xyz.size(), 3);
}

#[test]
fn coordinate_sequences_count_positions_not_values() {
    let mut coords = coords(Dim::Xyz, &[1.0, 2.0, 3.0]);
    assert_eq!(coords.len(), 1);
    assert!(!coords.is_empty());
    coords.push(&[4.0, 5.0, 6.0]);
    assert_eq!(coords.len(), 2);
    assert_eq!(coords.values, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

    assert!(Coords::default().is_empty());
    assert_eq!(Coords::default().len(), 0);
}

#[test]
fn sequences_can_be_reversed_and_tested_for_closure() {
    let mut coords = coords(Dim::Xy, &[0.0, 0.0, 1.0, 1.0, 2.0, 2.0]);
    coords.reverse();
    assert_eq!(coords.values, [2.0, 2.0, 1.0, 1.0, 0.0, 0.0]);
    assert!(!coords.is_closed());
    assert!(ring().coords.is_closed());
}

#[test]
fn joining_drops_a_repeated_position() {
    let mut first = coords(Dim::Xy, &[0.0, 0.0, 1.0, 1.0]);
    let result = first.append_joined(&coords(Dim::Xy, &[1.0, 1.0, 2.0, 0.0]), 1e-9);
    assert_eq!(result, JoinResult::Exact);
    assert_eq!(first.values, [0.0, 0.0, 1.0, 1.0, 2.0, 0.0]);
}

#[test]
fn joining_within_the_tolerance_drops_it_too_and_reports_the_distance() {
    let mut first = coords(Dim::Xy, &[0.0, 0.0, 1.0, 1.0]);
    let result = first.append_joined(&coords(Dim::Xy, &[1.0, 1.000_000_001, 2.0, 0.0]), 1e-6);
    assert!(
        matches!(result, JoinResult::WithinTolerance { .. }),
        "got {result:?}"
    );
    assert_eq!(first.len(), 3, "the near-duplicate position is dropped");
}

#[test]
fn a_gap_keeps_both_positions() {
    let mut first = coords(Dim::Xy, &[0.0, 0.0, 1.0, 1.0]);
    let result = first.append_joined(&coords(Dim::Xy, &[5.0, 5.0, 6.0, 6.0]), 1e-9);
    assert!(matches!(result, JoinResult::Gap { .. }), "got {result:?}");
    assert_eq!(first.len(), 4);
}

#[test]
fn curves_know_whether_they_are_linear() {
    assert!(Curve::Linear(line(&[0.0, 0.0, 1.0, 1.0])).is_linear());
    assert!(!Curve::Circular(arc()).is_linear());
    assert!(
        !Curve::Compound(CompoundCurve {
            parts: vec![CurvePart::Circular(arc())]
        })
        .is_linear()
    );
    // A compound curve of linear parts is linear, and can be flattened.
    let compound = Curve::Compound(CompoundCurve {
        parts: vec![
            CurvePart::Linear(line(&[0.0, 0.0, 1.0, 1.0])),
            CurvePart::Linear(line(&[1.0, 1.0, 2.0, 2.0])),
        ],
    });
    assert!(compound.is_linear());
    let flattened = compound.into_linear().expect("a line string");
    assert_eq!(flattened.coords.len(), 3, "the shared position appears once");
}

#[test]
fn geometries_report_their_output_kind_and_dimension() {
    assert_eq!(
        Geometry::Point(Point {
            coord: Some(vec![1.0, 2.0])
        })
        .kind(),
        OutputKind::Point
    );
    assert_eq!(
        Geometry::CircularString(arc()).kind(),
        OutputKind::CircularString
    );
    let point3d = Geometry::Point(Point {
        coord: Some(vec![1.0, 2.0, 3.0]),
    });
    assert_eq!(point3d.dim(), Some(Dim::Xyz));
    assert_eq!(
        Geometry::LineString(line(&[0.0, 0.0, 1.0, 1.0])).dim(),
        Some(Dim::Xy)
    );
}

#[test]
fn simple_geometries_are_the_ones_without_curves() {
    assert!(Geometry::LineString(line(&[0.0, 0.0, 1.0, 1.0])).is_simple());
    assert!(!Geometry::CircularString(arc()).is_simple());
    assert!(
        !Geometry::MultiSurface(MultiSurface(vec![Surface::CurvePolygon(CurvePolygon {
            exterior: Some(Curve::Circular(arc())),
            interiors: Vec::new(),
        })]))
        .is_simple()
    );
    assert!(
        Geometry::MultiSurface(MultiSurface(vec![Surface::Polygon(Polygon {
            exterior: Some(ring()),
            interiors: Vec::new(),
        })]))
        .is_simple()
    );
}

#[test]
fn curve_free_curve_types_simplify_to_simple_features() {
    let multi_surface = Geometry::MultiSurface(MultiSurface(vec![Surface::Polygon(Polygon {
        exterior: Some(ring()),
        interiors: Vec::new(),
    })]));
    assert_eq!(
        multi_surface.simplify_types().kind(),
        OutputKind::MultiPolygon
    );

    let multi_curve = Geometry::MultiCurve(MultiCurve(vec![Curve::Linear(line(&[
        0.0, 0.0, 1.0, 1.0,
    ]))]));
    assert_eq!(
        multi_curve.simplify_types().kind(),
        OutputKind::MultiLineString
    );

    let curve_polygon = Geometry::CurvePolygon(CurvePolygon {
        exterior: Some(Curve::Linear(ring())),
        interiors: Vec::new(),
    });
    assert_eq!(curve_polygon.simplify_types().kind(), OutputKind::Polygon);

    // A geometry with a curve keeps its curve type.
    let curved = Geometry::CurvePolygon(CurvePolygon {
        exterior: Some(Curve::Circular(arc())),
        interiors: Vec::new(),
    });
    assert_eq!(curved.simplify_types().kind(), OutputKind::CurvePolygon);
}

#[test]
fn bounding_boxes_cover_every_part() {
    let collection = Geometry::GeometryCollection(GeometryCollection(vec![
        Geometry::Point(Point {
            coord: Some(vec![0.0, 10.0]),
        }),
        Geometry::MultiPolygon(MultiPolygon(vec![Polygon {
            exterior: Some(line(&[-5.0, -5.0, 5.0, -5.0, 5.0, 5.0, -5.0, -5.0])),
            interiors: Vec::new(),
        }])),
    ]));
    assert_eq!(collection.bbox(), Some([-5.0, -5.0, 5.0, 10.0]));
    // An empty geometry has no bounding box.
    assert_eq!(Geometry::Point(Point { coord: None }).bbox(), None);
}
