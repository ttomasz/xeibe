//! The synthetic datasets the benchmarks read (`docs/benchmarks.md`): the
//! benchmarks measure a read that infers every column's intended type and
//! puts the geometry in the right axis order.

use arrow_schema::{DataType, TimeUnit};
use xeibe_arrow::ReadOptions;
use xeibe_geom::options::GeomEncoding;
use xeibe_testkit::synth::{LAYER, OTHER_LAYER, Shape, Synth};
use xeibe_testkit::wkt::G;

use crate::support::*;

#[test]
fn a_synthetic_layer_reads_with_a_column_of_each_type() {
    let synth = Synth {
        extra_properties: 2,
        other_layer_every: Some(3),
        ..Synth::new(
            30,
            Shape::Polygon {
                vertices: 8,
                holes: 1,
            },
        )
    };
    let read = read_document(&synth.document(), LAYER);
    assert_eq!(read.rows(), 30);
    for (column, data_type) in [
        ("@id", DataType::Utf8View),
        ("name", DataType::Utf8View),
        ("code", DataType::Utf8View),
        ("floors", DataType::Int64),
        ("area", DataType::Float64),
        ("built", DataType::Date32),
        ("updated", DataType::Timestamp(TimeUnit::Microsecond, None)),
        ("active", DataType::Boolean),
        ("attr1", DataType::Utf8View),
    ] {
        assert_eq!(read.data_type(column), data_type, "{column}");
    }
    assert!(
        matches!(read.data_type("tag"), DataType::List(_)),
        "{}",
        read.data_type("tag")
    );
    // Leading zeros are kept, and the entity is expanded.
    let codes = read.strings("code");
    assert!(
        codes.iter().all(|c| c.as_ref().unwrap().len() == 8),
        "{codes:?}"
    );
    assert_eq!(read.strings("name")[0].as_deref(), Some("Smith & Sons 0"));
}

#[test]
fn a_synthetic_layer_is_read_easting_first_with_its_rings() {
    let mut options = ReadOptions::default();
    options.inference.geometry_encoding = GeomEncoding::Wkb;
    let synth = Synth::new(
        200,
        Shape::Polygon {
            vertices: 8,
            holes: 2,
        },
    );
    let read = read_with(&synth.document(), LAYER, None, &options);
    // Each polygon lies within 120 m of a centre with an easting in
    // 200–800 km and a northing in 150–750 km of PL-1992; the srsName says
    // northing first. Only eastings reach past 751 km, only northings fall
    // below 199 km.
    let (mut max_x, mut min_y) = (f64::MIN, f64::MAX);
    for geometry in read.geometries("geometry") {
        let Some(G::Polygon(rings)) = geometry else {
            panic!("{geometry:?}")
        };
        assert_eq!(rings.len(), 3);
        for position in rings.iter().flatten() {
            max_x = max_x.max(position[0]);
            min_y = min_y.min(position[1]);
        }
    }
    assert!(max_x > 751_000.0 && min_y < 199_000.0, "{max_x} {min_y}");
}

#[test]
fn every_synthetic_layer_is_found_by_a_scan() {
    let synth = Synth {
        other_layer_every: Some(4),
        ..Synth::new(20, Shape::ArcPolygon)
    };
    let scan = scan_document(&synth.document());
    let layers: Vec<(String, u64)> = scan
        .layers()
        .into_iter()
        .map(|l| (l.name.local.to_string(), l.feature_count))
        .collect();
    assert_eq!(
        layers,
        [(LAYER.to_string(), 20), (OTHER_LAYER.to_string(), 5)]
    );
}
