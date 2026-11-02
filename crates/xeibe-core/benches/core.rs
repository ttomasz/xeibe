//! Splitting, decoding and tokenizing synthetic GML (`docs/benchmarks.md`).
//!
//! `cargo bench -p xeibe-core`; one group: `cargo bench -p xeibe-core -- split`.

use std::hint::black_box;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use xeibe_core::decode::decoded_reader;
use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_core::{FeatureSplitter, QName, SourceId, SplitterOptions};
use xeibe_testkit::gml::APP;
use xeibe_testkit::synth::{LAYER, Shape, Synth};

/// About 46 MB: 40,000 polygons of 24 vertices, a road after every fourth building.
fn document() -> String {
    Synth {
        other_layer_every: Some(4),
        ..Synth::new(
            40_000,
            Shape::Polygon {
                vertices: 24,
                holes: 0,
            },
        )
    }
    .document()
}

/// Split everything from `reader`; the number of features.
fn split(reader: impl Read, options: SplitterOptions) -> u64 {
    FeatureSplitter::new(reader, SourceId(0), options)
        .map(|chunk| chunk.expect("the document splits").features)
        .sum()
}

fn one_layer() -> SplitterOptions {
    SplitterOptions {
        layers: Some(vec![QName::new(Some(APP), LAYER)]),
        ..SplitterOptions::default()
    }
}

fn bench_split(c: &mut Criterion) {
    let doc = document();
    let mut group = c.benchmark_group("split");
    group.throughput(Throughput::Bytes(doc.len() as u64));
    group.sample_size(20);
    group.bench_function("all_layers", |b| {
        b.iter(|| split(black_box(doc.as_bytes()), SplitterOptions::default()))
    });
    // A read's splitter: the roads are skipped unparsed.
    group.bench_function("one_layer", |b| {
        b.iter(|| split(black_box(doc.as_bytes()), one_layer()))
    });
    group.finish();
}

/// Decompress or transcode, then split one layer, as a read's sources do.
/// The throughput is in decoded bytes.
fn bench_decode(c: &mut Criterion) {
    let doc = document();
    let gzip: Arc<[u8]> = {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(doc.as_bytes()).unwrap();
        encoder.finish().unwrap().into()
    };
    let zstd: Arc<[u8]> = zstd::encode_all(doc.as_bytes(), 3).unwrap().into();
    // The content is ASCII, so only the declaration changes.
    let latin2: Arc<[u8]> = doc
        .replacen("encoding=\"UTF-8\"", "encoding=\"ISO-8859-2\"", 1)
        .into_bytes()
        .into();
    let plain: Arc<[u8]> = doc.into_bytes().into();

    let mut group = c.benchmark_group("decode_and_split");
    group.throughput(Throughput::Bytes(plain.len() as u64));
    group.sample_size(20);
    for (name, bytes) in [
        ("plain", &plain),
        ("gzip", &gzip),
        ("zstd", &zstd),
        ("iso-8859-2", &latin2),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| {
                let raw = Box::new(Cursor::new(Arc::clone(bytes)));
                split(decoded_reader(raw).expect("a decoder"), one_layer())
            })
        });
    }
    group.finish();
}

/// The namespace-resolving tokenizer the parsers sit on, over one chunk.
fn bench_reader(c: &mut Criterion) {
    let chunk = FeatureSplitter::new(
        document().as_bytes(),
        SourceId(0),
        SplitterOptions::default(),
    )
    .next()
    .expect("a chunk")
    .expect("the document splits");
    let mut group = c.benchmark_group("reader");
    group.throughput(Throughput::Bytes(chunk.bytes.len() as u64));
    group.bench_function("events", |b| {
        b.iter(|| {
            let mut reader = GmlReader::for_chunk(black_box(&chunk));
            let mut events = 0u64;
            loop {
                match reader.next_event().expect("a chunk parses") {
                    XmlEvent::Eof => break events,
                    event => {
                        black_box(&event);
                        events += 1;
                    }
                }
            }
        })
    });
    // Skipping each top-level element whole, as a read does for subtrees
    // no column needs: no names resolved, no text unescaped.
    group.bench_function("skip_subtrees", |b| {
        b.iter(|| {
            let mut reader = GmlReader::for_chunk(black_box(&chunk));
            let mut skipped = 0u64;
            loop {
                match reader.next_event().expect("a chunk parses") {
                    XmlEvent::Start { .. } => {
                        reader.skip_element().expect("a chunk parses");
                        skipped += 1;
                    }
                    XmlEvent::Eof => break skipped,
                    _ => {}
                }
            }
        })
    });
    group.finish();
}

criterion_group!(benches, bench_split, bench_decode, bench_reader);
criterion_main!(benches);
