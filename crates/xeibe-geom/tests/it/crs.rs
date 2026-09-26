//! srsName parsing: which CRS, and in which *form* it was written
//! (`docs/geometry.md`, "srsName forms").
//!
//! The form matters, because `CrsHeuristic` decides by it. The forms listed
//! here all appear in the corpus.

use xeibe_geom::{CrsRef, SrsName, SrsNameForm};

fn epsg(code: &str) -> CrsRef {
    CrsRef::Code {
        authority: "EPSG".into(),
        code: code.into(),
    }
}

#[track_caller]
fn assert_srs(raw: &str, form: SrsNameForm, crs: Option<CrsRef>) {
    let parsed = SrsName::parse(raw);
    assert_eq!(parsed.raw, raw, "the original string is kept");
    assert_eq!(parsed.form, form, "form of {raw}");
    assert_eq!(parsed.crs, crs, "CRS of {raw}");
}

#[test]
fn short_and_legacy_forms() {
    assert_srs("EPSG:2180", SrsNameForm::Short, Some(epsg("2180")));
    assert_srs(
        "http://www.opengis.net/gml/srs/epsg.xml#2180",
        SrsNameForm::LegacyUrl,
        Some(epsg("2180")),
    );
    // A bare code is treated as the short form (seen in the corpus).
    assert_srs("25833", SrsNameForm::Short, Some(epsg("25833")));
}

#[test]
fn urn_forms() {
    assert_srs("urn:ogc:def:crs:EPSG::2180", SrsNameForm::OgcUrn, Some(epsg("2180")));
    // The version is ignored.
    assert_srs(
        "urn:ogc:def:crs:EPSG:6.6:4326",
        SrsNameForm::OgcUrn,
        Some(epsg("4326")),
    );
    assert_srs(
        "urn:x-ogc:def:crs:EPSG::4326",
        SrsNameForm::ExperimentalUrn,
        Some(epsg("4326")),
    );
    // GeoServer WFS 1.1 writes it without the empty version field.
    assert_srs(
        "urn:x-ogc:def:crs:EPSG:3301",
        SrsNameForm::ExperimentalUrn,
        Some(epsg("3301")),
    );
    assert_srs(
        "urn:EPSG:geographicCRS:4326",
        SrsNameForm::Wfs11Urn,
        Some(epsg("4326")),
    );
}

#[test]
fn http_uri_forms() {
    assert_srs(
        "http://www.opengis.net/def/crs/EPSG/0/2180",
        SrsNameForm::HttpUri,
        Some(epsg("2180")),
    );
    assert_srs(
        "http://www.opengis.net/def/crs?authority=EPSG&version=0&code=4326",
        SrsNameForm::HttpUriKvp,
        Some(epsg("4326")),
    );
}

#[test]
fn crs84_and_friends_are_the_ogc_authority() {
    let crs84 = CrsRef::Code {
        authority: "OGC".into(),
        code: "CRS84".into(),
    };
    assert_srs(
        "urn:ogc:def:crs:OGC:1.3:CRS84",
        SrsNameForm::OgcUrn,
        Some(crs84.clone()),
    );
    assert_srs(
        "http://www.opengis.net/def/crs/OGC/1.3/CRS84",
        SrsNameForm::HttpUri,
        Some(crs84.clone()),
    );
    assert!(crs84.is_lon_lat_by_definition());
    assert!(!epsg("4326").is_lon_lat_by_definition());
}

#[test]
fn adv_urns_map_to_epsg_codes() {
    // German ALKIS/NAS and XPlanung; 250 documents in the corpus.
    let parsed = SrsName::parse("urn:adv:crs:ETRS89_UTM32");
    assert_eq!(parsed.crs, Some(epsg("25832")));

    // A compound AdV URN joins the horizontal and vertical parts with `*`.
    let compound = SrsName::parse("urn:adv:crs:ETRS89_UTM32*DE_DHHN2016_NH");
    match compound.crs {
        Some(CrsRef::Compound(parts)) => assert_eq!(parts.first(), Some(&epsg("25832"))),
        other => panic!("expected a compound CRS, got {other:?}"),
    }
}

#[test]
fn compound_urns_keep_their_components_in_order() {
    let parsed = SrsName::parse("urn:ogc:def:crs,crs:EPSG::4269,crs:EPSG::5713");
    assert_eq!(parsed.form, SrsNameForm::CompoundUrn);
    assert_eq!(
        parsed.crs,
        Some(CrsRef::Compound(vec![epsg("4269"), epsg("5713")]))
    );
}

#[test]
fn compound_uris_keep_their_components_in_key_order() {
    let parsed = SrsName::parse(
        "http://www.opengis.net/def/crs-compound?2=http://www.opengis.net/def/crs/EPSG/0/7837\
         &1=http://www.opengis.net/def/crs/EPSG/0/25832",
    );
    assert_eq!(parsed.form, SrsNameForm::CompoundUri);
    assert_eq!(parsed.crs, Some(CrsRef::Compound(vec![epsg("25832"), epsg("7837")])));
}

#[test]
fn the_proj_compound_spelling_is_a_short_form() {
    // What `--crs` users write, and what `authority_code` writes back.
    let compound = CrsRef::Compound(vec![epsg("25832"), epsg("7837")]);
    for raw in ["EPSG:25832+7837", "EPSG:25832+EPSG:7837", "epsg:25832 + 7837"] {
        let parsed = SrsName::parse(raw);
        assert_eq!(parsed.form, SrsNameForm::Short, "{raw}");
        assert_eq!(parsed.crs.as_ref(), Some(&compound), "{raw}");
    }
    let written = compound.authority_code();
    assert_eq!(written, "EPSG:25832+7837");
    assert_eq!(SrsName::parse(&written).crs.map(|crs| crs.authority_code()), Some(written));
    assert_eq!(SrsName::parse("EPSG:25832+").crs, None);
}

#[test]
fn names_resolve_wherever_a_crs_can_stand() {
    // EPSG's aliases ("Poland alternative identifier") as a whole srsName…
    let parsed = SrsName::parse("PL-1992");
    assert_eq!((parsed.form, parsed.crs), (SrsNameForm::Short, Some(epsg("2180"))));
    assert_eq!(SrsName::parse("pl-2000/15").crs, Some(epsg("2176")));
    // …and as compound parts: GUGiK's 3D building models, LoD1 and LoD2.
    assert_eq!(
        SrsName::parse("urn:ogc:def:crs,crs:EPSG::2180,crs:EPSG::9651").crs,
        Some(CrsRef::Compound(vec![epsg("2180"), epsg("9651")]))
    );
    assert_eq!(
        SrsName::parse("urn:ogc:def:crs,crs:EPSG::2180,crs:PL-KRON86-NH").crs,
        Some(CrsRef::Compound(vec![epsg("2180"), epsg("9650")]))
    );
    // AdV names outside `urn:adv:crs:`, and versioned parts (GDAL's citygml.gml).
    assert_eq!(
        SrsName::parse("urn:ogc:def:crs,crs:EPSG::25832,crs:DE_DHHN2016_NH").crs,
        Some(CrsRef::Compound(vec![epsg("25832"), epsg("7837")]))
    );
    assert_eq!(
        SrsName::parse("urn:ogc:def:crs,crs:EPSG:6.12:3068,crs:EPSG:6.12:5783").crs,
        Some(CrsRef::Compound(vec![epsg("3068"), epsg("5783")]))
    );
    // A name EPSG gives to several CRSs is no CRS: no guessing.
    assert_eq!(SrsName::parse("PL-ETRF2000").crs, None);
}

#[test]
fn a_compound_crs_keeps_its_known_parts() {
    let parsed = SrsName::parse("urn:ogc:def:crs,crs:EPSG::2180,crs:PL-XYZ");
    assert_eq!(parsed.form, SrsNameForm::CompoundUrn);
    let crs = parsed.crs.expect("the known part is kept");
    assert_eq!(crs, CrsRef::Compound(vec![epsg("2180"), CrsRef::Unresolved("PL-XYZ".into())]));
    assert_eq!(crs.unresolved(), ["PL-XYZ"]);
    assert_eq!(crs.horizontal(), &epsg("2180"));
    // The output CRS is the known part alone.
    assert_eq!(crs.projjson().unwrap()["id"]["code"], 2180);
    // The fallback spelling keeps the unknown part, and reads back.
    assert_eq!(crs.authority_code(), "EPSG:2180+PL-XYZ");
    assert_eq!(SrsName::parse("EPSG:2180+PL-XYZ").crs, Some(crs));

    // With no known part there is no CRS.
    for raw in ["urn:ogc:def:crs,crs:PL-XYZ,crs:PL-ABC", "urn:adv:crs:DE_XYZ", "urn:adv:crs:DE_XYZ*DE_ABC"] {
        let parsed = SrsName::parse(raw);
        assert_eq!((parsed.form, parsed.crs), (SrsNameForm::Unknown, None), "{raw}");
    }
    // An empty part is malformed, not unknown.
    assert_eq!(SrsName::parse("urn:ogc:def:crs,crs:EPSG::2180,").crs, None);
}

#[test]
fn a_compound_crs_gets_projjson_built_from_its_components() {
    // As PROJ builds `EPSG:25832+7837`: "A + B", components without
    // `$schema`, and no `id`, since the pair has no code of its own.
    let srs = SrsName::parse("urn:adv:crs:ETRS89_UTM32*DE_DHHN2016_NH");
    let json = srs.crs.unwrap().projjson().expect("PROJJSON");
    assert_eq!(json["type"], "CompoundCRS");
    assert_eq!(json["name"], "ETRS89 / UTM zone 32N + DHHN2016 height");
    assert!(json["$schema"].as_str().is_some_and(|s| s.contains("projjson.schema.json")));
    assert!(json.get("id").is_none());
    let components = json["components"].as_array().unwrap();
    assert_eq!(components.len(), 2);
    assert_eq!(components[0]["type"], "ProjectedCRS");
    assert_eq!(components[0]["id"]["code"], 25832);
    assert_eq!(components[1]["type"], "VerticalCRS");
    assert_eq!(components[1]["id"]["code"], 7837);
    assert!(components.iter().all(|c| c.get("$schema").is_none()));
}

#[test]
fn a_compound_crs_has_no_projjson_when_a_component_has_none() {
    // Falls back to `authority_code`, like a single unknown code.
    assert!(CrsRef::Compound(vec![epsg("25832"), epsg("98765")]).projjson().is_none());
    assert!(epsg("98765").projjson().is_none());
    assert_eq!(epsg("2180").projjson().unwrap()["id"]["code"], 2180);
}

#[test]
fn unrecognised_srs_names_keep_the_raw_string() {
    // "Unknown srsName": read as written, with a warning in the read report.
    let parsed = SrsName::parse("AUT-GK31-5");
    assert_eq!(parsed.form, SrsNameForm::Unknown);
    assert_eq!(parsed.crs, None);
    assert_eq!(parsed.raw, "AUT-GK31-5");
    assert_eq!(SrsName::parse("").form, SrsNameForm::Unknown);
}

#[test]
fn authority_code_is_the_geoarrow_fallback_spelling() {
    assert_eq!(epsg("2180").authority_code(), "EPSG:2180");
    assert_eq!(
        CrsRef::Code {
            authority: "OGC".into(),
            code: "CRS84".into()
        }
        .authority_code(),
        "OGC:CRS84"
    );
}

#[test]
fn the_same_crs_in_different_forms_resolves_to_one_crs() {
    // Different spellings are different decision keys for the axis order, but
    // the same CRS in the output metadata.
    let forms = [
        "EPSG:4326",
        "urn:ogc:def:crs:EPSG::4326",
        "urn:x-ogc:def:crs:EPSG::4326",
        "http://www.opengis.net/def/crs/EPSG/0/4326",
        "http://www.opengis.net/gml/srs/epsg.xml#4326",
    ];
    for form in forms {
        assert_eq!(
            SrsName::parse(form).crs,
            Some(epsg("4326")),
            "{form} names EPSG:4326"
        );
    }
}
