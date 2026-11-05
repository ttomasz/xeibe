//! `Crs::from_user_input` and `user_input::parse`: the forms pyproj's
//! `CRS.from_user_input` reads text in.

use xeibe_reproject::user_input::{self, UserInput};
use xeibe_reproject::{AxisOrder, Crs, Error, Transform};

#[test]
fn epsg_codes_in_every_spelling() {
    for (text, code) in [
        ("4326", 4326),
        ("EPSG:4326", 4326),
        ("epsg:3857", 3857),
        ("EPSG: 2180", 2180),
        ("  EPSG:2180\n", 2180),
        ("urn:ogc:def:crs:EPSG::2180", 2180),
        ("URN:OGC:DEF:CRS:EPSG:9.8.1:2180", 2180),
        ("http://www.opengis.net/def/crs/EPSG/0/2180", 2180),
        ("https://www.opengis.net/def/crs/EPSG/0/2180/", 2180),
        ("http://www.opengis.net/gml/srs/epsg.xml#2180", 2180),
        // EPSG's names, ignoring case, and its aliases.
        ("WGS 84", 4326),
        ("wgs 84 / pseudo-mercator", 3857),
        ("PL-1992", 2180),
    ] {
        assert_eq!(
            user_input::parse(text).unwrap(),
            UserInput::Epsg(code),
            "{text}"
        );
        assert_eq!(
            Crs::from_user_input(text).unwrap().code(),
            Some(code),
            "{text}"
        );
    }
}

#[cfg(feature = "esri")]
#[test]
fn esri_codes_epsg_has_since_defined_are_epsg_crss() {
    assert_eq!(
        user_input::parse("ESRI:102100").unwrap(),
        UserInput::Epsg(3857)
    );
    assert_eq!(
        Crs::from_user_input("esri:102100").unwrap().code(),
        Some(3857)
    );
}

#[test]
fn compounds_are_built_as_proj_builds_them() {
    for text in [
        "EPSG:4326+5773",
        "urn:ogc:def:crs:EPSG::4326+5773",
        "urn:ogc:def:crs,crs:EPSG::4326,crs:EPSG::5773",
        "http://www.opengis.net/def/crs-compound?1=http://www.opengis.net/def/crs/EPSG/0/4326&2=http://www.opengis.net/def/crs/EPSG/0/5773",
    ] {
        assert_eq!(
            user_input::parse(text).unwrap(),
            UserInput::Compound(4326, 5773),
            "{text}"
        );
    }
    // Named "horizontal + vertical", without EPSG:9707's code, as
    // `projinfo EPSG:4326+5773` (PROJ 9.8) gives it.
    let input = user_input::parse("EPSG:4326+5773").unwrap();
    let json = input.projjson().unwrap();
    assert_eq!(json["name"], "WGS 84 + EGM96 height");
    assert!(json.get("id").is_none());
    assert!(json["$schema"].is_string());
    assert!(
        json["components"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.get("$schema").is_none())
    );
    let crs = input.crs().unwrap();
    assert_eq!(crs.code(), None);
    assert!(crs.equivalent(&Crs::from_epsg(9707).unwrap(), false));
    assert_eq!(crs.identify().first().map(|m| m.0), Some(9707));
}

#[test]
fn ogc_crss_are_longitude_first() {
    for text in [
        "OGC:CRS84",
        "ogc:crs84",
        "urn:ogc:def:crs:OGC:1.3:CRS84",
        "http://www.opengis.net/def/crs/OGC/1.3/CRS84",
    ] {
        assert_eq!(
            user_input::parse(text).unwrap(),
            UserInput::Ogc("CRS84"),
            "{text}"
        );
    }
    let input = user_input::parse("OGC:CRS84").unwrap();
    let json = input.projjson().unwrap();
    assert_eq!(json["name"], "WGS 84 (CRS84)");
    assert_eq!(json["id"]["authority"], "OGC");
    assert_eq!(json["coordinate_system"]["axis"][0]["direction"], "east");
    let crs84 = input.crs().unwrap();
    let wgs84 = Crs::from_epsg(4326).unwrap();
    assert!(!crs84.equivalent(&wgs84, false));
    assert!(crs84.equivalent(&wgs84, true));
    // Each in its own axis order: longitude first, latitude first.
    let t = Transform::new(&crs84, &wgs84, AxisOrder::Authority).unwrap();
    let (lat, lon, _) = t.convert(21.0, 52.0, None).unwrap();
    assert_eq!((lat, lon), (52.0, 21.0));
}

#[test]
fn proj_strings_are_crss_with_or_without_type_crs() {
    let utm = Crs::from_epsg(32633).unwrap();
    for text in [
        "+proj=utm +zone=33 +datum=WGS84",
        "+proj=utm +zone=33 +datum=WGS84 +type=crs",
    ] {
        assert!(
            matches!(user_input::parse(text).unwrap(), UserInput::ProjString(_)),
            "{text}"
        );
        assert!(
            Crs::from_user_input(text).unwrap().equivalent(&utm, false),
            "{text}"
        );
    }
    let json = user_input::parse("+proj=longlat +datum=WGS84")
        .unwrap()
        .projjson()
        .unwrap();
    assert_eq!(json["type"], "GeographicCRS");
}

#[test]
fn wkt_and_projjson_are_read_as_they_are() {
    let wgs84 = Crs::from_epsg(4326).unwrap();
    let wkt = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],
        PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433],AUTHORITY["EPSG","4326"]]"#;
    assert!(matches!(user_input::parse(wkt).unwrap(), UserInput::Wkt(_)));
    assert!(matches!(
        user_input::parse(" geogcrs [\"x\"]").unwrap(),
        UserInput::Wkt(_)
    ));
    assert!(Crs::from_user_input(wkt).unwrap().equivalent(&wgs84, true));
    let projjson = xeibe_crs::projjson(2180).unwrap();
    assert!(matches!(
        user_input::parse(projjson).unwrap(),
        UserInput::Projjson(_)
    ));
    assert_eq!(Crs::from_user_input(projjson).unwrap().code(), Some(2180));
}

#[test]
fn other_authorities_are_unsupported() {
    for text in [
        "ESRI:54009",
        "IGNF:LAMB93",
        "OGC:CRS1",
        "urn:ogc:def:crs:IAU_2015::49900",
    ] {
        assert!(
            matches!(user_input::parse(text), Err(Error::Unsupported { .. })),
            "{text}: {:?}",
            user_input::parse(text)
        );
    }
    let message = user_input::parse("ESRI:54009").unwrap_err().to_string();
    assert_eq!(
        message,
        "ESRI:54009: an ESRI code of a CRS EPSG doesn't define is not supported"
    );
}

#[test]
fn text_naming_nothing_is_an_error() {
    for text in [
        "not a crs",
        "",
        "EPSG:x",
        "EPSG::2180",
        "EPSG:1+2+3",
        "FOO:1",
        "urn:ogc:def:crs,crs:IGNF::LAMB93,crs:EPSG::5773",
        "http://www.opengis.net/def/crs-compound?1=nope",
        "99999999999",
    ] {
        assert!(
            matches!(user_input::parse(text), Err(Error::UserInput { .. })),
            "{text}: {:?}",
            user_input::parse(text)
        );
    }
    assert_eq!(
        user_input::parse("EPSG:1+2+3").unwrap_err().to_string(),
        "EPSG:1+2+3: a compound CRS needs two components, not 3"
    );
    // The form is right, but EPSG has no such CRS.
    assert_eq!(user_input::parse("EPSG:1").unwrap(), UserInput::Epsg(1));
    assert_eq!(
        Crs::from_user_input("EPSG:1").unwrap_err(),
        Error::UnknownCode(1)
    );
}
