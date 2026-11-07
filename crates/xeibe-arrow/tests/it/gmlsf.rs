//! The GML Simple Features profile 2.0 (OGC 10-100r3) read end to end
//! (`docs/geometry.md`, "GML Simple Features profile"): the profile's own
//! collections (§8.4.2), the property types of level SF-0 (§8.4.4), every
//! geometric property type of Tables 6 and 7 with the srsName inheritance of
//! §8.4.4.11.3, the complex, repeated and nil properties of level SF-1 (§9),
//! and the two instance documents of Annex D, verbatim.

use arrow_schema::{DataType, TimeUnit};
use xeibe_arrow::{OnFeatureError, ReadOptions, read};
use xeibe_schema::options::Lossless;
use xeibe_schema::rules::meta;
use xeibe_testkit::wkt::assert_wkt;

use crate::support::{Read, collect, read_document, read_with, scan_document, sources};

/// An SF-0 document in the profile's own collection (§8.4.2): the root is
/// the application's collection element and the members are its own
/// `featureMember`, in the application namespace. The feature types are
/// Annex D.1's `NewsItem` and `Reporter`, with one property of every basic
/// type of §8.4.4.1 added to `NewsItem`. The collection's envelope carries
/// the only srsName (§8.4.4.11.3).
const NEWS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<cw:NewsItems gml:id="c1" xmlns:cw="http://www.cubewerx.com/cw" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:xlink="http://www.w3.org/1999/xlink">
  <gml:boundedBy><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>50 20</gml:lowerCorner><gml:upperCorner>53 22</gml:upperCorner></gml:Envelope></gml:boundedBy>
  <cw:featureMember>
    <cw:NewsItem gml:id="n1">
      <cw:location><gml:Point gml:id="n1.p"><gml:pos>52.2 21.0</gml:pos></gml:Point></cw:location>
      <cw:reporterId>R1</cw:reporterId>
      <cw:eventDate>2010-05-01T10:30:00Z</cw:eventDate>
      <cw:day>2010-05-01</cw:day>
      <cw:age>42</cw:age>
      <cw:height uom="m">12.5</cw:height>
      <cw:score>0.25</cw:score>
      <cw:price>10.50</cw:price>
      <cw:urgent>true</cw:urgent>
      <cw:photo mimeType="image/png" role="thumbnail">iVBORw0KGgo=</cw:photo>
      <cw:homepage>http://example.com/news/n1</cw:homepage>
      <cw:reporter xlink:href="#r1"/>
      <cw:category codeSpace="http://example.com/codes/category">politics</cw:category>
    </cw:NewsItem>
  </cw:featureMember>
  <cw:featureMember>
    <cw:NewsItem gml:id="n2">
      <cw:location><gml:Point gml:id="n2.p"><gml:pos>51.0 20.5</gml:pos></gml:Point></cw:location>
      <cw:reporterId>R2</cw:reporterId>
      <cw:eventDate>2010-05-02T08:00:00Z</cw:eventDate>
      <cw:day>2010-05-02</cw:day>
      <cw:age>7</cw:age>
      <cw:height uom="m">3</cw:height>
      <cw:score>1e3</cw:score>
      <cw:price>3.00</cw:price>
      <cw:urgent>false</cw:urgent>
      <cw:homepage>http://example.com/news/n2</cw:homepage>
      <cw:reporter xlink:href="http://www.someserver.com/somedoc.xml#o1013"/>
      <cw:category codeSpace="http://example.com/codes/category">sport</cw:category>
    </cw:NewsItem>
  </cw:featureMember>
  <cw:featureMember>
    <cw:Reporter gml:id="r1">
      <cw:reporterId>R1</cw:reporterId>
      <cw:firstName>Ann</cw:firstName>
    </cw:Reporter>
  </cw:featureMember>
</cw:NewsItems>
"##;

/// One property per geometric property type of Table 6, with the member
/// containers of Table 7 and coordinates in `pos`/`posList` only
/// (§8.4.4.11.2 a). Inside a WFS 2.0 collection, as in §8.4.4.11.3's own
/// example, whose envelope (EPSG:4326) is overridden by the feature's
/// (EPSG:3857, the nearest).
const GEOMETRY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app">
  <wfs:boundedBy><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>50 20</gml:lowerCorner><gml:upperCorner>53 22</gml:upperCorner></gml:Envelope></wfs:boundedBy>
  <wfs:member>
    <app:Shapes gml:id="s1">
      <gml:boundedBy><gml:Envelope srsName="http://www.opengis.net/def/crs/EPSG/0/3857"><gml:lowerCorner>0 0</gml:lowerCorner><gml:upperCorner>7 7</gml:upperCorner></gml:Envelope></gml:boundedBy>
      <app:point><gml:Point gml:id="s1.1"><gml:pos>1 2</gml:pos></gml:Point></app:point>
      <app:lineString><gml:LineString gml:id="s1.2"><gml:posList>0 0 1 1 2 0</gml:posList></gml:LineString></app:lineString>
      <app:curve><gml:Curve gml:id="s1.3"><gml:segments><gml:LineStringSegment><gml:posList>0 0 1 1</gml:posList></gml:LineStringSegment><gml:LineStringSegment><gml:posList>1 1 2 0</gml:posList></gml:LineStringSegment></gml:segments></gml:Curve></app:curve>
      <app:arc><gml:Curve gml:id="s1.4"><gml:segments><gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc></gml:segments></gml:Curve></app:arc>
      <app:circle><gml:Curve gml:id="s1.5"><gml:segments><gml:Circle><gml:posList>0 0 1 1 2 0</gml:posList></gml:Circle></gml:segments></gml:Curve></app:circle>
      <app:circleByCenterPoint><gml:Curve gml:id="s1.6"><gml:segments><gml:CircleByCenterPoint numArc="1"><gml:pos>5 5</gml:pos><gml:radius uom="m">2</gml:radius></gml:CircleByCenterPoint></gml:segments></gml:Curve></app:circleByCenterPoint>
      <app:polygon><gml:Polygon gml:id="s1.7"><gml:exterior><gml:LinearRing><gml:posList>0 0 4 0 4 4 0 4 0 0</gml:posList></gml:LinearRing></gml:exterior><gml:interior><gml:LinearRing><gml:posList>1 1 1 2 2 2 2 1 1 1</gml:posList></gml:LinearRing></gml:interior></gml:Polygon></app:polygon>
      <app:surface><gml:Surface gml:id="s1.8"><gml:patches><gml:PolygonPatch><gml:exterior><gml:LinearRing><gml:posList>0 0 4 0 4 4 0 0</gml:posList></gml:LinearRing></gml:exterior></gml:PolygonPatch></gml:patches></gml:Surface></app:surface>
      <app:curvedSurface><gml:Surface gml:id="s1.9"><gml:patches><gml:PolygonPatch><gml:exterior><gml:Ring><gml:curveMember><gml:Curve gml:id="s1.9c"><gml:segments><gml:LineStringSegment><gml:posList>0 0 4 0 4 4</gml:posList></gml:LineStringSegment><gml:Arc><gml:posList>4 4 2 5 0 0</gml:posList></gml:Arc></gml:segments></gml:Curve></gml:curveMember></gml:Ring></gml:exterior></gml:PolygonPatch></gml:patches></gml:Surface></app:curvedSurface>
      <app:multiPoint><gml:MultiPoint gml:id="s1.10"><gml:pointMember><gml:Point gml:id="s1.10a"><gml:pos>1 1</gml:pos></gml:Point></gml:pointMember><gml:pointMember><gml:Point gml:id="s1.10b"><gml:pos>2 2</gml:pos></gml:Point></gml:pointMember></gml:MultiPoint></app:multiPoint>
      <app:multiCurve><gml:MultiCurve gml:id="s1.11"><gml:curveMember><gml:LineString gml:id="s1.11a"><gml:posList>0 0 1 1</gml:posList></gml:LineString></gml:curveMember><gml:curveMember><gml:Curve gml:id="s1.11b"><gml:segments><gml:Arc><gml:posList>0 0 1 1 2 0</gml:posList></gml:Arc></gml:segments></gml:Curve></gml:curveMember></gml:MultiCurve></app:multiCurve>
      <app:multiSurface><gml:MultiSurface gml:id="s1.12"><gml:surfaceMember><gml:Polygon gml:id="s1.12a"><gml:exterior><gml:LinearRing><gml:posList>0 0 1 0 1 1 0 0</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon></gml:surfaceMember><gml:surfaceMember><gml:Surface gml:id="s1.12b"><gml:patches><gml:PolygonPatch><gml:exterior><gml:LinearRing><gml:posList>5 5 6 5 6 6 5 5</gml:posList></gml:LinearRing></gml:exterior></gml:PolygonPatch></gml:patches></gml:Surface></gml:surfaceMember></gml:MultiSurface></app:multiSurface>
      <app:geometry><gml:MultiGeometry gml:id="s1.13"><gml:geometryMember><gml:Point gml:id="s1.13a"><gml:pos>1 1</gml:pos></gml:Point></gml:geometryMember><gml:geometryMember><gml:LineString gml:id="s1.13b"><gml:posList>0 0 1 1</gml:posList></gml:LineString></gml:geometryMember></gml:MultiGeometry></app:geometry>
      <app:point3d><gml:Point gml:id="s1.14" srsDimension="3"><gml:pos>1 2 3</gml:pos></gml:Point></app:point3d>
    </app:Shapes>
  </wfs:member>
</wfs:FeatureCollection>
"#;

/// Level SF-1 (§9): a complex property of a user-defined type (§9.3.1's
/// `Identifier`), repeated properties (maxOccurs > 1, §9.2.2) and a nil one
/// (§9.2.4, Table 8).
const WELLS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<fw:Wells gml:id="c1" xmlns:fw="http://example.com/fw" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <fw:featureMember>
    <fw:Well gml:id="w1">
      <fw:id><fw:Identifier><fw:identifier>W-1</fw:identifier><fw:idAuthority>USGS</fw:idAuthority></fw:Identifier></fw:id>
      <fw:depth uom="m">120.5</fw:depth>
      <fw:depth uom="m">130</fw:depth>
      <fw:owner xsi:nil="true" nilReason="unknown"/>
      <fw:pos><gml:Point gml:id="w1.p" srsName="http://www.opengis.net/def/crs/EPSG/0/3857"><gml:pos>500000 600000</gml:pos></gml:Point></fw:pos>
      <fw:pos><gml:Point gml:id="w1.q" srsName="http://www.opengis.net/def/crs/EPSG/0/3857"><gml:pos>500001 600001</gml:pos></gml:Point></fw:pos>
    </fw:Well>
  </fw:featureMember>
  <fw:featureMember>
    <fw:Well gml:id="w2">
      <fw:id><fw:Identifier><fw:identifier>W-2</fw:identifier><fw:description>dry</fw:description></fw:Identifier></fw:id>
      <fw:depth uom="m">80</fw:depth>
      <fw:owner>ACME</fw:owner>
      <fw:pos><gml:Point gml:id="w2.p" srsName="http://www.opengis.net/def/crs/EPSG/0/3857"><gml:pos>500002 600002</gml:pos></gml:Point></fw:pos>
    </fw:Well>
  </fw:featureMember>
</fw:Wells>
"#;

/// Annex D.5's instance document, verbatim.
const ANNEX_D5: &str = r#"<?xml version="1.0"?>
<test:Descriptions gml:id="ID0"
   xmlns:test="http://www.opengis.net/test"
   xmlns:gml="http://www.opengis.net/gml/3.2"
   xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
   xsi:schemaLocation="http://www.opengis.net/test ./test.xsd">
   <test:featureMember>
      <test:Description gml:id="ID1">
         <test:descId>12345</test:descId>
         <test:descText xml:lang="en">Hello world.</test:descText>
         <test:descText xml:lang="fr">Bonjour monde.</test:descText>
         <test:descText xml:lang="el">Γειά σου κόσµος</test:descText>
      </test:Description>
   </test:featureMember>
</test:Descriptions>
"#;

/// Annex D.6's instance document, verbatim: a single feature as the root.
const ANNEX_D6: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rz:RadZones
   gml:id="RZ001"
   xmlns:rz="http://www.someserver.com/rz"
   xmlns:gml="http://www.opengis.net/gml/3.2"
   xmlns:xlink="http://www.w3.org/1999/xlink"
   xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
   xsi:schemaLocation="http://www.someserver.com/rz
                        http://www.someserver.com/schemas/RadZones.xsd
                        http://www.opengis.net/gml/3.2
                        http://schemas.opengis.net/gml/3.2.1/gml.xsd">
   <rz:title>Radiation Hot Spots</rz:title>
   <rz:abstract>Radiation hot spots in the vicinity of the AnyTown nuclear reactor. Only
readings that exceed the background radiation of 100 millirems per hour.</rz:abstract>
   <rz:rems uom="REM">170</rz:rems>
   <rz:where>
      <gml:Surface
         gml:id="RZ001_1"
         srsName="http://www.opengis.net/def/crs/EPSG/0/4326">
         <gml:patches>
             <gml:PolygonPatch>
                <gml:exterior>
                   <gml:Ring>
                      <gml:curveMember>
                         <gml:Curve gml:id="C01">
                            <gml:segments>
                                <gml:CircleByCenterPoint numArc="1">
                                   <gml:pos>51.389 30.099</gml:pos>
                                   <gml:radius uom="m">20000</gml:radius>
                                </gml:CircleByCenterPoint>
                            </gml:segments>
                         </gml:Curve>
                      </gml:curveMember>
                   </gml:Ring>
                </gml:exterior>
             </gml:PolygonPatch>
         </gml:patches>
      </gml:Surface>
   </rz:where>
</rz:RadZones>
"#;

/// A constant attribute of a column's values, which moves into its metadata.
fn attribute(read: &Read, column: &str, attribute: &str) -> Option<String> {
    read.field(column)
        .metadata()
        .get(&format!("{}{attribute}", meta::ATTR_PREFIX))
        .cloned()
}

fn srs_name(read: &Read, column: &str) -> Option<String> {
    read.field(column).metadata().get(meta::SRS_NAME).cloned()
}

#[test]
fn the_profiles_own_collection_is_read_with_default_options() {
    // §8.4.2: `cw:NewsItems` with `cw:featureMember`s, no GML or WFS
    // container. Each feature type is a layer.
    let scan = scan_document(NEWS);
    let layers: Vec<_> = scan
        .layers()
        .iter()
        .map(|layer| (layer.name.local.to_string(), layer.feature_count))
        .collect();
    assert_eq!(
        layers,
        [("NewsItem".to_string(), 2), ("Reporter".to_string(), 1)]
    );
    assert_eq!(read_document(NEWS, "Reporter").rows(), 1);
}

#[test]
fn sf0_property_types_get_typed_columns() {
    let read = read_document(NEWS, "NewsItem");
    assert_eq!(
        read.column_names(),
        [
            "@id",
            "location",
            "reporterId",
            "eventDate",
            "day",
            "age",
            "height",
            "score",
            "price",
            "urgent",
            "photo",
            "homepage",
            "reporter",
            "category"
        ]
    );

    // §8.4.4.5 integers, §8.4.4.15 reals (`xsd:double` and `xsd:decimal`,
    // both `Float64`: there is no decimal type).
    assert_eq!(read.i64s("age"), [Some(42), Some(7)]);
    assert_eq!(read.f64s("score"), [Some(0.25), Some(1000.0)]);
    assert_eq!(read.f64s("price"), [Some(10.5), Some(3.0)]);

    // §8.4.4.6 measurements: the value, and its constant `uom` as metadata.
    assert_eq!(read.f64s("height"), [Some(12.5), Some(3.0)]);
    assert_eq!(attribute(&read, "height", "uom").as_deref(), Some("m"));

    // §8.4.4.7 strings, §8.4.4.8 `xsd:dateTime` and `xsd:date`, §8.4.4.9
    // booleans.
    assert_eq!(read.data_type("reporterId"), DataType::Utf8View);
    assert_eq!(
        read.data_type("eventDate"),
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
    );
    assert_eq!(read.data_type("day"), DataType::Date32);
    assert_eq!(read.bools("urgent"), [Some(true), Some(false)]);

    // §8.4.4.10 binary content stays base64 text, with its constant
    // `mimeType` and `role` as metadata. Table 5: an optional property that
    // is absent is null.
    assert_eq!(read.strings("photo"), [Some("iVBORw0KGgo=".into()), None]);
    assert_eq!(
        attribute(&read, "photo", "mimeType").as_deref(),
        Some("image/png")
    );
    assert_eq!(
        attribute(&read, "photo", "role").as_deref(),
        Some("thumbnail")
    );

    // §8.4.4.12 URIs are text; §8.4.4.13 references are their `xlink:href`,
    // a local one without its `#` so that it joins on `@id`.
    assert_eq!(read.data_type("homepage"), DataType::Utf8View);
    assert_eq!(
        read.strings("reporter"),
        [
            Some("r1".into()),
            Some("http://www.someserver.com/somedoc.xml#o1013".into())
        ]
    );

    // §8.4.4.14 code lists: the code, and its constant `codeSpace`.
    assert_eq!(
        read.strings("category"),
        [Some("politics".into()), Some("sport".into())]
    );
    assert_eq!(
        attribute(&read, "category", "codeSpace").as_deref(),
        Some("http://example.com/codes/category")
    );
}

#[test]
fn geometries_take_the_srs_name_of_the_collection_envelope() {
    // §8.4.4.11.3: no geometry has an srsName, so the collection's envelope
    // gives it; its URN puts latitude first, so the positions are swapped.
    let read = read_document(NEWS, "NewsItem");
    assert_eq!(
        srs_name(&read, "location").as_deref(),
        Some("urn:ogc:def:crs:EPSG::4326")
    );
    let points = read.geometries("location");
    assert_wkt(points[0].as_ref().expect("a point"), "POINT (21 52.2)");
    assert_wkt(points[1].as_ref().expect("a point"), "POINT (20.5 51)");
}

#[test]
fn sf_booleans_written_as_1_and_0_need_lossy_inference() {
    // §8.4.4.9 allows {true, 1, false, 0}. A lossless read keeps a column
    // that mixes the two spellings as text, since `1` and `0` alone are
    // integers; `Lossy` reads them as booleans (`docs/type-mapping.md`,
    // "Scalars").
    let document = NEWS.replace("<cw:urgent>false</cw:urgent>", "<cw:urgent>0</cw:urgent>");
    let lossless = read_document(&document, "NewsItem");
    assert_eq!(
        lossless.strings("urgent"),
        [Some("true".into()), Some("0".into())]
    );

    let mut options = ReadOptions::default();
    options.inference.types.lossless = Lossless::Lossy;
    let lossy = read_with(&document, "NewsItem", None, &options);
    assert_eq!(lossy.bools("urgent"), [Some(true), Some(false)]);
}

#[test]
fn every_geometric_property_type_of_table_6_reads() {
    let read = read_document(GEOMETRY, "Shapes");
    let expected = [
        ("point", "POINT (1 2)"),
        ("lineString", "LINESTRING (0 0,1 1,2 0)"),
        // A Curve of LineStringSegments is a LineString; Arc, Circle and
        // CircleByCenterPoint segments stay curves (`docs/geometry.md`).
        ("curve", "LINESTRING (0 0,1 1,2 0)"),
        ("arc", "CIRCULARSTRING (0 0,1 1,2 0)"),
        ("circle", "CIRCULARSTRING (0 0,1 1,2 0,1 -1,0 0)"),
        // West, north, east, south, west, as in GDAL.
        (
            "circleByCenterPoint",
            "CIRCULARSTRING (3 5,5 7,7 5,5 3,3 5)",
        ),
        (
            "polygon",
            "POLYGON ((0 0,4 0,4 4,0 4,0 0),(1 1,1 2,2 2,2 1,1 1))",
        ),
        ("surface", "POLYGON ((0 0,4 0,4 4,0 0))"),
        (
            "curvedSurface",
            "CURVEPOLYGON (COMPOUNDCURVE ((0 0,4 0,4 4),CIRCULARSTRING (4 4,2 5,0 0)))",
        ),
        ("multiPoint", "MULTIPOINT ((1 1),(2 2))"),
        (
            "multiCurve",
            "MULTICURVE ((0 0,1 1),CIRCULARSTRING (0 0,1 1,2 0))",
        ),
        (
            "multiSurface",
            "MULTIPOLYGON (((0 0,1 0,1 1,0 0)),((5 5,6 5,6 6,5 5)))",
        ),
        (
            "geometry",
            "GEOMETRYCOLLECTION (POINT (1 1),LINESTRING (0 0,1 1))",
        ),
        // §8.4.4.11.2 c: CRSs may have up to 3 dimensions.
        ("point3d", "POINT Z (1 2 3)"),
    ];
    let mut columns = vec!["@id"];
    columns.extend(expected.iter().map(|(column, _)| *column));
    assert_eq!(read.column_names(), columns);
    for (column, wkt) in expected {
        let geometries = read.geometries(column);
        assert_wkt(geometries[0].as_ref().expect(column), wkt);
        // §8.4.4.11.3: the nearest envelope, the feature's, gives the CRS,
        // not the collection's.
        assert_eq!(
            srs_name(&read, column).as_deref(),
            Some("http://www.opengis.net/def/crs/EPSG/0/3857"),
            "{column}"
        );
    }
    assert!(read.report.skipped.is_empty(), "{:?}", read.report.skipped);
}

#[test]
fn sf1_complex_repeated_and_nil_properties_follow_the_generic_rules() {
    let read = read_document(WELLS, "Well");
    assert_eq!(
        read.column_names(),
        [
            "@id",
            "identifier",
            "idAuthority",
            "description",
            "depth",
            "owner",
            "@nilReason",
            "pos"
        ]
    );
    // §9.3: the fields of a complex property are flat columns.
    assert_eq!(
        read.strings("identifier"),
        [Some("W-1".into()), Some("W-2".into())]
    );
    assert_eq!(read.strings("idAuthority"), [Some("USGS".into()), None]);
    assert_eq!(read.strings("description"), [None, Some("dry".into())]);

    // §9.2.2: a property seen more than once is a list, a geometry too.
    assert!(
        matches!(read.data_type("depth"), DataType::List(item) if item.data_type() == &DataType::Float64),
        "{}",
        read.data_type("depth")
    );
    let positions = read.geometry_lists("pos");
    let counts: Vec<_> = positions
        .iter()
        .map(|list| list.as_ref().map(Vec::len))
        .collect();
    assert_eq!(counts, [Some(2), Some(1)]);

    // §9.2.4, Table 8: `xsi:nil="true"` is null, and its `nilReason` is kept.
    assert_eq!(read.strings("owner"), [None, Some("ACME".into())]);
    assert_eq!(read.strings("@nilReason"), [Some("unknown".into()), None]);
}

#[test]
fn annex_d5_internationalized_strings_keep_their_language() {
    // §8.4.4.7.2: `descText` repeats, each with its `xml:lang`, which is a
    // list aligned with it.
    let read = read_document(ANNEX_D5, "Description");
    assert_eq!(read.column_names(), ["@id", "descId", "descText", "@lang"]);
    assert_eq!(read.strings("@id"), [Some("ID1".into())]);
    // Read without its schema, the string of digits is an integer.
    assert_eq!(read.i64s("descId"), [Some(12345)]);
    let texts = ["Hello world.", "Bonjour monde.", "Γειά σου κόσµος"];
    assert_eq!(
        read.string_lists("descText"),
        [Some(texts.map(|text| Some(text.to_string())).to_vec())]
    );
    assert_eq!(
        read.string_lists("@lang"),
        [Some(
            ["en", "fr", "el"]
                .map(|lang| Some(lang.to_string()))
                .to_vec()
        )]
    );
}

#[test]
fn annex_d6_reads_as_a_single_feature_but_its_geographic_circle_is_unsupported() {
    // The root is the feature itself, which a read takes with
    // `allow_single_feature_root`. Its circle has a radius in metres in
    // EPSG:4326, which would need a lossy geodesic linearization
    // (support matrix §5.2, 🤔 Considering): a feature error.
    let mut options = ReadOptions::default();
    options.splitter.allow_single_feature_root = true;
    let reader = read(sources(ANNEX_D6), "RadZones", None, &options).expect("a reader");
    let error = reader
        .into_iter()
        .find_map(Result::err)
        .expect("the circle is a feature error");
    assert!(
        error
            .to_string()
            .contains("CircleByCenterPoint in a geographic CRS"),
        "{error}"
    );

    // `NullGeometry` keeps the feature's other properties.
    options.on_feature_error = OnFeatureError::NullGeometry;
    let read = collect(read(sources(ANNEX_D6), "RadZones", None, &options).expect("a reader"));
    assert_eq!(read.strings("@id"), [Some("RZ001".into())]);
    assert_eq!(read.strings("title"), [Some("Radiation Hot Spots".into())]);
    let abstract_ = read.strings("abstract");
    assert!(
        abstract_[0].as_deref().is_some_and(|text| text
            .starts_with("Radiation hot spots in the vicinity")
            && text.ends_with("100 millirems per hour.")),
        "{abstract_:?}"
    );
    assert_eq!(read.i64s("rems"), [Some(170)]);
    assert_eq!(attribute(&read, "rems", "uom").as_deref(), Some("REM"));
    assert_eq!(read.geometries("where"), [None]);
}
