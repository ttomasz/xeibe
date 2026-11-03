//! `xeibe_reproject::arrow`: GeoArrow arrays that exist already, converted
//! in their own layout, checked against `Transform::convert_xy` on the same
//! coordinates. `crates/xeibe-proj-py/tests/test_arrow.py` goes through
//! every encoding from Python.

use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::Float64Type;
use arrow_array::{
    Array, ArrayRef, BinaryArray, FixedSizeListArray, Float64Array, ListArray, StringArray,
    StructArray,
};
use arrow_buffer::{NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow_schema::{DataType, Field, Fields};
use xeibe_reproject::arrow::{ArrayError, convert_array, convert_chunks, extent};
use xeibe_reproject::{AxisOrder, Transform};

const POINTS: [(f64, f64); 4] = [
    (500_000.0, 500_000.0),
    (600_000.0, 400_000.0),
    (700_000.0, 300_000.0),
    (450_000.0, 650_000.0),
];

fn poland() -> Transform {
    Transform::from_epsg(2180, 4258, AxisOrder::EastFirst).expect("a transform")
}

fn expected(t: &Transform, points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let (mut x, mut y): (Vec<f64>, Vec<f64>) = points.iter().copied().unzip();
    t.convert_xy(&mut x, &mut y).expect("the points convert");
    x.into_iter().zip(y).collect()
}

fn interleaved(points: &[(f64, f64)], nulls: Option<NullBuffer>) -> FixedSizeListArray {
    let flat: Vec<f64> = points.iter().flat_map(|&(x, y)| [x, y]).collect();
    let field = Arc::new(Field::new("xy", DataType::Float64, false));
    FixedSizeListArray::new(field, 2, Arc::new(Float64Array::from(flat)), nulls)
}

fn separated(points: &[(f64, f64)]) -> StructArray {
    let fields = Fields::from(vec![
        Field::new("x", DataType::Float64, false),
        Field::new("y", DataType::Float64, false),
    ]);
    let x: Vec<f64> = points.iter().map(|p| p.0).collect();
    let y: Vec<f64> = points.iter().map(|p| p.1).collect();
    StructArray::new(
        fields,
        vec![
            Arc::new(Float64Array::from(x)),
            Arc::new(Float64Array::from(y)),
        ],
        None,
    )
}

fn lines(values: ArrayRef, offsets: &[i32]) -> ListArray {
    let field = Arc::new(Field::new("vertices", values.data_type().clone(), false));
    ListArray::new(
        field,
        OffsetBuffer::new(ScalarBuffer::from(offsets.to_vec())),
        values,
        None,
    )
}

fn interleaved_coords(array: &FixedSizeListArray) -> Vec<(f64, f64)> {
    let v = array.values().as_primitive::<Float64Type>().values();
    v.chunks(2).map(|c| (c[0], c[1])).collect()
}

#[test]
fn interleaved_and_separated_linestrings() {
    let t = poland();
    let want = expected(&t, &POINTS);
    let array: ArrayRef = Arc::new(lines(Arc::new(interleaved(&POINTS, None)), &[0, 1, 4]));
    let out = convert_array(&array, &t).expect("it converts");
    let out = out.as_list::<i32>();
    assert_eq!(interleaved_coords(out.values().as_fixed_size_list()), want);
    // The offsets are the input's, not a copy.
    let before = array.as_list::<i32>().offsets().inner().inner().as_ptr();
    assert_eq!(out.offsets().inner().inner().as_ptr(), before);

    let array: ArrayRef = Arc::new(lines(Arc::new(separated(&POINTS)), &[0, 2, 4]));
    let out = convert_array(&array, &t).expect("it converts");
    let s = out.as_list::<i32>().values().as_struct().clone();
    let x = s.column(0).as_primitive::<Float64Type>().values().to_vec();
    let y = s.column(1).as_primitive::<Float64Type>().values().to_vec();
    assert_eq!(x.into_iter().zip(y).collect::<Vec<_>>(), want);
}

fn wkb_point(x: f64, y: f64, z: Option<f64>) -> Vec<u8> {
    let mut out = vec![1u8];
    out.extend(if z.is_some() { 1001u32 } else { 1u32 }.to_le_bytes());
    out.extend(x.to_le_bytes());
    out.extend(y.to_le_bytes());
    if let Some(z) = z {
        out.extend(z.to_le_bytes());
    }
    out
}

fn wkb_line(points: &[(f64, f64)]) -> Vec<u8> {
    let mut out = vec![0u8]; // big-endian
    out.extend(2u32.to_be_bytes());
    out.extend((points.len() as u32).to_be_bytes());
    for (x, y) in points {
        out.extend(x.to_be_bytes());
        out.extend(y.to_be_bytes());
    }
    out
}

#[test]
fn wkb_in_either_byte_order_and_with_z() {
    let t = Transform::from_epsg(4979, 4978, AxisOrder::EastFirst).expect("a transform");
    let line = [(19.0, 52.0), (20.0, 51.0)];
    let array: ArrayRef = Arc::new(BinaryArray::from_vec(vec![
        &wkb_point(19.0, 52.0, Some(100.0)),
        &wkb_line(&line),
    ]));
    let out = convert_array(&array, &t).expect("it converts");
    let out = out.as_binary::<i32>();
    let (x, y, z) = t.convert(19.0, 52.0, Some(100.0)).expect("a point");
    assert_eq!(out.value(0), wkb_point(x, y, z));
    // A 2D point of a 3D transform keeps no height.
    let converted: Vec<(f64, f64)> = line
        .iter()
        .map(|&(x, y)| {
            let (x, y, _) = t.convert(x, y, None).expect("a point");
            (x, y)
        })
        .collect();
    assert_eq!(out.value(1), wkb_line(&converted));
}

#[test]
fn coordinates_under_nulls_are_left_alone() {
    let t = poland();
    let bad = (f64::INFINITY, 1e300);
    let points = [POINTS[0], bad, POINTS[1]];
    let nulls = NullBuffer::from(vec![true, false, true]);
    let array: ArrayRef = Arc::new(interleaved(&points, Some(nulls)));
    let out = convert_array(&array, &t).expect("the null's coordinate isn't converted");
    let got = interleaved_coords(out.as_fixed_size_list());
    let want = expected(&t, &[POINTS[0], POINTS[1]]);
    assert_eq!(got, vec![want[0], bad, want[1]]);
}

#[test]
fn errors_name_the_geometry_over_every_chunk() {
    let t = Transform::from_epsg(4326, 3857, AxisOrder::EastFirst).expect("a transform");
    let chunks: Vec<ArrayRef> = vec![
        Arc::new(interleaved(&[(0.0, 0.0); 3], None)),
        Arc::new(interleaved(&[(1.0, 1.0), (10.0, 95.0)], None)),
    ];
    let error = convert_chunks(&chunks, &t).expect_err("latitude 95 fails");
    assert!(matches!(error, ArrayError::Convert { row: Some(4), .. }));
    assert!(
        error
            .to_string()
            .starts_with("geometry 4: coordinate (10, 95)")
    );

    let wkt: ArrayRef = Arc::new(StringArray::from(vec!["POINT (1 2)"]));
    let error = convert_array(&wkt, &t).expect_err("WKT isn't read");
    assert!(matches!(error, ArrayError::Layout(_)));
    let short: ArrayRef = Arc::new(BinaryArray::from_vec(vec![
        &wkb_point(1.0, 2.0, None)[..10],
    ]));
    let error = convert_array(&short, &t).expect_err("truncated WKB");
    assert!(error.to_string().starts_with("geometry 0: WKB"), "{error}");
}

#[test]
fn the_extent_and_boxes() {
    let array: ArrayRef = Arc::new(interleaved(&POINTS, None));
    let bounds = extent(&[array]).expect("an extent");
    assert_eq!(bounds, Some([450_000.0, 300_000.0, 700_000.0, 650_000.0]));
    assert_eq!(extent(&[]).expect("nothing"), None);

    let t = poland();
    let names = ["xmin", "ymin", "xmax", "ymax"];
    let row = [400_000.0, 300_000.0, 600_000.0, 500_000.0];
    let fields = Fields::from(
        names
            .iter()
            .map(|n| Field::new(*n, DataType::Float64, false))
            .collect::<Vec<_>>(),
    );
    let columns = row
        .iter()
        .map(|&v| Arc::new(Float64Array::from(vec![v])) as ArrayRef)
        .collect();
    let boxes: ArrayRef = Arc::new(StructArray::new(fields, columns, None));
    let out = convert_array(&boxes, &t).expect("it converts");
    let got: Vec<f64> = (0..4)
        .map(|k| {
            out.as_struct()
                .column(k)
                .as_primitive::<Float64Type>()
                .value(0)
        })
        .collect();
    assert_eq!(got, t.convert_bounds(row, 21).expect("a box").to_vec());
}
