//! CRS definitions as WKT (ISO 19162), read into PROJJSON and written
//! from it (`docs/crs-wkt.md`).
//!
//! WKT2, both the 2015 and the 2019 revision, and WKT1 as GDAL writes it
//! are read; WKT2 2019 and WKT1_GDAL are written. A CRS read from WKT is
//! converted like one from `xeibe-crs` ([`Crs::from_wkt`](crate::Crs::from_wkt)),
//! and [`Crs::identify`](crate::Crs::identify) finds its EPSG code.

pub(crate) mod datums;
mod esri;
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
/// (a derived geographic CRS, a parametric or temporal CRS). WKT1's
/// `TOWGS84` and WKT2's `BOUNDCRS` give a `BoundCRS`, as in PROJ.
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
    /// take. Geographic and projected 3D CRSs (but see [`AxisRule::Never`]),
    /// derived projected CRSs, and methods PROJ gives no WKT1 name, can't
    /// be written.
    Wkt1Gdal,
}

/// Which axes WKT output states: pyproj's `output_axis_rule` (`None`,
/// `True`, `False`), with PROJ's rules for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum AxisRule {
    /// PROJ's default. WKT2: every `CS` and its axes. WKT1_GDAL: `AXIS`
    /// for geocentric, vertical and local CRSs, and for a projected CRS
    /// only when it is easting, northing; none for a geographic CRS, whose
    /// WKT1 default is longitude, latitude.
    #[default]
    Auto,
    /// Every axis: in WKT1_GDAL, a geographic CRS's too, but not those of
    /// a projected CRS's base. WKT2 is as for `Auto`.
    Always,
    /// No axes: WKT2 without `CS` and `AXIS`, WKT1_GDAL without `AXIS`,
    /// but for an engineering CRS's. A geographic 3D CRS is then written
    /// in WKT1_GDAL, as a `GEOGCS` that can't say it has a height.
    Never,
}

/// A CRS's PROJJSON as WKT, on one line or, with `pretty`, indented by
/// 4 spaces per level as PROJ indents it.
///
/// The output follows PROJ's: ids on the CRS, its base CRS, the method and
/// the parameters only; each axis with its unit; numbers with 15
/// significant digits. Usages and remarks in the PROJJSON are written for
/// the outermost CRS.
///
/// A part of a CRS is written too, as PROJ writes it on its own, with its
/// ids: a datum or datum ensemble, an ellipsoid, a prime meridian, a
/// coordinate system (WKT2 only: its `CS` and `AXIS` nodes, separated by
/// commas), a conversion, or a transformation (WKT2 only, as
/// `COORDINATEOPERATION`, with its `source_crs` and `target_crs`). A
/// datum is followed by its prime meridian when that isn't Greenwich. In
/// WKT1_GDAL a conversion is its `PROJECTION` and `PARAMETER` nodes, in
/// degrees and metres.
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
    from_projjson_with(json, version, pretty, AxisRule::Auto)
}

/// [`from_projjson`] with the axes `axes` asks for.
///
/// ```
/// use xeibe_reproject::wkt::{self, AxisRule, Version};
///
/// let json = xeibe_crs::projjson(4326).unwrap();
/// let text = wkt::from_projjson_with(json, Version::Wkt1Gdal, false, AxisRule::Always).unwrap();
/// assert!(text.contains(r#"AXIS["Latitude",NORTH],AXIS["Longitude",EAST]"#));
/// ```
pub fn from_projjson_with(
    json: &str,
    version: Version,
    pretty: bool,
    axes: AxisRule,
) -> Result<String, Error> {
    let value: Value = serde_json::from_str(json).map_err(|e| Error::Projjson {
        crs: "PROJJSON".into(),
        message: e.to_string(),
    })?;
    let wkt1 = version == Version::Wkt1Gdal;
    let crs = value["type"].as_str().is_some_and(|t| t.ends_with("CRS"));
    let mut nodes = match (crs, wkt1) {
        (true, false) => vec![write::crs(&value)?],
        (true, true) => vec![write1::crs(&value, axes)?],
        (false, false) => write::object(&value)?,
        (false, true) => write1::object(&value)?,
    };
    if axes == AxisRule::Never {
        for n in &mut nodes {
            without_axes(n, wkt1);
        }
    }
    let texts: Vec<String> = nodes.iter().map(|n| write::format(n, pretty)).collect();
    Ok(texts.join(if pretty { ",\n" } else { "," }))
}

/// `node` without `AXIS`, and in WKT2 without `CS`; an engineering CRS
/// keeps its axes, as in PROJ.
fn without_axes(node: &mut parse::Node, wkt1: bool) {
    if node.keyword == "LOCAL_CS" || node.keyword == "ENGCRS" {
        return;
    }
    node.args.retain(|a| {
        !matches!(a, parse::Arg::Node(n)
            if n.keyword == "AXIS" || (!wkt1 && n.keyword == "CS"))
    });
    for a in &mut node.args {
        if let parse::Arg::Node(n) = a {
            without_axes(n, wkt1);
        }
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
        "GEOGCS" | "PROJCS" | "GEOCCS" | "VERT_CS" | "VERTCS" | "COMPD_CS" | "LOCAL_CS"
        | "FITTED_CS" => Ok((wkt1::crs(&node)?, wkt1::implicit_axes(&node))),
        _ => Ok((wkt2::crs(&node)?, false)),
    }
}
