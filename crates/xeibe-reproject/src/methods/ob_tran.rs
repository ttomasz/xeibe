//! General Oblique Transformation (`ob_tran`): another projection (its
//! `o_proj`) of the sphere turned to a new pole. With `o_proj=longlat` it
//! is a rotated pole, a derived geographic CRS whose output is the turned
//! longitude and latitude, in radians.
//!
//! Ported from PROJ 9.8.1 `src/projections/ob_tran.cpp` (MIT, see
//! PROJ-NOTICE.md). PROJ builds the inner projection from the same keys
//! with `o_proj` as its `proj`, and calls its formulas alone: without its
//! central meridian or false origin, which are the outer one's, and scaled
//! by the outer semi-major axis, not by the radius the inner set-up may
//! put in its place (HEALPix's authalic one).

use std::f64::consts::FRAC_PI_2;
use std::sync::Arc;

use super::authalic_q::Authalic;
use super::mod_ster::ModifiedStereographic;
use super::sphere::{aasin, aatan2};
use super::{BuildError, Params, Projection, adjlon};
use crate::crs::{Crs, Kind};
use crate::ellipsoid::Ellipsoid;
use crate::error::{CoordinateError, Error};

const TOL: f64 = 1e-10;

/// The `o_proj` names that are longitude and latitude.
const LONGLAT: &[&str] = &["longlat", "lonlat", "latlon", "latlong"];

#[derive(Debug)]
pub(crate) struct ObliqueTransformation {
    lon0: f64,
    fe: f64,
    fn_: f64,
    /// The projection turned, without its central meridian and false
    /// origin; `None` for longitude and latitude.
    inner: Option<Arc<dyn Projection>>,
    /// The outer semi-major axis over the inner projection's radius,
    /// forwards and backwards.
    scale: (f64, f64),
    lamp: f64,
    /// sin and cos of the new pole's latitude, `None` for a transverse
    /// aspect (the pole on the equator).
    oblique: Option<(f64, f64)>,
}

/// Whether a "PROJ" method's name is a rotated pole, a derived geographic
/// CRS.
pub(crate) fn is_geographic(name: &str) -> bool {
    LONGLAT
        .iter()
        .any(|p| name.starts_with(&format!("PROJ ob_tran o_proj={p}")))
}

impl ObliqueTransformation {
    pub(crate) fn new(params: &Params, ellipsoid: &Ellipsoid) -> Result<Self, BuildError> {
        let invalid = |m: &str| Err(BuildError::Invalid(m.into()));
        let Some(o_proj) = params.flag("o_proj") else {
            return invalid("Missing parameter: o_proj");
        };
        if o_proj == "ob_tran" {
            return invalid("Failed to find projection to be rotated");
        }
        let key = |k: &str| params.key(k).unwrap_or(0.0);
        let (lamp, phip) = if let Some(alpha) = params.key("o_alpha") {
            let (lamc, phic) = (key("o_lon_c"), key("o_lat_c"));
            if (phic.abs() - FRAC_PI_2).abs() <= TOL {
                return invalid("Invalid value for lat_c: |lat_c| should be < 90°");
            }
            let phip = aasin(phic.cos() * alpha.sin())
                .map_err(|_| BuildError::Invalid("Invalid value for o_alpha".into()))?;
            (lamc + aatan2(-alpha.cos(), -alpha.sin() * phic.sin()), phip)
        } else if let Some(phip) = params.key("o_lat_p") {
            (key("o_lon_p"), phip)
        } else {
            // The new equator through two points.
            let (lam1, phi1) = (key("o_lon_1"), key("o_lat_1"));
            let (lam2, phi2) = (key("o_lon_2"), key("o_lat_2"));
            if phi1.abs() > FRAC_PI_2 - TOL {
                return invalid("Invalid value for lat_1: |lat_1| should be < 90°");
            }
            if phi2.abs() > FRAC_PI_2 - TOL {
                return invalid("Invalid value for lat_2: |lat_2| should be < 90°");
            }
            if (phi1 - phi2).abs() < TOL {
                return invalid(
                    "Invalid value for lat_1 and lat_2: lat_1 should be different from lat_2",
                );
            }
            if phi1.abs() < TOL {
                return invalid("Invalid value for lat_1: lat_1 should be different from zero");
            }
            let lamp = (phi1.cos() * phi2.sin() * lam1.cos()
                - phi1.sin() * phi2.cos() * lam2.cos())
            .atan2(phi1.sin() * phi2.cos() * lam2.sin() - phi1.cos() * phi2.sin() * lam1.sin());
            (lamp, (-(lamp - lam1).cos() / phi1.tan()).atan())
        };
        let inner = if LONGLAT.contains(&o_proj) {
            None
        } else {
            Some(inner(o_proj, params, ellipsoid)?)
        };
        let (forward, backward) = inner_radii(o_proj, ellipsoid);
        Ok(ObliqueTransformation {
            lon0: key("lon_0"),
            fe: key("x_0"),
            fn_: key("y_0"),
            inner,
            scale: (ellipsoid.a / forward, ellipsoid.a / backward),
            lamp,
            oblique: (phip.abs() > TOL).then(|| phip.sin_cos()),
        })
    }

    /// The other PROJ keys the inner projection may read.
    pub(crate) fn inner_reads(params: &Params) -> &'static [&'static str] {
        params
            .flag("o_proj")
            .and_then(crate::proj_string::read::reads)
            .unwrap_or(&[])
    }
}

/// The radii the inner projection scales by, forwards and backwards,
/// where its PROJ set-up replaces the semi-major axis.
fn inner_radii(o_proj: &str, ellipsoid: &Ellipsoid) -> (f64, f64) {
    let a = ellipsoid.a;
    match o_proj {
        "healpix" | "rhealpix" if ellipsoid.es != 0.0 => {
            let r = a * (0.5 * Authalic::new(ellipsoid).qp).sqrt();
            (r, r)
        }
        "calcofi" => (1.0, 1.0),
        _ => match ModifiedStereographic::of(o_proj, ellipsoid.e).and_then(|(_, f)| f.a) {
            Some(forward) => (forward, a),
            None => (a, a),
        },
    }
}

/// The inner projection: a PROJ string of `o_proj` with the same keys but
/// `ob_tran`'s own and the central meridian and false origin, on the same
/// ellipsoid, built as any other.
fn inner(
    o_proj: &str,
    params: &Params,
    ellipsoid: &Ellipsoid,
) -> Result<Arc<dyn Projection>, BuildError> {
    let mut text = format!("+proj={o_proj}");
    for (k, v) in params.proj_key_values() {
        if k.starts_with("o_") || matches!(k, "lon_0" | "x_0" | "y_0") {
            continue;
        }
        let v = if crate::proj_string::read::is_angular_key(k) {
            v.to_degrees()
        } else {
            v
        };
        text.push_str(&format!(" +{k}={v}"));
    }
    for (k, v) in params.proj_flags() {
        if k == "o_proj" {
            continue;
        }
        if v.is_empty() {
            text.push_str(&format!(" +{k}"));
        } else {
            text.push_str(&format!(" +{k}={v}"));
        }
    }
    if ellipsoid.es == 0.0 {
        text.push_str(&format!(" +R={}", ellipsoid.a));
    } else {
        text.push_str(&format!(" +a={} +b={}", ellipsoid.a, ellipsoid.b));
    }
    text.push_str(" +type=crs");
    let refuse = |e: Error| match e {
        Error::Unsupported { what, .. } => BuildError::Unsupported(format!("o_proj: {what}")),
        e => BuildError::Invalid(format!("Projection to be rotated: {e}")),
    };
    let crs = Crs::from_proj_string(&text).map_err(refuse)?;
    match crs.kind {
        Kind::Projected { projection, .. } => Ok(projection),
        _ => Err(BuildError::Invalid(
            "Projection to be rotated is unknown".into(),
        )),
    }
}

impl Projection for ObliqueTransformation {
    fn forward(&self, lon: f64, phi: f64) -> Result<(f64, f64), CoordinateError> {
        let lam = adjlon(lon - self.lon0);
        let (sinphi, cosphi) = phi.sin_cos();
        let (sinlam, coslam) = lam.sin_cos();
        let (lam, phi) = match self.oblique {
            // Snyder's (5-8b) and (5-7).
            Some((sphip, cphip)) => (
                adjlon(
                    aatan2(cosphi * sinlam, sphip * cosphi * coslam + cphip * sinphi) + self.lamp,
                ),
                aasin(sphip * sinphi - cphip * cosphi * coslam)?,
            ),
            None => (
                adjlon(aatan2(cosphi * sinlam, sinphi) + self.lamp),
                aasin(-cosphi * coslam)?,
            ),
        };
        match &self.inner {
            Some(inner) => {
                let (x, y) = inner.forward(lam, phi)?;
                Ok((x * self.scale.0 + self.fe, y * self.scale.0 + self.fn_))
            }
            // Longitude and latitude, in radians and without a false origin.
            None => Ok((lam, phi)),
        }
    }

    fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), CoordinateError> {
        let (lam, phi) = match &self.inner {
            Some(inner) => {
                inner.inverse((x - self.fe) / self.scale.1, (y - self.fn_) / self.scale.1)?
            }
            None => (x, y),
        };
        let (lam, phi) = match self.oblique {
            // Snyder's (5-9) and (5-10b).
            Some((sphip, cphip)) => {
                let lam = lam - self.lamp;
                let (sinphi, cosphi) = phi.sin_cos();
                let coslam = lam.cos();
                (
                    aatan2(cosphi * lam.sin(), sphip * cosphi * coslam - cphip * sinphi),
                    aasin(sphip * sinphi + cphip * cosphi * coslam)?,
                )
            }
            None => {
                let cosphi = phi.cos();
                let t = lam - self.lamp;
                (
                    aatan2(cosphi * t.sin(), -phi.sin()),
                    aasin(cosphi * t.cos())?,
                )
            }
        };
        Ok((lam + self.lon0, phi))
    }
}
