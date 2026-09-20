//! `GmlReader`: namespace-aware pull parsing of one chunk or document, and the
//! XML rules of `docs/architecture.md` ("Input handling": no DTDs, no external
//! entities).
//!
//! The reader is always given a [`NamespaceContext`] with the declarations that
//! are in scope outside the buffer, because chunks are parsed on their own.

use xeibe_core::reader::XmlEvent;
use xeibe_core::{Error, QName, ns};
use xeibe_testkit::gml::APP;

use crate::support::{Event, app_context, events, reader};

#[test]
fn resolves_names_against_the_inherited_context() {
    // Neither prefix is declared in the buffer: both come from the context.
    let fragment = r#"<app:Parcel gml:id="p1"><app:area>12</app:area></app:Parcel>"#;
    assert_eq!(
        events(fragment, &app_context(ns::GML_32)),
        vec![
            Event::start("Parcel"),
            Event::start("area"),
            Event::text("12"),
            Event::end("area"),
            Event::end("Parcel"),
        ]
    );
}

#[test]
fn declarations_inside_the_buffer_override_the_context() {
    let fragment = r#"<app:Parcel xmlns:app="http://example.com/other"><app:area/></app:Parcel>"#;
    let events = events(fragment, &app_context(ns::GML_32));
    assert_eq!(
        events[0],
        Event::Start(QName::new(Some("http://example.com/other"), "Parcel"))
    );
}

#[test]
fn reads_attributes_by_qualified_name() {
    let fragment = r##"<app:Parcel gml:id="p1" xlink:href="#other" area="12"/>"##;
    let context = app_context(ns::GML_32);
    let mut reader = reader(fragment.as_bytes(), &context);
    let XmlEvent::Start { name, attrs } = reader.next_event().unwrap() else {
        panic!("expected a start element");
    };
    assert_eq!(name, QName::new(Some(APP), "Parcel"));
    assert_eq!(
        attrs.get(&QName::new(Some(ns::GML_32), "id")).as_deref(),
        Some("p1")
    );
    assert_eq!(
        attrs.get(&QName::new(Some(ns::XLINK), "href")).as_deref(),
        Some("#other")
    );
    // An unprefixed attribute is in no namespace, not in the default one.
    assert_eq!(attrs.get(&QName::new(None, "area")).as_deref(), Some("12"));
    assert_eq!(attrs.get(&QName::new(Some(APP), "area")), None);
    assert_eq!(attrs.get(&QName::new(None, "missing")), None);

    let listed: Vec<String> = attrs.iter().map(|(name, _)| name.local.to_string()).collect();
    assert!(listed.contains(&"id".to_string()), "{listed:?}");
    assert!(listed.contains(&"area".to_string()), "{listed:?}");
    // Namespace declarations are not attributes of the element.
    assert!(!listed.contains(&"xmlns".to_string()), "{listed:?}");
}

#[test]
fn unescapes_text_and_reads_cdata_as_text() {
    let context = app_context(ns::GML_32);
    assert_eq!(
        events("<app:name>A &amp; B &lt;C&gt; &#65;</app:name>", &context),
        vec![
            Event::start("name"),
            Event::text("A & B <C> A"),
            Event::end("name"),
        ]
    );
    assert_eq!(
        events("<app:name><![CDATA[A & B <C>]]></app:name>", &context),
        vec![
            Event::start("name"),
            Event::text("A & B <C>"),
            Event::end("name"),
        ]
    );
}

#[test]
fn empty_elements_produce_a_start_and_an_end() {
    assert_eq!(
        events("<app:a/>", &app_context(ns::GML_32)),
        vec![Event::start("a"), Event::end("a")]
    );
}

#[test]
fn rejects_document_type_declarations() {
    // XXE and billion-laughs protection: DTDs are never processed.
    let document = concat!(
        r#"<!DOCTYPE app:Parcel [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>"#,
        r#"<app:Parcel><app:name>&xxe;</app:name></app:Parcel>"#
    );
    let context = app_context(ns::GML_32);
    let mut reader = reader(document.as_bytes(), &context);
    let error = loop {
        match reader.next_event() {
            Ok(XmlEvent::Eof) => panic!("the DTD should have been rejected"),
            Ok(_) => continue,
            Err(error) => break error,
        }
    };
    assert!(
        matches!(error, Error::DtdNotSupported(_)),
        "expected DtdNotSupported, got {error:?}"
    );
}

#[test]
fn undefined_entities_are_an_error_not_an_expansion() {
    let context = app_context(ns::GML_32);
    let mut reader = reader(b"<app:name>&xxe;</app:name>", &context);
    let result = loop {
        match reader.next_event() {
            Ok(XmlEvent::Eof) => break Ok(()),
            Ok(_) => continue,
            Err(error) => break Err(error),
        }
    };
    assert!(result.is_err(), "an undefined entity must not be expanded");
}

#[test]
fn malformed_xml_is_an_error_with_a_location() {
    let context = app_context(ns::GML_32);
    let mut reader = reader(b"<app:a><app:b></app:a>", &context);
    let error = loop {
        match reader.next_event() {
            Ok(XmlEvent::Eof) => panic!("mismatched tags should be an error"),
            Ok(_) => continue,
            Err(error) => break error,
        }
    };
    assert!(matches!(error, Error::Xml { .. }), "got {error:?}");
}

#[test]
fn skip_element_jumps_over_a_subtree() {
    let fragment = "<app:a><app:skipped><app:deep>x</app:deep></app:skipped><app:kept/></app:a>";
    let context = app_context(ns::GML_32);
    let mut reader = reader(fragment.as_bytes(), &context);
    assert!(matches!(reader.next_event().unwrap(), XmlEvent::Start { .. })); // app:a
    assert!(matches!(reader.next_event().unwrap(), XmlEvent::Start { .. })); // app:skipped
    reader.skip_element().unwrap();
    let XmlEvent::Start { name, .. } = reader.next_event().unwrap() else {
        panic!("expected app:kept after the skipped subtree");
    };
    assert_eq!(name, QName::new(Some(APP), "kept"));
}

#[test]
fn capture_element_returns_the_raw_xml_of_a_subtree() {
    // Mixed content and unsupported geometry are kept as raw XML
    // (`docs/type-mapping.md`, `docs/geometry.md`).
    let fragment = r#"<app:a><app:note lang="pl">text <b>bold</b></app:note><app:kept/></app:a>"#;
    let context = app_context(ns::GML_32);
    let mut reader = reader(fragment.as_bytes(), &context);
    assert!(matches!(reader.next_event().unwrap(), XmlEvent::Start { .. })); // app:a
    assert!(matches!(reader.next_event().unwrap(), XmlEvent::Start { .. })); // app:note
    let raw = reader.capture_element().unwrap();
    assert!(raw.contains("text "), "{raw:?}");
    assert!(raw.contains("<b>bold</b>"), "{raw:?}");
    // The capture consumed the element, so the next event is its sibling.
    let XmlEvent::Start { name, .. } = reader.next_event().unwrap() else {
        panic!("expected app:kept after the captured subtree");
    };
    assert_eq!(name, QName::new(Some(APP), "kept"));
}

#[test]
fn locations_count_from_the_base_offset_of_the_chunk() {
    use xeibe_core::reader::GmlReader;

    let fragment = "<app:a><app:b/></app:a>";
    let context = app_context(ns::GML_32);
    let mut reader = GmlReader::new(fragment.as_bytes(), &context, 1_000);
    let start = reader.location();
    assert!(start.byte_offset >= 1_000, "{start:?}");
    reader.next_event().unwrap();
    reader.next_event().unwrap();
    let later = reader.location();
    assert!(
        later.byte_offset > start.byte_offset,
        "the offset advances: {start:?} then {later:?}"
    );
    assert!(later.byte_offset < 1_000 + fragment.len() as u64 + 1);
}
