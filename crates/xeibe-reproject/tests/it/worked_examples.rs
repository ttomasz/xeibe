//! EPSG's worked examples for the methods xeibe runs that PROJ runs too:
//! an oracle independent of PROJ, which the vectors elsewhere follow. The
//! examples are EPSG Guidance Note 7-2 (IOGP Report 373-07-2, July 2026
//! revision, © IOGP), most of which the dataset also quotes in each
//! method's example; its section is named on each. The methods PROJ can't
//! run are in `epsg_examples.rs`.
//!
//! A projection's example is checked with [`Projection`], which takes
//! longitude from Greenwich and latitude in degrees, as GN7-2 states them,
//! and gives the easting and northing in the CRS's unit (in the CRS's own
//! axes for the south-orientated methods). The result must round to
//! GN7-2's figure, and GN7-2's easting and northing must come back to its
//! point.

use std::path::{Path, PathBuf};

use xeibe_reproject::{AxisOrder, Crs, Projection, Transform};

fn dms(d: f64, m: f64, s: f64) -> f64 {
    d.signum() * (d.abs() + m / 60.0 + s / 3600.0)
}

/// A hypothetical CRS GN7-2 defines for its example only, in WKT2 with
/// EPSG's method and parameter ids.
fn wkt(text: &str) -> Crs {
    Crs::from_wkt(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

struct Example {
    /// GN7-2's section and the CRS.
    what: &'static str,
    crs: Crs,
    lon: f64,
    lat: f64,
    /// As GN7-2 gives them, in the projection's output order.
    x: f64,
    y: f64,
    /// Half a unit of GN7-2's last decimal, or, where GN7-2's figure is off
    /// (`note`), how far.
    tolerance: f64,
    /// Where GN7-2's figure is off, by how much and how that is known:
    /// xeibe agrees with PROJ and with the formula evaluated exactly.
    note: &'static str,
}

fn epsg(code: u32) -> Crs {
    Crs::from_epsg(code).unwrap()
}

const WGS84_ELLIPSOID: &str = r#"ELLIPSOID["WGS 84",6378137,298.257223563,LENGTHUNIT["metre",1]]"#;

fn projected(
    name: &str,
    ellipsoid: &str,
    method: &str,
    method_code: u32,
    params: &[(&str, u32, f64, &str)],
) -> Crs {
    let params: String = params
        .iter()
        .map(|(name, code, value, unit)| {
            let unit = match *unit {
                "deg" => r#"ANGLEUNIT["degree",0.0174532925199433]"#,
                "m" => r#"LENGTHUNIT["metre",1]"#,
                _ => r#"SCALEUNIT["unity",1]"#,
            };
            format!(r#",PARAMETER["{name}",{value},{unit},ID["EPSG",{code}]]"#)
        })
        .collect();
    wkt(&format!(
        r#"PROJCRS["{name}",BASEGEOGCRS["{name} base",DATUM["{name} datum",{ellipsoid}],PRIMEM["Greenwich",0]],CONVERSION["{name}",METHOD["{method}",ID["EPSG",{method_code}]]{params}],CS[Cartesian,2],AXIS["easting (E)",east],AXIS["northing (N)",north],LENGTHUNIT["metre",1]]"#
    ))
}

fn examples() -> Vec<Example> {
    let ex = |what, crs, lon, lat, x, y, tolerance| Example {
        what,
        crs,
        lon,
        lat,
        x,
        y,
        tolerance,
        note: "",
    };
    let off = |e: Example, tolerance, note| Example {
        tolerance,
        note,
        ..e
    };
    vec![
        ex(
            "§3.2.1.2 Lambert Conic Conformal (2SP): NAD27 / Texas South Central (ftUS)",
            epsg(32040),
            dms(-96.0, 0.0, 0.0),
            dms(28.0, 30.0, 0.0),
            2_963_503.91,
            254_759.80,
            0.005,
        ),
        ex(
            "§3.2.1.2 Lambert Conic Conformal (2SP): AGD66 / Vicgrid66",
            epsg(3110),
            dms(144.0, 45.0, 0.0),
            dms(-37.0, 45.0, 0.0),
            2_477_968.963,
            4_416_742.535,
            0.0005,
        ),
        ex(
            "§3.2.1.1 Lambert Conic Conformal (1SP): JAD69 / Jamaica National Grid",
            epsg(24200),
            dms(-76.0, 56.0, 37.26),
            dms(17.0, 55.0, 55.80),
            255_966.58,
            142_493.51,
            0.005,
        ),
        ex(
            "§3.2.1.3 Lambert Conic Conformal (1SP variant B): Lyon-Turin Ferroviaire 2004",
            epsg(9549),
            7.0,
            47.0,
            163_958.366,
            252_043.307,
            0.0005,
        ),
        ex(
            "§3.2.1.5 Lambert Conic Conformal (2SP Michigan): NAD27 / Michigan Central (ftUS)",
            epsg(6201),
            dms(-83.0, 10.0, 0.0),
            dms(43.0, 45.0, 0.0),
            2_308_335.75,
            160_210.48,
            0.005,
        ),
        off(
            ex(
                "§3.4.2.1 Krovak: S-JTSK (Ferro) / Krovak, southing and westing",
                epsg(2065),
                dms(16.0, 50.0, 59.179),
                dms(50.0, 12.0, 32.442),
                1_050_538.64,
                568_991.00,
                0.005,
            ),
            0.013,
            "GN7-2 prints 1050538.643 and 568990.997 here, but 1050538.631 and 568990.995 for the same point in §3.4.2.3, as xeibe and PROJ give",
        ),
        off(
            ex(
                "§3.4.2.2 Krovak (North Orientated): S-JTSK (Ferro) / Krovak East North",
                epsg(5221),
                dms(16.0, 50.0, 59.179),
                dms(50.0, 12.0, 32.442),
                -568_991.00,
                -1_050_538.64,
                0.005,
            ),
            0.013,
            "as §3.4.2.1",
        ),
        ex(
            "§3.4.2.3 Krovak Modified: S-JTSK/05 (Ferro) / Modified Krovak",
            epsg(5224),
            dms(16.0, 50.0, 59.179),
            dms(50.0, 12.0, 32.442),
            6_050_538.71,
            5_568_990.91,
            0.005,
        ),
        ex(
            "§3.4.2.4 Krovak Modified (North Orientated): S-JTSK/05 (Ferro) / Modified Krovak East North",
            epsg(5225),
            dms(16.0, 50.0, 59.179),
            dms(50.0, 12.0, 32.442),
            -5_568_990.91,
            -6_050_538.71,
            0.005,
        ),
        ex(
            "§3.4.4 Albers Equal Area: NAD83 / Great Lakes Albers",
            epsg(3174),
            dms(-78.0, 45.0, 0.0),
            dms(42.0, 45.0, 0.0),
            1_466_493.492,
            702_903.006,
            0.0005,
        ),
        off(
            ex(
                "§3.4.4 Albers Equal Area: GN7-2's South American example",
                projected(
                    "Albers example",
                    r#"ELLIPSOID["GRS 1967 Modified",6378160,298.25,LENGTHUNIT["metre",1]]"#,
                    "Albers Equal Area",
                    9822,
                    &[
                        ("Latitude of false origin", 8821, -32.0, "deg"),
                        ("Longitude of false origin", 8822, -60.0, "deg"),
                        ("Latitude of 1st standard parallel", 8823, -5.0, "deg"),
                        ("Latitude of 2nd standard parallel", 8824, -42.0, "deg"),
                        ("Easting at false origin", 8826, 0.0, "m"),
                        ("Northing at false origin", 8827, 0.0, "m"),
                    ],
                ),
                dms(-46.0, 0.0, 1.538),
                dms(-18.0, 30.0, 2.016),
                1_408_623.196,
                1_507_641.482,
                0.0005,
            ),
            0.007,
            "GN7-2's figures are 3 and 6 mm from the closed-form formula, which gives 1408623.1932 and 1507641.4883, as xeibe and PROJ",
        ),
        ex(
            "§3.4.5 Equidistant Conic: GN7-2's example on Clarke 1866",
            projected(
                "Equidistant Conic example",
                r#"ELLIPSOID["Clarke 1866",6378206.4,294.978698213898,LENGTHUNIT["metre",1]]"#,
                "Equidistant Conic",
                1119,
                &[
                    ("Latitude of false origin", 8821, 23.0, "deg"),
                    ("Longitude of false origin", 8822, -96.0, "deg"),
                    ("Latitude of 1st standard parallel", 8823, 29.5, "deg"),
                    ("Latitude of 2nd standard parallel", 8824, 45.5, "deg"),
                    ("Easting at false origin", 8826, 0.0, "m"),
                    ("Northing at false origin", 8827, 0.0, "m"),
                ],
            ),
            -75.0,
            35.0,
            1_885_051.86,
            1_540_507.64,
            0.005,
        ),
        ex(
            "§3.5.1.1 Mercator (variant A): Makassar / NEIEZ (GN7-2 prints its code as 30020)",
            epsg(3002),
            120.0,
            -3.0,
            5_009_726.58,
            569_150.82,
            0.005,
        ),
        ex(
            "§3.5.1.1 Mercator (variant B): Pulkovo 1942 / Caspian Sea Mercator",
            epsg(3388),
            53.0,
            53.0,
            165_704.29,
            5_171_848.07,
            0.005,
        ),
        ex(
            "§3.5.1.1 Mercator (Spherical): GN7-2's example on a sphere of 6371007 m",
            projected(
                "Spherical Mercator example",
                r#"ELLIPSOID["Sphere",6371007,0,LENGTHUNIT["metre",1]]"#,
                "Mercator (Spherical)",
                1026,
                &[
                    ("Latitude of natural origin", 8801, 0.0, "deg"),
                    ("Longitude of natural origin", 8802, 0.0, "deg"),
                    ("False easting", 8806, 0.0, "m"),
                    ("False northing", 8807, 0.0, "m"),
                ],
            ),
            -100.0 - 20.0 / 60.0,
            dms(24.0, 22.0, 54.433),
            -11_156_569.90,
            2_796_869.94,
            0.005,
        ),
        ex(
            "§3.5.1.2 Popular Visualisation Pseudo-Mercator: WGS 84 / Pseudo-Mercator",
            epsg(3857),
            -100.0 - 20.0 / 60.0,
            dms(24.0, 22.0, 54.433),
            -11_169_055.58,
            2_800_000.00,
            0.005,
        ),
        ex(
            "§3.5.2 Cassini-Soldner: Trinidad 1903 / Trinidad Grid (Clarke's links)",
            epsg(30200),
            -62.0,
            10.0,
            66_644.94,
            82_536.22,
            0.005,
        ),
        ex(
            "§3.5.2 Hyperbolic Cassini-Soldner: Vanua Levu 1915 / Vanua Levu Grid, in links \
             (GN7-2's 16015.2890 and 13369.6601 chains; EPSG:3139 is in Clarke's links)",
            epsg(3139),
            dms(179.0, 59.0, 39.6115),
            dms(-16.0, 50.0, 29.2435),
            1_601_528.90,
            1_336_966.01,
            0.005,
        ),
        off(
            ex(
                "§3.5.3 Transverse Mercator: OSGB36 / British National Grid",
                epsg(27700),
                0.5,
                50.5,
                577_274.99,
                69_740.50,
                0.005,
            ),
            0.008,
            "Krüger's series evaluated exactly gives 577274.9838 and 69740.4923, as xeibe and PROJ; GN7-2's are 6 and 8 mm off, through its constants rounded to 8 to 10 digits",
        ),
        ex(
            "§3.5.3.2 Transverse Mercator 3D: LUREF / Luxembourg TM (3D)",
            epsg(9895),
            dms(5.0, 56.0, 16.1460),
            dms(49.0, 34.0, 0.7381),
            63_444.834,
            70_387.372,
            0.0005,
        ),
        off(
            ex(
                "§3.5.3.3 Transverse Mercator (South Orientated): Hartebeesthoek94 / Lo29, westing and southing",
                epsg(2053),
                dms(28.0, 16.0, 57.479),
                dms(-25.0, 43.0, 55.302),
                71_984.49,
                2_847_342.74,
                0.005,
            ),
            0.011,
            "Krüger's series evaluated exactly gives 71984.4797 and 2847342.7352, as xeibe and PROJ; GN7-2's are 10 and 5 mm off",
        ),
        ex(
            "§3.5.4.2 Hotine Oblique Mercator (variant B): Timbalai 1948 / R.S.O. Borneo (m)",
            epsg(29873),
            dms(115.0, 48.0, 19.8196),
            dms(5.0, 23.0, 14.1129),
            679_245.73,
            596_562.78,
            0.005,
        ),
        ex(
            "§3.5.4.3 Laborde Oblique Mercator: Tananarive (Paris) / Laborde Grid",
            epsg(29701),
            dms(44.0, 27.0, 27.260),
            dms(-16.0, 11.0, 23.280),
            188_333.848,
            1_098_841.091,
            0.0005,
        ),
        ex(
            "§3.5.5 Equidistant Cylindrical: WGS 84 / World Equidistant Cylindrical",
            epsg(4087),
            10.0,
            55.0,
            1_113_194.91,
            6_097_230.31,
            0.005,
        ),
        ex(
            "§3.6.1 Oblique Stereographic: Amersfoort / RD New",
            epsg(28992),
            6.0,
            53.0,
            196_105.283,
            557_057.739,
            0.0005,
        ),
        ex(
            "§3.6.2 Polar Stereographic (variant A): WGS 84 / UPS North",
            epsg(5041),
            44.0,
            73.0,
            3_320_416.75,
            632_668.43,
            0.005,
        ),
        ex(
            "§3.6.2 Polar Stereographic (variant B): WGS 84 / Australian Antarctic Polar Stereographic",
            epsg(3032),
            120.0,
            -75.0,
            7_255_380.79,
            7_053_389.56,
            0.005,
        ),
        ex(
            "§3.6.3.1 Lambert Azimuthal Equal Area: ETRS89-extended / ETRS-LAEA",
            epsg(3035),
            5.0,
            50.0,
            3_962_799.45,
            2_999_718.85,
            0.005,
        ),
        ex(
            "§3.6.3.2 Modified Azimuthal Equidistant: Guam 1963 / Yap Islands",
            epsg(3295),
            dms(138.0, 11.0, 34.908),
            dms(9.0, 35.0, 47.493),
            42_665.90,
            65_509.82,
            0.005,
        ),
        ex(
            "§3.6.3.2 Guam Projection: Guam 1963 / Guam SPCS",
            epsg(3993),
            dms(144.0, 38.0, 7.19265),
            dms(13.0, 20.0, 20.53846),
            37_712.48,
            35_242.00,
            0.005,
        ),
        ex(
            "§3.6.3.3 Azimuthal Equidistant: WGS 84 / Equi7 Europe",
            epsg(27704),
            44.0,
            63.0,
            6_840_895.297,
            3_382_726.731,
            0.0005,
        ),
        ex(
            "§3.6.5 Orthographic: GN7-2's example on WGS 84",
            projected(
                "Orthographic example",
                WGS84_ELLIPSOID,
                "Orthographic",
                9840,
                &[
                    ("Latitude of natural origin", 8801, 55.0, "deg"),
                    ("Longitude of natural origin", 8802, 5.0, "deg"),
                    ("False easting", 8806, 0.0, "m"),
                    ("False northing", 8807, 0.0, "m"),
                ],
            ),
            dms(2.0, 7.0, 46.38),
            dms(53.0, 48.0, 33.82),
            -189_011.711,
            -128_640.567,
            0.0005,
        ),
        off(
            ex(
                "§3.6.6 Local Orthographic: GN7-2's example on GRS 1980",
                projected(
                    "Local Orthographic example",
                    r#"ELLIPSOID["GRS 1980",6378137,298.257222101,LENGTHUNIT["metre",1]]"#,
                    "Local Orthographic",
                    1130,
                    &[
                        (
                            "Latitude of projection centre",
                            8811,
                            dms(37.0, 37.0, 44.289),
                            "deg",
                        ),
                        (
                            "Longitude of projection centre",
                            8812,
                            dms(-122.0, 23.0, 38.190),
                            "deg",
                        ),
                        (
                            "Azimuth at projection centre",
                            8813,
                            dms(27.0, 47.0, 34.0),
                            "deg",
                        ),
                        (
                            "Scale factor at projection centre",
                            8815,
                            0.9999968,
                            "unity",
                        ),
                        ("Easting at projection centre", 8816, 0.0, "m"),
                        ("Northing at projection centre", 8817, 0.0, "m"),
                    ],
                ),
                dms(-122.0, 23.0, 4.700),
                dms(37.0, 37.0, 33.877),
                876.136,
                98.974,
                0.0005,
            ),
            0.001,
            "GN7-2 truncates 876.1368 to 876.136",
        ),
        ex(
            "§3.7.1 Colombia Urban: MAGNA-SIRGAS / Bogota urban grid",
            epsg(6247),
            dms(-74.0, 15.0, 0.0),
            dms(4.0, 48.0, 0.0),
            80_859.033,
            122_543.174,
            0.0005,
        ),
        off(
            ex(
                "§3.7.2 Equal Earth: WGS 84 / Equal Earth Americas",
                epsg(8858),
                dms(-117.0, 11.0, 48.349),
                dms(34.0, 3.0, 27.169),
                -2_390_749.042,
                4_242_849.758,
                0.0005,
            ),
            0.001,
            "GN7-2 truncates: the exact authalic latitude gives -2390749.0428 and 4242849.7576, as xeibe and PROJ",
        ),
    ]
}

#[test]
fn projections() {
    let mut failures = Vec::new();
    for e in examples() {
        let p = Projection::new(&e.crs).unwrap();
        let (x, y, _) = p.convert(e.lon, e.lat, None).unwrap();
        let (dx, dy) = (x - e.x, y - e.y);
        if dx.abs() > e.tolerance + 1e-9 || dy.abs() > e.tolerance + 1e-9 {
            failures.push(format!(
                "{}: forward ({x:.4}, {y:.4}), GN7-2 ({}, {}), off by ({dx:.4}, {dy:.4}) {}",
                e.what, e.x, e.y, e.note
            ));
        }
        // GN7-2's rounded easting and northing come back to its point, to
        // what their rounding moves it (a few millimetres in degrees).
        let (lon, lat, _) = p.inverse().convert(e.x, e.y, None).unwrap();
        let (dlon, dlat) = ((lon - e.lon) * 3600.0, (lat - e.lat) * 3600.0);
        if dlon.abs() > 0.001 || dlat.abs() > 0.001 {
            failures.push(format!(
                "{}: back ({lon:.9}, {lat:.9}), off by ({dlon:.5}\", {dlat:.5}\")",
                e.what
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

// Transformations: GN7-2's examples through EPSG's operation, forced, in
// the CRSs' own axis order.

fn forced(src: &Crs, dst: &Crs, op: u32) -> Transform {
    Transform::via(src, dst, op, AxisOrder::Authority).unwrap()
}

fn assert_within(got: f64, want: f64, tolerance: f64, what: &str) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: {got}, GN7-2 {want}"
    );
}

/// Seconds of arc, for angles GN7-2 gives to a decimal of a second.
fn arcsec(degrees: f64) -> f64 {
    degrees * 3600.0
}

/// §4.1.1 Geographic/geocentric conversions: WGS 84, 53°48′33.820″N
/// 2°07′46.380″E, 73 m.
#[test]
fn geographic_to_geocentric() {
    let t = Transform::new(&epsg(4979), &epsg(4978), AxisOrder::Authority).unwrap();
    let (lat, lon) = (dms(53.0, 48.0, 33.820), dms(2.0, 7.0, 46.380));
    let (x, y, z) = t.convert(lat, lon, Some(73.0)).unwrap();
    assert_within(x, 3_771_793.968, 0.0005, "X");
    assert_within(y, 140_253.342, 0.0005, "Y");
    assert_within(z.unwrap(), 5_124_304.349, 0.0005, "Z");
    let (lat2, lon2, h) = t.inverse().unwrap().convert(x, y, z).unwrap();
    assert_within(arcsec(lat2 - lat), 0.0, 1e-6, "latitude back");
    assert_within(arcsec(lon2 - lon), 0.0, 1e-6, "longitude back");
    assert_within(h.unwrap(), 73.0, 1e-6, "height back");
}

/// §4.3.3.1 Position Vector transformation (geocentric domain), with the
/// parameters of WGS 72 to WGS 84 (2). GN7-2's X is 6 mm off: M (XS − rZ
/// YS) = 1.000000219 × (3657660.66 − 0.68696) = 3657660.774, as xeibe and
/// PROJ give; it prints 3657660.78.
#[test]
fn position_vector() {
    let (x, y, z) = forced(&epsg(4984), &epsg(4978), 1238)
        .convert(3_657_660.66, 255_768.55, Some(5_201_382.11))
        .unwrap();
    assert_within(x, 3_657_660.78, 0.006, "X");
    assert_within(y, 255_778.43, 0.005, "Y");
    assert_within(z.unwrap(), 5_201_387.75, 0.005, "Z");
}

/// §4.3.3.3 Coordinate Frame rotation full matrix: Saba to BES2020 Saba
/// (1), whose rotations (up to 479″) are too large for the approximation.
#[test]
fn coordinate_frame_full_matrix() {
    let t = forced(&epsg(10634), &epsg(10637), 10646);
    let (x, y, z) = t
        .convert(2_738_467.761, -5_429_579.811, Some(1_919_164.067))
        .unwrap();
    assert_within(x, 2_738_386.686, 0.0005, "X");
    assert_within(y, -5_429_268.990, 0.0005, "Y");
    assert_within(z.unwrap(), 1_919_210.970, 0.0005, "Z");
    let (x2, y2, z2) = t.inverse().unwrap().convert(x, y, z).unwrap();
    assert_within(x2, 2_738_467.761, 1e-6, "X back");
    assert_within(y2, -5_429_579.811, 1e-6, "Y back");
    assert_within(z2.unwrap(), 1_919_164.067, 1e-6, "Z back");
}

/// §4.3.4.2 Time-dependent Coordinate Frame rotation: ITRF2008 to GDA94
/// (1) at 2013.90. (GN7-2 prints δrZ's product with 1.5551; the parameter
/// is 1.1551, as its rZ′ of 25.3880 msec needs.)
#[test]
fn time_dependent_coordinate_frame() {
    let (x, y, z) = forced(&epsg(5332), &epsg(4938), 6276)
        .convert_at(-3_789_470.710, 4_841_770.404, Some(-1_690_893.952), 2013.90)
        .unwrap();
    assert_within(x, -3_789_470.004, 0.0005, "X");
    assert_within(y, 4_841_770.686, 0.0005, "Y");
    assert_within(z.unwrap(), -1_690_895.108, 0.0005, "Z");
}

/// §4.3.5 Molodensky-Badekas (CF geog2D domain): La Canoa to REGVEN (1),
/// for 9°35′00.386″N 66°04′48.091″W at an ellipsoidal height of 201.46 m.
/// GN7-2 works it in 3D, to REGVEN's geocentric (2550138.46, −5749799.87,
/// 1054530.82), which PROJ's `molobadekas` reproduces to the centimetre.
/// EPSG's La Canoa is geographic 2D, though, and an operation between
/// geographic 2D CRSs carries a height through unchanged, in PROJ (`push
/// v_3`) as here, so EPSG:4247 to 4962 lands 21 m from it. The horizontal
/// position is the example's: GN7-2's geocentric result, as REGVEN
/// latitude and longitude, is where the 2D transformation puts the point,
/// to what 201 m of height changes in the shift (under 1 cm).
#[test]
fn molodensky_badekas() {
    let (lat, lon) = (dms(9.0, 35.0, 0.386), -dms(66.0, 4.0, 48.091));
    let (lat2, lon2, _) = forced(&epsg(4247), &epsg(4189), 1771)
        .convert(lat, lon, None)
        .unwrap();
    let regven = Transform::new(&epsg(4962), &epsg(4189), AxisOrder::Authority).unwrap();
    let (lat3, lon3, _) = regven
        .convert(2_550_138.46, -5_749_799.87, Some(1_054_530.82))
        .unwrap();
    // 1 cm is 0.0003″.
    assert_within(arcsec(lat2), arcsec(lat3), 0.0003, "latitude");
    assert_within(arcsec(lon2), arcsec(lon3), 0.0003, "longitude");
}

/// §4.5.2 Longitude rotation: MGI (Ferro) to MGI (1), 30°E of Ferro is
/// 12°20′E of Greenwich.
#[test]
fn longitude_rotation() {
    let t = forced(&epsg(4805), &epsg(4312), 3895);
    let (lat, lon, _) = t.convert(47.0, 30.0, None).unwrap();
    assert_within(lat, 47.0, 1e-12, "latitude");
    assert_within(lon, dms(12.0, 20.0, 0.0), 1e-12, "longitude");
}

/// §4.5.3 Geographic2D offsets: Greek to GGRS87 (1), −5.86″ and +0.28″.
#[test]
fn geographic2d_offsets() {
    let t = forced(&epsg(4120), &epsg(4121), 1891);
    let (lat, lon) = (dms(38.0, 8.0, 36.565), dms(23.0, 48.0, 16.235));
    let (lat2, lon2, _) = t.convert(lat, lon, None).unwrap();
    assert_within(
        arcsec(lat2),
        arcsec(dms(38.0, 8.0, 30.705)),
        1e-6,
        "latitude",
    );
    assert_within(
        arcsec(lon2),
        arcsec(dms(23.0, 48.0, 16.515)),
        1e-6,
        "longitude",
    );
    let (lat3, lon3, _) = t.inverse().unwrap().convert(lat2, lon2, None).unwrap();
    assert_within(arcsec(lat3), arcsec(lat), 1e-6, "latitude back");
    assert_within(arcsec(lon3), arcsec(lon), 1e-6, "longitude back");
}

/// §4.8.1 Affine Parametric Transformation: Jamaica 1875 / Jamaica (Old
/// Grid), in feet, to JAD69 / Jamaica National Grid.
#[test]
fn affine_parametric() {
    let t = forced(&epsg(24100), &epsg(24200), 10087);
    let (e, n, _) = t.convert(553_900.0, 482_500.0, None).unwrap();
    assert_within(e, 251_190.497, 0.0005, "easting");
    assert_within(n, 175_146.067, 0.0005, "northing");
    let (e2, n2, _) = t.inverse().unwrap().convert(e, n, None).unwrap();
    assert_within(e2, 553_900.0, 1e-6, "easting back");
    assert_within(n2, 482_500.0, 1e-6, "northing back");
}

/// §4.8.3 Similarity Transformation: ED50 / UTM zone 31N to ETRS89 / UTM
/// zone 31N (1).
#[test]
fn similarity() {
    let t = forced(&epsg(23031), &epsg(25831), 5166);
    let (e, n, _) = t.convert(300_000.0, 4_500_000.0, None).unwrap();
    assert_within(e, 299_905.060, 0.0005, "easting");
    assert_within(n, 4_499_796.515, 0.0005, "northing");
    let (e2, n2, _) = t
        .inverse()
        .unwrap()
        .convert(299_905.060, 4_499_796.515, None)
        .unwrap();
    assert_within(e2, 300_000.0, 0.0005, "easting back");
    assert_within(n2, 4_500_000.0, 0.0005, "northing back");
}

fn compound(horizontal: u32, vertical: u32) -> Crs {
    Crs::compound(&epsg(horizontal), &epsg(vertical)).unwrap()
}

/// §4.11.1 Vertical Offset: Baltic 1977 height to Black Sea height (1),
/// +0.4 m.
#[test]
fn vertical_offset() {
    let t = forced(&compound(4284, 5705), &compound(4284, 5735), 5447);
    let (_, _, h) = t.convert(46.0, 30.0, Some(2.55)).unwrap();
    assert_within(h.unwrap(), 2.95, 1e-9, "Black Sea height");
    let (_, _, back) = t.inverse().unwrap().convert(46.0, 30.0, h).unwrap();
    assert_within(back.unwrap(), 2.55, 1e-9, "Baltic height back");
}

/// §4.11.5 Vertical Offset and Slope: LN02 height to EVRF2000 height (1)
/// at 47°20′N 9°40′E: 473.0 m is 472.69 m.
#[test]
fn vertical_offset_and_slope() {
    let t = forced(&compound(4258, 5728), &compound(4258, 5730), 5431);
    let (lat, lon) = (dms(47.0, 20.0, 0.0), dms(9.0, 40.0, 0.0));
    let (_, _, h) = t.convert(lat, lon, Some(473.0)).unwrap();
    assert_within(h.unwrap(), 472.69, 0.005, "EVRF2000 height");
    let (_, _, back) = t
        .inverse()
        .unwrap()
        .convert(lat, lon, Some(472.69))
        .unwrap();
    assert_within(back.unwrap(), 473.00, 0.005, "LN02 height back");
}

/// The cropped grids committed with the level 3 vectors.
fn cropped() -> PathBuf {
    xeibe_testkit::data_dir().join("reprojection/grids/cropped")
}

fn with_grids(t: Transform, dir: &Path) -> Transform {
    t.load_grids_from_dir(dir, None)
        .unwrap_or_else(|e| panic!("{:?}: {e}", t.operations()));
    t
}

/// §4.5.5 Geographic3D offsets by NEU velocity grid: NAD83(CSRS)v6 at
/// 2010.0 to NAD83(CSRS)v3 at 1997.0 with Canada's velocity grid v7
/// (NAD83(CSRS)v6 to NAD83(CSRS)v3 (1), deprecated since, the method's
/// example).
#[test]
fn geographic3d_offsets_by_velocity_grid() {
    // The cropped grid holds only a window: load what the point needs.
    let (lat, lon) = (dms(49.0, 53.0, 9.2927), -dms(99.0, 54.0, 41.0572));
    let t = forced(&epsg(8251), &epsg(8239), 10122);
    t.load_grids_from_dir(
        cropped(),
        Some([lat - 0.01, lon - 0.01, lat + 0.01, lon + 0.01]),
    )
    .unwrap();
    let (lat, lon, h) = t.convert(lat, lon, Some(373.795)).unwrap();
    assert_within(
        arcsec(lat),
        arcsec(dms(49.0, 53.0, 9.2931)),
        0.000_05,
        "latitude",
    );
    assert_within(
        arcsec(lon),
        -arcsec(dms(99.0, 54.0, 41.0588)),
        0.000_05,
        "longitude",
    );
    assert_within(h.unwrap(), 373.819, 0.0005, "height");
}

/// PROJ-data's grids, whole: `target/proj-grids` (`scripts/fetch_proj_grids.py`)
/// or `XEIBE_GRID_DIR`.
fn proj_grids() -> PathBuf {
    let dir = match std::env::var_os("XEIBE_GRID_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/proj-grids"),
    };
    assert!(
        dir.is_dir(),
        "{}: run scripts/fetch_proj_grids.py",
        dir.display()
    );
    dir
}

/// §4.4.2 Geocentric translations by grid (IGN): NTF to RGF93 v1 (1),
/// `gr3df97a`, 48°50′40.2441″N 2°25′32.4187″E to 48°50′40.0050″N
/// 2°25′29.8960″E.
#[test]
#[ignore = "needs PROJ-data's grids in target/proj-grids"]
fn geocentric_translations_by_grid() {
    let t = with_grids(forced(&epsg(4275), &epsg(4171), 9327), &proj_grids());
    let (lat, lon, _) = t
        .convert(dms(48.0, 50.0, 40.2441), dms(2.0, 25.0, 32.4187), None)
        .unwrap();
    assert_within(
        arcsec(lat),
        arcsec(dms(48.0, 50.0, 40.0050)),
        0.000_05,
        "latitude",
    );
    assert_within(
        arcsec(lon),
        arcsec(dms(2.0, 25.0, 29.8960)),
        0.000_05,
        "longitude",
    );
}

/// §4.5.6 Geographic2D Offsets by TIN Interpolation: ETRS89 to NGO 1948
/// (2), Norway's three test points.
#[test]
#[ignore = "needs PROJ-data's grids in target/proj-grids"]
fn geographic2d_offsets_by_tin() {
    let t = with_grids(forced(&epsg(4258), &epsg(4273), 10854), &proj_grids());
    for (from, to) in [
        (
            (dms(60.0, 0.0, 0.0), dms(10.0, 0.0, 0.0)),
            (dms(59.0, 59.0, 57.2912), dms(10.0, 0.0, 17.1796)),
        ),
        (
            (dms(60.0, 37.0, 0.0), dms(4.0, 51.0, 0.0)),
            (dms(60.0, 36.0, 58.0042), dms(4.0, 51.0, 15.8944)),
        ),
        (
            (dms(70.0, 40.0, 50.0), dms(23.0, 40.0, 0.0)),
            (dms(70.0, 40.0, 49.3770), dms(23.0, 40.0, 29.8224)),
        ),
    ] {
        let (lat, lon, _) = t.convert(from.0, from.1, None).unwrap();
        assert_within(arcsec(lat), arcsec(to.0), 0.000_05, "latitude");
        assert_within(arcsec(lon), arcsec(to.1), 0.000_05, "longitude");
    }
}

/// §4.6.2 Cartesian Grid Offsets by TIN Interpolation: YKJ to ETRS89-FIN /
/// TM35FIN(E,N) (1); YKJ's axes are northing first.
#[test]
#[ignore = "needs PROJ-data's grids in target/proj-grids"]
fn cartesian_grid_offsets_by_tin() {
    let t = with_grids(forced(&epsg(2393), &epsg(3067), 10703), &proj_grids());
    let (e, n, _) = t.convert(6_652_430.684, 3_284_859.82, None).unwrap();
    assert_within(e, 284_777.842, 0.0005, "easting");
    assert_within(n, 6_649_637.325, 0.0005, "northing");
}

/// §4.11.4 Vertical Offset by TIN Interpolation: N60 height to N2000
/// height (1), 101.000 m at 61°N 26°E is 101.236 m.
#[test]
#[ignore = "needs PROJ-data's grids in target/proj-grids"]
fn vertical_offset_by_tin() {
    // The TIN is on YKJ's grid (its JSON's `input_crs`), so the point is
    // given there: KKJ's 61°N 26°E, in Finland Uniform Coordinate System.
    // GN7-2 doesn't say on which datum its 61°N 26°E is; PROJ's route from
    // ETRS89 + N60 puts that point ~100 m away and gives 101.2364 m, this
    // one 101.2365 m: within a millimetre of GN7-2 either way.
    let (e, n, _) = Projection::new(&epsg(2393))
        .unwrap()
        .convert(26.0, 61.0, None)
        .unwrap();
    let t = with_grids(
        forced(&compound(2393, 5717), &compound(2393, 3900), 10705),
        &proj_grids(),
    );
    let (_, _, h) = t.convert(n, e, Some(101.0)).unwrap();
    assert_within(h.unwrap(), 101.236, 0.001, "N2000 height");
}

/// §4.11.2 Vertical Offset by Grid Interpolation: NZVD2016 height to
/// Dunedin 1958 height (2), 50.000 m at 44.42°S 168.92°E is 50.304 m.
#[test]
#[ignore = "needs PROJ-data's grids in target/proj-grids"]
fn vertical_offset_by_grid() {
    let t = with_grids(
        forced(&compound(4167, 7839), &compound(4167, 5761), 9314),
        &proj_grids(),
    );
    let (_, _, h) = t.convert(-44.42, 168.92, Some(50.0)).unwrap();
    // GN7-2 rounds the offset to the millimetre: its own corner values, to
    // 0.1 mm, interpolate to 0.3043 m; PROJ-data's grid to 0.3046 m.
    assert_within(h.unwrap(), 50.304, 0.001, "Dunedin 1958 height");
}

/// §4.2.3 Point motion by NEU velocity grid: Canada's velocity grid v7
/// (EPSG:9483), Centennial Monument NCC100 from 2010.0 to 2002.0. (GN7-2
/// gives it in NAD83(CSRS)v6 with v7's grid; the motion that grid belongs
/// to is NAD83(CSRS)v7's.) GN7-2's velocities (VN −1.56, VE +1.77, VU
/// +2.02 mm/yr) aren't the grid's bilinearly, as PROJ and NRCan's tools
/// interpolate it (−1.93, +1.95, +2.09; "Velocity grids are interpolated
/// bilinearly" in `docs/reprojection.md`): over 8 years the point lands 3
/// mm from GN7-2's (0.0001″), where PROJ puts it.
#[test]
#[ignore = "needs PROJ-data's grids in target/proj-grids"]
fn point_motion_by_velocity_grid() {
    let crs = epsg(8254);
    let t = Transform::new(&crs, &crs, AxisOrder::Authority)
        .unwrap()
        .with_epochs(Some(2010.0), Some(2002.0))
        .unwrap();
    let t = with_grids(t, &proj_grids());
    let (lat, lon, h) = t
        .convert(
            dms(45.0, 25.0, 45.714920),
            -dms(75.0, 42.0, 5.960075),
            Some(39.524),
        )
        .unwrap();
    assert_within(
        arcsec(lat),
        arcsec(dms(45.0, 25.0, 45.715324)),
        0.000_1,
        "latitude",
    );
    assert_within(
        arcsec(lon),
        -arcsec(dms(75.0, 42.0, 5.960726)),
        0.000_1,
        "longitude",
    );
    assert_within(h.unwrap(), 39.508, 0.001, "height");
}

/// §3.6.4.2 Vertical Perspective, on EPSG's parameters, which have no
/// false easting or northing (nor has the WKT PROJ writes for `nsper`).
/// Like PROJ's `nsper` xeibe projects on the sphere of the semi-major
/// axis, the heights unused, where EPSG's method is ellipsoidal and uses
/// them: GN7-2's example (−188878.767, −128550.090) is 410 m and 18 m from
/// PROJ's result, which this checks. No EPSG CRS uses the method.
#[test]
fn vertical_perspective_on_epsg_parameters() {
    let crs = wkt(
        r#"PROJCRS["Vertical Perspective example",BASEGEOGCRS["WGS 84",DATUM["World Geodetic System 1984",ELLIPSOID["WGS 84",6378137,298.257223563,LENGTHUNIT["metre",1]]],PRIMEM["Greenwich",0]],CONVERSION["Vertical Perspective example",METHOD["Vertical Perspective",ID["EPSG",9838]],PARAMETER["Latitude of topocentric origin",55,ANGLEUNIT["degree",0.0174532925199433],ID["EPSG",8834]],PARAMETER["Longitude of topocentric origin",5,ANGLEUNIT["degree",0.0174532925199433],ID["EPSG",8835]],PARAMETER["Ellipsoidal height of topocentric origin",200,LENGTHUNIT["metre",1],ID["EPSG",8836]],PARAMETER["Viewpoint height",5900000,LENGTHUNIT["metre",1],ID["EPSG",8840]]],CS[Cartesian,2],AXIS["easting (E)",east],AXIS["northing (N)",north],LENGTHUNIT["metre",1]]"#,
    );
    let p = Projection::new(&crs).unwrap();
    let (x, y, _) = p
        .convert(dms(2.0, 7.0, 46.38), dms(53.0, 48.0, 33.82), None)
        .unwrap();
    // pyproj 3.8.0 (PROJ 9.8.1) on the same WKT.
    assert_within(x, -188_468.619_172, 1e-5, "easting");
    assert_within(y, -128_568.271_067, 1e-5, "northing");
}
