//! CRS definitions as PROJ strings (`+proj=utm +zone=33 +datum=WGS84`),
//! read into PROJJSON and written from it (`docs/pyproj-phase3.md`, "PROJ
//! strings").
//!
//! PROJ strings have no specification: what one means as a CRS is what
//! PROJ 9.8.1's `PROJStringParser` makes of it, and that is what this
//! module follows. A string is read when PROJ reads it into a geographic,
//! geocentric or projected CRS (with EPSG's method, one of PROJ's own, or
//! the "PROJ hammer" method PROJ makes for a projection with neither), a
//! rotated pole, or such a CRS bound to WGS 84 by `+towgs84` or
//! `+nadgrids`. Anything else is an error naming what isn't supported:
//! options PROJ keeps only as an extension of the CRS (`+over`,
//! `+lon_wrap`), pipelines, and `+geoidgrids`.

pub(crate) mod names;
pub(crate) mod read;
mod tokens;
pub(crate) mod write;

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
    from_projjson_value(&value)
}

/// [`from_projjson`] for PROJJSON already parsed, by `serde_json` or from
/// another language's objects.
pub fn from_projjson_value(value: &serde_json::Value) -> Result<String, Error> {
    write::crs(value)
}
