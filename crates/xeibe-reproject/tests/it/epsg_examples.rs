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
