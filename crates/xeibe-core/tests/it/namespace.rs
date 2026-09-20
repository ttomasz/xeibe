//! `QName` and `NamespaceContext`: elements are matched by namespace URI, never
//! by prefix (`docs/architecture.md`).

use xeibe_core::{NamespaceContext, QName, ns};

#[test]
fn qname_keeps_namespace_and_local_name() {
    let name = QName::new(Some(ns::GML_32), "Point");
    assert_eq!(&*name.local, "Point");
    assert_eq!(name.ns.as_deref(), Some(ns::GML_32));
    assert_eq!(name.to_clark(), "{http://www.opengis.net/gml/3.2}Point");

    let no_namespace = QName::new(None, "Point");
    assert_eq!(no_namespace.to_clark(), "Point");
    assert_ne!(no_namespace, name);
}

#[test]
fn qname_recognises_both_gml_namespaces() {
    // GML 2 and 3.1 share one namespace, 3.2 has its own.
    assert!(QName::new(Some(ns::GML), "Point").is_gml());
    assert!(QName::new(Some(ns::GML_32), "Point").is_gml());
    assert!(!QName::new(Some(ns::WFS_20), "member").is_gml());
    assert!(!QName::new(Some("http://example.com/app"), "Point").is_gml());
    assert!(!QName::new(None, "Point").is_gml());
    // A namespace that merely starts with the GML URI is a different namespace.
    assert!(!QName::new(Some("http://www.opengis.net/gml/3.3/ce"), "SimplePolygon").is_gml());
}

#[test]
fn qnames_of_the_same_name_are_equal_and_hash_alike() {
    use std::collections::HashSet;

    let a = QName::new(Some(ns::GML_32), "Point");
    let b = QName::new(Some(ns::GML_32), "Point");
    assert_eq!(a, b);
    let mut set = HashSet::new();
    set.insert(a);
    assert!(set.contains(&b));
}

#[test]
fn context_resolves_prefixes_and_the_default_namespace() {
    let mut context = NamespaceContext::new();
    context.declare(Some("gml"), ns::GML_32);
    context.declare(None, "http://example.com/app");

    assert_eq!(context.resolve_prefix(Some("gml")), Some(ns::GML_32));
    assert_eq!(context.resolve_prefix(None), Some("http://example.com/app"));
    assert_eq!(context.resolve_prefix(Some("missing")), None);

    assert_eq!(
        context.resolve("gml:Point"),
        Some(QName::new(Some(ns::GML_32), "Point"))
    );
    // An unprefixed name takes the default namespace.
    assert_eq!(
        context.resolve("Feature"),
        Some(QName::new(Some("http://example.com/app"), "Feature"))
    );
    assert_eq!(context.resolve("nope:Point"), None);
}

#[test]
fn a_later_declaration_of_a_prefix_wins() {
    let mut context = NamespaceContext::new();
    context.declare(Some("app"), "http://example.com/one");
    context.declare(Some("app"), "http://example.com/two");
    assert_eq!(
        context.resolve_prefix(Some("app")),
        Some("http://example.com/two")
    );
}

#[test]
fn context_serializes_back_to_xmlns_attributes() {
    let mut context = NamespaceContext::new();
    context.declare(Some("gml"), ns::GML_32);
    context.declare(None, "http://example.com/app");
    let attributes = context.to_xmlns_attributes();
    assert!(
        attributes.contains(&format!(r#"xmlns:gml="{}""#, ns::GML_32)),
        "{attributes:?}"
    );
    assert!(
        attributes.contains(r#"xmlns="http://example.com/app""#),
        "{attributes:?}"
    );
}

#[test]
fn a_context_with_nothing_declared_resolves_nothing() {
    let context = NamespaceContext::new();
    assert_eq!(context.resolve_prefix(Some("gml")), None);
    assert_eq!(context.resolve_prefix(None), None);
    // An unprefixed name with no default namespace has no namespace.
    assert_eq!(context.resolve("Point"), Some(QName::new(None, "Point")));
    assert_eq!(context.to_xmlns_attributes(), "");
}
