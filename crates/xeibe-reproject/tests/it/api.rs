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

/// PROJ 9.8.1's `always_xy` (pyproj 3.8.0) for (1000 km, 2000 km), to each
/// CRS's geographic CRS. Of the polar CRSs with both axes along meridians,
/// only UPS North and South (N,E) and RSPS2000 are swapped: the others are
/// as written (EPSG:3031's "E" points north along 90°E, its "N" along 0°E).
const POLAR: &[(u32, u32, f64, f64)] = &[
    (3031, 4326, 26.56505117707799, -69.62866938575715),
    (3032, 4326, -58.65980825409011, -35.43650425675358),
    (3276, 4326, -108.43494882292201, -70.03090169172592),
    (3413, 4326, 108.43494882292201, 69.56876575655875),
    (3976, 4326, 26.56505117707799, -69.56876575655875),
    (5041, 4326, -90.0, 81.01066326450201),
    (5042, 4326, -90.0, -81.01066326450201),
    (5482, 4764, 104.03624346792647, -54.04366174636288),
    (6932, 4326, 26.56505117707799, -69.86894542320586),
    (9354, 4326, 26.56505117707799, -69.21874355612178),
    (32661, 4326, -90.0, 81.01066326450201),
    (32761, 4326, -90.0, -81.01066326450201),
];

#[test]
fn east_first_order_swaps_polar_axes_as_proj_does() {
    for &(code, geographic, lon, lat) in POLAR {
        let t = Transform::from_epsg(code, geographic, AxisOrder::EastFirst).unwrap();
        let (x, y, _) = t.convert(1_000_000.0, 2_000_000.0, None).unwrap();
        assert!(
            (x - lon).abs() < 1e-9 && (y - lat).abs() < 1e-9,
            "EPSG:{code}: {x} {y}, PROJ {lon} {lat}"
        );
        let (e, n, _) = t.inverse().unwrap().convert(lon, lat, None).unwrap();
        assert!(
            (e - 1_000_000.0).abs() < 1e-6 && (n - 2_000_000.0).abs() < 1e-6,
            "EPSG:{code}: {e} {n}"
        );
    }
}

#[test]
fn east_first_order_keeps_northing_westing_axes() {
    // Astra Minas Grid: northing, then westing. A swap wouldn't make it x/y,
    // and PROJ's always_xy keeps it as it is: PROJ 9.8.1 gives
    // (4915661.955434943, 2590394.630374936) in Campo Inchauspe /
    // Argentina 2.
    let t = Transform::from_epsg(5800, 22192, AxisOrder::EastFirst).unwrap();
    let (x, y, _) = t.convert(10_000.0, 20_000.0, None).unwrap();
    assert!(
        (x - 4_915_661.955_434_943).abs() < 1e-6 && (y - 2_590_394.630_374_936).abs() < 1e-6,
        "{x} {y}"
    );
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
    // EPSG:22300 is on the Tunisia Mining Grid, a method not implemented.
    let err = Crs::from_epsg(22300).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err}");
}

#[test]
fn a_vertical_crs_needs_a_horizontal_one() {
    // EPSG:5783 is DHHN92 height: heights alone have no position.
    let err = Transform::from_epsg(5783, 5783, AxisOrder::EastFirst).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err}");
}

#[test]
fn compound_crss_convert_their_heights_on_one_vertical_datum() {
    let crs = |h: u32, v: u32| {
        Crs::compound(&Crs::from_epsg(h).unwrap(), &Crs::from_epsg(v).unwrap()).unwrap()
    };
    // PL-1992 + PL-EVRF2007 height to ETRS89 + the same height: the
    // horizontal part converts, the height stays.
    let t = Transform::new(&crs(2180, 9651), &crs(4258, 9651), AxisOrder::EastFirst).unwrap();
    assert!(t.operations().is_empty());
    let (lon, lat, h) = t.convert(500_000.0, 500_000.0, Some(123.4)).unwrap();
    assert!(
        (lon - 19.0).abs() < 1e-9 && (lat - 52.366).abs() < 1e-3,
        "{lon} {lat}"
    );
    assert_eq!(h, Some(123.4));
    // MSL height to MSL depth (EPSG:5715): the sign.
    let t = Transform::new(&crs(4326, 5714), &crs(4326, 5715), AxisOrder::EastFirst).unwrap();
    assert_eq!(t.convert(10.0, 50.0, Some(12.5)).unwrap().2, Some(-12.5));
    // NAVD88 height in metres to US survey feet (EPSG:6360).
    let t = Transform::new(&crs(4269, 5703), &crs(4269, 6360), AxisOrder::EastFirst).unwrap();
    let ft = t.convert(-100.0, 40.0, Some(1200.0)).unwrap().2.unwrap();
    assert!(
        (ft - 1200.0 / 0.304_800_609_601_219_24).abs() < 1e-9,
        "{ft}"
    );
    // To a CRS without heights, the height is dropped (handed back as is).
    let t = Transform::new(
        &crs(2180, 9651),
        &Crs::from_epsg(4258).unwrap(),
        AxisOrder::EastFirst,
    )
    .unwrap();
    assert_eq!(
        t.convert(500_000.0, 500_000.0, Some(5.0)).unwrap().2,
        Some(5.0)
    );
    // An EPSG compound CRS reads the same: ETRS89 + EVRF2000 height.
    let epsg = Crs::from_epsg(7409).unwrap();
    assert_eq!(epsg.dimension(), 3);
    assert_eq!(epsg.name(), "ETRS89 + EVRF2000 height");
}

#[test]
fn offsets_run_between_axes_of_one_direction() {
    let crs = |h: u32, v: u32| {
        Crs::compound(&Crs::from_epsg(h).unwrap(), &Crs::from_epsg(v).unwrap()).unwrap()
    };
    // Hong Kong Principal Datum height to Chart Datum depth: EPSG's
    // operation converts the height to one above Chart Datum, 0.146 m
    // lower, and turns it into a depth.
    let (src, dst) = (crs(4326, 5738), crs(4326, 5739));
    let t = Transform::new(&src, &dst, AxisOrder::EastFirst).unwrap();
    let depth = t.convert(114.2, 22.3, Some(10.0)).unwrap().2.unwrap();
    assert!((depth + 10.146).abs() < 1e-9, "{depth}");
    // EPSG's deprecated offset straight from the height to the depth is
    // outside what its formula covers (one axis direction and unit).
    let err = Transform::via(&src, &dst, 5454, AxisOrder::EastFirst).unwrap_err();
    assert!(matches!(err, Error::Unsupported { .. }), "{err}");
}

#[test]
fn engineering_crss_convert_on_their_datum_and_through_epsg_operations() {
    // Barcelona Grid B1 and B2: one engineering datum, so one grid.
    let t = Transform::from_epsg(5801, 5802, AxisOrder::Authority).unwrap();
    assert!(t.operations().is_empty());
    assert_eq!(
        t.convert(300_000.0, 500_000.0, None).unwrap(),
        (300_000.0, 500_000.0, None)
    );
    // Heathrow Airport Grid from British National Grid: EPSG's similarity
    // transformation (PROJ: 6809.315948037705, 5889.0115063533885).
    let t = Transform::from_epsg(27700, 11378, AxisOrder::Authority).unwrap();
    let (e, n, _) = t.convert(507_000.0, 176_000.0, None).unwrap();
    assert!(
        (e - 6_809.315_948).abs() < 1e-6 && (n - 5_889.011_506).abs() < 1e-6,
        "{e} {n}"
    );
    // Nothing ties a local grid to anything else.
    let err = Transform::from_epsg(11378, 4326, AxisOrder::EastFirst).unwrap_err();
    assert!(matches!(err, Error::NoTransformation { .. }), "{err}");
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
