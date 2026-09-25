//! Feature properties read end to end: the standard GML properties, XLink
//! attributes, and subtrees beyond the `Limits` (`docs/type-mapping.md`,
//! "Special GML/XLink properties"; `docs/schema-inference.md` §3.4–§3.5).

use arrow_array::Array;
use arrow_array::cast::AsArray;
use xeibe_arrow::ReadOptions;
use xeibe_schema::rules::meta;
use xeibe_testkit::gml;

use crate::support::{Read, read_document, read_with};

fn parcel(id: &str, body: &str) -> String {
    gml::feature("Parcel", id, body)
}

fn s(text: &str) -> Option<String> {
    Some(text.to_string())
}

fn path(read: &Read, column: &str) -> String {
    read.field(column).metadata().get(meta::PATH).cloned().unwrap_or_default()
}

/// `path → text` entries of a map column, per row.
fn maps(read: &Read, column: &str) -> Vec<Option<Vec<(String, String)>>> {
    let mut rows = Vec::new();
    for batch in &read.batches {
        let maps = batch.column_by_name(column).expect("a map column").as_map();
        for i in 0..maps.len() {
            rows.push((!maps.is_null(i)).then(|| {
                let entries = maps.value(i);
                let keys = entries.column(0).as_string_view();
                let values = entries.column(1).as_string_view();
                (0..entries.len()).map(|j| (keys.value(j).to_string(), values.value(j).to_string())).collect()
            }));
        }
    }
    rows
}

#[test]
fn the_standard_gml_properties_follow_the_generic_rules() {
    let document = gml::gml32_collection(&[
        &parcel(
            "p1",
            concat!(
                "<gml:description>Opis</gml:description>",
                r#"<gml:descriptionReference xlink:href="http://example.com/d"/>"#,
                r#"<gml:identifier codeSpace="http://example.com/ids">ID1</gml:identifier>"#,
                r#"<gml:name codeSpace="PRNG">Warszawa</gml:name><gml:name>Warsaw</gml:name>"#,
            ),
        ),
        &parcel(
            "p2",
            concat!(
                r#"<gml:identifier codeSpace="http://example.com/ids">ID2</gml:identifier>"#,
                "<gml:name>Kraków</gml:name>",
            ),
        ),
    ]);
    let read = read_document(&document, "Parcel");
    assert_eq!(
        read.column_names(),
        ["@id", "description", "descriptionReference", "identifier", "name", "@codeSpace"]
    );

    assert_eq!(read.strings("description"), [s("Opis"), None]);
    // A property given only by reference holds its href.
    assert_eq!(path(&read, "descriptionReference"), "descriptionReference/@href");
    assert_eq!(read.strings("descriptionReference"), [s("http://example.com/d"), None]);
    // A codeSpace that is the same everywhere moves into field metadata.
    assert_eq!(read.strings("identifier"), [s("ID1"), s("ID2")]);
    assert_eq!(
        read.field("identifier").metadata().get(&format!("{}codeSpace", meta::ATTR_PREFIX)).map(String::as_str),
        Some("http://example.com/ids")
    );
    // `gml:name` repeats: a list, with its codeSpace aligned to it.
    assert_eq!(path(&read, "name"), "name[]");
    assert_eq!(path(&read, "@codeSpace"), "name[]/@codeSpace");
    assert_eq!(read.string_lists("name"), [Some(vec![s("Warszawa"), s("Warsaw")]), Some(vec![s("Kraków")])]);
    assert_eq!(read.string_lists("@codeSpace"), [Some(vec![s("PRNG"), None]), Some(vec![None])]);
}

#[test]
fn metadata_property_is_kept_as_raw_xml() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        "<gml:metaDataProperty><app:Meta><app:source>survey</app:source></app:Meta></gml:metaDataProperty><app:n>1</app:n>",
    )]);
    let read = read_document(&document, "Parcel");
    assert_eq!(path(&read, "metaDataProperty"), "metaDataProperty");
    let xml = read.strings("metaDataProperty")[0].clone().expect("raw XML");
    assert!(xml.contains("<app:source>survey</app:source>"), "{xml}");
}

#[test]
fn control_attributes_are_dropped_and_xlink_title_needs_full_mode() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        r##"<app:ref xlink:type="simple" xlink:href="#a" xlink:title="A" owns="true"/>"##,
    )]);
    let read = read_document(&document, "Parcel");
    assert_eq!(read.column_names(), ["@id", "ref"], "xlink:type, owns and xlink:title are not data");
    assert_eq!(read.strings("ref"), [s("a")]);

    let mut full = ReadOptions::default();
    full.inference.gml.xlink = xeibe_schema::options::XlinkMode::Full;
    let read = read_with(&document, "Parcel", None, &full);
    assert_eq!(read.column_names(), ["@id", "ref", "@title"]);
    assert_eq!(path(&read, "@title"), "ref/@title");
    assert_eq!(read.strings("@title"), [s("A")]);
}

#[test]
fn a_property_with_an_href_and_inline_content_keeps_both() {
    // The link is authoritative and the content a cached copy (07-036
    // §7.2.3.4); links are not resolved, so the content is read and the href
    // kept as a column of its own.
    let document = gml::gml32_collection(&[
        &parcel("p1", r##"<app:owner xlink:href="#o1"/>"##),
        &parcel(
            "p2",
            r##"<app:owner xlink:href="#o2"><app:Person><app:nazwa>Jan</app:nazwa></app:Person></app:owner>"##,
        ),
        &parcel("p3", "<app:owner><app:Person><app:nazwa>Ewa</app:nazwa></app:Person></app:owner>"),
    ]);
    let read = read_document(&document, "Parcel");
    assert_eq!(read.column_names(), ["@id", "@href", "nazwa"]);
    assert_eq!(path(&read, "@href"), "owner/@href");
    assert_eq!(path(&read, "nazwa"), "owner/*/nazwa");
    assert_eq!(read.strings("@href"), [s("o1"), s("o2"), None]);
    assert_eq!(read.strings("nazwa"), [None, s("Jan"), s("Ewa")]);
}

#[test]
fn subtrees_beyond_the_limits_become_map_columns() {
    let document = gml::gml32_collection(&[&parcel(
        "p1",
        concat!(
            "<app:deep><app:a><app:b>1</app:b></app:a></app:deep>",
            "<app:wide><app:x>1</app:x><app:y>2</app:y><app:z>3</app:z></app:wide>",
        ),
    )]);
    let mut options = ReadOptions::default();
    options.inference.limits.max_depth = 2;
    options.inference.limits.max_children = 2;
    let read = read_with(&document, "Parcel", None, &options);
    // Elements down to depth 2 are tracked (`deep/a`); what lies deeper
    // becomes one map at that depth.
    assert!(matches!(read.data_type("a"), arrow_schema::DataType::Map(..)), "{:?}", read.data_type("a"));
    assert_eq!(path(&read, "a"), "deep/a");
    assert_eq!(maps(&read, "a"), [Some(vec![("b".to_string(), "1".to_string())])]);
    // More distinct child names than `max_children`: the element is a map.
    assert_eq!(
        maps(&read, "wide"),
        [Some(vec![("x".to_string(), "1".to_string()), ("y".to_string(), "2".to_string()), ("z".to_string(), "3".to_string())])]
    );
}

#[test]
fn a_gml_2_fid_is_the_id_column() {
    // `gml:id` (3.x) and `fid` (2) are both the feature's `@id`.
    let document = gml::gml2_collection(&[
        r#"<app:Parcel fid="p1"><app:n>1</app:n></app:Parcel>"#,
        r#"<app:Parcel fid="p2"><app:n>2</app:n></app:Parcel>"#,
    ]);
    let read = read_document(&document, "Parcel");
    assert_eq!(read.column_names(), ["@id", "n"]);
    assert_eq!(path(&read, "@id"), "@fid");
    assert_eq!(read.strings("@id"), [s("p1"), s("p2")]);
}

#[test]
fn utf_16_input_is_read_like_utf_8() {
    let document = gml::gml32_collection(&[&parcel("p1", "<app:nazwa>Łódź</app:nazwa>")])
        .replacen("encoding=\"UTF-8\"", "encoding=\"UTF-16\"", 1);
    let mut bytes = vec![0xFF, 0xFE];
    for unit in document.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let sources = xeibe_core::Sources::from(xeibe_core::Source::reader("utf16.gml", Box::new(std::io::Cursor::new(bytes))));
    let reader = xeibe_arrow::read(sources, "Parcel", None, &ReadOptions::default()).expect("a reader");
    let read = crate::support::collect(reader);
    assert_eq!(read.strings("nazwa"), [s("Łódź")]);
}
