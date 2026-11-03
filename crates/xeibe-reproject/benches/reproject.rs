//! CRS parsing, operation selection, coordinate conversion and geodesics
//! (`docs/benchmarks.md`). Nothing here needs grids.
//!
//! `cargo bench -p xeibe-reproject`; one group: `cargo bench -p xeibe-reproject -- convert`.

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use xeibe_reproject::geodesic::Geodesic;
use xeibe_reproject::wkt::{self, Version};
use xeibe_reproject::{AxisOrder, Crs, Transform};

/// Points per conversion benchmark.
#[cfg(feature = "arrow")]
mod arrow;

const POINTS: usize = 10_000;

/// Source and target CRSs, with a box of longitudes and latitudes inside
/// both areas of use, east first.
struct Case {
    name: &'static str,
    src: u32,
    dst: u32,
    area: [f64; 4],
}

const POLAND: [f64; 4] = [14.5, 49.2, 23.8, 54.7];

const CASES: &[Case] = &[
    // Transverse Mercator, one datum.
    Case {
        name: "4258_to_2180",
        src: 4258,
        dst: 2180,
        area: POLAND,
    },
    // Its inverse.
    Case {
        name: "2180_to_4258",
        src: 2180,
        dst: 4258,
        area: POLAND,
    },
    // Web Mercator.
    Case {
        name: "4326_to_3857",
        src: 4326,
        dst: 3857,
        area: [-170.0, -80.0, 170.0, 80.0],
    },
    // Lambert azimuthal equal area.
    Case {
        name: "4258_to_3035",
        src: 4258,
        dst: 3035,
        area: [-10.0, 35.0, 30.0, 70.0],
    },
    // Lambert conic conformal (2SP).
    Case {
        name: "4258_to_3034",
        src: 4258,
        dst: 3034,
        area: [-10.0, 35.0, 30.0, 70.0],
    },
    // Between two projections across datums: PL-1992 to UTM 34N on WGS 84.
    Case {
        name: "2180_to_32634",
        src: 2180,
        dst: 32634,
        area: [18.5, 49.5, 23.5, 54.5],
    },
];

/// `POINTS` positions spread over `area`, east first, from a fixed seed.
fn geographic(area: [f64; 4]) -> (Vec<f64>, Vec<f64>) {
    let mut state = 0x5eed_u64;
    let mut unit = move || {
        // SplitMix64.
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..POINTS)
        .map(|_| {
            (
                area[0] + unit() * (area[2] - area[0]),
                area[1] + unit() * (area[3] - area[1]),
            )
        })
        .unzip()
}

/// The case's points in its source CRS, east first.
fn points(case: &Case) -> (Vec<f64>, Vec<f64>) {
    let (mut x, mut y) = geographic(case.area);
    let src = Crs::from_epsg(case.src).expect("the source CRS");
    if src.method().is_some() {
        // A projected source: project the points into it first.
        let geographic = Crs::from_epsg(4258).expect("ETRS89");
        Transform::new(&geographic, &src, AxisOrder::EastFirst)
            .expect("a transform to the source")
            .convert_xy(&mut x, &mut y)
            .expect("the points convert");
    }
    (x, y)
}

fn bench_convert(c: &mut Criterion) {
    let mut group = c.benchmark_group("convert_xy");
    group.throughput(Throughput::Elements(POINTS as u64));
    for case in CASES {
        let transform =
            Transform::from_epsg(case.src, case.dst, AxisOrder::EastFirst).expect("a transform");
        let points = points(case);
        group.bench_with_input(BenchmarkId::from_parameter(case.name), &points, |b, p| {
            b.iter_batched_ref(
                || p.clone(),
                |(x, y)| transform.convert_xy(x, y).expect("the points convert"),
                BatchSize::LargeInput,
            )
        });
    }
    group.finish();
}

/// Choosing the operation: what a read with `target_crs` does once.
fn bench_select(c: &mut Criterion) {
    let mut group = c.benchmark_group("transform_new");
    for case in CASES {
        let src = Crs::from_epsg(case.src).expect("the source CRS");
        let dst = Crs::from_epsg(case.dst).expect("the target CRS");
        group.bench_function(case.name, |b| {
            b.iter(|| {
                Transform::new(black_box(&src), &dst, AxisOrder::EastFirst).expect("a transform")
            })
        });
    }
    group.finish();
}

fn bench_crs(c: &mut Criterion) {
    let projjson = xeibe_crs::projjson(2180).expect("EPSG:2180");
    let wkt2 = wkt::from_projjson(projjson, Version::Wkt2_2019, false).expect("WKT2");
    let wkt1 = wkt::from_projjson(projjson, Version::Wkt1Gdal, false).expect("WKT1");
    let proj = "+proj=tmerc +lat_0=0 +lon_0=19 +k=0.9993 +x_0=500000 +y_0=-5300000 \
                +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs +type=crs";

    let mut group = c.benchmark_group("crs");
    group.bench_function("from_epsg/2180", |b| {
        b.iter(|| Crs::from_epsg(black_box(2180)).expect("EPSG:2180"))
    });
    group.bench_function("from_projjson/2180", |b| {
        b.iter(|| Crs::from_projjson(black_box(projjson)).expect("PROJJSON"))
    });
    group.bench_function("from_wkt/wkt2_2180", |b| {
        b.iter(|| Crs::from_wkt(black_box(&wkt2)).expect("WKT2"))
    });
    group.bench_function("from_wkt/wkt1_2180", |b| {
        b.iter(|| Crs::from_wkt(black_box(&wkt1)).expect("WKT1"))
    });
    group.bench_function("from_proj_string/tmerc", |b| {
        b.iter(|| Crs::from_proj_string(black_box(proj)).expect("a PROJ string"))
    });
    group.bench_function("to_wkt/wkt2_2180", |b| {
        b.iter(|| wkt::from_projjson(black_box(projjson), Version::Wkt2_2019, false).expect("WKT2"))
    });
    group.bench_function("to_wkt/wkt1_2180", |b| {
        b.iter(|| wkt::from_projjson(black_box(projjson), Version::Wkt1Gdal, false).expect("WKT1"))
    });
    group.finish();
}

/// Karney's algorithms on WGS 84: Warsaw to points spread over the globe.
fn bench_geodesic(c: &mut Criterion) {
    let wgs84 = Geodesic::new(6_378_137.0, 1.0 / 298.257_223_563);
    let (lon, lat) = geographic([-180.0, -90.0, 180.0, 90.0]);
    let mut group = c.benchmark_group("geodesic");
    group.throughput(Throughput::Elements(POINTS as u64));
    group.bench_function("inverse", |b| {
        b.iter(|| {
            lon.iter()
                .zip(&lat)
                .map(|(&lon2, &lat2)| wgs84.inverse(52.23, 21.01, lat2, lon2).0)
                .sum::<f64>()
        })
    });
    group.bench_function("direct", |b| {
        b.iter(|| {
            lon.iter()
                .zip(&lat)
                .map(|(&azimuth, &distance)| {
                    // Azimuths of ±180°, distances up to 9,000 km.
                    wgs84
                        .direct(52.23, 21.01, azimuth, distance.abs() * 100_000.0)
                        .0
                })
                .sum::<f64>()
        })
    });
    group.finish();
}

/// GeoArrow arrays converted in their own layout (`benches/arrow/mod.rs`).
fn bench_arrow(c: &mut Criterion) {
    #[cfg(feature = "arrow")]
    arrow::bench(c);
    #[cfg(not(feature = "arrow"))]
    let _ = c;
}

criterion_group!(
    benches,
    bench_convert,
    bench_select,
    bench_crs,
    bench_geodesic,
    bench_arrow
);
criterion_main!(benches);
