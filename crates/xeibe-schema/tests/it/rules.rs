//! The rule engine: path tree + `InferenceOptions` → Arrow schema
//! (`docs/schema-inference.md` §3–§4, `docs/type-mapping.md`).
//!
//! Schemas are flat: one column per leaf path, never a `Struct` for XML
//! structure. Each column records its path in the settings-file syntax
//! (`gml:path`: local names, `@attr`, `*` for a type wrapper, `[]` for the
//! anchor of a list) and gets the shortest unique name.

use arrow_schema::{DataType, TimeUnit};
use xeibe_schema::options::{AllNull, IntWidth, Lossless, MixedContent};
use xeibe_schema::rules::meta;
use xeibe_schema::{InferenceOptions, SampleOptions, TypeSet};
use xeibe_testkit::gml;

use crate::support::{
    column_names, data_type, extension_name, field, layer_schema, path, sampled_schema, scan,
    schema,
};

fn one(body: &str) -> String {
    gml::gml32_collection(&[&gml::feature("Parcel", "p1", body)])
}

fn many(bodies: &[&str]) -> String {
    let features: Vec<String> = bodies
        .iter()
        .enumerate()
        .map(|(i, body)| gml::feature("Parcel", &format!("p{i}"), body))
        .collect();
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    gml::gml32_collection(&refs)
}

fn utf8() -> DataType {
    DataType::Utf8View
}

/// The item type of a list column, or a panic.
#[track_caller]
fn list_item(data_type: DataType) -> arrow_schema::FieldRef {
    match data_type {
        DataType::List(item) => item,
        other => panic!("expected a list, got {other}"),
    }
}

/// No column of the schema is a `Struct`, except inside GeoArrow types.
#[track_caller]
fn assert_flat(schema: &arrow_schema::Schema) {
    for f in schema.fields() {
        if extension_name(f).is_some() {
            continue;
        }
        let inner = match f.data_type() {
            DataType::List(item) => item.data_type().clone(),
            other => other.clone(),
        };
        assert!(
            !matches!(inner, DataType::Struct(_)),
            "{} is {}: XML structure never becomes a Struct",
            f.name(),
            f.data_type()
        );
    }
}

#[test]
fn scalars_get_the_first_matching_type() {
    // BOOL → INT → FLOAT → DATE → DATETIME → TIME → STRING.
    let document = many(&[concat!(
        "<app:flag>true</app:flag>",
        "<app:count>42</app:count>",
        "<app:area>1523.40</app:area>",
        "<app:day>2021-03-04</app:day>",
        "<app:moment>2017-04-05T14:53:55</app:moment>",
        "<app:utc>2017-04-05T14:53:55+02:00</app:utc>",
        "<app:clock>14:30:00</app:clock>",
        "<app:code>0012</app:code>",
        "<app:duration>P1Y2M3DT4H</app:duration>",
    )]);
    let schema = schema(&document, "Parcel");
    assert_eq!(data_type(&schema, "flag"), DataType::Boolean);
    assert_eq!(data_type(&schema, "count"), DataType::Int64);
    assert_eq!(data_type(&schema, "area"), DataType::Float64);
    assert_eq!(data_type(&schema, "day"), DataType::Date32);
    assert_eq!(
        data_type(&schema, "moment"),
        DataType::Timestamp(TimeUnit::Microsecond, None),
        "no time zone: local time"
    );
    assert_eq!(
        data_type(&schema, "utc"),
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
    );
    assert_eq!(
        data_type(&schema, "clock"),
        DataType::Time64(TimeUnit::Microsecond)
    );
    assert_eq!(data_type(&schema, "code"), utf8(), "leading zeros: an identifier");
    assert_eq!(
        data_type(&schema, "duration"),
        utf8(),
        "xs:duration stays ISO 8601 text"
    );
}

#[test]
fn a_float_column_records_its_scale_in_metadata() {
    let schema = schema(&one("<app:area>1523.40</app:area>"), "Parcel");
    assert_eq!(
        field(&schema, "area").metadata().get(meta::MAX_SCALE).map(String::as_str),
        Some("2")
    );
}

#[test]
fn a_timestamp_column_with_one_offset_keeps_it_in_metadata() {
    let document = many(&[
        "<app:t>2017-04-05T14:53:55+02:00</app:t>",
        "<app:t>2017-05-05T14:53:55+02:00</app:t>",
    ]);
    let schema = schema(&document, "Parcel");
    assert_eq!(
        data_type(&schema, "t"),
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
    );
    assert_eq!(
        field(&schema, "t").metadata().get(meta::TZ_OFFSET).map(String::as_str),
        Some("+02:00")
    );
}

#[test]
fn mixed_offsets_are_stored_as_utc_without_an_offset_column() {
    // The instant is exact; the original offsets (almost always daylight-saving
    // time) are not kept (`docs/type-mapping.md`, "Mixed time-zone offsets").
    let document = many(&[
        "<app:t>2017-01-05T14:53:55+01:00</app:t>",
        "<app:t>2017-07-05T14:53:55+02:00</app:t>",
    ]);
    let schema = schema(&document, "Parcel");
    assert_eq!(
        data_type(&schema, "t"),
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
    );
    assert_eq!(column_names(&schema), ["@id", "t"], "no offset column");
    assert!(
        field(&schema, "t").metadata().get(meta::TZ_OFFSET).is_none(),
        "the offsets differ, so none is recorded"
    );
}

#[test]
fn values_with_and_without_a_time_zone_stay_text() {
    let document = many(&[
        "<app:t>2017-04-05T14:53:55+02:00</app:t>",
        "<app:t>2017-04-05T14:53:55</app:t>",
    ]);
    assert_eq!(data_type(&schema(&document, "Parcel"), "t"), utf8());
}

#[test]
fn one_value_that_does_not_fit_widens_the_whole_column() {
    let document = many(&["<app:n>33</app:n>", "<app:n>27a</app:n>"]);
    assert_eq!(data_type(&schema(&document, "Parcel"), "n"), utf8());
}

#[test]
fn every_field_is_nullable() {
    // A scan can't prove an element is always present, and the next file may
    // not have it (`docs/type-mapping.md`, "Nullability").
    let document = many(&[
        "<app:always>1</app:always><app:sometimes>2</app:sometimes>",
        "<app:always>3</app:always>",
    ]);
    let schema = schema(&document, "Parcel");
    assert!(schema.fields().iter().all(|f| f.is_nullable()));
}

#[test]
fn columns_keep_the_order_they_were_first_seen_in() {
    let document = many(&[
        "<app:b>1</app:b><app:a>2</app:a>",
        "<app:c>3</app:c><app:a>4</app:a>",
    ]);
    let names = column_names(&schema(&document, "Parcel"));
    assert_eq!(names, ["@id", "b", "a", "c"]);
}

#[test]
fn attributes_are_prefixed_and_gml_id_becomes_a_column() {
    let schema = schema(&one(r#"<app:area uom="m2">12</app:area>"#), "Parcel");
    assert_eq!(data_type(&schema, "@id"), utf8(), "gml:id → @id");
    // A constant attribute moves into field metadata instead of a column.
    assert_eq!(data_type(&schema, "area"), DataType::Int64);
    assert_eq!(
        field(&schema, "area")
            .metadata()
            .get(&format!("{}uom", meta::ATTR_PREFIX))
            .map(String::as_str),
        Some("m2")
    );
}

#[test]
fn text_and_attributes_are_separate_columns() {
    let document = many(&[
        r#"<app:area uom="m2">12</app:area>"#,
        r#"<app:area uom="ha">1</app:area>"#,
    ]);
    let schema = schema(&document, "Parcel");
    assert_eq!(data_type(&schema, "area"), DataType::Int64, "the element's own value");
    assert_eq!(path(&schema, "area"), "area");
    assert_eq!(data_type(&schema, "@uom"), utf8(), "the shortest unique name");
    assert_eq!(path(&schema, "@uom"), "area/@uom");
    assert_flat(&schema);
}

#[test]
fn repeated_elements_become_lists() {
    let document = many(&["<app:tag>a</app:tag><app:tag>b</app:tag>"]);
    let schema = schema(&document, "Parcel");
    let item = list_item(data_type(&schema, "tag"));
    assert_eq!(item.data_type(), &utf8());
    assert!(item.is_nullable(), "list items are nullable too");
    assert_eq!(path(&schema, "tag"), "tag[]", "the element is its own anchor");
}

#[test]
fn nested_elements_become_one_column_per_leaf() {
    let document = one("<app:owner><app:name>B</app:name><app:since>2020</app:since></app:owner>");
    let schema = schema(&document, "Parcel");
    assert_eq!(column_names(&schema), ["@id", "name", "since"]);
    assert_eq!(data_type(&schema, "name"), utf8());
    assert_eq!(path(&schema, "name"), "owner/name");
    assert_eq!(data_type(&schema, "since"), DataType::Int64);
    assert_eq!(path(&schema, "since"), "owner/since");
    assert_flat(&schema);
}

#[test]
fn names_grow_from_the_end_until_they_are_unique() {
    // `docs/schema-inference.md` §3.1: start with the last step; while two
    // columns share a name, each takes one more step from the front.
    let document = one(concat!(
        "<app:inspireId><app:Identifier>",
        "<app:localId>1</app:localId><app:namespace>PL.A</app:namespace>",
        "</app:Identifier></app:inspireId>",
        "<app:hydroId><app:HydroIdentifier>",
        "<app:namespace>PL.B</app:namespace>",
        "</app:HydroIdentifier></app:hydroId>",
    ));
    let schema = schema(&document, "Parcel");
    assert_eq!(
        column_names(&schema),
        ["@id", "localId", "inspireId.namespace", "hydroId.namespace"]
    );
    assert_eq!(path(&schema, "inspireId.namespace"), "inspireId/*/namespace");
    assert_eq!(path(&schema, "hydroId.namespace"), "hydroId/*/namespace");
}

#[test]
fn a_name_that_is_taken_by_a_shorter_path_grows() {
    // The wrapper's own `gml:id` would be `@id`, which the feature's has.
    let document = one(concat!(
        r#"<app:idIIP><app:AD_IdentyfikatorIIP gml:id="w1">"#,
        "<app:lokalnyId>a</app:lokalnyId>",
        "</app:AD_IdentyfikatorIIP></app:idIIP>",
    ));
    let schema = schema(&document, "Parcel");
    assert_eq!(path(&schema, "@id"), "@id");
    assert_eq!(path(&schema, "idIIP.@id"), "idIIP/*/@id");
}

#[test]
fn siblings_that_differ_only_by_namespace_keep_their_prefix() {
    let document = one(concat!(
        "<app:code>1</app:code>",
        r#"<x:code xmlns:x="http://example.com/x">A</x:code>"#,
    ));
    let schema = schema(&document, "Parcel");
    assert_eq!(path(&schema, "app:code"), "app:code");
    assert_eq!(path(&schema, "x:code"), "x:code");
    let namespaces = schema.metadata().get(meta::NS).cloned().unwrap_or_default();
    assert!(
        namespaces.contains("http://example.com/x") && namespaces.contains(xeibe_testkit::gml::APP),
        "the prefixes are declared in the schema metadata: {namespaces:?}"
    );
}

#[test]
fn inspire_type_wrappers_are_collapsed() {
    let document = one(concat!(
        "<app:idIIP><app:AD_IdentyfikatorIIP>",
        "<app:lokalnyId>a339e481</app:lokalnyId>",
        "<app:przestrzenNazw>PL.PZGIK.200</app:przestrzenNazw>",
        "</app:AD_IdentyfikatorIIP></app:idIIP>"
    ));
    let schema = schema(&document, "Parcel");
    assert_eq!(data_type(&schema, "lokalnyId"), utf8());
    // The wrapper step is `*` in the path and left out of the name.
    assert_eq!(path(&schema, "lokalnyId"), "idIIP/*/lokalnyId");
    assert_eq!(path(&schema, "przestrzenNazw"), "idIIP/*/przestrzenNazw");
}

#[test]
fn different_wrapper_types_under_one_property_are_merged() {
    // XPlanung's `externeReferenz` holds an `XP_ExterneReferenz` or an
    // `XP_SpezExterneReferenz`: `*` matches either.
    let document = many(&[
        concat!(
            "<app:externeReferenz><app:XP_ExterneReferenz>",
            "<app:referenzName>Plan</app:referenzName>",
            "</app:XP_ExterneReferenz></app:externeReferenz>"
        ),
        concat!(
            "<app:externeReferenz><app:XP_SpezExterneReferenz>",
            "<app:referenzName>Begründung</app:referenzName><app:typ>1010</app:typ>",
            "</app:XP_SpezExterneReferenz></app:externeReferenz>"
        ),
    ]);
    let schema = schema(&document, "Parcel");
    assert_eq!(column_names(&schema), ["@id", "referenzName", "typ"]);
    assert_eq!(path(&schema, "referenzName"), "externeReferenz/*/referenzName");
    assert_eq!(path(&schema, "typ"), "externeReferenz/*/typ");
}

#[test]
fn xlink_references_become_string_columns() {
    // They are foreign keys, never resolved.
    let document = many(&[
        r##"<app:miejsce xlink:href="#PL.X.1"/>"##,
        r##"<app:miejsce xlink:href="#PL.X.2"/>"##,
    ]);
    let schema_ = schema(&document, "Parcel");
    assert_eq!(data_type(&schema_, "miejsce"), utf8());
    assert_eq!(
        path(&schema_, "miejsce"),
        "miejsce/@href",
        "named after the property, which holds only the href"
    );

    // Repeated references become a list of strings (PRG's `adres2`).
    let repeated = many(&[concat!(
        r##"<app:adres2 xlink:href="#a"/>"##,
        r##"<app:adres2 xlink:href="#b"/>"##
    )]);
    let schema_ = schema(&repeated, "Parcel");
    assert_eq!(list_item(data_type(&schema_, "adres2")).data_type(), &utf8());
    assert_eq!(path(&schema_, "adres2"), "adres2[]/@href");
}

#[test]
fn mixed_content_is_kept_as_raw_xml() {
    let schema = schema(&one("<app:note>text <b>bold</b></app:note>"), "Parcel");
    assert_eq!(data_type(&schema, "note"), utf8());

    let text_only = InferenceOptions {
        structure: xeibe_schema::options::StructureOptions {
            mixed_content: MixedContent::TextOnly,
            ..InferenceOptions::default().structure
        },
        ..InferenceOptions::default()
    };
    let observation = scan(&one("<app:note>text <b>bold</b></app:note>"));
    let schema = layer_schema(&observation, "Parcel", &text_only).schema;
    assert_eq!(data_type(&schema, "note"), utf8());
}

#[test]
fn empty_and_nil_elements_are_null() {
    let document = many(&[
        "<app:a/><app:b xsi:nil=\"true\"/><app:c>1</app:c>",
        "<app:a/><app:b xsi:nil=\"true\"/><app:c>2</app:c>",
    ]);
    let schema = schema(&document, "Parcel");
    // A column that never had a value stays a string column by default.
    assert_eq!(data_type(&schema, "a"), utf8());
    assert_eq!(data_type(&schema, "b"), utf8());
    assert_eq!(data_type(&schema, "c"), DataType::Int64);

    let as_null = InferenceOptions {
        types: xeibe_schema::options::TypeOptions {
            all_null: AllNull::Null,
            ..InferenceOptions::default().types
        },
        ..InferenceOptions::default()
    };
    let observation = scan(&document);
    let schema = layer_schema(&observation, "Parcel", &as_null).schema;
    assert_eq!(data_type(&schema, "a"), DataType::Null);
}

#[test]
fn a_nil_reason_is_kept_beside_the_column() {
    let document = many(&[
        r#"<app:b xsi:nil="true" nilReason="unknown"/>"#,
        r#"<app:b xsi:nil="true" nilReason="withheld"/>"#,
    ]);
    let schema = schema(&document, "Parcel");
    assert_eq!(data_type(&schema, "@nilReason"), utf8());
    assert_eq!(path(&schema, "@nilReason"), "b/@nilReason");
}

#[test]
fn a_geometry_property_becomes_a_geoarrow_column() {
    let document = many(&[
        "<app:geom><gml:Point srsName=\"EPSG:2180\"><gml:pos>1 2</gml:pos></gml:Point></app:geom>",
        "<app:geom><gml:Point srsName=\"EPSG:2180\"><gml:pos>3 4</gml:pos></gml:Point></app:geom>",
    ]);
    let schema = schema(&document, "Parcel");
    let geometry = field(&schema, "geom");
    let extension = extension_name(geometry).unwrap_or_default();
    assert!(
        extension.starts_with("geoarrow."),
        "the GeoArrow extension type is recorded on the field, got {extension:?}"
    );
    assert_eq!(
        extension, "geoarrow.point",
        "one simple kind, no curves → the native type"
    );
    assert_eq!(
        geometry.metadata().get(meta::SRS_NAME).map(String::as_str),
        Some("EPSG:2180")
    );
    assert!(geometry.metadata().contains_key(meta::AXIS_SWAPPED));
}

#[test]
fn a_column_with_curves_is_written_as_wkb() {
    let document = many(&[concat!(
        "<app:geom><gml:Curve srsName=\"EPSG:2180\"><gml:segments>",
        "<gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc>",
        "</gml:segments></gml:Curve></app:geom>"
    )]);
    let schema = schema(&document, "Parcel");
    assert_eq!(extension_name(field(&schema, "geom")), Some("geoarrow.wkb"));
}

#[test]
fn a_kind_and_its_multi_form_become_the_multi_type() {
    // As GDAL's `PROMOTE_TO_MULTI` (`docs/geometry.md`, "Column encoding").
    let ring = "<gml:exterior><gml:LinearRing><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gml:LinearRing></gml:exterior>";
    let document = many(&[
        &format!("<app:geom><gml:Polygon srsName=\"EPSG:2180\">{ring}</gml:Polygon></app:geom>"),
        &format!(
            "<app:geom><gml:MultiSurface srsName=\"EPSG:2180\"><gml:surfaceMember><gml:Polygon>{ring}</gml:Polygon></gml:surfaceMember></gml:MultiSurface></app:geom>"
        ),
    ]);
    let schema = schema(&document, "Parcel");
    assert_eq!(extension_name(field(&schema, "geom")), Some("geoarrow.multipolygon"));
}

#[test]
fn unrelated_kinds_are_wkb_not_a_union() {
    // No `geoarrow.geometry`: poor downstream support.
    let document = many(&[
        "<app:geom><gml:Point srsName=\"EPSG:2180\"><gml:pos>1 2</gml:pos></gml:Point></app:geom>",
        "<app:geom><gml:LineString srsName=\"EPSG:2180\"><gml:posList>1 2 3 4</gml:posList></gml:LineString></app:geom>",
    ]);
    let schema = schema(&document, "Parcel");
    assert_eq!(extension_name(field(&schema, "geom")), Some("geoarrow.wkb"));
}

#[test]
fn a_geometry_below_a_repeated_element_is_a_list_of_wkb() {
    // GDAL's `gmlsubfeature.gml`: each `content` holds an object with its own
    // geometry (`docs/schema-inference.md`, "Lists and alignment").
    let object = |id: &str, x: &str, foo: &str| {
        format!(
            concat!(
                "<app:content><app:Object gml:id=\"{}\"><app:geometry>",
                "<gml:Point srsName=\"EPSG:4326\"><gml:pos>{} 2</gml:pos></gml:Point>",
                "</app:geometry><app:foo>{}</app:foo></app:Object></app:content>"
            ),
            id, x, foo
        )
    };
    let body = format!("{}{}", object("o1", "48", "bar"), object("o2", "-48", "baz"));
    let schema = schema(&one(&body), "Parcel");
    let item = list_item(data_type(&schema, "geometry"));
    assert_eq!(
        extension_name(&item),
        Some("geoarrow.wkb"),
        "always WKB: no native type holds several geometries per row"
    );
    assert_eq!(path(&schema, "geometry"), "content[]/*/geometry");
    assert_eq!(path(&schema, "foo"), "content[]/*/foo");
}

#[test]
fn bounded_by_is_dropped_by_default() {
    let document = one(concat!(
        "<gml:boundedBy><gml:Envelope><gml:lowerCorner>0 0</gml:lowerCorner>",
        "<gml:upperCorner>1 1</gml:upperCorner></gml:Envelope></gml:boundedBy>",
        "<app:area>1</app:area>"
    ));
    let names = column_names(&schema(&document, "Parcel"));
    assert!(!names.iter().any(|n| n.contains("boundedBy")), "{names:?}");
}

#[test]
fn every_field_records_where_it_came_from() {
    let schema = schema(&one("<app:area>1</app:area>"), "Parcel");
    let metadata = field(&schema, "area").metadata().clone();
    assert_eq!(metadata.get(meta::PATH).map(String::as_str), Some("area"), "{metadata:?}");
    assert!(
        schema.metadata().get(meta::NS).is_none(),
        "namespaces are only declared when two siblings differ only by namespace"
    );
}

#[test]
fn the_type_set_can_be_narrowed_to_strings() {
    let document = one("<app:count>42</app:count><app:day>2021-03-04</app:day>");
    let observation = scan(&document);
    let options = InferenceOptions {
        types: xeibe_schema::options::TypeOptions {
            enabled: TypeSet::STRING,
            ..InferenceOptions::default().types
        },
        ..InferenceOptions::default()
    };
    let schema = layer_schema(&observation, "Parcel", &options).schema;
    assert_eq!(data_type(&schema, "count"), utf8());
    assert_eq!(data_type(&schema, "day"), utf8());
}

#[test]
fn integer_width_is_stable_by_default() {
    let document = one("<app:small>1</app:small>");
    let observation = scan(&document);
    assert_eq!(
        data_type(
            &layer_schema(&observation, "Parcel", &InferenceOptions::default()).schema,
            "small"
        ),
        DataType::Int64
    );

    let smallest = InferenceOptions {
        types: xeibe_schema::options::TypeOptions {
            integers: IntWidth::Smallest,
            ..InferenceOptions::default().types
        },
        ..InferenceOptions::default()
    };
    assert_eq!(
        data_type(&layer_schema(&observation, "Parcel", &smallest).schema, "small"),
        DataType::Int8
    );
}

#[test]
fn lossy_mode_accepts_what_value_lossless_mode_rejects() {
    let document = many(&["<app:flag>1</app:flag>", "<app:flag>0</app:flag>"]);
    let observation = scan(&document);
    assert_eq!(
        data_type(
            &layer_schema(&observation, "Parcel", &InferenceOptions::default()).schema,
            "flag"
        ),
        DataType::Int64,
        "1/0 are integers, not booleans, unless lossy"
    );

    let lossy = InferenceOptions {
        types: xeibe_schema::options::TypeOptions {
            lossless: Lossless::Lossy,
            ..InferenceOptions::default().types
        },
        ..InferenceOptions::default()
    };
    assert_eq!(
        data_type(&layer_schema(&observation, "Parcel", &lossy).schema, "flag"),
        DataType::Boolean
    );
}

#[test]
fn every_column_below_a_repeated_element_is_a_list_anchored_on_it() {
    // `docs/schema-inference.md`, "Lists and alignment": one entry per
    // occurrence of the anchor, so the lists stay aligned.
    let document = many(&[
        concat!(
            "<app:adres><app:ulica>Polna</app:ulica><app:numer>1</app:numer></app:adres>",
            "<app:adres><app:numer>2</app:numer></app:adres>"
        ),
        "<app:adres><app:numer>3</app:numer></app:adres>",
    ]);
    let schema = schema(&document, "Parcel");
    assert_eq!(list_item(data_type(&schema, "ulica")).data_type(), &utf8());
    assert_eq!(list_item(data_type(&schema, "numer")).data_type(), &DataType::Int64);
    assert_eq!(path(&schema, "ulica"), "adres[]/ulica");
    assert_eq!(path(&schema, "numer"), "adres[]/numer");
    assert_flat(&schema);
}

#[test]
fn a_child_seen_once_per_feature_is_still_a_list_under_a_repeated_parent() {
    // `ulica` never occurs twice in one feature, but its siblings are lists.
    let document = many(&[concat!(
        "<app:adres><app:ulica>Polna</app:ulica><app:numer>1</app:numer></app:adres>",
        "<app:adres><app:numer>2</app:numer></app:adres>"
    )]);
    let schema = schema(&document, "Parcel");
    assert!(matches!(data_type(&schema, "ulica"), DataType::List(_)));
}

#[test]
fn repetition_at_two_levels_anchors_on_the_inner_element() {
    // No lists of lists: columns below the inner repeated element are anchored
    // on it, the others on the outer one.
    let name = |lang: &str, a: &str, b: &str| {
        format!(
            concat!(
                "<app:name><app:language>{}</app:language>",
                "<app:spelling><app:text>{}</app:text></app:spelling>",
                "<app:spelling><app:text>{}</app:text></app:spelling>",
                "</app:name>"
            ),
            lang, a, b
        )
    };
    let body = format!("{}{}", name("pol", "Łódź", "Lodz"), name("deu", "Lodsch", "Litzmannstadt"));
    let schema = schema(&one(&body), "Parcel");
    assert_eq!(path(&schema, "language"), "name[]/language");
    assert_eq!(path(&schema, "text"), "name/spelling[]/text");
    assert_eq!(
        list_item(data_type(&schema, "text")).data_type(),
        &utf8(),
        "a list of strings, not a list of lists"
    );
}

#[test]
fn conservative_sampling_keeps_weakly_evidenced_columns_as_text() {
    // A sampled schema must accept data it hasn't seen
    // (`docs/schema-inference.md` §6.2).
    let document = many(&["<app:count>42</app:count>", "<app:count>43</app:count>"]);
    let sample = SampleOptions {
        min_typed_values: 100,
        conservative: true,
        ..SampleOptions::default()
    };
    assert_eq!(
        data_type(&sampled_schema(&document, "Parcel", &sample), "count"),
        utf8(),
        "only 2 of the required 100 values"
    );

    let permissive = SampleOptions {
        min_typed_values: 1,
        conservative: true,
        ..SampleOptions::default()
    };
    assert_eq!(
        data_type(&sampled_schema(&document, "Parcel", &permissive), "count"),
        DataType::Int64
    );
}

#[test]
fn conservative_sampling_writes_geometry_as_wkb() {
    // A Polygon sample says nothing about later MultiPolygons.
    let document = many(&[
        "<app:geom><gml:Point srsName=\"EPSG:2180\"><gml:pos>1 2</gml:pos></gml:Point></app:geom>",
    ]);
    let sample = SampleOptions {
        min_typed_values: 1,
        conservative: true,
        ..SampleOptions::default()
    };
    let schema = sampled_schema(&document, "Parcel", &sample);
    assert_eq!(extension_name(field(&schema, "geom")), Some("geoarrow.wkb"));
}

#[test]
fn the_defaults_match_the_documented_ones() {
    let options = InferenceOptions::default();
    assert!(options.structure.collapse_type_wrappers);
    assert_eq!(options.types.lossless, Lossless::Value);
    assert!(options.types.string_view);
    assert!(options.types.empty_as_null);
    assert_eq!(options.limits.max_depth, 16);
    assert_eq!(options.limits.max_children, 512);
    assert_eq!(options.limits.distinct_values, 64);

    let sample = SampleOptions::default();
    assert_eq!(sample.features_per_layer, 10_000);
    assert_eq!(sample.min_typed_values, 100);
    assert!(sample.conservative);
    assert_eq!(sample.max_buffer_bytes, 256 << 20);
}
