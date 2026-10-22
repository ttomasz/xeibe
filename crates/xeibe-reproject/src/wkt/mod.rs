//! CRS definitions as WKT (ISO 19162), read into PROJJSON
//! (`docs/crs-wkt.md`).
//!
//! WKT2, both the 2015 and the 2019 revision, is read now. WKT1 in GDAL's
//! and ESRI's forms comes next; until then it is an
//! [`Error::Unsupported`]. A CRS read from WKT is converted like one from
//! `xeibe-crs`: [`Crs::from_wkt`](crate::Crs::from_wkt).

mod parse;
mod wkt2;

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

pub(crate) fn to_value(text: &str) -> Result<Value, Error> {
    let node = parse::parse(text)?;
    match node.keyword.as_str() {
        "GEOGCS" | "PROJCS" | "GEOCCS" | "VERT_CS" | "COMPD_CS" | "LOCAL_CS" | "FITTED_CS" => {
            Err(Error::Unsupported {
                crs: format!("{} WKT", node.keyword),
                what: "WKT1".into(),
            })
        }
        _ => wkt2::crs(&node),
    }
}
