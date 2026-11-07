//! `DescribeFeatureType` schemas read for a feature type's direct properties
//! (`docs/wfs.md`, "Schema hint from `DescribeFeatureType`"), in the forms
//! GeoServer, MapServer and ArcGIS Server write them.

use xeibe_wfs::describe::DeclaredProperty;
use xeibe_wfs::{Error, FeatureTypeSchema};

const XSD: &str = "http://www.w3.org/2001/XMLSchema";
const GML_32: &str = "http://www.opengis.net/gml/3.2";
const GML: &str = "http://www.opengis.net/gml";
const APP: &str = "http://example.com/app";
const MS: &str = "http://mapserver.gis.umn.edu/mapserver";

/// `(name, (type namespace, type), repeated)` of a property.
type Summary<'a> = (&'a str, Option<(Option<&'a str>, &'a str)>, bool);

fn summary(schema: &FeatureTypeSchema) -> Vec<Summary<'_>> {
    schema
        .properties
        .iter()
        .map(|property: &DeclaredProperty| {
            (
                property.name.as_str(),
                property
                    .type_name
                    .as_ref()
                    .map(|(ns, local)| (ns.as_deref(), local.as_str())),
                property.repeated,
            )
        })
        .collect()
}

/// GeoServer, WFS 2.0: prefixed XSD, a named type extending
/// `gml:AbstractFeatureType`, and the element after its type.
const GEOSERVER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:app="http://example.com/app" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:wfs="http://www.opengis.net/wfs/2.0" elementFormDefault="qualified" targetNamespace="http://example.com/app">
  <xsd:import namespace="http://www.opengis.net/gml/3.2" schemaLocation="http://localhost/geoserver/schemas/gml/3.2.1/gml.xsd"/>
  <xsd:complexType name="ParcelType">
    <xsd:complexContent>
      <xsd:extension base="gml:AbstractFeatureType">
        <xsd:sequence>
          <xsd:element maxOccurs="1" minOccurs="0" name="the_geom" nillable="true" type="gml:MultiSurfacePropertyType"/>
          <xsd:element maxOccurs="1" minOccurs="0" name="code" nillable="true" type="xsd:string"/>
          <xsd:element maxOccurs="1" minOccurs="0" name="area" nillable="true" type="xsd:double"/>
          <xsd:element maxOccurs="1" minOccurs="0" name="count" nillable="true" type="xsd:long"/>
          <xsd:element maxOccurs="1" minOccurs="0" name="updated" nillable="true" type="xsd:dateTime"/>
          <xsd:element maxOccurs="unbounded" minOccurs="0" name="tag" type="xsd:string"/>
        </xsd:sequence>
      </xsd:extension>
    </xsd:complexContent>
  </xsd:complexType>
  <xsd:element name="Parcel" substitutionGroup="gml:AbstractFeature" type="app:ParcelType"/>
</xsd:schema>"#;

#[test]
fn a_geoserver_schema_declares_its_properties_in_order() {
    let schema = FeatureTypeSchema::parse(GEOSERVER.as_bytes(), Some(APP), "Parcel")
        .expect("the schema reads");
    assert_eq!(schema.namespace.as_deref(), Some(APP));
    assert_eq!(schema.name, "Parcel");
    assert_eq!(
        summary(&schema),
        [
            (
                "the_geom",
                Some((Some(GML_32), "MultiSurfacePropertyType")),
                false
            ),
            ("code", Some((Some(XSD), "string")), false),
            ("area", Some((Some(XSD), "double")), false),
            ("count", Some((Some(XSD), "long")), false),
            ("updated", Some((Some(XSD), "dateTime")), false),
            ("tag", Some((Some(XSD), "string")), true),
        ]
    );
    assert!(
        schema
            .properties
            .iter()
            .all(|property| property.namespace.as_deref() == Some(APP)),
        "elementFormDefault=qualified"
    );
}

/// MapServer: XSD as the default namespace, so the built-in types are
/// unprefixed, and GML 3.1.
const MAPSERVER: &str = r#"<?xml version='1.0' encoding="UTF-8" ?>
<schema targetNamespace="http://mapserver.gis.umn.edu/mapserver" xmlns:ms="http://mapserver.gis.umn.edu/mapserver" xmlns:ogc="http://www.opengis.net/ogc" xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns="http://www.w3.org/2001/XMLSchema" xmlns:gml="http://www.opengis.net/gml" elementFormDefault="qualified" version="0.1" >
  <import namespace="http://www.opengis.net/gml" schemaLocation="http://schemas.opengis.net/gml/3.1.1/base/gml.xsd" />
  <element name="roads" type="ms:roadsType" substitutionGroup="gml:_Feature" />
  <complexType name="roadsType">
    <complexContent>
      <extension base="gml:AbstractFeatureType">
        <sequence>
          <element name="msGeometry" type="gml:GeometryPropertyType" minOccurs="0" maxOccurs="1"/>
          <element name="name" minOccurs="0" type="string"/>
          <element name="lanes" minOccurs="0" type="integer"/>
          <element name="opened" minOccurs="0" type="date"/>
        </sequence>
      </extension>
    </complexContent>
  </complexType>
</schema>"#;

#[test]
fn unprefixed_types_resolve_in_the_default_namespace() {
    let schema = FeatureTypeSchema::parse(MAPSERVER.as_bytes(), Some(MS), "roads")
        .expect("the schema reads");
    assert_eq!(
        summary(&schema),
        [
            (
                "msGeometry",
                Some((Some(GML), "GeometryPropertyType")),
                false
            ),
            ("name", Some((Some(XSD), "string")), false),
            ("lanes", Some((Some(XSD), "integer")), false),
            ("opened", Some((Some(XSD), "date")), false),
        ]
    );
}

/// ArcGIS Server: the element first, and inline simple types restricting a
/// built-in one (`maxLength`, `totalDigits`).
const ARCGIS: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<xs:schema targetNamespace="https://example.com/arcgis/wells" xmlns:esri="https://example.com/arcgis/wells" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:gml="http://www.opengis.net/gml/3.2" elementFormDefault="qualified" attributeFormDefault="unqualified">
  <xs:import namespace="http://www.opengis.net/gml/3.2" schemaLocation="http://schemas.opengis.net/gml/3.2.1/gml.xsd"/>
  <xs:element name="Wells" type="esri:WellsFeatureType" substitutionGroup="gml:AbstractFeature"/>
  <xs:complexType name="WellsFeatureType">
    <xs:complexContent>
      <xs:extension base="gml:AbstractFeatureType">
        <xs:sequence>
          <xs:element minOccurs="0" maxOccurs="1" name="OBJECTID" type="xs:int"/>
          <xs:element minOccurs="0" maxOccurs="1" name="NAME" nillable="true">
            <xs:simpleType><xs:restriction base="xs:string"><xs:maxLength value="50"/></xs:restriction></xs:simpleType>
          </xs:element>
          <xs:element minOccurs="0" maxOccurs="1" name="DEPTH" nillable="true">
            <xs:simpleType><xs:restriction base="xs:decimal"><xs:totalDigits value="38"/></xs:restriction></xs:simpleType>
          </xs:element>
          <xs:element minOccurs="0" maxOccurs="1" name="Shape" nillable="true" type="gml:PointPropertyType"/>
        </xs:sequence>
      </xs:extension>
    </xs:complexContent>
  </xs:complexType>
</xs:schema>"#;

#[test]
fn an_inline_simple_type_is_its_built_in_base() {
    let schema = FeatureTypeSchema::parse(
        ARCGIS.as_bytes(),
        Some("https://example.com/arcgis/wells"),
        "Wells",
    )
    .expect("the schema reads");
    assert_eq!(
        summary(&schema),
        [
            ("OBJECTID", Some((Some(XSD), "int")), false),
            ("NAME", Some((Some(XSD), "string")), false),
            ("DEPTH", Some((Some(XSD), "decimal")), false),
            ("Shape", Some((Some(GML_32), "PointPropertyType")), false),
        ]
    );
}

/// What the reader follows within the document, and what it leaves out.
const STRUCTURES: &str = r###"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:app="http://example.com/app" xmlns:gml="http://www.opengis.net/gml/3.2" targetNamespace="http://example.com/app" elementFormDefault="qualified">
  <xs:simpleType name="Code"><xs:restriction base="app:ShortCode"/></xs:simpleType>
  <xs:simpleType name="ShortCode"><xs:restriction base="xs:token"><xs:maxLength value="4"/></xs:restriction></xs:simpleType>
  <xs:complexType name="BaseType">
    <xs:complexContent>
      <xs:extension base="gml:AbstractFeatureType">
        <xs:sequence><xs:element name="created" type="xs:date"/></xs:sequence>
      </xs:extension>
    </xs:complexContent>
  </xs:complexType>
  <xs:element name="Building">
    <xs:complexType>
      <xs:complexContent>
        <xs:extension base="app:BaseType">
          <xs:sequence>
            <xs:element ref="gml:boundedBy"/>
            <xs:element name="code" type="app:Code"/>
            <xs:element name="address">
              <xs:complexType><xs:sequence><xs:element name="street" type="xs:string"/></xs:sequence></xs:complexType>
            </xs:element>
            <xs:choice maxOccurs="unbounded">
              <xs:element name="phone" type="xs:string"/>
              <xs:element name="email" type="xs:string"/>
            </xs:choice>
            <xs:element name="local" form="unqualified" type="xs:boolean"/>
            <xs:any namespace="##other" minOccurs="0"/>
          </xs:sequence>
        </xs:extension>
      </xs:complexContent>
    </xs:complexType>
  </xs:element>
</xs:schema>"###;

#[test]
fn bases_simple_types_and_groups_in_the_document_are_followed() {
    let schema = FeatureTypeSchema::parse(STRUCTURES.as_bytes(), None, "Building")
        .expect("the schema reads");
    assert_eq!(
        summary(&schema),
        [
            // The base type's properties first.
            ("created", Some((Some(XSD), "date")), false),
            // `ref` (gml:boundedBy) and `xs:any` are left out; a simple
            // type is followed to the built-in one it restricts.
            ("code", Some((Some(XSD), "token")), false),
            // An inline complex type has no type name.
            ("address", None, false),
            // A repeated group repeats its elements.
            ("phone", Some((Some(XSD), "string")), true),
            ("email", Some((Some(XSD), "string")), true),
            ("local", Some((Some(XSD), "boolean")), false),
        ]
    );
    let local = schema.properties.last().expect("local");
    assert_eq!(local.namespace, None, "form=unqualified");
}

#[test]
fn a_response_that_is_no_schema_of_the_type_is_an_error() {
    let describe = |xsd: &str, ns: Option<&str>, name: &str| {
        FeatureTypeSchema::parse(xsd.as_bytes(), ns, name).expect_err("an error")
    };
    let error = describe(GEOSERVER, Some(APP), "Road");
    assert!(
        matches!(&error, Error::Describe(message) if message.contains("no element Road")),
        "{error}"
    );
    let error = describe(GEOSERVER, Some("http://example.com/other"), "Parcel");
    assert!(
        matches!(&error, Error::Describe(message) if message.contains("namespace")),
        "{error}"
    );
    let error = describe("<html><body>Not found</body></html>", None, "Parcel");
    assert!(
        matches!(&error, Error::Describe(message) if message.contains("not an XML schema")),
        "{error}"
    );
    let exception = r#"<ows:ExceptionReport xmlns:ows="http://www.opengis.net/ows/1.1" version="2.0.0">
  <ows:Exception exceptionCode="OperationNotSupported"><ows:ExceptionText>no</ows:ExceptionText></ows:Exception>
</ows:ExceptionReport>"#;
    assert!(matches!(
        describe(exception, None, "Parcel"),
        Error::Exception(_)
    ));
}
