//! The geodesic port against PROJ's own tests, `src/tests/geodtest.c`
//! (C. F. F. Karney, MIT), case by case with the same names and
//! tolerances, and against pyproj's `Geod` (`tests/data/geodesic/`).

// The test cases' literals are geodtest.c's, digits and all.
#![allow(clippy::excessive_precision)]

use xeibe_reproject::geodesic::{Flags, Geodesic, Mask, Polygon};

const WGS84_A: f64 = 6378137.0;
const WGS84_F: f64 = 1.0 / 298.257223563;

fn wgs84() -> Geodesic {
    Geodesic::new(WGS84_A, WGS84_F)
}

#[track_caller]
fn eq(x: f64, y: f64, d: f64) {
    assert!((x - y).abs() <= d, "{x} != {y} ± {d}");
}

#[track_caller]
fn nan(x: f64) {
    assert!(x.is_nan(), "{x} is not NaN");
}

const UNROLL: Flags = Flags {
    arc_mode: false,
    long_unroll: true,
};
const ARC_UNROLL: Flags = Flags {
    arc_mode: true,
    long_unroll: true,
};
const NO_FLAGS: Flags = Flags {
    arc_mode: false,
    long_unroll: false,
};

/// lat1, lon1, azi1, lat2, lon2, azi2, s12, a12, m12, M12, M21, S12.
const TESTCASES: [[f64; 12]; 20] = [
    [
        35.60777,
        -139.44815,
        111.098748429560326,
        -11.17491,
        -69.95921,
        129.289270889708762,
        8935244.5604818305,
        80.50729714281974,
        6273170.2055303837,
        0.16606318447386067,
        0.16479116945612937,
        12841384694976.432,
    ],
    [
        55.52454,
        106.05087,
        22.020059880982801,
        77.03196,
        197.18234,
        109.112041110671519,
        4105086.1713924406,
        36.892740690445894,
        3828869.3344387607,
        0.80076349608092607,
        0.80101006984201008,
        61674961290615.615,
    ],
    [
        -21.97856,
        142.59065,
        -32.44456876433189,
        41.84138,
        98.56635,
        -41.84359951440466,
        8394328.894657671,
        75.62930491011522,
        6161154.5773110616,
        0.24816339233950381,
        0.24930251203627892,
        -6637997720646.717,
    ],
    [
        -66.99028,
        112.2363,
        173.73491240878403,
        -12.70631,
        285.90344,
        2.512956620913668,
        11150344.2312080241,
        100.278634181155759,
        6289939.5670446687,
        -0.17199490274700385,
        -0.17722569526345708,
        -121287239862139.744,
    ],
    [
        -17.42761,
        173.34268,
        -159.033557661192928,
        -15.84784,
        5.93557,
        -20.787484651536988,
        16076603.1631180673,
        144.640108810286253,
        3732902.1583877189,
        -0.81273638700070476,
        -0.81299800519154474,
        97825992354058.708,
    ],
    [
        32.84994,
        48.28919,
        150.492927788121982,
        -56.28556,
        202.29132,
        48.113449399816759,
        16727068.9438164461,
        150.565799985466607,
        3147838.1910180939,
        -0.87334918086923126,
        -0.86505036767110637,
        -72445258525585.010,
    ],
    [
        6.96833,
        52.74123,
        92.581585386317712,
        -7.39675,
        206.17291,
        90.721692165923907,
        17102477.2496958388,
        154.147366239113561,
        2772035.6169917581,
        -0.89991282520302447,
        -0.89986892177110739,
        -1311796973197.995,
    ],
    [
        -50.56724,
        -16.30485,
        -105.439679907590164,
        -33.56571,
        -94.97412,
        -47.348547835650331,
        6455670.5118668696,
        58.083719495371259,
        5409150.7979815838,
        0.53053508035997263,
        0.52988722644436602,
        41071447902810.047,
    ],
    [
        -58.93002,
        -8.90775,
        140.965397902500679,
        -8.91104,
        133.13503,
        19.255429433416599,
        11756066.0219864627,
        105.755691241406877,
        6151101.2270708536,
        -0.26548622269867183,
        -0.27068483874510741,
        -86143460552774.735,
    ],
    [
        -68.82867,
        -74.28391,
        93.774347763114881,
        -50.63005,
        -8.36685,
        34.65564085411343,
        3956936.926063544,
        35.572254987389284,
        3708890.9544062657,
        0.81443963736383502,
        0.81420859815358342,
        -41845309450093.787,
    ],
    [
        -10.62672,
        -32.0898,
        -86.426713286747751,
        5.883,
        -134.31681,
        -80.473780971034875,
        11470869.3864563009,
        103.387395634504061,
        6184411.6622659713,
        -0.23138683500430237,
        -0.23155097622286792,
        4198803992123.548,
    ],
    [
        -21.76221,
        166.90563,
        29.319421206936428,
        48.72884,
        213.97627,
        43.508671946410168,
        9098627.3986554915,
        81.963476716121964,
        6299240.9166992283,
        0.13965943368590333,
        0.14152969707656796,
        10024709850277.476,
    ],
    [
        -19.79938,
        -174.47484,
        71.167275780171533,
        -11.99349,
        -154.35109,
        65.589099775199228,
        2319004.8601169389,
        20.896611684802389,
        2267960.8703918325,
        0.93427001867125849,
        0.93424887135032789,
        -3935477535005.785,
    ],
    [
        -11.95887,
        -116.94513,
        92.712619830452549,
        4.57352,
        7.16501,
        78.64960934409585,
        13834722.5801401374,
        124.688684161089762,
        5228093.177931598,
        -0.56879356755666463,
        -0.56918731952397221,
        -9919582785894.853,
    ],
    [
        -87.85331,
        85.66836,
        -65.120313040242748,
        66.48646,
        16.09921,
        -4.888658719272296,
        17286615.3147144645,
        155.58592449699137,
        2635887.4729110181,
        -0.90697975771398578,
        -0.91095608883042767,
        42667211366919.534,
    ],
    [
        1.74708,
        128.32011,
        -101.584843631173858,
        -11.16617,
        11.87109,
        -86.325793296437476,
        12942901.1241347408,
        116.650512484301857,
        5682744.8413270572,
        -0.44857868222697644,
        -0.44824490340007729,
        10763055294345.653,
    ],
    [
        -25.72959,
        -144.90758,
        -153.647468693117198,
        -57.70581,
        -269.17879,
        -48.343983158876487,
        9413446.7452453107,
        84.664533838404295,
        6356176.6898881281,
        0.09492245755254703,
        0.09737058264766572,
        74515122850712.444,
    ],
    [
        -41.22777,
        122.32875,
        14.285113402275739,
        -7.57291,
        130.37946,
        10.805303085187369,
        3812686.035106021,
        34.34330804743883,
        3588703.8812128856,
        0.82605222593217889,
        0.82572158200920196,
        -2456961531057.857,
    ],
    [
        11.01307,
        138.25278,
        79.43682622782374,
        6.62726,
        247.05981,
        103.708090215522657,
        11911190.819018408,
        107.341669954114577,
        6070904.722786735,
        -0.29767608923657404,
        -0.29785143390252321,
        17121631423099.696,
    ],
    [
        -29.47124,
        95.14681,
        -163.779130441688382,
        -27.46601,
        -69.15955,
        -15.909335945554969,
        13487015.8381145492,
        121.294026715742277,
        5481428.9945736388,
        -0.51527225545373252,
        -0.51556587964721788,
        104679964020340.318,
    ],
];

#[test]
fn testinverse() {
    let g = wgs84();
    for c in TESTCASES {
        let r = g.gen_inverse(c[0], c[1], c[3], c[4], Mask::ALL);
        eq(r.azi1, c[2], 1e-13);
        eq(r.azi2, c[5], 1e-13);
        eq(r.s12, c[6], 1e-8);
        eq(r.a12, c[7], 1e-13);
        eq(r.m12, c[8], 1e-8);
        eq(r.M12, c[9], 1e-15);
        eq(r.M21, c[10], 1e-15);
        eq(r.S12, c[11], 0.1);
    }
}

#[test]
fn testdirect() {
    let g = wgs84();
    for c in TESTCASES {
        let p = g.gen_direct(c[0], c[1], c[2], UNROLL, c[6], Mask::ALL);
        eq(p.lat2, c[3], 1e-13);
        eq(p.lon2, c[4], 1e-13);
        eq(p.azi2, c[5], 1e-13);
        eq(p.s12, c[6], 0.0);
        eq(p.a12, c[7], 1e-13);
        eq(p.m12, c[8], 1e-8);
        eq(p.M12, c[9], 1e-15);
        eq(p.M21, c[10], 1e-15);
        eq(p.S12, c[11], 0.1);
    }
}

#[test]
fn testarcdirect() {
    let g = wgs84();
    for c in TESTCASES {
        let p = g.gen_direct(c[0], c[1], c[2], ARC_UNROLL, c[7], Mask::ALL);
        eq(p.lat2, c[3], 1e-13);
        eq(p.lon2, c[4], 1e-13);
        eq(p.azi2, c[5], 1e-13);
        eq(p.s12, c[6], 1e-8);
        eq(p.a12, c[7], 0.0);
        eq(p.m12, c[8], 1e-8);
        eq(p.M12, c[9], 1e-15);
        eq(p.M21, c[10], 1e-15);
        eq(p.S12, c[11], 0.1);
    }
}

#[test]
fn geod_solve_0_1() {
    let g = wgs84();
    let (s12, azi1, azi2) = g.inverse(40.6, -73.8, 49.01666667, 2.55);
    eq(azi1, 53.47022, 0.5e-5);
    eq(azi2, 111.59367, 0.5e-5);
    eq(s12, 5853226.0, 0.5);
    let (lat2, lon2, azi2) = g.direct(40.63972222, -73.77888889, 53.5, 5850e3);
    eq(lat2, 49.01467, 0.5e-5);
    eq(lon2, 2.56106, 0.5e-5);
    eq(azi2, 111.62947, 0.5e-5);
}

/// The antipodal prolate bug found 2010-09-04.
#[test]
fn geod_solve_2() {
    let g = Geodesic::new(6.4e6, -1.0 / 150.0);
    let (s12, azi1, azi2) = g.inverse(0.07476, 0.0, -0.07476, 180.0);
    eq(azi1, 90.00078, 0.5e-5);
    eq(azi2, 90.00078, 0.5e-5);
    eq(s12, 20106193.0, 0.5);
    let (s12, azi1, azi2) = g.inverse(0.1, 0.0, -0.1, 180.0);
    eq(azi1, 90.00105, 0.5e-5);
    eq(azi2, 90.00105, 0.5e-5);
    eq(s12, 20106193.0, 0.5);
}

/// The short line bug found 2010-05-21, and point 2 at the pole
/// (2010-05-03).
#[test]
fn geod_solve_4_5() {
    let g = wgs84();
    eq(
        g.inverse(36.493349428792, 0.0, 36.49334942879201, 0.0000008)
            .0,
        0.072,
        0.5e-3,
    );
    let (lat2, lon2, azi2) = g.direct(0.01777745589997, 30.0, 0.0, 10e6);
    eq(lat2, 90.0, 0.5e-5);
    if lon2 < 0.0 {
        eq(lon2, -150.0, 0.5e-5);
        eq(azi2.abs(), 180.0, 0.5e-5);
    } else {
        eq(lon2, 30.0, 0.5e-5);
        eq(azi2, 0.0, 0.5e-5);
    }
}

/// Nearly antipodal lines that broke with particular compilers.
#[test]
fn geod_solve_6_9_10_11() {
    let g = wgs84();
    let cases = [
        (
            88.202499451857,
            -88.202499451857,
            179.981022032992859592,
            20003898.214,
        ),
        (
            89.262080389218,
            -89.262080389218,
            179.992207982775375662,
            20003925.854,
        ),
        (
            89.333123580033,
            -89.333123580032997687,
            179.99295812360148422,
            20003926.881,
        ),
        (
            56.320923501171,
            -56.320923501171,
            179.664747671772880215,
            19993558.287,
        ),
        (
            52.784459512564,
            -52.784459512563990912,
            179.634407464943777557,
            19991596.095,
        ),
        (
            48.522876735459,
            -48.52287673545898293,
            179.599720456223079643,
            19989144.774,
        ),
    ];
    for (lat1, lat2, lon2, s) in cases {
        eq(g.inverse(lat1, 0.0, lat2, lon2).0, s, 0.5e-3);
    }
}

/// Inverse geodesics on extreme prolate and oblate ellipsoids.
#[test]
fn geod_solve_12() {
    let g = Geodesic::new(89.8, -1.83);
    let (s12, azi1, azi2) = g.inverse(0.0, 0.0, -10.0, 160.0);
    eq(azi1, 120.27, 1e-2);
    eq(azi2, 105.15, 1e-2);
    eq(s12, 266.7, 1e-1);
}

#[test]
fn geod_solve_14_55_94_nan() {
    let g = wgs84();
    for (lat1, lon1, lat2, lon2) in [
        (0.0, 0.0, 1.0, f64::NAN),
        (f64::NAN, 0.0, 0.0, 90.0),
        (f64::NAN, 0.0, 90.0, 9.0),
        (0.0, 0.0, f64::NAN, 90.0),
    ] {
        let (s12, azi1, azi2) = g.inverse(lat1, lon1, lat2, lon2);
        nan(azi1);
        nan(azi2);
        nan(s12);
    }
}

/// Area on a prolate ellipsoid (e² < 0).
#[test]
fn geod_solve_15() {
    let g = Geodesic::new(6.4e6, -1.0 / 150.0);
    eq(
        g.gen_direct(1.0, 2.0, 3.0, NO_FLAGS, 4.0, Mask::AREA).S12,
        23700.0,
        0.5,
    );
}

/// LONG_UNROLL.
#[test]
fn geod_solve_17() {
    let g = wgs84();
    let all = Mask::LATITUDE | Mask::LONGITUDE | Mask::AZIMUTH;
    let p = g.gen_direct(40.0, -75.0, -10.0, UNROLL, 2e7, all);
    eq(p.lat2, -39.0, 1.0);
    eq(p.lon2, -254.0, 1.0);
    eq(p.azi2, -170.0, 1.0);
    let l = g.line(40.0, -75.0, -10.0, Mask::NONE);
    let p = l.gen_position(UNROLL, 2e7, all);
    eq(p.lat2, -39.0, 1.0);
    eq(p.lon2, -254.0, 1.0);
    eq(p.azi2, -170.0, 1.0);
    let (lat2, lon2, azi2) = g.direct(40.0, -75.0, -10.0, 2e7);
    eq(lat2, -39.0, 1.0);
    eq(lon2, 105.0, 1.0);
    eq(azi2, -170.0, 1.0);
    let (lat2, lon2, azi2) = l.position(2e7);
    eq(lat2, -39.0, 1.0);
    eq(lon2, 105.0, 1.0);
    eq(azi2, -170.0, 1.0);
}

/// 0/0 in the area on a sphere, and a12 with |f| > 0.01.
#[test]
fn geod_solve_26_28() {
    let g = Geodesic::new(6.4e6, 0.0);
    eq(
        g.gen_inverse(1.0, 2.0, 3.0, 4.0, Mask::AREA).S12,
        49911046115.0,
        0.5,
    );
    let g = Geodesic::new(6.4e6, 0.1);
    eq(
        g.gen_direct(1.0, 2.0, 10.0, NO_FLAGS, 5e6, Mask::NONE).a12,
        48.55570690,
        0.5e-8,
    );
}

/// max(−0, +0) issues.
#[test]
fn geod_solve_33() {
    let check = |g: &Geodesic, lat2: f64, lon2: f64, a1: f64, a2: f64, s: f64| {
        let (s12, azi1, azi2) = g.inverse(0.0, 0.0, lat2, lon2);
        eq(azi1, a1, 0.5e-5);
        if a2 == 180.0 {
            eq(azi2.abs(), a2, 0.5e-5);
        } else {
            eq(azi2, a2, 0.5e-5);
        }
        eq(s12, s, 0.5);
    };
    let g = wgs84();
    check(&g, 0.0, 179.0, 90.0, 90.0, 19926189.0);
    check(&g, 0.0, 179.5, 55.96650, 124.03350, 19980862.0);
    check(&g, 0.0, 180.0, 0.0, 180.0, 20003931.0);
    check(&g, 1.0, 180.0, 0.0, 180.0, 19893357.0);
    let g = Geodesic::new(6.4e6, 0.0);
    check(&g, 0.0, 179.0, 90.0, 90.0, 19994492.0);
    check(&g, 0.0, 180.0, 0.0, 180.0, 20106193.0);
    check(&g, 1.0, 180.0, 0.0, 180.0, 19994492.0);
    let g = Geodesic::new(6.4e6, -1.0 / 300.0);
    check(&g, 0.0, 179.0, 90.0, 90.0, 19994492.0);
    check(&g, 0.0, 180.0, 90.0, 90.0, 20106193.0);
    check(&g, 0.5, 180.0, 33.02493, 146.97364, 20082617.0);
    check(&g, 1.0, 180.0, 0.0, 180.0, 20027270.0);
}

/// Points close together with longitudes nearly 180° apart.
#[test]
fn geod_solve_59() {
    let (s12, azi1, azi2) = wgs84().inverse(5.0, 0.00000000000001, 10.0, 180.0);
    eq(azi1, 0.000000000000035, 1.5e-14);
    eq(azi2, 179.99999999999996, 1.5e-14);
    eq(s12, 18345191.174332713, 5e-9);
}

/// Small negative azimuths are west-going.
#[test]
fn geod_solve_61() {
    let g = wgs84();
    let all = Mask::LATITUDE | Mask::LONGITUDE | Mask::AZIMUTH;
    let p = g.gen_direct(45.0, 0.0, -0.000000000000000003, UNROLL, 1e7, all);
    eq(p.lat2, 45.30632, 0.5e-5);
    eq(p.lon2, -180.0, 0.5e-5);
    eq(p.azi2.abs(), 180.0, 0.5e-5);
    let l = g.inverse_line(45.0, 0.0, 80.0, -0.000000000000000003, Mask::NONE);
    let p = l.gen_position(UNROLL, 1e7, all);
    eq(p.lat2, 45.30632, 0.5e-5);
    eq(p.lon2, -180.0, 0.5e-5);
    eq(p.azi2.abs(), 180.0, 0.5e-5);
}

/// The east-going check in a line (sign of 0) and the sign of the area.
#[test]
fn geod_solve_65() {
    let g = wgs84();
    let l = g.inverse_line(30.0, -0.000000000000000001, -31.0, 180.0, Mask::ALL);
    let p = l.gen_position(UNROLL, 1e7, Mask::ALL);
    eq(p.lat2, -60.23169, 0.5e-5);
    eq(p.lon2, -0.00000, 0.5e-5);
    eq(p.azi2.abs(), 180.00000, 0.5e-5);
    eq(p.s12, 10000000.0, 0.5);
    eq(p.a12, 90.06544, 0.5e-5);
    eq(p.m12, 6363636.0, 0.5);
    eq(p.M12, -0.0012834, 0.5e-7);
    eq(p.M21, 0.0013749, 0.5e-7);
    eq(p.S12, 0.0, 0.5);
    let p = l.gen_position(UNROLL, 2e7, Mask::ALL);
    eq(p.lat2, -30.03547, 0.5e-5);
    eq(p.lon2, -180.00000, 0.5e-5);
    eq(p.azi2, -0.00000, 0.5e-5);
    eq(p.s12, 20000000.0, 0.5);
    eq(p.a12, 179.96459, 0.5e-5);
    eq(p.m12, 54342.0, 0.5);
    eq(p.M12, -1.0045592, 0.5e-7);
    eq(p.M21, -0.9954339, 0.5e-7);
    eq(p.S12, 127516405431022.0, 0.5);
}

/// An inverse line slightly west of south sets s13; so does a direct line.
#[test]
fn geod_solve_67_71() {
    let g = wgs84();
    let all = Mask::LATITUDE | Mask::LONGITUDE | Mask::AZIMUTH;
    let l = g.inverse_line(-5.0, -0.000000000000002, -10.0, 180.0, Mask::NONE);
    let p = l.gen_position(UNROLL, 2e7, all);
    eq(p.lat2, 4.96445, 0.5e-5);
    eq(p.lon2, -180.00000, 0.5e-5);
    eq(p.azi2, -0.00000, 0.5e-5);
    let p = l.gen_position(UNROLL, 0.5 * l.distance(), all);
    eq(p.lat2, -87.52461, 0.5e-5);
    eq(p.lon2, -0.00000, 0.5e-5);
    eq(p.azi2, -180.00000, 0.5e-5);
    let l = g.direct_line(1.0, 2.0, 45.0, 1e7, Mask::NONE);
    let (lat2, lon2, azi2) = l.position(0.5 * l.distance());
    eq(lat2, 30.92625, 0.5e-5);
    eq(lon2, 37.54640, 0.5e-5);
    eq(azi2, 55.43104, 0.5e-5);
}

/// Backwards from the pole; azi2 is +0, not −0.
#[test]
fn geod_solve_73() {
    let (lat2, lon2, azi2) = wgs84().direct(90.0, 10.0, 180.0, -1e6);
    eq(lat2, 81.04623, 0.5e-5);
    eq(lon2, -170.0, 0.5e-5);
    assert!(azi2 == 0.0 && azi2.is_sign_positive());
}

/// Accurate areas of small polygons.
#[test]
fn geod_solve_74() {
    let r = wgs84().gen_inverse(54.1589, 15.3872, 54.1591, 15.3877, Mask::ALL);
    eq(r.azi1, 55.723110355, 5e-9);
    eq(r.azi2, 55.723515675, 5e-9);
    eq(r.s12, 39.527686385, 5e-9);
    eq(r.a12, 0.000355495, 5e-9);
    eq(r.m12, 39.527686385, 5e-9);
    eq(r.M12, 0.999999995, 5e-9);
    eq(r.M21, 0.999999995, 5e-9);
    eq(r.S12, 286698586.30197, 5e-4);
}

/// Wellington to Salamanca, where Vincenty fails, and a line where the
/// NGS calculator doesn't converge.
#[test]
fn geod_solve_76_78() {
    let g = wgs84();
    let (s12, azi1, azi2) = g.inverse(
        -(41.0 + 19.0 / 60.0),
        174.0 + 49.0 / 60.0,
        40.0 + 58.0 / 60.0,
        -(5.0 + 30.0 / 60.0),
    );
    eq(azi1, 160.39137649664, 0.5e-11);
    eq(azi2, 19.50042925176, 0.5e-11);
    eq(s12, 19960543.857179, 0.5e-6);
    let (s12, azi1, azi2) = g.inverse(27.2, 0.0, -27.1, 179.5);
    eq(azi1, 45.82468716758, 0.5e-11);
    eq(azi2, 134.22776532670, 0.5e-11);
    eq(s12, 19974354.765767, 0.5e-6);
}

/// Scales in special cases, zero-length geodesics and a line that can't
/// take a distance.
#[test]
fn geod_solve_80() {
    let g = wgs84();
    let r = g.gen_inverse(0.0, 0.0, 0.0, 90.0, Mask::GEODESIC_SCALE);
    eq(r.M12, -0.00528427534, 0.5e-10);
    eq(r.M21, -0.00528427534, 0.5e-10);
    let r = g.gen_inverse(0.0, 0.0, 1e-6, 1e-6, Mask::GEODESIC_SCALE);
    eq(r.M12, 1.0, 0.5e-10);
    eq(r.M21, 1.0, 0.5e-10);
    let r = g.gen_inverse(20.001, 0.0, 20.001, 0.0, Mask::ALL);
    eq(r.a12, 0.0, 1e-13);
    eq(r.s12, 0.0, 1e-8);
    eq(r.azi1, 180.0, 1e-13);
    eq(r.azi2, 180.0, 1e-13);
    eq(r.m12, 0.0, 1e-8);
    eq(r.M12, 1.0, 1e-15);
    eq(r.M21, 1.0, 1e-15);
    eq(r.S12, 0.0, 1e-10);
    assert!(r.a12.is_sign_positive() && r.s12.is_sign_positive() && r.m12.is_sign_positive());
    let r = g.gen_inverse(90.0, 0.0, 90.0, 180.0, Mask::ALL);
    eq(r.a12, 0.0, 1e-13);
    eq(r.s12, 0.0, 1e-8);
    eq(r.azi1, 0.0, 1e-13);
    eq(r.azi2, 180.0, 1e-13);
    eq(r.m12, 0.0, 1e-8);
    eq(r.M12, 1.0, 1e-15);
    eq(r.M21, 1.0, 1e-15);
    eq(r.S12, 127516405431022.0, 0.5);
    let l = g.line(1.0, 2.0, 90.0, Mask::LATITUDE);
    nan(l.gen_position(NO_FLAGS, 1000.0, Mask::NONE).a12);
}

/// Infinite and NaN input.
#[test]
fn geod_solve_84() {
    let g = wgs84();
    let inf = f64::INFINITY;
    for (lat1, lon1, azi1, s12) in [
        (0.0, 0.0, 90.0, inf),
        (0.0, 0.0, 90.0, f64::NAN),
        (0.0, 0.0, inf, 1000.0),
        (0.0, 0.0, f64::NAN, 1000.0),
        (inf, 0.0, 90.0, 1000.0),
        (f64::NAN, 0.0, 90.0, 1000.0),
    ] {
        let (lat2, lon2, azi2) = g.direct(lat1, lon1, azi1, s12);
        nan(lat2);
        nan(lon2);
        nan(azi2);
    }
    for lon1 in [inf, f64::NAN] {
        let (lat2, lon2, azi2) = g.direct(0.0, lon1, 90.0, 1000.0);
        assert_eq!(lat2, 0.0);
        nan(lon2);
        assert_eq!(azi2, 90.0);
    }
}

/// Inaccurate hypot, the sign of somg12, sincosd(±45), and a strongly
/// prolate meridian.
#[test]
fn geod_solve_92_96_99_100() {
    let g = wgs84();
    let (s12, azi1, azi2) = g.inverse(37.757540000000006, -122.47018, 37.75754, -122.470177);
    eq(azi1, 89.99999923, 1e-7);
    eq(azi2, 90.00000106, 1e-7);
    eq(s12, 0.264, 0.5e-3);
    let grs80 = Geodesic::new(6378137.0, 1.0 / 298.257222101);
    eq(
        grs80
            .gen_inverse(0.0, 0.0, 60.0832522871723, 89.8492185074635, Mask::AREA)
            .S12,
        42426932221845.0,
        0.5,
    );
    let (s12, azi1, azi2) = g.inverse(45.0, 0.0, -45.0, 179.572719);
    eq(azi1, 90.00000028, 1e-8);
    eq(azi2, 90.00000028, 1e-8);
    eq(s12, 19987083.007, 0.5e-3);
    let g = Geodesic::new(1e6, -3.0);
    let (s12, azi1, azi2) = g.inverse(30.0, 0.0, 30.0, 180.0);
    eq(azi1, 22.368806, 1.0);
    eq(azi2, 157.631194, 1.0);
    eq(s12, 1074081.6, 1e3);
}

fn planimeter(g: &Geodesic, points: &[(f64, f64)]) -> (f64, f64) {
    let mut p = Polygon::new(false);
    for &(lat, lon) in points {
        p.add_point(g, lat, lon);
    }
    let (_, area, perimeter) = p.compute(g, false, true);
    (perimeter, area)
}

fn polylength(g: &Geodesic, points: &[(f64, f64)]) -> f64 {
    let mut p = Polygon::new(true);
    for &(lat, lon) in points {
        p.add_point(g, lat, lon);
    }
    p.compute(g, false, true).2
}

/// Polygons around the poles, across the pole and with tiny longitude
/// offsets.
#[test]
fn planimeter_0_5_6_12_13() {
    let g = wgs84();
    let check = |points: &[(f64, f64)], perimeter: f64, dp: f64, area: f64| {
        let (p, a) = planimeter(&g, points);
        eq(p, perimeter, dp);
        eq(a, area, 1.0);
    };
    check(
        &[(89.0, 0.0), (89.0, 90.0), (89.0, 180.0), (89.0, 270.0)],
        631819.8745,
        1e-4,
        24952305678.0,
    );
    check(
        &[(-89.0, 0.0), (-89.0, 90.0), (-89.0, 180.0), (-89.0, 270.0)],
        631819.8745,
        1e-4,
        -24952305678.0,
    );
    check(
        &[(0.0, -1.0), (-1.0, 0.0), (0.0, 1.0), (1.0, 0.0)],
        627598.2731,
        1e-4,
        24619419146.0,
    );
    let pd = [(90.0, 0.0), (0.0, 0.0), (0.0, 90.0)];
    check(&pd, 30022685.0, 1.0, 63758202715511.0);
    eq(polylength(&g, &pd), 20020719.0, 1.0);
    check(
        &[(89.0, 0.1), (89.0, 90.1), (89.0, -179.9)],
        539297.0,
        1.0,
        12476152838.5,
    );
    for points in [
        [(9.0, -0.00000000000001), (9.0, 180.0), (9.0, 0.0)],
        [(9.0, 0.00000000000001), (9.0, 0.0), (9.0, 180.0)],
        [(9.0, 0.00000000000001), (9.0, 180.0), (9.0, 0.0)],
        [(9.0, -0.00000000000001), (9.0, 0.0), (9.0, 180.0)],
    ] {
        check(&points, 36026861.0, 1.0, 0.0);
    }
    check(
        &[
            (66.562222222, 0.0),
            (66.562222222, 180.0),
            (66.562222222, 360.0),
        ],
        10465729.0,
        1.0,
        0.0,
    );
    check(
        &[
            (66.562222222, -0.0),
            (66.562222222, -180.0),
            (66.562222222, -360.0),
        ],
        10465729.0,
        1.0,
        0.0,
    );
    check(
        &[
            (89.0, -360.0),
            (89.0, -240.0),
            (89.0, -120.0),
            (89.0, 0.0),
            (89.0, 120.0),
            (89.0, 240.0),
        ],
        1160741.0,
        1.0,
        32415230256.0,
    );
}

/// Every combination of `reverse` and `sign`, through test_point,
/// test_edge, compute and polygon_area.
#[test]
fn planimeter_15() {
    let g = wgs84();
    let (lat, lon) = ([2.0, 1.0, 3.0], [1.0, 2.0, 3.0]);
    let r = 18454562325.45119;
    let a0 = 510065621724088.5093;
    let mut p = Polygon::new(false);
    p.add_point(&g, lat[0], lon[0]);
    p.add_point(&g, lat[1], lon[1]);
    let expected = [
        ((false, true), r),
        ((false, false), r),
        ((true, true), -r),
        ((true, false), a0 - r),
    ];
    for ((reverse, sign), area) in expected {
        eq(p.test_point(&g, lat[2], lon[2], reverse, sign).1, area, 0.5);
    }
    let (s12, azi1, _) = g.inverse(lat[1], lon[1], lat[2], lon[2]);
    for ((reverse, sign), area) in expected {
        eq(p.test_edge(&g, azi1, s12, reverse, sign).1, area, 0.5);
    }
    p.add_point(&g, lat[2], lon[2]);
    for ((reverse, sign), area) in expected {
        eq(p.compute(&g, reverse, sign).1, area, 0.5);
    }
    eq(g.polygon_area(&lat, &lon).0, r, 0.5);
}

/// Degenerate polygons and polylines.
#[test]
fn planimeter_19() {
    let g = wgs84();
    let mut p = Polygon::new(false);
    let (_, area, perim) = p.compute(&g, false, true);
    assert!(area == 0.0 && perim == 0.0);
    let (_, area, perim) = p.test_point(&g, 1.0, 1.0, false, true);
    assert!(area == 0.0 && perim == 0.0);
    let (_, area, perim) = p.test_edge(&g, 90.0, 1000.0, false, true);
    nan(area);
    nan(perim);
    p.add_point(&g, 1.0, 1.0);
    let (_, area, perim) = p.compute(&g, false, true);
    assert!(area == 0.0 && perim == 0.0);
    let mut p = Polygon::new(true);
    assert_eq!(p.compute(&g, false, true).2, 0.0);
    assert_eq!(p.test_point(&g, 1.0, 1.0, false, true).2, 0.0);
    nan(p.test_edge(&g, 90.0, 1000.0, false, true).2);
    p.add_point(&g, 1.0, 1.0);
    assert_eq!(p.compute(&g, false, true).2, 0.0);
    p.add_point(&g, 1.0, 1.0);
    eq(p.test_edge(&g, 90.0, 1000.0, false, true).2, 1000.0, 1e-10);
    eq(
        p.test_point(&g, 2.0, 2.0, false, true).2,
        156876.149,
        0.5e-3,
    );
}

/// Circling the pole several times.
#[test]
fn planimeter_21() {
    let g = wgs84();
    let lat = 45.0;
    let a = 39.2144607176828184218;
    let s = 8420705.40957178156285;
    let r = 39433884866571.4277;
    let a0 = 510065621724088.5093;
    let mut p = Polygon::new(false);
    for lon in [60.0, 180.0, -60.0, 60.0, 180.0, -60.0] {
        p.add_point(&g, lat, lon);
    }
    for i in [3.0, 4.0] {
        p.add_point(&g, lat, 60.0);
        p.add_point(&g, lat, 180.0);
        let expected = [
            ((false, true), i * r),
            ((false, false), i * r),
            ((true, true), -i * r),
            ((true, false), -i * r + a0),
        ];
        for ((reverse, sign), area) in expected {
            eq(p.test_point(&g, lat, -60.0, reverse, sign).1, area, 0.5);
            eq(p.test_edge(&g, a, s, reverse, sign).1, area, 0.5);
        }
        p.add_point(&g, lat, -60.0);
        for ((reverse, sign), area) in expected {
            eq(p.compute(&g, reverse, sign).1, area, 0.5);
        }
    }
}

/// transit_direct and transit agree on zero.
#[test]
fn planimeter_29() {
    let g = wgs84();
    let mut p = Polygon::new(false);
    p.add_point(&g, 0.0, 0.0);
    p.add_edge(&g, 90.0, 1000.0);
    p.add_edge(&g, 0.0, 1000.0);
    p.add_edge(&g, -90.0, 1000.0);
    eq(p.compute(&g, false, true).1, 1000000.0, 0.01);
}

/// The distance between two f64s in units in the last place, with ±0
/// apart by one and NaN equal only to NaN.
fn ulps(x: f64, y: f64) -> u64 {
    if x.is_nan() || y.is_nan() {
        return if x.is_nan() && y.is_nan() {
            0
        } else {
            u64::MAX
        };
    }
    let key = |v: f64| {
        let b = v.to_bits() as i64;
        if b < 0 { i64::MIN - b } else { b }
    };
    key(x).abs_diff(key(y))
}

/// Ours against pyproj's for every record of a `gen_geodesic_vectors.py`
/// file: the number of values compared, and those more than `tolerance`
/// ulps apart.
fn compare_with_pyproj(path: &std::path::Path, tolerance: u64) -> (usize, Vec<String>) {
    use serde_json::Value;
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (mut compared, mut off) = (0, Vec::new());
    let mut check = |what: &str, r: &Value, ours: &[f64], theirs: &[f64]| {
        assert_eq!(ours.len(), theirs.len(), "{what} {r}");
        for (i, (&o, &t)) in ours.iter().zip(theirs).enumerate() {
            compared += 1;
            let u = ulps(o, t);
            if u > tolerance {
                off.push(format!(
                    "{what} {} [{i}]: {o:e} vs {t:e} ({u} ulps)",
                    r["in"]
                ));
            }
        }
    };
    let floats = |v: &Value| -> Vec<f64> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap())
            .collect()
    };
    for line in text.lines().skip(1) {
        let r: Value = serde_json::from_str(line).unwrap();
        let g = Geodesic::new(r["a"].as_f64().unwrap(), r["f"].as_f64().unwrap());
        let kind = r["kind"].as_str().unwrap();
        match kind {
            "inv" => {
                let i = floats(&r["in"]);
                let (s12, azi1, azi2) = g.inverse(i[0], i[1], i[2], i[3]);
                check(kind, &r, &[s12, azi1, azi2], &floats(&r["out"]));
            }
            "fwd" => {
                let i = floats(&r["in"]);
                let (lat2, lon2, azi2) = g.direct(i[0], i[1], i[2], i[3]);
                check(kind, &r, &[lat2, lon2, azi2], &floats(&r["out"]));
            }
            "inv_line" | "fwd_line" => {
                let i = floats(&r["in"]);
                let npts = i[4] as usize;
                let (line, del_s, dist) = if kind == "inv_line" {
                    let line = g.inverse_line(i[0], i[1], i[2], i[3], Mask::NONE);
                    let del_s = line.distance() / (npts as f64 - 1.0);
                    let dist = line.distance();
                    (line, del_s, dist)
                } else {
                    (
                        g.line(i[0], i[1], i[2], Mask::NONE),
                        i[3],
                        i[3] * (npts as f64 - 1.0),
                    )
                };
                let (mut lats, mut lons, mut azis) = (vec![], vec![], vec![]);
                for k in 0..npts {
                    let (lat, lon, azi) = line.position(k as f64 * del_s);
                    lats.push(lat);
                    lons.push(lon);
                    azis.push(azi);
                }
                let out = &r["out"];
                check(kind, &r, &lats, &floats(&out["lats"]));
                check(kind, &r, &lons, &floats(&out["lons"]));
                check(kind, &r, &azis, &floats(&out["azis"]));
                check(
                    kind,
                    &r,
                    &[del_s, dist],
                    &[
                        out["del_s"].as_f64().unwrap(),
                        out["dist"].as_f64().unwrap(),
                    ],
                );
            }
            "polygon" => {
                let (area, perimeter) = g.polygon_area(&floats(&r["lats"]), &floats(&r["lons"]));
                check(kind, &r, &[area, perimeter], &floats(&r["out"]));
            }
            other => panic!("unknown kind {other}"),
        }
    }
    (compared, off)
}

/// pyproj's vectors come from glibc's libm, so elsewhere sin, cos and
/// atan2 may round differently.
const LIBM_ULPS: u64 = if cfg!(all(target_os = "linux", target_env = "gnu")) {
    0
} else {
    4
};

/// Bit for bit on Linux, where the committed vectors were made.
#[test]
fn geodesics_match_pyproj() {
    let (n, off) = compare_with_pyproj(
        &xeibe_testkit::data_dir().join("geodesic/geodesic.jsonl"),
        LIBM_ULPS,
    );
    assert!(n > 3000, "{n} values");
    assert!(
        off.is_empty(),
        "{} of {n} differ:\n{}",
        off.len(),
        off[..off.len().min(20)].join("\n")
    );
}

/// Within 4 ulps: Rust's `cbrt` is `compiler_builtins`' (musl's), not
/// glibc's, and differs by an ulp now and then, which moves a nearly
/// antipodal line's starting guess (`Astroid`) and so its last bits; 21
/// of the 441,495 values on Linux.
#[test]
#[ignore = "needs target/geodesic/ (scripts/gen_geodesic_vectors.py --all)"]
fn every_geodesic_matches_pyproj() {
    let path = xeibe_testkit::data_dir().join("../../target/geodesic/geodesic.jsonl");
    let (n, off) = compare_with_pyproj(&path, 4);
    assert!(
        off.is_empty(),
        "{} of {n} differ:\n{}",
        off.len(),
        off[..off.len().min(20)].join("\n")
    );
}
