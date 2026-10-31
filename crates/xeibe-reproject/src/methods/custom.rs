//! The projections PROJ has no CRS method for, which it names "PROJ" and
//! the projection (`PROJ hammer`, `PROJ ob_tran o_proj=moll`), with the
//! PROJ string's flags in the name and its other keys as parameters
//! (`docs/pyproj-phase3.md`, "PROJ's own methods").
//!
//! Each is a port of PROJ 9.8.1's `src/projections/` code (MIT, see
//! PROJ-NOTICE.md), set up from the keys as PROJ sets it up from the
//! string: the keys every projection reads (`lon_0`, `lat_0`, `x_0`, `y_0`,
//! `k_0`) and the projection's own. A key PROJ reads but that changes more
//! than the projection (`+over`, `+lon_wrap`, `+geoc`, `+axis`, `+pm`) is
//! refused.

use std::sync::Arc;

use super::adams::Adams;
use super::aea::AlbersEqualArea;
use super::bipc::Bipolar;
use super::conic::{CentralConic, Lcca, Simple, k0};
use super::misc::Misc;
use super::mod_ster::ModifiedStereographic;
use super::ob_tran::ObliqueTransformation;
use super::pcyl::Pcyl;
use super::perspective::NearSidedPerspective;
use super::rouss::Roussilhe;
use super::sphere::Spherical;
use super::stere::Stereographic;
use super::world::World;
use super::{BuildError, Params, Projection};
use crate::ellipsoid::Ellipsoid;
use crate::proj_string::names::PROJECTION_READS;

/// The keys every projection reads that the set-up takes into account.
const GENERIC: &[&str] = &["lat_0", "lon_0", "x_0", "y_0", "k", "k_0"];

/// The projection of a "PROJ" method, its name without the "PROJ ".
pub(crate) fn build(
    name: &str,
    params: &Params,
    ellipsoid: &Ellipsoid,
) -> Result<Arc<dyn Projection>, BuildError> {
    let proj = name.split(' ').next().unwrap_or("");
    let Ok(i) = PROJECTION_READS.binary_search_by(|(n, _)| (*n).cmp(proj)) else {
        return Err(BuildError::UnknownMethod);
    };
    let own = PROJECTION_READS[i].1;
    // `ob_tran` hands the keys of its `o_proj` on.
    let inner = if proj == "ob_tran" {
        ObliqueTransformation::inner_reads(params)
    } else {
        &[]
    };
    if let Some(k) = params
        .proj_keys()
        .find(|k| !GENERIC.contains(k) && !own.contains(k) && !inner.contains(k))
    {
        return Err(BuildError::Unsupported(format!("+{k} with +proj={proj}")));
    }
    let a = ellipsoid.a;
    let spherical = |w: World| -> Result<Arc<dyn Projection>, BuildError> {
        Ok(Arc::new(Spherical::from_keys(w, params, a)))
    };
    let required = |k: &str| {
        params
            .key(k)
            .ok_or_else(|| BuildError::Invalid(format!("Missing parameter {k}.")))
    };
    if let Some(p) = Pcyl::of(proj, params.key("n")).map_err(BuildError::Invalid)? {
        return Ok(Arc::new(Spherical::from_keys(p, params, a)));
    }
    let flag = |k: &str| params.flag(k).is_some();
    if let Some(p) = Misc::of(proj, |k| params.key(k), flag).map_err(BuildError::Invalid)? {
        return Ok(Arc::new(Spherical::from_keys(p, params, a)));
    }
    let shape = params.flag("shape");
    if let Some(p) = Adams::of(proj, shape, |k| params.key(k)).map_err(BuildError::Invalid)? {
        return Ok(Arc::new(Spherical::from_keys(p, params, a)));
    }
    if let Some(p) = Simple::of(proj, params)? {
        return Ok(Arc::new(Spherical::from_keys(p, params, a)));
    }
    let key = |k: &str| params.key(k).unwrap_or(0.0);
    match proj {
        "leac" => Ok(Arc::new(AlbersEqualArea::leac(params, ellipsoid)?)),
        "lcca" => Ok(Arc::new(Lcca::new(params, ellipsoid)?)),
        "ccon" => Ok(Arc::new(Spherical::from_keys(
            CentralConic::new(params)?,
            params,
            a,
        ))),
        "bipc" => Ok(Arc::new(Spherical::from_keys(
            Bipolar { noskew: flag("ns") },
            params,
            a,
        ))),
        "rouss" => Ok(Arc::new(Roussilhe::new(params, ellipsoid)?)),
        "ups" => Ok(Arc::new(Stereographic::ups(ellipsoid, flag("south"))?)),
        "tpers" => {
            let unit = NearSidedPerspective::new(key("lat_0"), key("h"), a)?
                .tilted(key("tilt"), key("azi"));
            Ok(Arc::new(Spherical::from_keys(unit, params, a)))
        }
        "ocea" => ocea(params, a),
        "mil_os" | "lee_os" | "gs48" | "alsk" | "gs50" => {
            let (unit, fixed) =
                ModifiedStereographic::of(proj, ellipsoid.e).ok_or(BuildError::UnknownMethod)?;
            let mut s = Spherical::from_keys(unit, params, a);
            s.set_lon0(fixed.lam0);
            if let Some(a) = fixed.a {
                s.forward_radius(a);
            }
            Ok(Arc::new(s))
        }
        "ob_tran" => Ok(Arc::new(ObliqueTransformation::new(params, ellipsoid)?)),
        "putp1" => spherical(World::Eckert3 {
            cx: 1.89490,
            cy: 0.94745,
            a: -0.5,
            b: 0.303_963_550_927_013_3,
        }),
        "kav7" => spherical(World::Eckert3 {
            cx: 0.866_025_403_784_4,
            cy: 1.0,
            a: 0.0,
            b: 0.303_963_550_927_013_3,
        }),
        "kav5" => spherical(World::sts(1.50488, 1.35439, false)),
        "fouc" => spherical(World::sts(2.0, 2.0, true)),
        "mbt_s" => spherical(World::sts(1.48875, 1.36509, false)),
        "mbtfps" => spherical(World::sinusoidal(0.5, 1.785_398_163_397_448_3)),
        "gn_sinu" => {
            let n = required("n")?;
            let m = required("m")?;
            if n <= 0.0 {
                return Err(BuildError::Invalid(
                    "Invalid value for n: it should be > 0.".into(),
                ));
            }
            if m < 0.0 {
                return Err(BuildError::Invalid(
                    "Invalid value for m: it should be >= 0.".into(),
                ));
            }
            spherical(World::sinusoidal(m, n))
        }
        "urmfps" => {
            let n = required("n")?;
            if n <= 0.0 || n > 1.0 {
                return Err(BuildError::Invalid(
                    "Invalid value for n: it should be in ]0,1] range.".into(),
                ));
            }
            spherical(World::Urmaev {
                n,
                cy: 1.139_753_528_477 / n,
            })
        }
        _ => Err(BuildError::UnknownMethod),
    }
}

/// Oblique Cylindrical Equal Area: its pole from a point and an azimuth
/// (`alpha`, `lonc`), else from two points; PROJ sets the central meridian
/// from the pole.
fn ocea(params: &Params, a: f64) -> Result<Arc<dyn Projection>, BuildError> {
    use std::f64::consts::{FRAC_PI_2, PI};
    let key = |k: &str| params.key(k).unwrap_or(0.0);
    let phi0 = key("lat_0");
    let (lam_p, phi_p) = if let Some(alpha) = params.key("alpha") {
        let alpha = PI + alpha;
        let lonz = key("lonc");
        (
            (-alpha.cos()).atan2(-phi0.sin() * alpha.sin()) + lonz,
            (phi0.cos() * alpha.sin()).asin(),
        )
    } else {
        let (phi_1, phi_2) = (key("lat_1"), key("lat_2"));
        let (lam_1, lam_2) = (key("lon_1"), key("lon_2"));
        let mut lam_p = (phi_1.cos() * phi_2.sin() * lam_1.cos()
            - phi_1.sin() * phi_2.cos() * lam_2.cos())
        .atan2(phi_1.sin() * phi_2.cos() * lam_2.sin() - phi_1.cos() * phi_2.sin() * lam_1.sin());
        // P->lam0's wrap-around when lon_1 = -90°.
        if lam_1 == -FRAC_PI_2 {
            lam_p = -lam_p;
        }
        let c = (lam_p - lam_1).cos();
        let tan_phi_1 = phi_1.tan();
        let phi_p = if tan_phi_1 == 0.0 {
            if c >= 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 }
        } else {
            (-c / tan_phi_1).atan()
        };
        (lam_p, phi_p)
    };
    let k = k0(params);
    let unit = Misc::ObliqueCylindricalEqualArea {
        rok: 1.0 / k,
        rtk: k,
        sinphi: phi_p.sin(),
        cosphi: phi_p.cos(),
    };
    let mut s = Spherical::from_keys(unit, params, a);
    s.set_lon0(lam_p + FRAC_PI_2);
    Ok(Arc::new(s))
}
