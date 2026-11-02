//! Writing a read's batches to Parquet and Arrow IPC in memory
//! (`docs/benchmarks.md`). The read happens once, outside the measurement.
//!
//! `cargo bench -p xeibe-export`; one group: `cargo bench -p xeibe-export -- parquet`.

use std::io::Cursor;

use arrow_array::{RecordBatch, RecordBatchReader};
use arrow_schema::SchemaRef;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use xeibe_arrow::ReadOptions;
use xeibe_core::{Source, Sources};
use xeibe_export::{BboxColumn, IpcWriter, ParquetOptions, ParquetWriter};
use xeibe_geom::options::GeomEncoding;
use xeibe_testkit::synth::{LAYER, Shape, Synth};

/// `synth`'s layer, read with `encoding`.
fn batches(synth: &Synth, encoding: GeomEncoding) -> (SchemaRef, Vec<RecordBatch>) {
    let mut options = ReadOptions::default();
    options.inference.geometry_encoding = encoding;
    let sources = Sources::from(Source::reader(
        "synth.gml",
        Box::new(Cursor::new(synth.document().into_bytes())),
    ));
    let reader = xeibe_arrow::read(sources, LAYER, None, &options).expect("a reader");
    let schema = reader.schema();
    let batches = reader.map(|b| b.expect("a batch")).collect();
    (schema, batches)
}

fn in_memory_size(batches: &[RecordBatch]) -> u64 {
    batches
        .iter()
        .map(|b| b.get_array_memory_size() as u64)
        .sum()
}

fn write_parquet(schema: &SchemaRef, batches: &[RecordBatch], options: &ParquetOptions) -> usize {
    let mut out = Vec::new();
    let mut writer = ParquetWriter::new(&mut out, schema.clone(), options.clone(), Vec::new());
    for batch in batches {
        writer.write(batch).expect("the batch is written");
    }
    writer.finish().expect("the file is finished");
    out.len()
}

fn write_ipc(schema: &SchemaRef, batches: &[RecordBatch]) -> usize {
    let mut out = Vec::new();
    let mut writer = IpcWriter::new(&mut out, schema, Vec::new()).expect("a writer");
    for batch in batches {
        writer.write(batch).expect("the batch is written");
    }
    writer.finish().expect("the file is finished");
    out.len()
}

/// 20,000 polygons of 32 vertices with the usual properties.
fn polygons() -> Synth {
    Synth::new(
        20_000,
        Shape::Polygon {
            vertices: 32,
            holes: 0,
        },
    )
}

/// The throughput is in bytes of the batches in memory.
fn bench_parquet(c: &mut Criterion) {
    let mut group = c.benchmark_group("parquet");
    group.sample_size(10);
    for (name, encoding) in [("native", GeomEncoding::Auto), ("wkb", GeomEncoding::Wkb)] {
        let (schema, batches) = batches(&polygons(), encoding);
        group.throughput(Throughput::Bytes(in_memory_size(&batches)));
        for (bbox, bbox_column) in [("bbox", BboxColumn::Auto), ("no_bbox", BboxColumn::Never)] {
            let options = ParquetOptions {
                bbox_column,
                ..ParquetOptions::default()
            };
            group.bench_with_input(BenchmarkId::new(name, bbox), &options, |b, options| {
                b.iter(|| write_parquet(&schema, &batches, options))
            });
        }
    }
    group.finish();
}

fn bench_ipc(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc");
    group.sample_size(20);
    for (name, encoding) in [("native", GeomEncoding::Auto), ("wkb", GeomEncoding::Wkb)] {
        let (schema, batches) = batches(&polygons(), encoding);
        group.throughput(Throughput::Bytes(in_memory_size(&batches)));
        group.bench_function(name, |b| b.iter(|| write_ipc(&schema, &batches)));
    }
    group.finish();
}

criterion_group!(benches, bench_parquet, bench_ipc);
criterion_main!(benches);
