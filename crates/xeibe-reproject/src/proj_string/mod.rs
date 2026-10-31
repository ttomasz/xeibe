//! CRS definitions as PROJ strings (`+proj=utm +zone=33 +datum=WGS84`),
//! read into PROJJSON and written from it (`docs/pyproj-phase3.md`, "PROJ
//! strings").
//!
//! PROJ strings have no specification: what one means as a CRS is what
//! PROJ 9.8.1's `PROJStringParser` makes of it, and that is what this
//! module follows. A string is read when PROJ reads it into a geographic,
//! geocentric or projected CRS with an EPSG method, or into such a CRS
//! bound to WGS 84 by `+towgs84` or `+nadgrids`. Anything else is an error
//! naming what isn't supported: projections only PROJ defines (`robin`,
//! `moll`), options PROJ keeps only as an extension of the CRS (`+over`,
//! `+lon_wrap`), pipelines, and `+geoidgrids`.

mod names;
pub(crate) mod read;
mod tokens;
mod write;

use crate::error::Error;

/// The PROJJSON for a CRS given as a PROJ string, as a JSON string.
///
/// The string must say it is a CRS (`+type=crs`), as PROJ requires; pyproj
/// adds it before handing strings over. `+init=epsg:CODE` gives the EPSG
/// CRS, with longitude first for a geographic CRS and easting first for a
/// projected one, as PROJ's PROJ.4 rules have it.
///
/// ```
/// let json = xeibe_reproject::proj_string::to_projjson(
///     "+proj=utm +zone=33 +datum=WGS84 +type=crs",
/// )
/// .unwrap();
/// assert!(json.contains(r#""name":"UTM zone 33N""#));
/// ```
pub fn to_projjson(text: &str) -> Result<String, Error> {
    Ok(read::crs(text)?.to_string())
}

/// A CRS's PROJJSON as a PROJ string, as PROJ writes it (pyproj's
/// `to_proj4()`; PROJ writes the same in its PROJ_4 and PROJ_5
/// conventions): the projection and its parameters, `+datum` for WGS 84,
/// NAD83 and NAD27 or else the ellipsoid and prime meridian, `+towgs84` or
/// `+nadgrids` for a Bound CRS, `+units`, `+no_defs` and `+type=crs`. A CRS
/// PROJ can't write as a PROJ string (an engineering CRS, a method PROJ has
/// no PROJ name for, some variants' parameter values) is an error, as in
/// PROJ.
///
/// ```
/// let json = xeibe_crs::projjson(2180).unwrap();
/// assert_eq!(
///     xeibe_reproject::proj_string::from_projjson(json).unwrap(),
///     "+proj=tmerc +lat_0=0 +lon_0=19 +k=0.9993 +x_0=500000 +y_0=-5300000 +ellps=GRS80 +units=m +no_defs +type=crs",
/// );
/// ```
pub fn from_projjson(json: &str) -> Result<String, Error> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| Error::Projjson {
        crs: "PROJJSON".into(),
        message: e.to_string(),
    })?;
    write::crs(&value)
}

/// PROJ's `internal::toString(double)`: `%.15g`, or `%.14g` when that
/// shows a run of nines.
pub(crate) fn to_string(v: f64) -> String {
    let s = format_g(v, 15);
    if s.contains("9999999999") {
        format_g(v, 14)
    } else {
        s
    }
}

/// C's `%.{precision}g`.
pub(crate) fn format_g(v: f64, precision: usize) -> String {
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    if !v.is_finite() {
        return if v.is_nan() {
            "nan".into()
        } else if v > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        };
    }
    let p = precision.max(1);
    let e = format!("{:.*e}", p - 1, v);
    let (mantissa, exponent) = e.split_once('e').expect("an exponent");
    let x: i32 = exponent.parse().expect("an integer");
    let trim = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s.to_owned()
        }
    };
    if x < -4 || x >= p as i32 {
        let sign = if x < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa), x.abs())
    } else {
        let decimals = (p as i32 - 1 - x).max(0) as usize;
        trim(&format!("{v:.decimals$}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_as_c() {
        assert_eq!(format_g(0.3048006096012192, 15), "0.304800609601219");
        assert_eq!(format_g(1640416.666666667, 15), "1640416.66666667");
        assert_eq!(format_g(500000.0, 15), "500000");
        assert_eq!(format_g(1e-6, 15), "1e-06");
        assert_eq!(format_g(1.5e20, 15), "1.5e+20");
        assert_eq!(format_g(-0.0001, 15), "-0.0001");
        assert_eq!(format_g(123456789012345680.0, 15), "1.23456789012346e+17");
        assert_eq!(to_string(0.1 + 0.2), "0.3");
    }
}
