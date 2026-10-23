//! PROJ's built-in tables of named ellipsoids (`+ellps=`, pyproj's
//! `Geod(ellps=…)` and `get_ellps_map`) and prime meridians (`+pm=`,
//! `get_prime_meridians_map`), copied from `src/ellps.cpp` and
//! `src/datums.cpp` in PROJ 9.8.1 (MIT, see PROJ-NOTICE.md), and its unit
//! short names and own units (`+units=`, `get_units_map`), from
//! `data/sql/customizations.sql`; values and names unchanged.

/// The second parameter of a named ellipsoid, as PROJ gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// `rf=`: the inverse flattening.
    InverseFlattening(f64),
    /// `b=`: the semi-minor axis, in metres.
    SemiMinor(f64),
}

/// One of PROJ's named ellipsoids.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NamedEllipsoid {
    /// PROJ's name for it, as in `+ellps=GRS80`.
    pub id: &'static str,
    /// The semi-major axis, in metres.
    pub a: f64,
    pub shape: Shape,
    /// PROJ's description, e.g. `GRS 1980(IUGG, 1980)`.
    pub description: &'static str,
}

impl NamedEllipsoid {
    /// The flattening.
    pub fn f(&self) -> f64 {
        match self.shape {
            Shape::InverseFlattening(rf) => 1.0 / rf,
            Shape::SemiMinor(b) => (self.a - b) / self.a,
        }
    }
}

/// PROJ's named ellipsoids, in PROJ's order.
pub static ELLIPSOIDS: &[NamedEllipsoid] = &[
    NamedEllipsoid {
        id: "MERIT",
        a: 6378137.0,
        shape: Shape::InverseFlattening(298.257),
        description: "MERIT 1983",
    },
    NamedEllipsoid {
        id: "SGS85",
        a: 6378136.0,
        shape: Shape::InverseFlattening(298.257),
        description: "Soviet Geodetic System 85",
    },
    NamedEllipsoid {
        id: "GRS80",
        a: 6378137.0,
        shape: Shape::InverseFlattening(298.257222101),
        description: "GRS 1980(IUGG, 1980)",
    },
    NamedEllipsoid {
        id: "IAU76",
        a: 6378140.0,
        shape: Shape::InverseFlattening(298.257),
        description: "IAU 1976",
    },
    NamedEllipsoid {
        id: "airy",
        a: 6377563.396,
        shape: Shape::InverseFlattening(299.3249646),
        description: "Airy 1830",
    },
    NamedEllipsoid {
        id: "APL4.9",
        a: 6378137.0,
        shape: Shape::InverseFlattening(298.25),
        description: "Appl. Physics. 1965",
    },
    NamedEllipsoid {
        id: "NWL9D",
        a: 6378145.0,
        shape: Shape::InverseFlattening(298.25),
        description: "Naval Weapons Lab., 1965",
    },
    NamedEllipsoid {
        id: "mod_airy",
        a: 6377340.189,
        shape: Shape::SemiMinor(6356034.446),
        description: "Modified Airy",
    },
    NamedEllipsoid {
        id: "andrae",
        a: 6377104.43,
        shape: Shape::InverseFlattening(300.0),
        description: "Andrae 1876 (Den., Iclnd.)",
    },
    NamedEllipsoid {
        id: "danish",
        a: 6377019.2563,
        shape: Shape::InverseFlattening(300.0),
        description: "Andrae 1876 (Denmark, Iceland)",
    },
    NamedEllipsoid {
        id: "aust_SA",
        a: 6378160.0,
        shape: Shape::InverseFlattening(298.25),
        description: "Australian Natl & S. Amer. 1969",
    },
    NamedEllipsoid {
        id: "GRS67",
        a: 6378160.0,
        shape: Shape::InverseFlattening(298.2471674270),
        description: "GRS 67(IUGG 1967)",
    },
    NamedEllipsoid {
        id: "GSK2011",
        a: 6378136.5,
        shape: Shape::InverseFlattening(298.2564151),
        description: "GSK-2011",
    },
    NamedEllipsoid {
        id: "bessel",
        a: 6377397.155,
        shape: Shape::InverseFlattening(299.1528128),
        description: "Bessel 1841",
    },
    NamedEllipsoid {
        id: "bess_nam",
        a: 6377483.865,
        shape: Shape::InverseFlattening(299.1528128),
        description: "Bessel 1841 (Namibia)",
    },
    NamedEllipsoid {
        id: "clrk66",
        a: 6378206.4,
        shape: Shape::SemiMinor(6356583.8),
        description: "Clarke 1866",
    },
    NamedEllipsoid {
        id: "clrk80",
        a: 6378249.145,
        shape: Shape::InverseFlattening(293.4663),
        description: "Clarke 1880 mod.",
    },
    NamedEllipsoid {
        id: "clrk80ign",
        a: 6378249.2,
        shape: Shape::InverseFlattening(293.4660212936269),
        description: "Clarke 1880 (IGN).",
    },
    NamedEllipsoid {
        id: "CPM",
        a: 6375738.7,
        shape: Shape::InverseFlattening(334.29),
        description: "Comm. des Poids et Mesures 1799",
    },
    NamedEllipsoid {
        id: "delmbr",
        a: 6376428.0,
        shape: Shape::InverseFlattening(311.5),
        description: "Delambre 1810 (Belgium)",
    },
    NamedEllipsoid {
        id: "engelis",
        a: 6378136.05,
        shape: Shape::InverseFlattening(298.2566),
        description: "Engelis 1985",
    },
    NamedEllipsoid {
        id: "evrst30",
        a: 6377276.345,
        shape: Shape::InverseFlattening(300.8017),
        description: "Everest 1830",
    },
    NamedEllipsoid {
        id: "evrst48",
        a: 6377304.063,
        shape: Shape::InverseFlattening(300.8017),
        description: "Everest 1948",
    },
    NamedEllipsoid {
        id: "evrst56",
        a: 6377301.243,
        shape: Shape::InverseFlattening(300.8017),
        description: "Everest 1956",
    },
    NamedEllipsoid {
        id: "evrst69",
        a: 6377295.664,
        shape: Shape::InverseFlattening(300.8017),
        description: "Everest 1969",
    },
    NamedEllipsoid {
        id: "evrstSS",
        a: 6377298.556,
        shape: Shape::InverseFlattening(300.8017),
        description: "Everest (Sabah & Sarawak)",
    },
    NamedEllipsoid {
        id: "fschr60",
        a: 6378166.0,
        shape: Shape::InverseFlattening(298.3),
        description: "Fischer (Mercury Datum) 1960",
    },
    NamedEllipsoid {
        id: "fschr60m",
        a: 6378155.0,
        shape: Shape::InverseFlattening(298.3),
        description: "Modified Fischer 1960",
    },
    NamedEllipsoid {
        id: "fschr68",
        a: 6378150.0,
        shape: Shape::InverseFlattening(298.3),
        description: "Fischer 1968",
    },
    NamedEllipsoid {
        id: "helmert",
        a: 6378200.0,
        shape: Shape::InverseFlattening(298.3),
        description: "Helmert 1906",
    },
    NamedEllipsoid {
        id: "hough",
        a: 6378270.0,
        shape: Shape::InverseFlattening(297.0),
        description: "Hough",
    },
    NamedEllipsoid {
        id: "intl",
        a: 6378388.0,
        shape: Shape::InverseFlattening(297.0),
        description: "International 1924 (Hayford 1909, 1910)",
    },
    NamedEllipsoid {
        id: "krass",
        a: 6378245.0,
        shape: Shape::InverseFlattening(298.3),
        description: "Krassovsky, 1942",
    },
    NamedEllipsoid {
        id: "kaula",
        a: 6378163.0,
        shape: Shape::InverseFlattening(298.24),
        description: "Kaula 1961",
    },
    NamedEllipsoid {
        id: "lerch",
        a: 6378139.0,
        shape: Shape::InverseFlattening(298.257),
        description: "Lerch 1979",
    },
    NamedEllipsoid {
        id: "mprts",
        a: 6397300.0,
        shape: Shape::InverseFlattening(191.0),
        description: "Maupertius 1738",
    },
    NamedEllipsoid {
        id: "new_intl",
        a: 6378157.5,
        shape: Shape::SemiMinor(6356772.2),
        description: "New International 1967",
    },
    NamedEllipsoid {
        id: "plessis",
        a: 6376523.0,
        shape: Shape::SemiMinor(6355863.0),
        description: "Plessis 1817 (France)",
    },
    NamedEllipsoid {
        id: "PZ90",
        a: 6378136.0,
        shape: Shape::InverseFlattening(298.25784),
        description: "PZ-90",
    },
    NamedEllipsoid {
        id: "SEasia",
        a: 6378155.0,
        shape: Shape::SemiMinor(6356773.3205),
        description: "Southeast Asia",
    },
    NamedEllipsoid {
        id: "walbeck",
        a: 6376896.0,
        shape: Shape::SemiMinor(6355834.8467),
        description: "Walbeck",
    },
    NamedEllipsoid {
        id: "WGS60",
        a: 6378165.0,
        shape: Shape::InverseFlattening(298.3),
        description: "WGS 60",
    },
    NamedEllipsoid {
        id: "WGS66",
        a: 6378145.0,
        shape: Shape::InverseFlattening(298.25),
        description: "WGS 66",
    },
    NamedEllipsoid {
        id: "WGS72",
        a: 6378135.0,
        shape: Shape::InverseFlattening(298.26),
        description: "WGS 72",
    },
    NamedEllipsoid {
        id: "WGS84",
        a: 6378137.0,
        shape: Shape::InverseFlattening(298.257223563),
        description: "WGS 84",
    },
    NamedEllipsoid {
        id: "sphere",
        a: 6370997.0,
        shape: Shape::SemiMinor(6370997.0),
        description: "Normal Sphere (r=6370997)",
    },
];

/// The named ellipsoid with this id (case-sensitive, as in PROJ).
pub fn ellipsoid(id: &str) -> Option<&'static NamedEllipsoid> {
    ELLIPSOIDS.iter().find(|e| e.id == id)
}

/// PROJ's named prime meridians, as (id, longitude from Greenwich in
/// PROJ's degrees-minutes-seconds notation), in PROJ's order. PROJ notes
/// that the list is no longer updated and that some values differ from
/// EPSG's (Madrid, Copenhagen).
pub static PRIME_MERIDIANS: &[(&str, &str)] = &[
    ("greenwich", "0dE"),
    ("lisbon", "9d07'54.862\"W"),
    ("paris", "2d20'14.025\"E"),
    ("bogota", "74d04'51.3\"W"),
    ("madrid", "3d41'16.58\"W"),
    ("rome", "12d27'8.4\"E"),
    ("bern", "7d26'22.5\"E"),
    ("jakarta", "106d48'27.79\"E"),
    ("ferro", "17d40'W"),
    ("brussels", "4d22'4.71\"E"),
    ("stockholm", "18d3'29.8\"E"),
    ("athens", "23d42'58.815\"E"),
    ("oslo", "10d43'22.5\"E"),
    ("copenhagen", "12d34'40.35\"E"),
];

/// PROJ's short names of EPSG's units of measure (`+units=us-ft`), from
/// PROJ's `data/sql/customizations.sql`, by EPSG code.
pub static UNIT_SHORT_NAMES: &[(u32, &str)] = &[
    (1025, "mm"),
    (1033, "cm"),
    (9001, "m"),
    (9002, "ft"),
    (9003, "us-ft"),
    (9014, "fath"),
    (9030, "kmi"),
    (9033, "us-ch"),
    (9035, "us-mi"),
    (9036, "km"),
    (9081, "ind-ft"),
    (9085, "ind-yd"),
    (9093, "mi"),
    (9096, "yd"),
    (9097, "ch"),
    (9098, "link"),
    (9101, "rad"),
    (9102, "deg"),
    (9105, "grad"),
];

/// PROJ's short name of an EPSG unit, e.g. `"us-ft"` for 9003.
pub fn unit_short_name(code: u32) -> Option<&'static str> {
    UNIT_SHORT_NAMES
        .binary_search_by_key(&code, |(c, _)| *c)
        .ok()
        .map(|i| UNIT_SHORT_NAMES[i].1)
}

/// A length unit PROJ defines itself (authority `PROJ`), from the same file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjUnit {
    /// PROJ's code, e.g. `"US_IN"`.
    pub code: &'static str,
    pub name: &'static str,
    /// The unit in metres.
    pub factor: f64,
    /// The short name, e.g. `"us-in"`.
    pub short_name: &'static str,
}

/// PROJ's own length units, by code.
pub static PROJ_UNITS: &[ProjUnit] = &[
    ProjUnit {
        code: "DM",
        name: "decimeter",
        factor: 0.01,
        short_name: "dm",
    },
    ProjUnit {
        code: "IN",
        name: "inch",
        factor: 0.0254,
        short_name: "in",
    },
    ProjUnit {
        code: "IND_CH",
        name: "Indian chain",
        factor: 20.11669506,
        short_name: "ind-ch",
    },
    ProjUnit {
        code: "US_IN",
        name: "US survey inch",
        factor: 0.025400050800101,
        short_name: "us-in",
    },
    ProjUnit {
        code: "US_YD",
        name: "US survey yard",
        factor: 0.914401828803658,
        short_name: "us-yd",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        assert_eq!(ELLIPSOIDS.len(), 46);
        let wgs84 = ellipsoid("WGS84").unwrap();
        assert_eq!((wgs84.a, wgs84.f()), (6378137.0, 1.0 / 298.257223563));
        assert_eq!(
            ellipsoid("clrk66").unwrap().shape,
            Shape::SemiMinor(6356583.8)
        );
        assert!(ellipsoid("wgs84").is_none());
        assert_eq!(ellipsoid("sphere").unwrap().f(), 0.0);
        assert_eq!(PRIME_MERIDIANS.len(), 14);
        assert_eq!(unit_short_name(9003), Some("us-ft"));
        assert_eq!(unit_short_name(9122), None);
        assert!(UNIT_SHORT_NAMES.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(PROJ_UNITS.len(), 5);
    }
}
