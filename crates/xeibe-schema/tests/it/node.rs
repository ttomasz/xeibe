//! The path tree's nodes: what they record and how they merge
//! (`docs/schema-inference.md` §2.2, §2.5).

use xeibe_core::QName;
use xeibe_schema::node::{NameShape, Shape};
use xeibe_schema::{ElementNode, Merge};
use xeibe_testkit::gml::APP;

use crate::support::scan;

fn node(document: &str, layer: &str) -> ElementNode {
    scan(document)
        .layer(layer)
        .expect("the layer was found")
        .1
        .root
        .clone()
}

fn child<'a>(node: &'a ElementNode, local: &str) -> &'a ElementNode {
    node.children
        .get(&QName::new(Some(APP), local))
        .unwrap_or_else(|| {
            panic!(
                "no child {local:?}; there are {:?}",
                node.children.keys().collect::<Vec<_>>()
            )
        })
}

#[test]
fn name_shapes_tell_objects_from_properties() {
    // INSPIRE wraps data types in UpperCamel elements inside lowerCamel
    // properties; this is what `collapse_type_wrappers` keys on.
    assert_eq!(NameShape::of("AD_IdentyfikatorIIP"), NameShape::UpperCamel);
    assert_eq!(NameShape::of("Building"), NameShape::UpperCamel);
    assert_eq!(NameShape::of("lokalnyId"), NameShape::LowerCamel);
    assert_eq!(NameShape::of("area"), NameShape::LowerCamel);
    assert_eq!(NameShape::of("TERYTGminy"), NameShape::UpperCamel);
    assert_eq!(NameShape::of("_odd"), NameShape::Other);
}

#[test]
fn nodes_record_how_often_an_element_occurred() {
    let document = xeibe_testkit::gml::gml32_collection(&[
        &xeibe_testkit::gml::feature(
            "Parcel",
            "p1",
            "<app:area>1</app:area><app:tag>a</app:tag><app:tag>b</app:tag>",
        ),
        &xeibe_testkit::gml::feature("Parcel", "p2", "<app:area>2</app:area>"),
    ]);
    let root = node(&document, "Parcel");
    assert_eq!(root.instances, 2);

    let area = child(&root, "area");
    assert_eq!(area.instances, 2);
    assert_eq!(area.parents_with, 2);
    assert_eq!(area.max_occurs, 1);

    let tag = child(&root, "tag");
    assert_eq!(tag.instances, 2, "two in the first feature");
    assert_eq!(tag.parents_with, 1, "present in one of the two features");
    assert_eq!(tag.max_occurs, 2, "the list rule keys on this");
    assert!(
        tag.first_multi.is_some(),
        "--explain shows where a column became a list"
    );
}

#[test]
fn shapes_describe_the_content_of_an_element() {
    let document = xeibe_testkit::gml::gml32_collection(&[&xeibe_testkit::gml::feature(
        "Parcel",
        "p1",
        concat!(
            "<app:area uom=\"m2\">12</app:area>",
            "<app:name>A</app:name>",
            "<app:owner><app:name>B</app:name></app:owner>",
            "<app:note>text <b>bold</b></app:note>",
            "<app:empty/>",
            "<app:ref xlink:href=\"#other\"/>",
        ),
    )]);
    let root = node(&document, "Parcel");
    assert_eq!(child(&root, "name").shape(), Shape::TextOnly);
    assert_eq!(child(&root, "area").shape(), Shape::TextAndAttributes);
    assert_eq!(child(&root, "owner").shape(), Shape::ElementsOnly);
    assert_eq!(child(&root, "note").shape(), Shape::Mixed);
    assert_eq!(child(&root, "empty").shape(), Shape::Empty);
    assert_eq!(child(&root, "ref").shape(), Shape::ByReferenceOnly);
    assert_eq!(child(&root, "ref").by_reference, 1);
}

#[test]
fn a_geometry_property_is_a_leaf() {
    // The scan must not descend into `exterior/LinearRing/posList`.
    let document = xeibe_testkit::gml::gml32_collection(&[&xeibe_testkit::gml::feature(
        "Parcel",
        "p1",
        concat!(
            "<app:geom><gml:Point srsName=\"EPSG:2180\"><gml:pos>1 2</gml:pos>",
            "</gml:Point></app:geom>"
        ),
    )]);
    let root = node(&document, "Parcel");
    let geom = child(&root, "geom");
    assert_eq!(geom.shape(), Shape::Geometry);
    assert!(geom.geometry.is_some());
    assert!(geom.children.is_empty(), "a geometry node has no children");
}

#[test]
fn type_wrappers_are_recognised() {
    let document = xeibe_testkit::gml::gml32_collection(&[&xeibe_testkit::gml::feature(
        "Parcel",
        "p1",
        concat!(
            "<app:idIIP><app:AD_IdentyfikatorIIP><app:lokalnyId>x</app:lokalnyId>",
            "</app:AD_IdentyfikatorIIP></app:idIIP>",
            "<app:owner><app:name>B</app:name></app:owner>"
        ),
    )]);
    let root = node(&document, "Parcel");
    assert!(
        child(&root, "idIIP").is_type_wrapper(),
        "one UpperCamel child, no text, no attributes"
    );
    assert!(
        !child(&root, "owner").is_type_wrapper(),
        "a lowerCamel child is data, not a wrapper"
    );
}

#[test]
fn merging_nodes_adds_counts_and_keeps_the_widest_shape() {
    let one = xeibe_testkit::gml::gml32_collection(&[&xeibe_testkit::gml::feature(
        "Parcel",
        "p1",
        "<app:area>1</app:area>",
    )]);
    let two = xeibe_testkit::gml::gml32_collection(&[&xeibe_testkit::gml::feature(
        "Parcel",
        "p2",
        "<app:area>2</app:area><app:area>3</app:area><app:extra>x</app:extra>",
    )]);

    let mut merged = node(&one, "Parcel");
    merged.merge(node(&two, "Parcel"));
    assert_eq!(merged.instances, 2);
    let area = child(&merged, "area");
    assert_eq!(area.instances, 3);
    assert_eq!(area.max_occurs, 2, "the maximum, not the sum");
    assert_eq!(child(&merged, "extra").instances, 1);
    assert_eq!(
        merged.children.keys().map(|k| k.local.to_string()).collect::<Vec<_>>(),
        ["area", "extra"],
        "children keep their first-seen order"
    );
}
