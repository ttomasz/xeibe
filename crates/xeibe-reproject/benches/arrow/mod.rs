//! GeoArrow arrays that exist already, converted in their own layout
//! (`xeibe_reproject::arrow`, what `xeibe_proj.arrow` runs;
//! `docs/benchmarks.md`).
//!
//! `cargo bench -p xeibe-reproject -- arrow/`; with the `arrow` feature,
//! which is on by default. Every group
//! converts the same 320,000 coordinates (10,000 polygons of 32 vertices in
//! EPSG:2180) to ETRS89 (EPSG:4258), and `convert_xy` on them alone is the
//! baseline: the difference is what taking the arrays apart and putting
//! them back together costs.

use std::hint::black_box;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, BinaryArray, FixedSizeListArray, Float64Array, ListArray, StructArray,
};
use arrow_buffer::{OffsetBuffer, ScalarBuffer};
use arrow_schema::{DataType, Field, Fields};
use criterion::{BatchSize, Criterion, Throughput};
use xeibe_reproject::arrow::{convert_array, extent};
use xeibe_reproject::{AxisOrder, Transform};

const POLYGONS: usize = 10_000;
const VERTICES: usize = 32;

/// Rings of `VERTICES` (closed: the last is the first) around random
/// centres in Poland, in EPSG:2180, easting first. Fixed seed.
fn rings() -> Vec<Vec<(f64, f64)>> {
    let mut state = 0x5eed_u64;
    let mut unit = move || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..POLYGONS)
        .map(|_| {
            let (cx, cy) = (
                200_000.0 + unit() * 600_000.0,
                150_000.0 + unit() * 600_000.0,
            );
            let r = 50.0 + unit() * 450.0;
            let mut ring: Vec<(f64, f64)> = (0..VERTICES - 1)
                .map(|k| {
                    let a = k as f64 / (VERTICES - 1) as f64 * std::f64::consts::TAU;
                    (cx + r * a.cos(), cy + r * a.sin())
                })
                .collect();
            ring.push(ring[0]);
            ring
        })
        .collect()
}

fn interleaved(points: &[(f64, f64)]) -> ArrayRef {
    let flat: Vec<f64> = points.iter().flat_map(|&(x, y)| [x, y]).collect();
    let field = Arc::new(Field::new("xy", DataType::Float64, false));
    Arc::new(FixedSizeListArray::new(
        field,
        2,
        Arc::new(Float64Array::from(flat)),
        None,
    ))
}

fn separated(points: &[(f64, f64)]) -> ArrayRef {
    let fields = Fields::from(vec![
        Field::new("x", DataType::Float64, false),
        Field::new("y", DataType::Float64, false),
    ]);
    let x: Vec<f64> = points.iter().map(|p| p.0).collect();
    let y: Vec<f64> = points.iter().map(|p| p.1).collect();
    Arc::new(StructArray::new(
        fields,
        vec![
            Arc::new(Float64Array::from(x)),
            Arc::new(Float64Array::from(y)),
        ],
        None,
    ))
}

/// A list with `size` values in each element.
fn list(values: ArrayRef, size: usize) -> ArrayRef {
    let n = values.len() / size;
    let offsets: Vec<i32> = (0..=n).map(|i| (i * size) as i32).collect();
    let field = Arc::new(Field::new("item", values.data_type().clone(), false));
    Arc::new(ListArray::new(
        field,
        OffsetBuffer::new(ScalarBuffer::from(offsets)),
        values,
        None,
    ))
}

/// GeoArrow polygons (one ring each) over `coords`.
fn polygons(coords: ArrayRef) -> ArrayRef {
    list(list(coords, VERTICES), 1)
}

fn wkb_polygon(ring: &[(f64, f64)]) -> Vec<u8> {
    let mut out = vec![1u8];
    out.extend(3u32.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out.extend((ring.len() as u32).to_le_bytes());
    for (x, y) in ring {
        out.extend(x.to_le_bytes());
        out.extend(y.to_le_bytes());
    }
    out
}

fn wkb_point(&(x, y): &(f64, f64)) -> Vec<u8> {
    let mut out = vec![1u8];
    out.extend(1u32.to_le_bytes());
    out.extend(x.to_le_bytes());
    out.extend(y.to_le_bytes());
    out
}

fn wkb(values: Vec<Vec<u8>>) -> ArrayRef {
    Arc::new(BinaryArray::from_vec(
        values.iter().map(Vec::as_slice).collect(),
    ))
}

pub fn bench(c: &mut Criterion) {
    let transform = Transform::from_epsg(2180, 4258, AxisOrder::EastFirst).expect("a transform");
    let rings = rings();
    let points: Vec<(f64, f64)> = rings.iter().flatten().copied().collect();
    let arrays: [(&str, ArrayRef); 5] = [
        ("polygon/separated", polygons(separated(&points))),
        ("polygon/interleaved", polygons(interleaved(&points))),
        (
            "polygon/wkb",
            wkb(rings.iter().map(|r| wkb_polygon(r)).collect()),
        ),
        ("point/interleaved", interleaved(&points)),
        ("point/wkb", wkb(points.iter().map(wkb_point).collect())),
    ];

    let mut group = c.benchmark_group("arrow/convert_array");
    group.throughput(Throughput::Elements(points.len() as u64));
    let xy: (Vec<f64>, Vec<f64>) = points.iter().copied().unzip();
    group.bench_function("baseline/convert_xy", |b| {
        b.iter_batched_ref(
            || xy.clone(),
            |(x, y)| transform.convert_xy(x, y).expect("the points convert"),
            BatchSize::LargeInput,
        )
    });
    for (name, array) in &arrays {
        group.bench_function(*name, |b| {
            b.iter(|| convert_array(black_box(array), &transform).expect("it converts"))
        });
    }
    group.finish();

    // The pass over the coordinates that the grid check takes first.
    let mut group = c.benchmark_group("arrow/extent");
    group.throughput(Throughput::Elements(points.len() as u64));
    for (name, array) in &arrays {
        let chunks = std::slice::from_ref(array);
        group.bench_function(*name, |b| {
            b.iter(|| extent(black_box(chunks)).expect("an extent"))
        });
    }
    group.finish();
}
