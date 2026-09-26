//! srsName parsing: which CRS, and which *form* it was written in (the form
//! matters for `AxisOrderMode::CrsHeuristic`). See `docs/geometry.md`.

use serde::{Deserialize, Serialize};

/// How an srsName was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SrsNameForm {
    /// `EPSG:2180`
    Short,
    /// `http://www.opengis.net/gml/srs/epsg.xml#2180`
    LegacyUrl,
    /// `urn:ogc:def:crs:EPSG::2180`, versioned `urn:ogc:def:crs:EPSG:6.6:4326`
    OgcUrn,
    /// `urn:x-ogc:def:crs:EPSG::4326`, `urn:x-ogc:def:crs:EPSG:4326`
    ExperimentalUrn,
    /// `urn:EPSG:geographicCRS:4326` (WFS 1.1)
    Wfs11Urn,
    /// `http://www.opengis.net/def/crs/EPSG/0/2180`
    HttpUri,
    /// `http://www.opengis.net/def/crs?authority=EPSG&version=0&code=4326`
    HttpUriKvp,
    /// `urn:ogc:def:crs,crs:EPSG::4269,crs:EPSG::5713`
    CompoundUrn,
    /// `http://www.opengis.net/def/crs-compound?1=…&2=…`
    CompoundUri,
    /// `urn:adv:crs:ETRS89_UTM32`, compound `urn:adv:crs:ETRS89_UTM32*DE_DHHN2016_NH`
    /// (German surveying; mapped to EPSG codes, read in authority order)
    AdvUrn,
    Unknown,
}

/// A CRS identified by authority and code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CrsRef {
    /// e.g. `EPSG`, `2180`; `OGC`, `CRS84`.
    Code { authority: String, code: String },
    /// Horizontal + vertical (+ …) components, in order.
    Compound(Vec<CrsRef>),
    /// A compound CRS's part that names no CRS we know, as written:
    /// `PL-XYZ` in `urn:ogc:def:crs,crs:EPSG::2180,crs:PL-XYZ`. The compound
    /// keeps its known parts, and the read report names this one. Never a
    /// whole CRS: an srsName that is only unknown names has no CRS.
    Unresolved(String),
}

impl CrsRef {
    pub fn epsg(code: impl Into<String>) -> Self {
        CrsRef::Code { authority: "EPSG".into(), code: code.into() }
    }

    /// `EPSG:2180`, `OGC:CRS84` — the value written to GeoArrow `crs`
    /// metadata with `crs_type = "authority_code"`.
    ///
    /// A compound CRS has no such form; it is written PROJ-style,
    /// `EPSG:4269+5713` (or `EPSG:25832+OGC:…` across authorities), with an
    /// unresolved part as written (`EPSG:2180+PL-XYZ`).
    pub fn authority_code(&self) -> String {
        match self {
            CrsRef::Code { authority, code } => format!("{authority}:{code}"),
            CrsRef::Unresolved(name) => name.clone(),
            CrsRef::Compound(parts) => {
                let mut out = String::new();
                let mut previous_authority: Option<&str> = None;
                for part in parts {
                    if !out.is_empty() {
                        out.push('+');
                    }
                    match part {
                        CrsRef::Code { authority, code } if previous_authority == Some(authority) => {
                            out.push_str(code);
                        }
                        CrsRef::Code { authority, .. } => {
                            out.push_str(&part.authority_code());
                            previous_authority = Some(authority);
                        }
                        nested => out.push_str(&nested.authority_code()),
                    }
                }
                out
            }
        }
    }

    /// CRS84/CRS83/CRS27: longitude/latitude regardless of authority rules.
    /// For a compound CRS, the horizontal (first) component decides.
    pub fn is_lon_lat_by_definition(&self) -> bool {
        match self {
            CrsRef::Code { authority, code } => {
                authority.eq_ignore_ascii_case("OGC")
                    && ["CRS84", "CRS83", "CRS27", "CRS84h"]
                        .iter()
                        .any(|c| code.eq_ignore_ascii_case(c))
            }
            CrsRef::Compound(parts) => parts.first().is_some_and(CrsRef::is_lon_lat_by_definition),
            CrsRef::Unresolved(_) => false,
        }
    }

    /// The horizontal component: the CRS itself, or a compound's first part.
    pub fn horizontal(&self) -> &CrsRef {
        match self {
            CrsRef::Compound(parts) => parts.first().map_or(self, CrsRef::horizontal),
            code => code,
        }
    }

    /// The parts named as written because they name no known CRS.
    pub fn unresolved(&self) -> Vec<&str> {
        match self {
            CrsRef::Unresolved(name) => vec![name.as_str()],
            CrsRef::Compound(parts) => parts.iter().flat_map(CrsRef::unresolved).collect(),
            CrsRef::Code { .. } => Vec::new(),
        }
    }

    /// PROJJSON from the built-in EPSG tables, or `None` when a part isn't
    /// in them (another authority, or a code EPSG doesn't define).
    ///
    /// A compound CRS is a `CompoundCRS` built from its parts, the way PROJ
    /// builds `EPSG:25832+7837`: named `"A + B"`, and without an `id`, even
    /// when EPSG registers the same pair under a code of its own. Unresolved
    /// parts are left out, so `EPSG:2180+PL-XYZ` is EPSG:2180.
    pub fn projjson(&self) -> Option<serde_json::Value> {
        match self {
            CrsRef::Code { authority, code } if authority.eq_ignore_ascii_case("EPSG") => {
                serde_json::from_str(xeibe_crs::projjson(code.parse().ok()?)?).ok()
            }
            CrsRef::Code { .. } | CrsRef::Unresolved(_) => None,
            CrsRef::Compound(parts) => {
                let known: Vec<&CrsRef> = parts.iter().filter(|p| !matches!(p, CrsRef::Unresolved(_))).collect();
                if known.len() < 2 {
                    return known.first()?.projjson();
                }
                let mut schema = serde_json::Value::Null;
                let mut components = Vec::new();
                for part in known {
                    let mut json = part.projjson()?;
                    let object = json.as_object_mut()?;
                    schema = object.remove("$schema").unwrap_or(schema);
                    // The components of a compound CRS are single CRSs.
                    match object.remove("components") {
                        Some(serde_json::Value::Array(nested)) => components.extend(nested),
                        _ => components.push(json),
                    }
                }
                let names = components.iter().map(|c| c["name"].as_str()).collect::<Option<Vec<_>>>()?;
                Some(serde_json::json!({
                    "$schema": schema,
                    "type": "CompoundCRS",
                    "name": names.join(" + "),
                    "components": components,
                }))
            }
        }
    }
}

/// A parsed srsName, keeping the original string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SrsName {
    pub raw: String,
    pub form: SrsNameForm,
    pub crs: Option<CrsRef>,
}

impl SrsName {
    /// Recognise the form and the CRS. Prefixes and authorities are matched
    /// case-insensitively and surrounding whitespace is ignored; authorities
    /// are normalised to upper case (`epsg` → `EPSG`). Anything unrecognised is
    /// [`SrsNameForm::Unknown`] with no CRS.
    pub fn parse(raw: &str) -> Self {
        let (form, crs) = parse_form(raw.trim()).unwrap_or((SrsNameForm::Unknown, None));
        let form = if crs.is_none() { SrsNameForm::Unknown } else { form };
        SrsName { raw: raw.to_string(), form, crs }
    }
}

fn parse_form(s: &str) -> Option<(SrsNameForm, Option<CrsRef>)> {
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = strip_prefix_ci(s, "urn:ogc:def:crs,") {
        // `crs:EPSG::4269,crs:EPSG::5713`
        let parts = rest.split(',').map(|p| part(strip_prefix_ci(p.trim(), "crs:").unwrap_or(p.trim())));
        return Some((SrsNameForm::CompoundUrn, compound(parts.collect::<Option<_>>()?)));
    }
    if let Some(rest) = strip_prefix_ci(s, "urn:ogc:def:crs:") {
        return Some((SrsNameForm::OgcUrn, Some(ogc_urn_code(rest)?)));
    }
    if let Some(rest) = strip_prefix_ci(s, "urn:x-ogc:def:crs:") {
        return Some((SrsNameForm::ExperimentalUrn, Some(ogc_urn_code(rest)?)));
    }
    if let Some(rest) = strip_prefix_ci(s, "urn:adv:crs:") {
        // A `*` joins a horizontal and a vertical CRS.
        return Some((SrsNameForm::AdvUrn, compound(rest.split('*').map(part).collect::<Option<_>>()?)));
    }
    if let Some(rest) = strip_prefix_ci(s, "urn:epsg:") {
        // `urn:EPSG:geographicCRS:4326`
        let code = rest.rsplit(':').next()?;
        return Some((SrsNameForm::Wfs11Urn, Some(CrsRef::epsg(epsg_code(code)?))));
    }
    if let Some(rest) = strip_http(s) {
        return parse_http(rest);
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        // A bare code: treated as the short form.
        return Some((SrsNameForm::Short, Some(CrsRef::epsg(epsg_code(s)?))));
    }
    if let Some(crs) = named(s) {
        return Some((SrsNameForm::Short, Some(crs)));
    }
    let (authority, code) = s.split_once(':')?;
    // `EPSG:2180`; `EPSG::2180` is tolerated.
    let code = code.trim_start_matches(':');
    if authority.eq_ignore_ascii_case("EPSG") {
        return Some((SrsNameForm::Short, Some(epsg_codes(code)?)));
    }
    None
}

/// `2180`, or PROJ's compound spelling `25832+7837` (also `25832+EPSG:7837`,
/// `2180+PL-KRON86-NH`), which [`CrsRef::authority_code`] writes.
fn epsg_codes(codes: &str) -> Option<CrsRef> {
    let mut codes = codes.split('+').map(str::trim);
    let first = CrsRef::epsg(epsg_code(codes.next()?)?);
    let rest = codes.map(|code| epsg_code(code).map_or_else(|| part(code), |code| Some(CrsRef::epsg(code))));
    compound(std::iter::once(Some(first)).chain(rest).collect::<Option<_>>()?)
}

/// One part of a compound CRS: an srsName of its own, `AUTH:[VERSION]:CODE`
/// or a name, else [`CrsRef::Unresolved`]. An empty part is malformed.
fn part(text: &str) -> Option<CrsRef> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let crs = parse_form(text).and_then(|(_, crs)| crs).or_else(|| ogc_urn_code(text));
    Some(crs.unwrap_or_else(|| CrsRef::Unresolved(text.to_string())))
}

/// The CRS of these parts: one part is itself, several a compound. Without a
/// part that names a known CRS there is none.
fn compound(parts: Vec<CrsRef>) -> Option<CrsRef> {
    if parts.iter().all(|p| matches!(p, CrsRef::Unresolved(_))) {
        return None;
    }
    match <[CrsRef; 1]>::try_from(parts) {
        Ok([single]) => Some(single),
        Err(parts) => Some(CrsRef::Compound(parts)),
    }
}

/// `www.opengis.net/…` after `http://` or `https://`.
fn parse_http(rest: &str) -> Option<(SrsNameForm, Option<CrsRef>)> {
    let rest = strip_prefix_ci(rest, "www.opengis.net/")?;
    if let Some(code) = strip_prefix_ci(rest, "gml/srs/epsg.xml#") {
        return Some((SrsNameForm::LegacyUrl, Some(CrsRef::epsg(epsg_code(code)?))));
    }
    if let Some(query) = strip_prefix_ci(rest, "def/crs-compound?") {
        // `1=<uri>&2=<uri>`, in key order.
        let mut parts: Vec<(u32, CrsRef)> = Vec::new();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            parts.push((key.trim().parse().ok()?, part(value)?));
        }
        parts.sort_by_key(|(key, _)| *key);
        let parts = parts.into_iter().map(|(_, crs)| crs).collect();
        return Some((SrsNameForm::CompoundUri, compound(parts)));
    }
    if let Some(query) = strip_prefix_ci(rest, "def/crs?") {
        let mut authority = None;
        let mut code = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            if key.eq_ignore_ascii_case("authority") {
                authority = Some(value);
            } else if key.eq_ignore_ascii_case("code") {
                code = Some(value);
            }
        }
        return Some((SrsNameForm::HttpUriKvp, Some(code_ref(authority?, code?)?)));
    }
    if let Some(path) = strip_prefix_ci(rest, "def/crs/") {
        // `AUTH/VERSION/CODE`, maybe with a trailing slash.
        let segments: Vec<&str> = path.trim_end_matches('/').split('/').collect();
        let [authority, _version, code] = segments.as_slice() else {
            return None;
        };
        return Some((SrsNameForm::HttpUri, Some(code_ref(authority, code)?)));
    }
    None
}

/// The part of an OGC URN after `crs:`: `AUTH:[VERSION]:CODE` or `AUTH:CODE`.
fn ogc_urn_code(rest: &str) -> Option<CrsRef> {
    let (authority, tail) = rest.split_once(':')?;
    // The version, when present, sits between the authority and the code.
    let code = tail.rsplit(':').next()?;
    code_ref(authority, code)
}

fn code_ref(authority: &str, code: &str) -> Option<CrsRef> {
    let authority = authority.trim().to_ascii_uppercase();
    let code = code.trim();
    if authority.is_empty() || code.is_empty() {
        return None;
    }
    if authority == "EPSG" {
        return Some(CrsRef::epsg(epsg_code(code)?));
    }
    Some(CrsRef::Code { authority, code: code.to_string() })
}

/// An EPSG code is a positive integer; leading zeros are dropped.
fn epsg_code(code: &str) -> Option<String> {
    let code = code.trim();
    if code.is_empty() || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u64 = code.parse().ok()?;
    Some(number.to_string())
}

/// A CRS given by name: ours ([`NAMES`]) first, then EPSG's aliases
/// (`PL-1992` → EPSG:2180). Case is ignored.
fn named(name: &str) -> Option<CrsRef> {
    let name = name.trim();
    if let Some((_, authority, code)) = NAMES.iter().find(|(known, _, _)| known.eq_ignore_ascii_case(name)) {
        return Some(CrsRef::Code { authority: (*authority).into(), code: (*code).into() });
    }
    xeibe_crs::alias(name).map(|code| CrsRef::epsg(code.to_string()))
}

/// Names seen in real data that EPSG's aliases lack, or spell otherwise.
const NAMES: &[(&str, &str, &str)] = &[
    // The AdV CRS register (German surveying authorities; ALKIS/NAS, XPlanung).
    ("ETRS89_UTM31", "EPSG", "25831"),
    ("ETRS89_UTM32", "EPSG", "25832"),
    ("ETRS89_UTM33", "EPSG", "25833"),
    ("ETRS89_Lat-Lon", "EPSG", "4258"),
    ("DE_DHDN_3GK2", "EPSG", "31466"),
    ("DE_DHDN_3GK3", "EPSG", "31467"),
    ("DE_DHDN_3GK4", "EPSG", "31468"),
    ("DE_DHDN_3GK5", "EPSG", "31469"),
    ("DE_DHHN92_NH", "EPSG", "5783"),
    ("DE_DHHN2016_NH", "EPSG", "7837"),
    // GUGiK's 3D building models; EPSG's Polish alias is `PL-KRON86`.
    ("PL-KRON86-NH", "EPSG", "9650"),
    ("osgb:BNG", "EPSG", "27700"),
    ("CRS:84", "OGC", "CRS84"),
    ("OGC:CRS84", "OGC", "CRS84"),
    ("CRS:83", "OGC", "CRS83"),
    ("CRS:27", "OGC", "CRS27"),
];

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

fn strip_http(s: &str) -> Option<&str> {
    strip_prefix_ci(s, "http://").or_else(|| strip_prefix_ci(s, "https://"))
}
