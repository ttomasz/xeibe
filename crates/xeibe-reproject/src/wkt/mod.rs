//! CRS definitions as WKT (ISO 19162), read into PROJJSON and written
//! from it (`docs/crs-wkt.md`).
//!
//! WKT2, both the 2015 and the 2019 revision, and WKT1 as GDAL writes it
//! are read; WKT2 2019 and WKT1_GDAL are written. A CRS read from WKT is
//! converted like one from `xeibe-crs` ([`Crs::from_wkt`](crate::Crs::from_wkt)),
//! and [`Crs::identify`](crate::Crs::identify) finds its EPSG code.

pub(crate) mod datums;
mod names;
mod parse;
mod wkt1;
mod wkt2;
mod write;
mod write1;

use serde_json::Value;

use crate::error::Error;

/// The PROJJSON for a CRS given as WKT, as a JSON string.
///
/// Everything WKT2 says that PROJJSON has a place for is kept: ids,
/// usages, remarks, anchor epochs, geoid models. A keyword this reader
/// doesn't know is an error naming it, as is a CRS type it doesn't read
/// (a derived geographic CRS, a parametric or temporal CRS, `BOUNDCRS`).
///
/// ```
/// let json = xeibe_reproject::wkt::to_projjson(
///     r#"GEOGCRS["WGS 84",DATUM["World Geodetic System 1984",
///          ELLIPSOID["WGS 84",6378137,298.257223563]],
///        CS[ellipsoidal,2],
///          AXIS["latitude",north],AXIS["longitude",east],
///          ANGLEUNIT["degree",0.0174532925199433],
///        ID["EPSG",4326]]"#,
/// )
/// .unwrap();
/// assert!(json.contains(r#""type":"GeographicCRS""#));
/// ```
pub fn to_projjson(text: &str) -> Result<String, Error> {
    Ok(to_value(text)?.to_string())
}

/// A WKT version this crate writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Version {
    /// ISO 19162:2019, as PROJ writes it by default (`WKT2_2019`).
    Wkt2_2019,
    /// WKT1 as GDAL writes it (`WKT1_GDAL`): what pyogrio and rasterio
    /// take. Geographic 3D and derived projected CRSs, and methods PROJ
    /// gives no WKT1 name, can't be written.
    Wkt1Gdal,
}

/// A CRS's PROJJSON as WKT, on one line or, with `pretty`, indented by
/// 4 spaces per level as PROJ indents it.
///
/// The output follows PROJ's: ids on the CRS, its base CRS, the method and
/// the parameters only; each axis with its unit; numbers with 15
/// significant digits. Usages and remarks in the PROJJSON are written for
/// the outermost CRS.
///
/// ```
/// use xeibe_reproject::wkt::{self, Version};
///
/// let json = xeibe_crs::projjson(2180).unwrap();
/// let text = wkt::from_projjson(json, Version::Wkt2_2019, false).unwrap();
/// assert!(text.starts_with(r#"PROJCRS["ETRS89 / PL-1992",BASEGEOGCRS["ETRS89""#));
/// assert!(text.ends_with(r#"ID["EPSG",2180]]"#));
/// ```
pub fn from_projjson(json: &str, version: Version, pretty: bool) -> Result<String, Error> {
    let value: Value = serde_json::from_str(json).map_err(|e| Error::Projjson {
        crs: "PROJJSON".into(),
        message: e.to_string(),
    })?;
    match version {
        Version::Wkt2_2019 => Ok(write::format(&write::crs(&value)?, pretty)),
        Version::Wkt1Gdal => Ok(write::format(&write1::crs(&value)?, pretty)),
    }
}

pub(crate) fn to_value(text: &str) -> Result<Value, Error> {
    read(text).map(|(value, _)| value)
}

/// The PROJJSON for a CRS given as WKT, and whether its horizontal axes
/// are WKT1's defaults (a `GEOGCS` or `PROJCS` without `AXIS`), which
/// identification compares by unit only.
pub(crate) fn read(text: &str) -> Result<(Value, bool), Error> {
    let node = parse::parse(text)?;
    match node.keyword.as_str() {
        "GEOGCS" | "PROJCS" | "GEOCCS" | "VERT_CS" | "COMPD_CS" | "LOCAL_CS" | "FITTED_CS" => {
            Ok((wkt1::crs(&node)?, wkt1::implicit_axes(&node)))
        }
        _ => Ok((wkt2::crs(&node)?, false)),
    }
}
