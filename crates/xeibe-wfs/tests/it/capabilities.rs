//! `GetCapabilities` parsing for WFS 1.0, 1.1 and 2.0 (`docs/wfs.md`,
//! "Capabilities").

use xeibe_wfs::{Capabilities, WfsVersion};

const WFS_20: &str = r#"<?xml version="1.0"?>
<wfs:WFS_Capabilities xmlns:wfs="http://www.opengis.net/wfs/2.0"
    xmlns:ows="http://www.opengis.net/ows/1.1" xmlns:xlink="http://www.w3.org/1999/xlink"
    version="2.0.0">
  <ows:ServiceIdentification><ows:Title>Test service</ows:Title></ows:ServiceIdentification>
  <ows:OperationsMetadata>
    <ows:Operation name="GetFeature">
      <ows:DCP><ows:HTTP><ows:Get xlink:href="https://example.com/wfs?"/></ows:HTTP></ows:DCP>
      <ows:Parameter name="outputFormat">
        <ows:AllowedValues>
          <ows:Value>application/gml+xml; version=3.2</ows:Value>
          <ows:Value>text/xml; subtype=gml/3.1.1</ows:Value>
        </ows:AllowedValues>
      </ows:Parameter>
      <ows:Constraint name="CountDefault"><ows:DefaultValue>5000</ows:DefaultValue></ows:Constraint>
    </ows:Operation>
    <ows:Constraint name="ImplementsResultPaging"><ows:DefaultValue>TRUE</ows:DefaultValue></ows:Constraint>
    <ows:Constraint name="KVPEncoding"><ows:DefaultValue>TRUE</ows:DefaultValue></ows:Constraint>
  </ows:OperationsMetadata>
  <wfs:FeatureTypeList>
    <wfs:FeatureType xmlns:ms="http://mapserver.gis.umn.edu/mapserver">
      <wfs:Name>ms:AD.Address</wfs:Name>
      <wfs:Title>Addresses</wfs:Title>
      <wfs:DefaultCRS>urn:ogc:def:crs:EPSG::2180</wfs:DefaultCRS>
      <wfs:OtherCRS>urn:ogc:def:crs:EPSG::4326</wfs:OtherCRS>
      <ows:WGS84BoundingBox>
        <ows:LowerCorner>14.1 49.0</ows:LowerCorner>
        <ows:UpperCorner>24.2 54.9</ows:UpperCorner>
      </ows:WGS84BoundingBox>
    </wfs:FeatureType>
  </wfs:FeatureTypeList>
</wfs:WFS_Capabilities>"#;

const WFS_11: &str = r#"<?xml version="1.0"?>
<wfs:WFS_Capabilities xmlns:wfs="http://www.opengis.net/wfs"
    xmlns:ows="http://www.opengis.net/ows" xmlns:xlink="http://www.w3.org/1999/xlink"
    version="1.1.0">
  <ows:ServiceIdentification><ows:Title>Old service</ows:Title></ows:ServiceIdentification>
  <ows:OperationsMetadata>
    <ows:Operation name="GetFeature">
      <ows:DCP><ows:HTTP><ows:Get xlink:href="https://example.com/wfs11?"/></ows:HTTP></ows:DCP>
    </ows:Operation>
  </ows:OperationsMetadata>
  <wfs:FeatureTypeList>
    <wfs:FeatureType>
      <wfs:Name>sa:StationMesureEauxSurface</wfs:Name>
      <wfs:DefaultSRS>urn:ogc:def:crs:EPSG::4326</wfs:DefaultSRS>
      <wfs:OtherSRS>EPSG:2154</wfs:OtherSRS>
    </wfs:FeatureType>
  </wfs:FeatureTypeList>
</wfs:WFS_Capabilities>"#;

const WFS_10: &str = r#"<?xml version="1.0"?>
<WFS_Capabilities xmlns="http://www.opengis.net/wfs" version="1.0.0">
  <Service><Name>WFS</Name><Title>Ancient service</Title></Service>
  <Capability><Request><GetFeature>
    <ResultFormat><GML2/></ResultFormat>
    <DCPType><HTTP><Get onlineResource="https://example.com/wfs10?"/></HTTP></DCPType>
  </GetFeature></Request></Capability>
  <FeatureTypeList>
    <FeatureType>
      <Name>ogdwien:SCOOTERABSTELLOGD</Name>
      <Title>Scooters</Title>
      <SRS>EPSG:31256</SRS>
      <LatLongBoundingBox minx="16.1" miny="48.1" maxx="16.6" maxy="48.4"/>
    </FeatureType>
  </FeatureTypeList>
</WFS_Capabilities>"#;

#[test]
fn reads_a_wfs_20_capabilities_document() {
    let capabilities = Capabilities::parse(WFS_20.as_bytes()).expect("parsed");
    assert_eq!(capabilities.version, WfsVersion::V2_0_0);
    assert_eq!(capabilities.service_title.as_deref(), Some("Test service"));
    assert_eq!(capabilities.get_feature_url, "https://example.com/wfs?");
    assert_eq!(capabilities.constraints.kvp_encoding, Some(true));
    assert_eq!(capabilities.constraints.implements_result_paging, Some(true));
    assert_eq!(capabilities.constraints.count_default, Some(5000));

    let feature_type = capabilities.feature_type("ms:AD.Address").expect("the type");
    assert_eq!(feature_type.title.as_deref(), Some("Addresses"));
    assert_eq!(
        feature_type.default_crs.as_deref(),
        Some("urn:ogc:def:crs:EPSG::2180")
    );
    assert_eq!(feature_type.other_crs, ["urn:ogc:def:crs:EPSG::4326"]);
    assert_eq!(
        feature_type.namespace.as_deref(),
        Some("http://mapserver.gis.umn.edu/mapserver"),
        "the prefix must be resolvable for the NAMESPACES parameter"
    );
    assert_eq!(feature_type.wgs84_bbox, Some([14.1, 49.0, 24.2, 54.9]));
}

#[test]
fn reads_the_older_versions_spellings() {
    let wfs11 = Capabilities::parse(WFS_11.as_bytes()).expect("parsed");
    assert_eq!(wfs11.version, WfsVersion::V1_1_0);
    let feature_type = wfs11
        .feature_type("sa:StationMesureEauxSurface")
        .expect("the type");
    assert_eq!(
        feature_type.default_crs.as_deref(),
        Some("urn:ogc:def:crs:EPSG::4326"),
        "DefaultSRS in 1.1"
    );
    assert_eq!(feature_type.other_crs, ["EPSG:2154"]);

    let wfs10 = Capabilities::parse(WFS_10.as_bytes()).expect("parsed");
    assert_eq!(wfs10.version, WfsVersion::V1_0_0);
    assert_eq!(wfs10.get_feature_url, "https://example.com/wfs10?");
    let feature_type = wfs10
        .feature_type("ogdwien:SCOOTERABSTELLOGD")
        .expect("the type");
    assert_eq!(feature_type.default_crs.as_deref(), Some("EPSG:31256"), "SRS in 1.0");
    assert_eq!(
        feature_type.wgs84_bbox,
        Some([16.1, 48.1, 16.6, 48.4]),
        "LatLongBoundingBox is lon/lat"
    );
}

#[test]
fn the_best_gml_output_format_is_picked() {
    let capabilities = Capabilities::parse(WFS_20.as_bytes()).expect("parsed");
    let feature_type = capabilities.feature_type("ms:AD.Address").expect("the type");
    assert_eq!(
        capabilities.preferred_output_format(feature_type).as_deref(),
        Some("application/gml+xml; version=3.2"),
        "GML 3.2 before 3.1.1 before 2"
    );

    let wfs10 = Capabilities::parse(WFS_10.as_bytes()).expect("parsed");
    let feature_type = wfs10.feature_type("ogdwien:SCOOTERABSTELLOGD").expect("the type");
    assert_eq!(
        wfs10.preferred_output_format(feature_type).as_deref(),
        Some("GML2"),
        "1.0 advertises formats as ResultFormat children"
    );
}

#[test]
fn a_type_can_be_found_with_or_without_its_prefix() {
    let capabilities = Capabilities::parse(WFS_20.as_bytes()).expect("parsed");
    assert!(capabilities.feature_type("AD.Address").is_some());
    assert!(capabilities.feature_type("ms:AD.Address").is_some());
    assert!(capabilities.feature_type("nope").is_none());
}

#[test]
fn a_document_that_is_not_capabilities_is_an_error() {
    assert!(Capabilities::parse(b"<html><body>404</body></html>").is_err());
    assert!(Capabilities::parse(b"not xml at all").is_err());
}

#[test]
fn missing_constraints_are_unknown_rather_than_false() {
    let wfs11 = Capabilities::parse(WFS_11.as_bytes()).expect("parsed");
    assert_eq!(wfs11.constraints.implements_result_paging, None);
    assert_eq!(wfs11.constraints.count_default, None);
    assert_eq!(wfs11.constraints.paging_is_transaction_safe, None);
}
