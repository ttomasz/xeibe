//! End-to-end scans and reads of synthetic GML into Arrow
//! (`docs/benchmarks.md`).
//!
//! `cargo bench -p xeibe-arrow`; one group: `cargo bench -p xeibe-arrow -- read/`.
//! The `all` thread count is one worker per core
//! (`xeibe_core::parallel::default_threads`).

use std::io::Cursor;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use xeibe_arrow::{ReadOptions, read, scan};
use xeibe_core::{Source, Sources};
use xeibe_geom::options::GeomEncoding;
use xeibe_schema::ScanExtent;
use xeibe_testkit::synth::{LAYER, Shape, Synth};

/// One source over the bytes, which every iteration shares.
fn sources(bytes: &Arc<[u8]>) -> Sources {
    Sources::from(Source::reader(
        "synth.gml",
        Box::new(Cursor::new(Arc::clone(bytes))),
    ))
}

fn options(threads: usize) -> ReadOptions {
    ReadOptions {
        threads,
        ..ReadOptions::default()
    }
}

/// `1` and every core, named so that results compare across machines.
fn thread_counts() -> [(&'static str, usize); 2] {
    [("1", 1), ("all", xeibe_core::parallel::default_threads())]
}

/// Read `LAYER` to the end; the number of rows.
fn read_rows(bytes: &Arc<[u8]>, schema: Option<SchemaRef>, options: &ReadOptions) -> usize {
    read(sources(bytes), LAYER, schema, options)
        .expect("a reader")
        .map(|batch| batch.as_ref().map(RecordBatch::num_rows).expect("a batch"))
        .sum()
}

/// About 12.5 MB: 10,000 polygons of 32 vertices, a road after every tenth.
fn polygons() -> Synth {
    Synth {
        other_layer_every: Some(10),
        ..Synth::new(
            10_000,
            Shape::Polygon {
                vertices: 32,
                holes: 0,
            },
        )
    }
}

fn bench_scan(c: &mut Criterion) {
    let bytes: Arc<[u8]> = polygons().document().into_bytes().into();
    let mut group = c.benchmark_group("scan");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.sample_size(10);
    for (name, threads) in thread_counts() {
        group.bench_with_input(BenchmarkId::new("full", name), &threads, |b, &threads| {
            b.iter(|| {
                scan(sources(&bytes), ScanExtent::Full, &options(threads))
                    .expect("the document scans")
                    .layers()
                    .len()
            })
        });
    }
    group.finish();
}

fn bench_read(c: &mut Criterion) {
    let synth = polygons();
    let bytes: Arc<[u8]> = synth.document().into_bytes().into();
    let schema = scan(sources(&bytes), ScanExtent::Full, &options(0))
        .expect("the document scans")
        .arrow_schema(LAYER)
        .expect("the layer's schema");
    assert_eq!(
        read_rows(&bytes, Some(schema.clone()), &options(0)),
        synth.features
    );

    let mut group = c.benchmark_group("read");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.sample_size(10);
    for (name, threads) in thread_counts() {
        // A schema sampled from the layer's first features, then the rest.
        group.bench_with_input(BenchmarkId::new("sampled", name), &threads, |b, &t| {
            b.iter(|| read_rows(&bytes, None, &options(t)))
        });
        // The schema a scan saved: one pass, no inference.
        group.bench_with_input(BenchmarkId::new("given", name), &threads, |b, &t| {
            b.iter(|| read_rows(&bytes, Some(schema.clone()), &options(t)))
        });
    }

    let mut wkb = options(1);
    wkb.inference.geometry_encoding = GeomEncoding::Wkb;
    group.bench_function("wkb/1", |b| b.iter(|| read_rows(&bytes, None, &wkb)));

    // Projection pushdown: the other properties are skipped unparsed.
    let geometry_only = ReadOptions {
        projection: Some(vec!["geometry".to_string()]),
        ..options(1)
    };
    group.bench_function("geometry_only/1", |b| {
        b.iter(|| read_rows(&bytes, Some(schema.clone()), &geometry_only))
    });

    // PL-1992 to ETRS89 geographic: the inverse projection on every vertex.
    let mut reprojected = options(1);
    reprojected.geometry.target_crs = Some("EPSG:4258".to_string());
    group.bench_function("to_epsg_4258/1", |b| {
        b.iter(|| read_rows(&bytes, Some(schema.clone()), &reprojected))
    });
    group.finish();
}

/// Feature-heavy rather than coordinate-heavy: 25,000 points with ten
/// extra string properties, about 19 MB.
fn bench_read_points(c: &mut Criterion) {
    let synth = Synth {
        extra_properties: 10,
        ..Synth::new(25_000, Shape::Point)
    };
    let bytes: Arc<[u8]> = synth.document().into_bytes().into();
    let mut group = c.benchmark_group("read_points");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.sample_size(10);
    for (name, threads) in thread_counts() {
        group.bench_with_input(BenchmarkId::new("sampled", name), &threads, |b, &t| {
            b.iter(|| read_rows(&bytes, None, &options(t)))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_scan, bench_read, bench_read_points);
criterion_main!(benches);
