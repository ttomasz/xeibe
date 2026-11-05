//! A CRS given as text in any of the forms pyproj's `CRS.from_user_input`
//! reads ([`Crs::from_user_input`]): EPSG codes in their several
//! spellings, OGC's CRS84 and its kin, EPSG names and aliases, Esri's old
//! codes of EPSG CRSs, PROJ strings, WKT and PROJJSON. [`parse`] tells which
//! one a text is without reading it.
//!
//! ```
//! use xeibe_reproject::user_input::{self, UserInput};
//!
//! assert_eq!(user_input::parse("urn:ogc:def:crs:EPSG::2180").unwrap(), UserInput::Epsg(2180));
//! assert_eq!(user_input::parse("EPSG:4326+5773").unwrap(), UserInput::Compound(4326, 5773));
//! assert_eq!(user_input::parse("OGC:CRS84").unwrap(), UserInput::Ogc("CRS84"));
//! ```

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::crs::Crs;
use crate::error::Error;

/// What a CRS given as text names, as [`parse`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserInput<'a> {
    /// An EPSG CRS: `EPSG:2180`, `epsg:2180`, `2180`, its OGC URN or URL
    /// (`urn:ogc:def:crs:EPSG::2180`,
    /// `http://www.opengis.net/def/crs/EPSG/0/2180`, GML's
    /// `http://www.opengis.net/gml/srs/epsg.xml#2180`), its name or an
    /// alias EPSG gives it (`ETRS89 / Poland CS92`, `PL-1992`), or an old
    /// Esri code of a CRS EPSG has since defined (`ESRI:102100` is
    /// EPSG:3857; with the `esri` feature).
    Epsg(u32),
    /// OGC's longitude-first twin of an EPSG geographic CRS: `CRS84`
    /// (EPSG:4326), `CRS83` (EPSG:4269) or `CRS27` (EPSG:4267), as
    /// `OGC:CRS84` or its URN or URL.
    Ogc(&'static str),
    /// Two EPSG CRSs, horizontal and vertical: `EPSG:2180+9650`, a compound
    /// URN (`urn:ogc:def:crs,crs:EPSG::4258,crs:EPSG::7837`) or URL
    /// (`http://www.opengis.net/def/crs-compound?1=…&2=…`). PROJ makes a
    /// compound CRS of them named "horizontal + vertical", without a code,
    /// even when EPSG registers the pair under one.
    Compound(u32, u32),
    /// PROJJSON: text starting with `{`.
    Projjson(&'a str),
    /// WKT1 (GDAL's or ESRI's) or WKT2: text starting with a CRS keyword
    /// and `[`.
    Wkt(&'a str),
    /// A PROJ string: text with a `=` that isn't WKT, PROJJSON or a URL.
    /// It is read as a CRS, with `+type=crs` added when missing, as pyproj
    /// adds it; a bare PROJ string is an operation in PROJ.
    ProjString(&'a str),
}

impl UserInput<'_> {
    /// The CRS, read.
    pub fn crs(&self) -> Result<Crs, Error> {
        match *self {
            UserInput::Epsg(code) => Crs::from_epsg(code),
            UserInput::Projjson(text) => Crs::from_projjson(text),
            UserInput::Wkt(text) => Crs::from_wkt(text),
            UserInput::ProjString(text) => Crs::from_proj_string(&typed(text)),
            UserInput::Ogc(_) | UserInput::Compound(..) => {
                Crs::from_projjson_value(&self.projjson()?)
            }
        }
    }

    /// The CRS's PROJJSON. For an OGC CRS, its EPSG twin's with the axes
    /// swapped, as PROJ has it; for a compound, PROJ's `CompoundCRS` of the
    /// two.
    pub fn projjson(&self) -> Result<Value, Error> {
        match *self {
            UserInput::Epsg(code) => epsg_projjson(code),
            UserInput::Ogc(code) => {
                let &(_, twin, name) = OGC
                    .iter()
                    .find(|(c, _, _)| *c == code)
                    .ok_or_else(|| unsupported_ogc(code))?;
                let mut out = epsg_projjson(twin)?;
                out["name"] = name.into();
                if let Some(axes) = out["coordinate_system"]["axis"].as_array_mut() {
                    axes.reverse();
                }
                out["id"] = json!({"authority": "OGC", "code": code});
                Ok(out)
            }
            UserInput::Compound(horizontal, vertical) => {
                let mut schema = Value::Null;
                let mut components = Vec::with_capacity(2);
                for code in [horizontal, vertical] {
                    let mut part = epsg_projjson(code)?;
                    if let Some(s) = part.as_object_mut().and_then(|o| o.remove("$schema")) {
                        schema = s;
                    }
                    components.push(part);
                }
                let name = format!(
                    "{} + {}",
                    components[0]["name"].as_str().unwrap_or_default(),
                    components[1]["name"].as_str().unwrap_or_default()
                );
                Ok(json!({
                    "$schema": schema,
                    "type": "CompoundCRS",
                    "name": name,
                    "components": components,
                }))
            }
            UserInput::Projjson(text) => serde_json::from_str(text).map_err(|e| Error::Projjson {
                crs: "PROJJSON".into(),
                message: e.to_string(),
            }),
            UserInput::Wkt(text) => crate::wkt::read(text).map(|(value, _)| value),
            UserInput::ProjString(text) => crate::proj_string::read::crs(&typed(text)),
        }
    }
}

/// OGC's longitude-first variants of three EPSG CRSs: code, EPSG twin and
/// name, as PROJ has them.
const OGC: [(&str, u32, &str); 3] = [
    ("CRS84", 4326, "WGS 84 (CRS84)"),
    ("CRS83", 4269, "NAD83 (CRS83)"),
    ("CRS27", 4267, "NAD27 (CRS27)"),
];

/// The authorities in PROJ's database besides EPSG: their codes are known,
/// but aren't EPSG's.
const OTHER_AUTHORITIES: [&str; 7] = ["ESRI", "IGNF", "IAU_2015", "NKG", "PROJ", "OGC", "NRCAN"];

/// The CRS keywords of WKT1 (GDAL's and ESRI's) and WKT2.
const WKT_KEYWORDS: [&str; 26] = [
    "GEOGCS",
    "PROJCS",
    "GEOCCS",
    "VERT_CS",
    "VERTCS",
    "COMPD_CS",
    "LOCAL_CS",
    "FITTED_CS",
    "GEOGCRS",
    "GEODCRS",
    "PROJCRS",
    "VERTCRS",
    "COMPOUNDCRS",
    "ENGCRS",
    "BOUNDCRS",
    "GEODETICCRS",
    "GEOGRAPHICCRS",
    "PROJECTEDCRS",
    "VERTICALCRS",
    "ENGINEERINGCRS",
    "DERIVEDPROJCRS",
    "DERIVEDPROJECTEDCRS",
    "BASEGEOGCRS",
    "TIMECRS",
    "TEMPORALCRS",
    "PARAMETRICCRS",
];

/// What `text` names, in the order pyproj tries the forms: PROJJSON, a
/// PROJ string, WKT, GML's `epsg.xml#` URL, OGC URNs and URLs (compound
/// ones too), `AUTHORITY:CODE`, a bare number (an EPSG code), then EPSG's
/// names and aliases. Prefixes, authorities and names are matched ignoring
/// case.
///
/// Only the form is checked, not the definition: an EPSG code EPSG doesn't
/// define, or WKT that doesn't parse, fails when it is read
/// ([`UserInput::crs`]). A code of another authority PROJ knows (ESRI's
/// own, IGNF's) is [`Error::Unsupported`]; text that names nothing is
/// [`Error::UserInput`].
pub fn parse(text: &str) -> Result<UserInput<'_>, Error> {
    let s = text.trim();
    let invalid = |message: String| Error::UserInput {
        input: s.to_owned(),
        message,
    };
    if s.starts_with('{') {
        return Ok(UserInput::Projjson(s));
    }
    if is_proj_string(s) {
        return Ok(UserInput::ProjString(s));
    }
    if is_wkt(s) {
        return Ok(UserInput::Wkt(s));
    }
    let http = strip_prefix_ci(s, "http://")
        .or_else(|| strip_prefix_ci(s, "https://"))
        .and_then(|rest| strip_prefix_ci(rest, "www.opengis.net/"));
    if let Some(code) = http.and_then(|rest| strip_prefix_ci(rest, "gml/srs/epsg.xml#"))
        && is_digits(code)
    {
        return epsg_code(code).map(UserInput::Epsg).map_err(invalid);
    }
    if let Some(parts) = strip_prefix_ci(s, "urn:ogc:def:crs,")
        && !parts.is_empty()
    {
        // `crs:EPSG::4258,crs:EPSG::7837`: a comma before each `crs:`.
        let mut codes = Vec::new();
        for part in split_before(parts, ",crs:") {
            match strip_prefix_ci(part.trim(), "crs:").and_then(urn_code) {
                Some((auth, code)) if auth.eq_ignore_ascii_case("EPSG") && !code.contains(',') => {
                    codes.push(code);
                }
                _ => return Err(invalid("unsupported compound URN".into())),
            }
        }
        return compound(&codes).map_err(invalid);
    }
    if let Some((auth, code)) = strip_prefix_ci(s, "urn:ogc:def:crs:").and_then(urn_code) {
        return authority(auth, code, &invalid);
    }
    if let Some(query) = http.and_then(|rest| strip_prefix_ci(rest, "def/crs-compound?"))
        && !query.is_empty()
    {
        // `1=<URL>&2=<URL>`, in the order given.
        let mut codes = Vec::new();
        for pair in query.split('&') {
            let url = pair.split_once('=').map_or(pair, |(_, url)| url);
            let found = strip_prefix_ci(url, "http://")
                .or_else(|| strip_prefix_ci(url, "https://"))
                .and_then(|rest| strip_prefix_ci(rest, "www.opengis.net/def/crs/"))
                .and_then(url_code);
            match found {
                Some((auth, code)) if auth.eq_ignore_ascii_case("EPSG") => codes.push(code),
                _ => return Err(invalid("unsupported compound URL".into())),
            }
        }
        return compound(&codes).map_err(invalid);
    }
    if let Some((auth, code)) = http
        .and_then(|rest| strip_prefix_ci(rest, "def/crs/"))
        .and_then(url_code)
    {
        return authority(auth, code, &invalid);
    }
    if let Some((auth, code)) = s.split_once(':') {
        let auth = auth.trim_end();
        let code = code.trim_start();
        let auth_ok =
            !auth.is_empty() && auth.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if auth_ok && !code.is_empty() && !code.contains(':') {
            return authority(auth, code, &invalid);
        }
    }
    if is_digits(s) {
        return epsg_code(s).map(UserInput::Epsg).map_err(invalid);
    }
    if let Some(code) = code_for_name(s) {
        return Ok(UserInput::Epsg(code));
    }
    Err(invalid(
        "not an EPSG code, OGC CRS, URN or URL, EPSG name, PROJ string, WKT or PROJJSON".into(),
    ))
}

/// `AUTHORITY:CODE`, from a URN, a URL or as it is.
fn authority<'a>(
    auth: &str,
    code: &str,
    invalid: &dyn Fn(String) -> Error,
) -> Result<UserInput<'a>, Error> {
    let auth = auth.trim().to_ascii_uppercase();
    let code = code.trim();
    if auth == "EPSG" {
        if code.contains('+') {
            return compound(&code.split('+').collect::<Vec<_>>()).map_err(invalid);
        }
        return epsg_code(code).map(UserInput::Epsg).map_err(invalid);
    }
    if auth == "OGC" {
        let code = code.to_ascii_uppercase();
        return OGC
            .iter()
            .find(|(c, _, _)| *c == code)
            .map(|(c, _, _)| UserInput::Ogc(c))
            .ok_or_else(|| unsupported_ogc(&code));
    }
    if auth == "ESRI"
        && is_digits(code)
        && let Some(moved) = code.parse().ok().and_then(esri_epsg_code)
    {
        return Ok(UserInput::Epsg(moved));
    }
    if OTHER_AUTHORITIES.contains(&auth.as_str()) {
        let what = if auth == "ESRI" {
            "an ESRI code of a CRS EPSG doesn't define".to_owned()
        } else {
            format!("a code of {auth}")
        };
        return Err(Error::Unsupported {
            crs: format!("{auth}:{code}"),
            what,
        });
    }
    Err(invalid(format!("unknown authority {auth}")))
}

fn unsupported_ogc(code: &str) -> Error {
    Error::Unsupported {
        crs: format!("OGC:{code}"),
        what: "an OGC code other than CRS84, CRS83 and CRS27".into(),
    }
}

/// Two EPSG codes, horizontal and vertical.
fn compound<'a>(codes: &[&str]) -> Result<UserInput<'a>, String> {
    match *codes {
        [horizontal, vertical] => Ok(UserInput::Compound(
            epsg_code(horizontal)?,
            epsg_code(vertical)?,
        )),
        _ => Err(format!(
            "a compound CRS needs two components, not {}",
            codes.len()
        )),
    }
}

fn epsg_code(code: &str) -> Result<u32, String> {
    let code = code.trim();
    if !is_digits(code) {
        return Err(format!("not an EPSG code: {code:?}"));
    }
    code.parse()
        .map_err(|_| format!("not an EPSG code: {code:?}"))
}

fn epsg_projjson(code: u32) -> Result<Value, Error> {
    let text = xeibe_crs::projjson(code).ok_or(Error::UnknownCode(code))?;
    serde_json::from_str(text).map_err(|e| Error::Projjson {
        crs: format!("EPSG:{code}"),
        message: e.to_string(),
    })
}

/// The code of the EPSG CRS with this name, ignoring case, else equal by
/// [`xeibe_crs::name_key`], else with this EPSG alias. The current CRSs
/// come first: `WGS 84` is EPSG:4326, not the deprecated geocentric one.
fn code_for_name(name: &str) -> Option<u32> {
    struct Names {
        lowercase: HashMap<String, u32>,
        keyed: HashMap<String, u32>,
    }
    static NAMES: OnceLock<Names> = OnceLock::new();
    let names = NAMES.get_or_init(|| {
        let mut records: Vec<_> = xeibe_crs::CRS.iter().collect();
        records.sort_by_key(|r| (r.deprecated, r.code));
        let mut names = Names {
            lowercase: HashMap::with_capacity(records.len()),
            keyed: HashMap::with_capacity(records.len()),
        };
        for r in records {
            names
                .lowercase
                .entry(r.name.to_lowercase())
                .or_insert(r.code);
            names
                .keyed
                .entry(xeibe_crs::name_key(r.name))
                .or_insert(r.code);
        }
        names
    });
    names
        .lowercase
        .get(&name.trim().to_lowercase())
        .or_else(|| names.keyed.get(&xeibe_crs::name_key(name)))
        .copied()
        .or_else(|| xeibe_crs::alias(name))
}

/// The EPSG code of an old Esri code; none without the `esri` feature.
#[cfg(feature = "esri")]
fn esri_epsg_code(code: u32) -> Option<u32> {
    xeibe_esri::epsg_code(code)
}

#[cfg(not(feature = "esri"))]
fn esri_epsg_code(_code: u32) -> Option<u32> {
    None
}

/// pyproj's `is_proj`: a `=`, and not WKT, PROJJSON or a URL, which may
/// have one too.
fn is_proj_string(s: &str) -> bool {
    s.contains('=')
        && !is_wkt(s)
        && !s.starts_with('{')
        && strip_prefix_ci(s, "http://").is_none()
        && strip_prefix_ci(s, "https://").is_none()
}

fn is_wkt(s: &str) -> bool {
    let s = s.trim_start();
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    WKT_KEYWORDS
        .iter()
        .any(|k| k.eq_ignore_ascii_case(&s[..end]))
        && s[end..].trim_start().starts_with('[')
}

/// A PROJ string as a CRS: with `+type=crs`, as pyproj adds it.
fn typed(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains("type=crs") {
        text.into()
    } else if text.contains('+') {
        format!("{text} +type=crs").into()
    } else {
        format!("{text} type=crs").into()
    }
}

/// `AUTH:VERSION:CODE`, the version possibly empty, of a URN.
fn urn_code(rest: &str) -> Option<(&str, &str)> {
    let mut fields = rest.split(':');
    let (auth, _version, code) = (fields.next()?, fields.next()?, fields.next()?);
    (fields.next().is_none() && !auth.is_empty() && !code.is_empty()).then_some((auth, code))
}

/// `AUTH/VERSION/CODE`, maybe with a trailing slash, of a URL.
fn url_code(rest: &str) -> Option<(&str, &str)> {
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let mut fields = rest.split('/');
    let (auth, _version, code) = (fields.next()?, fields.next()?, fields.next()?);
    let code_ok = !code.is_empty() && !code.contains(['?', '#']);
    (fields.next().is_none() && !auth.is_empty() && code_ok).then_some((auth, code))
}

/// `text` split at each `separator`, which starts with a one-byte
/// delimiter (`,crs:`): the delimiter goes, the rest of the separator
/// stays with the part it starts.
fn split_before<'a>(text: &'a str, separator: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(separator) {
        parts.push(&rest[..i]);
        rest = &rest[i + 1..];
    }
    parts.push(rest);
    parts
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}
