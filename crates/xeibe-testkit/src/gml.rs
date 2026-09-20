//! Builders for synthetic GML documents.
//!
//! Tests write the interesting part (a feature body, a geometry) and let these
//! wrap it in a well-formed document with the usual namespace declarations.
//! The application namespace is [`APP`], prefix `app`.

pub const GML: &str = "http://www.opengis.net/gml";
pub const GML_32: &str = "http://www.opengis.net/gml/3.2";
pub const WFS: &str = "http://www.opengis.net/wfs";
pub const WFS_20: &str = "http://www.opengis.net/wfs/2.0";
pub const XLINK: &str = "http://www.w3.org/1999/xlink";
pub const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
pub const FME: &str = "http://www.safe.com/gml/fme";
/// The application namespace used by synthetic documents.
pub const APP: &str = "http://example.com/app";

fn decls(gml_ns: &str, extra: &str) -> String {
    format!(
        r#"xmlns:gml="{gml_ns}" xmlns:app="{APP}" xmlns:xlink="{XLINK}" xmlns:xsi="{XSI}"{extra}"#
    )
}

/// `<app:Type gml:id="id">body</app:Type>`; GML 3 puts the id in the GML namespace.
pub fn feature(type_name: &str, id: &str, body: &str) -> String {
    format!(r#"<app:{type_name} gml:id="{id}">{body}</app:{type_name}>"#)
}

/// A GML 3.2 collection with one `gml:featureMember` per member.
pub fn gml32_collection(members: &[&str]) -> String {
    collection(GML_32, "gml:featureMember", members, "", "")
}

/// A GML 3.2 collection with all members inside one `gml:featureMembers`.
pub fn gml32_collection_members(members: &[&str]) -> String {
    let inner: String = members.concat();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <gml:FeatureCollection {}>\n<gml:featureMembers>{inner}</gml:featureMembers>\n\
         </gml:FeatureCollection>\n",
        decls(GML_32, "")
    )
}

/// A GML 3.1 collection (GML 2 and 3.1 share the namespace).
pub fn gml31_collection(members: &[&str]) -> String {
    collection(GML, "gml:featureMember", members, "", "")
}

/// A GML 2 collection, with a GML 2 `xsi:schemaLocation`.
pub fn gml2_collection(members: &[&str]) -> String {
    collection(
        GML,
        "gml:featureMember",
        members,
        r#" xsi:schemaLocation="http://www.opengis.net/gml http://schemas.opengis.net/gml/2.1.2/feature.xsd""#,
        "",
    )
}

/// A WFS 2.0 response: `wfs:member` wrappers and the response attributes.
///
/// `attributes` is written on the collection element, e.g.
/// `numberMatched="unknown" numberReturned="2"`.
pub fn wfs20_collection(attributes: &str, members: &[&str]) -> String {
    let inner: String = members
        .iter()
        .map(|m| format!("<wfs:member>{m}</wfs:member>"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <wfs:FeatureCollection xmlns:wfs=\"{WFS_20}\" {} {attributes}>\n{inner}\n\
         </wfs:FeatureCollection>\n",
        decls(GML_32, "")
    )
}

/// A WFS 1.1 response: `gml:featureMember` wrappers and `numberOfFeatures`.
pub fn wfs11_collection(attributes: &str, members: &[&str]) -> String {
    let inner: String = members
        .iter()
        .map(|m| format!("<gml:featureMember>{m}</gml:featureMember>"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <wfs:FeatureCollection xmlns:wfs=\"{WFS}\" {} {attributes}>\n{inner}\n\
         </wfs:FeatureCollection>\n",
        decls(GML, "")
    )
}

/// A GML 3.2 collection whose root declares the FME namespace (an axis-order
/// quirk, see `docs/geometry.md`).
pub fn fme_collection(members: &[&str]) -> String {
    collection(
        GML_32,
        "gml:featureMember",
        members,
        &format!(r#" xmlns:fme="{FME}""#),
        "",
    )
}

/// The general form: any GML namespace, member element and extra root attributes.
pub fn collection(
    gml_ns: &str,
    member_element: &str,
    members: &[&str],
    root_attributes: &str,
    boundedby: &str,
) -> String {
    let inner: String = members
        .iter()
        .map(|m| format!("<{member_element}>{m}</{member_element}>"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <gml:FeatureCollection {}{root_attributes}>\n{boundedby}{inner}\n\
         </gml:FeatureCollection>\n",
        decls(gml_ns, "")
    )
}

/// A document whose root element is a feature (a `GetFeatureById` response).
pub fn single_feature_document(type_name: &str, id: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <app:{type_name} {} gml:id=\"{id}\">{body}</app:{type_name}>\n",
        decls(GML_32, "")
    )
}

/// Wrap a geometry snippet so it can be parsed on its own: the `gml` and
/// `xlink` prefixes are declared, in the GML 3.2 namespace.
pub fn geometry_document(geometry: &str) -> String {
    format!(r#"<app:geom {}>{geometry}</app:geom>"#, decls(GML_32, ""))
}

/// The same in the shared GML 2/3.1 namespace.
pub fn geometry_document_gml31(geometry: &str) -> String {
    format!(r#"<app:geom {}>{geometry}</app:geom>"#, decls(GML, ""))
}
