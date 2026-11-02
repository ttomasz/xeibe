//! Geometry parsing, WKB writing and arc linearization on synthetic GML
//! (`docs/benchmarks.md`).
//!
//! `cargo bench -p xeibe-geom`; one group: `cargo bench -p xeibe-geom -- parse`.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_core::{Dialect, NamespaceContext};
use xeibe_geom::linearize::linearize;
use xeibe_geom::options::LinearizeOptions;
use xeibe_geom::parse::{AxisResolver, GeometryParser, ParseContext};
use xeibe_geom::wkb::{Endianness, write_wkb};
use xeibe_geom::{AxisDecision, Geometry, GeometryOptions};
use xeibe_testkit::gml::geometry_document;
use xeibe_testkit::synth::Shape;

/// Northing first, as the srsName says: every position is swapped.
struct Swap;

impl AxisResolver for Swap {
    fn resolve(&self, _srs_name: Option<&str>, _dialect: Dialect) -> AxisDecision {
        AxisDecision {
            swap: true,
            reason: "benchmark".into(),
            conflicts: Vec::new(),
        }
    }
}

const SHAPES: &[(&str, Shape)] = &[
    ("point", Shape::Point),
    ("linestring_1000", Shape::LineString { vertices: 1000 }),
    (
        "polygon_256_holes_4",
        Shape::Polygon {
            vertices: 256,
            holes: 4,
        },
    ),
    (
        "multisurface_8x64",
        Shape::MultiSurface {
            parts: 8,
            vertices: 64,
        },
    ),
    ("arc_polygon", Shape::ArcPolygon),
];

/// At least 1000 positions of `shape`, in as many geometries as that takes,
/// in one wrapper element: setting up the reader costs about as much as
/// parsing a point.
fn document(shape: Shape) -> (String, u64) {
    let count = (1000 / shape.positions()).max(1);
    let snippets: String = (0..count as u64).map(|i| shape.snippet(i)).collect();
    (
        geometry_document(&snippets),
        (count * shape.positions()) as u64,
    )
}

/// Parse every geometry element in `document`, the way a feature's parser
/// meets each: positioned just after its start event.
fn parse_all(
    document: &str,
    options: &GeometryOptions,
    namespaces: &NamespaceContext,
) -> Vec<Geometry> {
    let parser = GeometryParser::new(options);
    let mut reader = GmlReader::new(document.as_bytes(), namespaces, 0);
    let mut geometries = Vec::new();
    let mut seen_wrapper = false;
    loop {
        match reader.next_event().expect("the document parses") {
            XmlEvent::Start { .. } if !seen_wrapper => seen_wrapper = true,
            XmlEvent::Start { .. } => geometries.push(
                parser
                    .parse(&mut reader, &ParseContext::default(), &Swap)
                    .expect("the geometry parses")
                    .geometry
                    .expect("a geometry"),
            ),
            XmlEvent::Eof => break geometries,
            _ => {}
        }
    }
}

fn bench_parse(c: &mut Criterion) {
    let options = GeometryOptions::default();
    let namespaces = NamespaceContext::new();
    let mut group = c.benchmark_group("parse");
    for &(name, shape) in SHAPES {
        let (document, positions) = document(shape);
        group.throughput(Throughput::Elements(positions));
        group.bench_with_input(BenchmarkId::from_parameter(name), &document, |b, doc| {
            b.iter(|| parse_all(black_box(doc), &options, &namespaces))
        });
    }
    group.finish();
}

fn bench_wkb(c: &mut Criterion) {
    let options = GeometryOptions::default();
    let namespaces = NamespaceContext::new();
    let mut group = c.benchmark_group("write_wkb");
    for &(name, shape) in SHAPES {
        let (document, positions) = document(shape);
        let geometries = parse_all(&document, &options, &namespaces);
        group.throughput(Throughput::Elements(positions));
        let mut out = Vec::new();
        group.bench_with_input(BenchmarkId::from_parameter(name), &geometries, |b, gs| {
            b.iter(|| {
                out.clear();
                for g in black_box(gs) {
                    write_wkb(g, Endianness::Little, &mut out);
                }
                out.len()
            })
        });
    }
    group.finish();
}

/// A circle in two arcs, at GDAL's default 4° step and at a finer one.
fn bench_linearize(c: &mut Criterion) {
    let geometry = parse_all(
        &geometry_document(&Shape::ArcPolygon.snippet(1)),
        &GeometryOptions::default(),
        &NamespaceContext::new(),
    )
    .remove(0);
    let mut group = c.benchmark_group("linearize");
    for step in [4.0, 0.5] {
        let options = LinearizeOptions {
            max_angle_step_deg: step,
            max_gap: None,
        };
        group.bench_with_input(
            BenchmarkId::new("arc_polygon_step_deg", step),
            &options,
            |b, options| b.iter(|| linearize(black_box(geometry.clone()), options)),
        );
    }
    group.finish();
}

criterion_group!(benches, bench_parse, bench_wkb, bench_linearize);
criterion_main!(benches);
