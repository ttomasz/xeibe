//! CRS definitions as PROJ strings (`+proj=utm +zone=33 +datum=WGS84`),
//! read into PROJJSON (`docs/pyproj-phase3.md`, "PROJ strings").
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
mod read;
mod tokens;

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
