//! KVP request building (see the table in `docs/wfs.md` "Request encoding").

use url::Url;

use crate::WfsVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultType {
    Results,
    Hits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct GetFeature {
    pub version: WfsVersion,
    pub type_name: String,
    /// `(prefix, uri)` for `NAMESPACES` (2.0) / `NAMESPACE` (1.1).
    pub namespace: Option<(String, String)>,
    pub srs_name: Option<String>,
    pub output_format: Option<String>,
    /// In the axis order assumed for `srs_name`; optional CRS as 5th value.
    pub bbox: Option<([f64; 4], Option<String>)>,
    /// FES/OGC filter XML, passed through.
    pub filter: Option<String>,
    pub property_names: Vec<String>,
    pub sort_by: Vec<(String, SortOrder)>,
    pub result_type: ResultType,
    /// `COUNT` (2.0) / `MAXFEATURES` (1.x).
    pub count: Option<u64>,
    /// `STARTINDEX` (2.0, or vendor on 1.x).
    pub start_index: Option<u64>,
    /// Vendor parameters, e.g. `CQL_FILTER`.
    pub vendor: Vec<(String, String)>,
}

impl GetFeature {
    /// `base` with the request's KVP parameters. Parameters already in `base`
    /// (`map=…`) are kept unless the request sets them itself.
    pub fn to_url(&self, base: &Url) -> crate::Result<Url> {
        let v2 = self.version.is_2();
        let v1_0 = self.version == WfsVersion::V1_0_0;
        let mut params: Vec<(&str, String)> = vec![
            ("SERVICE", "WFS".into()),
            ("VERSION", self.version.as_str().into()),
            ("REQUEST", "GetFeature".into()),
            (
                if v2 { "TYPENAMES" } else { "TYPENAME" },
                self.type_name.clone(),
            ),
        ];
        if let Some((prefix, uri)) = &self.namespace {
            if v2 {
                params.push(("NAMESPACES", format!("xmlns({prefix},{uri})")));
            } else if !v1_0 {
                params.push(("NAMESPACE", format!("xmlns({prefix}={uri})")));
            }
        }
        if let Some(srs_name) = &self.srs_name {
            params.push(("SRSNAME", srs_name.clone()));
        }
        if let Some(format) = &self.output_format {
            params.push(("OUTPUTFORMAT", format.clone()));
        }
        if let Some((bbox, crs)) = &self.bbox {
            let mut value = bbox.map(|v| v.to_string()).join(",");
            if let Some(crs) = crs {
                value.push(',');
                value.push_str(crs);
            }
            params.push(("BBOX", value));
        }
        if let Some(filter) = &self.filter {
            params.push(("FILTER", filter.clone()));
        }
        if !self.property_names.is_empty() {
            params.push(("PROPERTYNAME", self.property_names.join(",")));
        }
        // 1.0 has no SORTBY and no RESULTTYPE.
        if !self.sort_by.is_empty() && !v1_0 {
            let value = self
                .sort_by
                .iter()
                .map(|(property, order)| {
                    let order = match (order, v2) {
                        (SortOrder::Asc, true) => "ASC",
                        (SortOrder::Desc, true) => "DESC",
                        (SortOrder::Asc, false) => "A",
                        (SortOrder::Desc, false) => "D",
                    };
                    format!("{property} {order}")
                })
                .collect::<Vec<_>>()
                .join(",");
            params.push(("SORTBY", value));
        }
        if self.result_type == ResultType::Hits && !v1_0 {
            params.push(("RESULTTYPE", "hits".into()));
        }
        if let Some(count) = self.count {
            params.push((if v2 { "COUNT" } else { "MAXFEATURES" }, count.to_string()));
        }
        if let Some(start_index) = self.start_index {
            params.push(("STARTINDEX", start_index.to_string()));
        }
        for (key, value) in &self.vendor {
            params.push((key, value.clone()));
        }
        Ok(with_params(base, &params))
    }
}

pub fn get_capabilities_url(base: &Url, version: Option<WfsVersion>) -> crate::Result<Url> {
    let mut params = vec![("SERVICE", "WFS".to_string())];
    // Without a version the server answers with its highest one.
    if let Some(version) = version {
        params.push(("VERSION", version.as_str().to_string()));
    }
    params.push(("REQUEST", "GetCapabilities".to_string()));
    Ok(with_params(base, &params))
}

/// `base` with `params` appended. A parameter of `base` with the same name
/// (in any case: KVP names are case-insensitive) is dropped, and so are
/// the parameters that belong to another request.
fn with_params(base: &Url, params: &[(&str, String)]) -> Url {
    const REQUEST_SPECIFIC: &[&str] = &[
        "ACCEPTVERSIONS",
        "SECTIONS",
        "TYPENAME",
        "TYPENAMES",
        "NAMESPACE",
        "NAMESPACES",
        "COUNT",
        "MAXFEATURES",
        "STARTINDEX",
        "RESULTTYPE",
    ];
    let kept: Vec<(String, String)> = base
        .query_pairs()
        .filter(|(key, _)| {
            !params.iter().any(|(k, _)| k.eq_ignore_ascii_case(key))
                && !REQUEST_SPECIFIC.iter().any(|k| k.eq_ignore_ascii_case(key))
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let mut url = base.clone();
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in &kept {
            query.append_pair(key, value);
        }
        for (key, value) in params {
            query.append_pair(key, value);
        }
    }
    url
}
