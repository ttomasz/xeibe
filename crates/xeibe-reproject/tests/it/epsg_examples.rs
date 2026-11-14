//! What PROJ can't run, against EPSG's own worked examples (EPSG Guidance
//! Note 7-2, as the dataset transcribes them in each method's example), or
//! against PROJ's results for an equivalent where there is one. Each
//! method's section of `docs/reprojection.md` says which.

use xeibe_reproject::{AxisOrder, Crs, Transform};

fn crs(code: u32) -> Crs {
    Crs::from_epsg(code).unwrap()
}

fn east_first(src: u32, dst: u32) -> Transform {
    Transform::new(&crs(src), &crs(dst), AxisOrder::EastFirst).unwrap()
}

fn dms(d: f64, m: f64, s: f64) -> f64 {
    d.signum() * (d.abs() + m / 60.0 + s / 3600.0)
}

fn assert_near(got: f64, want: f64, tolerance: f64, what: &str) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: {got}, want {want}"
    );
}

/// A projected CRS's example: the forward to the published easting and
/// northing (given to the centimetre), and the published point back.
fn projection_example(geog: u32, projected: u32, lon: f64, lat: f64, e: f64, n: f64) {
    let t = east_first(geog, projected);
    let (x, y, _) = t.convert(lon, lat, None).unwrap();
    assert_near(x, e, 0.005, "easting");
    assert_near(y, n, 0.005, "northing");
    let (lon2, lat2, _) = t.inverse().unwrap().convert(x, y, None).unwrap();
    assert_near(lon2, lon, 1e-9, "longitude back");
    assert_near(lat2, lat, 1e-9, "latitude back");
}

/// Polar Stereographic (variant C): Petrels 1972 / Terre Adelie Polar
/// Stereographic.
#[test]
fn polar_stereographic_variant_c() {
    projection_example(
        4636,
        2985,
        dms(140.0, 4.0, 17.040),
        dms(-66.0, 36.0, 18.820),
        303_169.52,
        244_055.72,
    );
}

/// Lambert Conic Near-Conformal: Deir ez Zor / Levant Zone. EPSG gives the
/// near-conformal result (15707.96 E, 623165.96 N; the full Lambert Conic
/// would be 15708.00, 623167.20).
#[test]
fn lambert_conic_near_conformal() {
    projection_example(
        4227,
        22700,
        dms(34.0, 8.0, 11.291),
        dms(37.0, 31.0, 17.625),
        15_707.96,
        623_165.96,
    );
}

/// Tunisia Mining Grid: grid reference 302598 (302 km E, 598 km N, the
/// CRS's kilometres) is 35.08197°N 9.73916°E of Greenwich on Carthage.
#[test]
fn tunisia_mining_grid() {
    let t = east_first(22300, 4223);
    let (lon, lat, _) = t.convert(302.0, 598.0, None).unwrap();
    assert_near(lat, 35.081_97, 5e-6, "latitude");
    assert_near(lon, 9.739_16, 5e-6, "longitude");
    // South of the false origin, a kilometre is 0.01002 grad, not 0.010015.
    let (_, south, _) = t.convert(270.0, 40.0, None).unwrap();
    assert_near(south, (36.5964 - 320.0 * 0.01002) * 0.9, 1e-9, "south");
    let (e, n, _) = t.inverse().unwrap().convert(lon, lat, None).unwrap();
    assert_near(e, 302.0, 1e-9, "easting back");
    assert_near(n, 598.0, 1e-9, "northing back");
}

/// Lambert Conic Conformal (West Orientated): Scoresbysund 1952 /
/// Greenland zone 5 east, northing then westing. EPSG gives no example; the
/// 1SP formulas but for W = FE − r sin θ, so with FE = 0 the westing is
/// PROJ's `lcc` easting negated (PROJ 9.8.1, `+proj=lcc +lat_1=70.5
/// +lat_0=70.5 +lon_0=-24 +k_0=1 +ellps=intl`).
#[test]
fn lambert_conic_conformal_west_orientated() {
    let t = Transform::new(&crs(4195), &crs(2218), AxisOrder::Authority).unwrap();
    for ((lat, lon), (x, y)) in [
        ((71.0, -22.0), (72_695.498_968_957_92, 56_985.226_404_903_7)),
        (
            (69.8, -26.5),
            (-96_365.541_611_184_67, -76_118.396_184_135_64),
        ),
    ] {
        let (n, w, _) = t.convert(lat, lon, None).unwrap();
        assert_near(n, y, 1e-6, "northing");
        assert_near(w, -x, 1e-6, "westing");
        let (lat2, lon2, _) = t.inverse().unwrap().convert(n, w, None).unwrap();
        assert_near(lat2, lat, 1e-10, "latitude back");
        assert_near(lon2, lon, 1e-10, "longitude back");
    }
}

/// Transverse Mercator Zoned Grid System: WGS 84 / UTM grid system, the
/// zone's UTM coordinates with the zone number in the millions of the
/// easting. EPSG gives no example; each zone is its UTM CRS.
#[test]
fn transverse_mercator_zoned_grid_system() {
    let north = east_first(4326, 32600);
    let south = east_first(4326, 32700);
    for (lon, lat) in [
        (3.5_f64, 50.0),
        (-179.0, 10.0),
        (179.9, 84.0),
        (-0.1, 0.5),
        (0.0, 0.0),
    ] {
        let zone = ((lon + 180.0) / 6.0).floor().min(59.0) + 1.0;
        for (grid, utm, lat) in [(&north, 32600, lat), (&south, 32700, -lat)] {
            let utm = east_first(4326, utm + zone as u32);
            let (e, n, _) = grid.convert(lon, lat, None).unwrap();
            let (ue, un, _) = utm.convert(lon, lat, None).unwrap();
            assert_near(e, zone * 1e6 + ue, 1e-6, "easting");
            assert_near(n, un, 1e-6, "northing");
            let (lon2, lat2, _) = grid.inverse().unwrap().convert(e, n, None).unwrap();
            assert_near(lon2, lon, 1e-9, "longitude back");
            assert_near(lat2, lat, 1e-9, "latitude back");
        }
    }
    // An easting with no zone in it.
    assert!(
        north
            .inverse()
            .unwrap()
            .convert(500_000.0, 0.0, None)
            .is_err()
    );
}

/// EVRF2019 zero-tide to mean-tide heights (method 1107, which PROJ doesn't
/// import): 100.154 m at 50°55′01.5″N 14°29′13.5″E is 100.147 m.
#[test]
fn evrf2019_zero_tide_to_mean_tide() {
    let compound = |v: u32| Crs::compound(&crs(4258), &crs(v)).unwrap();
    let (zero, mean) = (compound(9389), compound(9390));
    let (lon, lat) = (dms(14.0, 29.0, 13.5), dms(50.0, 55.0, 1.5));
    for t in [
        Transform::via(&zero, &mean, 9710, AxisOrder::EastFirst).unwrap(),
        Transform::new(&zero, &mean, AxisOrder::EastFirst).unwrap(),
    ] {
        assert_eq!(
            t.operations()[0],
            "EVRF2019 height to EVRF2019 mean-tide height (1)"
        );
        let (_, _, h) = t.convert(lon, lat, Some(100.154)).unwrap();
        // EPSG rounds to the millimetre; the formula's value.
        assert_near(h.unwrap(), 100.146_818_784_980_81, 1e-9, "mean-tide height");
        let (_, _, back) = t.inverse().unwrap().convert(lon, lat, h).unwrap();
        assert_near(back.unwrap(), 100.154, 1e-9, "back");
    }
}

/// Time-specific Coordinate Frame rotation (method 1066, which PROJ imports
/// but can't run): EPSG's example, ITRF2008 to PZ-90.11 at 2010.0 by the
/// inverse of "PZ-90.11 to ITRF2008 (1)". Its step 2 is the Helmert alone:
/// EPSG's X and Y (2845455.9772, 2160954.3078) agree, its Z (5265993.2652)
/// is 1.2 mm from what its own parameters give; PROJ's `helmert` with them
/// (`+x=-0.003 +y=-0.001 +rx=0.000019 +ry=-0.000042 +rz=0.000002
/// +convention=coordinate_frame`, inverse) gives what is checked here.
#[test]
fn time_specific_coordinate_frame() {
    let (itrf2008, pz90) = (crs(5332), crs(7679));
    let t = Transform::via(&itrf2008, &pz90, 7960, AxisOrder::Authority).unwrap();
    let input = (2_845_455.975_3, 2_160_954.307_3, 5_265_993.265_6);
    let want = [
        2_845_455.977_206_776,
        2_160_954.307_842_516,
        5_265_993.266_378_452,
    ];
    for got in [
        t.convert(input.0, input.1, Some(input.2)).unwrap(),
        t.convert_at(input.0, input.1, Some(input.2), 2010.0)
            .unwrap(),
    ] {
        for (g, w) in [got.0, got.1, got.2.unwrap()].into_iter().zip(want) {
            assert_near(g, w, 1e-6, "geocentric");
        }
    }
    // At another epoch the point would first have to move by its own
    // velocity, which a coordinate doesn't carry.
    assert_eq!(
        t.convert_at(input.0, input.1, Some(input.2), 2013.9),
        Err(xeibe_reproject::CoordinateError::EpochOutOfRange)
    );
    // In the choice for a coordinate at the epoch only: "WGS 84 to ETRS89
    // (2)" holds at 2014.81 in the German and Dutch North Sea (0.1 m), where
    // otherwise the null "ETRS89 to WGS 84 (1)" (1 m) is PROJ's.
    let t = Transform::new(&crs(4937), &crs(4979), AxisOrder::EastFirst).unwrap();
    let (lon, lat, h) = (6.5, 54.5, Some(30.0));
    let through = |t: &Transform| {
        t.operation_for(lon, lat, h)
            .is_some_and(|o| o.contains("WGS 84 to ETRS89 (2)"))
    };
    assert!(!through(&t));
    assert!(through(&t.with_epochs(None, Some(2014.81)).unwrap()));
    assert!(!through(&t.with_epochs(None, Some(2020.0)).unwrap()));
    // Between ITRF2008 and PZ-90.11 only the time-specific route: a
    // coordinate needs its epoch.
    let pz = Transform::new(&itrf2008, &pz90, AxisOrder::Authority).unwrap();
    assert_eq!(
        pz.convert(input.0, input.1, Some(input.2)),
        Err(xeibe_reproject::CoordinateError::MissingEpoch)
    );
    let got = pz
        .convert_at(input.0, input.1, Some(input.2), 2010.0)
        .unwrap();
    assert_near(got.2.unwrap(), want[2], 1e-6, "chosen at 2010.0");
}

/// An operation between two EPSG CRSs, forced, in their own axis order.
fn forced(src: u32, dst: u32, op: u32) -> Transform {
    Transform::via(&crs(src), &crs(dst), op, AxisOrder::Authority).unwrap()
}

/// Complex polynomial of degree 4: RD to ED50 / UTM zone 31N (1). EPSG's
/// reverse operation (6303, its own coefficients) and the iterated reverse
/// agree to half a millimetre.
#[test]
fn complex_polynomial_of_degree_4() {
    let t = forced(28992, 23031, 1044);
    let (e, n, _) = t.convert(200_000.0, 500_000.0, None).unwrap();
    assert_near(e, 707_155.557, 5e-4, "easting");
    assert_near(n, 5_819_663.128, 5e-4, "northing");
    let (e1, n1, _) = t.inverse().unwrap().convert(e, n, None).unwrap();
    assert_near(e1, 200_000.0, 1e-6, "easting back");
    assert_near(n1, 500_000.0, 1e-6, "northing back");
    let (e2, n2, _) = forced(23031, 28992, 6303).convert(e, n, None).unwrap();
    assert_near(e2, 200_000.0, 5e-4, "6303 easting");
    assert_near(n2, 500_000.0, 5e-4, "6303 northing");
}

/// Complex polynomial of degree 3: BD72 / Belge Lambert 72 to ED50 / UTM
/// zone 31N (1).
#[test]
fn complex_polynomial_of_degree_3() {
    let (e, n, _) = forced(31300, 23031, 1048)
        .convert(200_000.0, 100_000.0, None)
        .unwrap();
    assert_near(e, 647_737.377, 5e-4, "easting");
    assert_near(n, 5_564_124.227, 5e-4, "northing");
}

/// Reversible polynomial of degree 4: ED50 to ED87 (1), forwards, and back
/// with the coefficients' signs reversed.
#[test]
fn reversible_polynomial_of_degree_4() {
    let t = forced(4230, 4231, 15753);
    let lat = dms(52.0, 30.0, 30.0);
    let (lat2, lon2, _) = t.convert(lat, 2.0, None).unwrap();
    assert_near(lat2 - lat, -3.129_58e-6, 5e-12, "latitude difference");
    assert_near(lon2 - 2.0, 9.801_26e-6, 5e-12, "longitude difference");
    // EPSG's reverse gives −3.12957e-06 and +9.80124e-06 degrees back.
    let (lat3, lon3, _) = t.inverse().unwrap().convert(lat2, lon2, None).unwrap();
    assert_near(lat3 - lat2, 3.129_57e-6, 5e-12, "latitude back");
    assert_near(lon3 - lon2, -9.801_24e-6, 5e-12, "longitude back");
}

/// Reversible polynomial of degree 13: ED50 to WGS 84 (35), TR8350.2's
/// regression equations, in arc-seconds. EPSG: "May be approximated to
/// better than 0.5m by transformation code 1998"; over its area (the German
/// North Sea) they agree to 0.6 m.
#[test]
fn reversible_polynomial_of_degree_13() {
    let (mre, helmert) = (forced(4230, 4326, 1052), forced(4230, 4326, 1998));
    for lat in [53.7, 54.3, 55.0, 55.8] {
        for lon in [3.5, 5.0, 6.5, 8.7] {
            let (a, b, _) = mre.convert(lat, lon, None).unwrap();
            let (c, d, _) = helmert.convert(lat, lon, None).unwrap();
            let metres =
                ((a - c) * 111_200.0).hypot((b - d) * 111_200.0 * f64::cos(lat.to_radians()));
            assert!(metres < 0.6, "{lat} {lon}: {metres} m");
        }
    }
}

/// Madrid to ED50 polynomial: the north zone, Madrid 1870 (Madrid) to ED50
/// (2); longitudes from the Madrid meridian in, from Greenwich out.
#[test]
fn madrid_to_ed50_polynomial() {
    let (lat, lon) = (42.647_992, 3.659_603);
    let (lat2, lon2, _) = forced(4903, 4230, 1027).convert(lat, lon, None).unwrap();
    assert_near((lat2 - lat) * 3600.0, 4.05, 0.005, "dLat seconds");
    assert_near((lon2 - lon) * 3600.0, -13_270.54, 0.005, "dLon seconds");
    let (lat3, lon3, _) = forced(4903, 4230, 1027)
        .inverse()
        .unwrap()
        .convert(lat2, lon2, None)
        .unwrap();
    assert_near(lat3, lat, 1e-10, "latitude back");
    assert_near(lon3, lon, 1e-10, "longitude back");
}

/// General polynomials: EPSG gives no example. TM75 to ETRS89 (1) (degree
/// 6, 0.4 m) agrees with the 1 m Helmert "TM75 to ETRS89 (2)" to 1.6 m over
/// Ireland, and Viti Levu Grid to Fiji Map Grid (degree 2, in links to
/// metres, northing first to easting first) with the route through both
/// CRSs' transformations to WGS 84 to 4 m. Their reverse is by iteration.
#[test]
fn general_polynomials() {
    let poly = forced(4300, 4258, 1041);
    let helmert = forced(4300, 4258, 1953);
    for (lat, lon) in [(51.6, -10.0), (53.35, -6.26), (55.2, -7.0), (52.5, -8.5)] {
        let (a, b, _) = poly.convert(lat, lon, None).unwrap();
        let (c, d, _) = helmert.convert(lat, lon, None).unwrap();
        let metres = ((a - c) * 111_200.0).hypot((b - d) * 111_200.0 * f64::cos(lat.to_radians()));
        assert!(metres < 1.6, "TM75 {lat} {lon}: {metres} m");
        let (lat2, lon2, _) = poly.inverse().unwrap().convert(a, b, None).unwrap();
        assert_near(lat2, lat, 1e-10, "TM75 latitude back");
        assert_near(lon2, lon, 1e-10, "TM75 longitude back");
    }
    let fiji = forced(3140, 3460, 15950);
    // Viti Levu 1912 to WGS 84 (2), then the inverse of Fiji 1986 to WGS 84
    // (1): PROJ's route.
    let (n, e) = (814_029.175, 544_000.0);
    let (a, b, _) = fiji.convert(n, e, None).unwrap();
    let (lat, lon, _) = forced(3140, 4326, 6895).convert(n, e, None).unwrap();
    let (c, d, _) = forced(4326, 3460, 15876).convert(lat, lon, None).unwrap();
    assert!((a - c).hypot(b - d) < 4.0, "Fiji: {a} {b} against {c} {d}");
    let (n2, e2, _) = fiji.inverse().unwrap().convert(a, b, None).unwrap();
    assert_near(n2, n, 1e-6, "Fiji northing back");
    assert_near(e2, e, 1e-6, "Fiji easting back");
    // In the choice: OSi's and OSNI's 0.4 m polynomial over the 1 m
    // Helmert PROJ takes (`operation::polynomial::COMPETING`).
    let chosen = Transform::new(&crs(4300), &crs(4258), AxisOrder::Authority).unwrap();
    assert_eq!(chosen.operations()[0], "TM75 to ETRS89 (1)");
}

/// Lambert Conic Conformal (2SP Belgium): BD72 / Belge Lambert 72, with
/// EPSG's turn of the grid by 29.2985″, which PROJ leaves out (it puts the
/// point at 252508.51 E, 153048.64 N, 745 m east).
#[test]
fn lambert_conic_conformal_2sp_belgium() {
    projection_example(
        4313,
        31300,
        dms(5.0, 48.0, 26.533),
        dms(50.0, 40.0, 46.461),
        251_763.20,
        153_034.13,
    );
    // Belgian Lambert 72, the same grid defined without the turn, agrees.
    let (e, n, _) = east_first(4313, 31370)
        .convert(dms(5.0, 48.0, 26.533), dms(50.0, 40.0, 46.461), None)
        .unwrap();
    assert_near(e, 251_763.20, 0.01, "31370 easting");
    assert_near(n, 153_034.13, 0.05, "31370 northing");
}
