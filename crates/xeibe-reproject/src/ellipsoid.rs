//! Reference ellipsoids and the quantities the projection formulas derive from them.

/// An ellipsoid of revolution, or a sphere when `f == 0`. Lengths in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ellipsoid {
    /// Semi-major axis.
    pub a: f64,
    /// Semi-minor axis.
    pub b: f64,
    /// Flattening, (a − b) / a.
    pub f: f64,
    /// First eccentricity squared, e² = f (2 − f). PROJ's `es`.
    pub es: f64,
    /// First eccentricity, e.
    pub e: f64,
    /// Second eccentricity squared, e′² = e² / (1 − e²). PROJ's `e2s`.
    pub e2s: f64,
    /// Third flattening, n = (a − b) / (a + b).
    pub n: f64,
}

impl Ellipsoid {
    /// From the semi-major axis and the inverse flattening. An inverse
    /// flattening of 0 means a sphere, as in EPSG.
    pub fn from_inverse_flattening(a: f64, rf: f64) -> Self {
        let f = if rf == 0.0 { 0.0 } else { 1.0 / rf };
        Self::from_flattening(a, f)
    }

    /// From both semi-axes.
    pub fn from_semi_axes(a: f64, b: f64) -> Self {
        Self::from_flattening(a, (a - b) / a)
    }

    /// A sphere of the given radius.
    pub fn sphere(radius: f64) -> Self {
        Self::from_flattening(radius, 0.0)
    }

    fn from_flattening(a: f64, f: f64) -> Self {
        let es = f * (2.0 - f);
        Ellipsoid {
            a,
            b: a * (1.0 - f),
            f,
            es,
            e: es.sqrt(),
            e2s: es / (1.0 - es),
            n: f / (2.0 - f),
        }
    }

    pub fn is_sphere(&self) -> bool {
        self.es == 0.0
    }
}
