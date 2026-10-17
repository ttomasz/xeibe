//! The public API: axis order, buffers, errors (`docs/reprojection.md`, level 1).

use xeibe_reproject::{AxisOrder, CoordinateError, Crs, DEFAULT_DENSIFY, Error, Transform};

/// Warsaw, Palace of Culture, roughly.
const WARSAW: (f64, f64) = (21.0067, 52.2319);

#[test]
fn east_first_order_swaps_northing_first_crss_only() {
    // EPSG:4258 is latitude first and EPSG:2180 northing first.
    let east = Transform::from_epsg(4258, 2180, AxisOrder::EastFirst).unwrap();
    let auth = Transform::from_epsg(4258, 2180, AxisOrder::Authority).unwrap();
    let (x, y, _) = east.convert(WARSAW.0, WARSAW.1, None).unwrap();
    let (n, e, _) = auth.convert(WARSAW.1, WARSAW.0, None).unwrap();
    assert_eq!((x, y), (e, n));
    // Easting about 637 km, northing about 487 km in PL-1992.
    assert!((600_000.0..700_000.0).contains(&x), "{x}");
    assert!((450_000.0..520_000.0).contains(&y), "{y}");
}

#[test]
fn round_trip_through_a_projection() {
    let t = Transform::from_epsg(4258, 2180, AxisOrder::EastFirst).unwrap();
    let (x, y, _) = t.convert(WARSAW.0, WARSAW.1, None).unwrap();
    let (lon, lat, _) = t.inverse().unwrap().convert(x, y, None).unwrap();
    assert!((lon - WARSAW.0).abs() < 1e-11 && (lat - WARSAW.1).abs() < 1e-11);
}

#[test]
fn between_two_projections_on_one_datum() {
    // PL-1992 to UTM zone 34N (EPSG:25834), both ETRS89. (PL-2000,
    // EPSG:2176-2179, is on ETRF2000-PL since EPSG 12, so not on one datum.)
    let t = Transform::from_epsg(2180, 25834, AxisOrder::EastFirst).unwrap();
    let to_1992 = Transform::from_epsg(4258, 2180, AxisOrder::EastFirst).unwrap();
    let to_utm = Transform::from_epsg(4258, 25834, AxisOrder::EastFirst).unwrap();
    let (x92, y92, _) = to_1992.convert(WARSAW.0, WARSAW.1, None).unwrap();
    let (xu, yu, _) = to_utm.convert(WARSAW.0, WARSAW.1, None).unwrap();
    let (x, y, _) = t.convert(x92, y92, None).unwrap();
    assert!((x - xu).abs() < 1e-6 && (y - yu).abs() < 1e-6);
}

#[test]
fn different_datums_go_through_a_transformation() {
    // ETRS89 / PL-1992 to WGS 84: EPSG's null transformation, good to 1 m.
    let t = Transform::from_epsg(2180, 4326, AxisOrder::EastFirst).unwrap();
    let (x, y, _) = t.convert(637_000.0, 485_000.0, None).unwrap();
    assert_eq!(
        t.operation_for(637_000.0, 485_000.0, None),
        Some("Inverse of Poland CS92 + ETRS89 to WGS 84 (1)")
    );
    assert!(
        (x - 21.005).abs() < 0.01 && (y - 52.215).abs() < 0.01,
        "{x} {y}"
    );
}

#[test]
fn routes_through_a_national_realization_both_ways() {
    // RT90 to ETRS89: EPSG's operation to SWEREF 99 (0.1 m) and the ensemble
    // identity (0.1 m) beat the direct "RT90 to ETRS89 (1)" (0.5 m).
    let via = "RT90 to ETRS89-SWE [SWEREF 99] (1)";
    let there = Transform::from_epsg(4124, 4258, AxisOrder::EastFirst).unwrap();
    let back = Transform::from_epsg(4258, 4124, AxisOrder::EastFirst).unwrap();
    let there_op = there.operation_for(18.07, 59.33, None).unwrap();
    let back_op = back.operation_for(18.07, 59.33, None).unwrap();
    assert!(there_op.starts_with(via), "{there_op}");
    assert!(back_op.contains(&format!("Inverse of {via}")), "{back_op}");
}

#[test]
fn unknown_codes_are_an_error() {
    assert_eq!(Crs::from_epsg(1).unwrap_err(), Error::UnknownCode(1));
}

#[test]
fn unsupported_crs_types_are_an_error() {
    // EPSG:5783 is a vertical CRS (DHHN92 height).
    let err = Crs::from_epsg(5783).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err}");
}

#[test]
fn nan_coordinates_pass_through() {
    // GeoArrow stores an empty point as NaN, NaN.
    let t = Transform::from_epsg(4258, 2180, AxisOrder::EastFirst).unwrap();
    let (x, y, _) = t.convert(f64::NAN, f64::NAN, None).unwrap();
    assert!(x.is_nan() && y.is_nan());
}

#[test]
fn bad_coordinates_name_their_index() {
    let t = Transform::from_epsg(4258, 2180, AxisOrder::EastFirst).unwrap();
    let mut xs = [WARSAW.0, 21.0, 21.0];
    let mut ys = [WARSAW.1, 95.0, 52.0];
    let err = t.convert_xy(&mut xs, &mut ys).unwrap_err();
    assert_eq!(
        err,
        Error::Coordinate {
            index: 1,
            x: 21.0,
            y: 95.0,
            reason: CoordinateError::InvalidGeographic
        }
    );
}

#[test]
fn buffers_convert_in_place() {
    let t = Transform::from_epsg(4258, 2180, AxisOrder::EastFirst).unwrap();
    let (x, y, _) = t.convert(WARSAW.0, WARSAW.1, None).unwrap();

    let mut xs = [WARSAW.0, WARSAW.0];
    let mut ys = [WARSAW.1, WARSAW.1];
    t.convert_xy(&mut xs, &mut ys).unwrap();
    assert_eq!((xs, ys), ([x, x], [y, y]));

    // Interleaved xyzm: z is carried through for a 2D CRS, m is untouched.
    let mut coords = [WARSAW.0, WARSAW.1, 120.0, 7.0];
    t.convert_interleaved(&mut coords, 4).unwrap();
    assert_eq!(coords, [x, y, 120.0, 7.0]);
}

#[test]
fn geographic_to_geocentric() {
    // ETRS89 geographic 3D (EPSG:4937) to geocentric (EPSG:4936), at the
    // equator on the prime meridian: X is a + h.
    let t = Transform::from_epsg(4937, 4936, AxisOrder::Authority).unwrap();
    let (x, y, z) = t.convert(0.0, 0.0, Some(100.0)).unwrap();
    assert!((x - 6_378_237.0).abs() < 1e-6 && y.abs() < 1e-9 && z.unwrap().abs() < 1e-9);
}

/// Expected values from pyproj 3.8.0 (PROJ 9.8.1) `transform_bounds` with
/// `always_xy=True, densify_pts=21`.
fn assert_bounds(src: u32, dst: u32, bounds: [f64; 4], expected: [f64; 4]) {
    let t = Transform::from_epsg(src, dst, AxisOrder::EastFirst).unwrap();
    let got = t.convert_bounds(bounds, DEFAULT_DENSIFY).unwrap();
    for (g, e) in got.iter().zip(expected) {
        assert!(
            (g - e).abs() < 1e-6,
            "EPSG:{src} -> EPSG:{dst}: {got:?}, PROJ {expected:?}"
        );
    }
}

#[test]
fn bounds_are_densified() {
    // Poland's area of use into PL-1992: the corners alone would miss the
    // bulge of the parallels.
    assert_bounds(
        4258,
        2180,
        [14.14, 49.0, 24.15, 55.93],
        [
            144693.2803694181,
            125837.01797721721,
            876500.3590720864,
            908411.1932242773,
        ],
    );
    assert_bounds(
        2180,
        4258,
        [170000.0, 130000.0, 870000.0, 780000.0],
        [
            13.866065593321276,
            48.926725651575815,
            24.752931601974275,
            54.88333990754053,
        ],
    );
}

#[test]
fn bounds_across_the_antimeridian() {
    // WGS 84 / PDC Mercator (central meridian 150° E) to geographic: the
    // result has max_x < min_x.
    assert_bounds(
        3832,
        4326,
        [2000000.0, -2000000.0, 4000000.0, 2000000.0],
        [
            167.96630568239044,
            -17.790560385794684,
            -174.06738863521912,
            17.790560385794684,
        ],
    );
    // And a geographic box given across the antimeridian.
    assert_bounds(
        4326,
        3832,
        [170.0, -20.0, -170.0, 20.0],
        [
            2226389.8158654715,
            -2258423.6490963795,
            4452779.631730937,
            2258423.6490963795,
        ],
    );
}
