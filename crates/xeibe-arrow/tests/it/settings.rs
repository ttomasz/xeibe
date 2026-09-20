//! The settings file: read options and per-layer schemas as JSON
//! (`docs/architecture.md`, "Settings file").
//!
//! Types are Arrow `DataType` strings, as `arrow-schema` prints and parses
//! them; geometry columns use `Geometry` or `Geometry(<kind>[, <dims>])`.

use arrow_schema::{DataType, Field, Schema, TimeUnit};
use xeibe_arrow::{ColumnSpec, ReadOptions, Settings};
use xeibe_geom::AxisOrderMode;

use crate::support::temp_dir;

const EXAMPLE: &str = r#"{
  "format_version": 1,
  "options": {
    "inference": { "naming": { "attribute_prefix": "@" }, "geometry": { "axis": "XY" } },
    "on_mismatch": "Overflow"
  },
  "layers": {
    "prgad:AD_PunktAdresowy": {
      "@id": "Utf8View",
      "idIIP": "Struct(\"lokalnyId\": Utf8View, \"wersjaId\": Timestamp(Microsecond, \"UTC\"))",
      "georeferencja": "Geometry(Point)",
      "dataNadania": "Date32"
    }
  }
}"#;

fn load(json: &str) -> Settings {
    let dir = temp_dir("settings");
    let path = dir.join("settings.json");
    std::fs::write(&path, json).unwrap();
    Settings::load(&path).expect("the settings file loads")
}

#[test]
fn loads_the_documented_example() {
    let settings = load(EXAMPLE);
    assert_eq!(settings.format_version, Settings::FORMAT_VERSION);
    assert_eq!(
        settings.options.inference.geometry.axis.mode,
        AxisOrderMode::XY
    );
    assert_eq!(settings.layers.len(), 1);
    let layer = &settings.layers["prgad:AD_PunktAdresowy"];
    assert_eq!(
        layer.keys().collect::<Vec<_>>(),
        ["@id", "idIIP", "georeferencja", "dataNadania"],
        "columns keep their order"
    );
}

#[test]
fn turns_column_specs_into_an_arrow_schema() {
    let settings = load(EXAMPLE);
    let schema = settings.schema("prgad:AD_PunktAdresowy").expect("a schema");
    assert_eq!(schema.field(0).data_type(), &DataType::Utf8View);
    assert!(matches!(schema.field(1).data_type(), DataType::Struct(_)));
    assert_eq!(schema.field(3).data_type(), &DataType::Date32);
    assert!(
        schema.fields().iter().all(|f| f.is_nullable()),
        "every column is nullable"
    );
    // The geometry column gets its GeoArrow extension type back.
    assert_eq!(
        schema
            .field(2)
            .metadata()
            .get("ARROW:extension:name")
            .map(String::as_str),
        Some("geoarrow.point")
    );
}

#[test]
fn a_schema_can_be_stored_and_read_back() {
    let schema = Schema::new(vec![
        Field::new("@id", DataType::Utf8View, true),
        Field::new("count", DataType::Int64, true),
        Field::new(
            "t",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
        Field::new(
            "tags",
            DataType::List(std::sync::Arc::new(Field::new(
                "item",
                DataType::Utf8View,
                true,
            ))),
            true,
        ),
    ]);
    let mut settings = Settings {
        format_version: Settings::FORMAT_VERSION,
        options: ReadOptions::default(),
        layers: Default::default(),
    };
    settings.set_schema("Parcel", &schema).expect("stored");

    let dir = temp_dir("settings-round-trip");
    let path = dir.join("out.json");
    settings.save(&path).expect("saved");

    let reloaded = Settings::load(&path).expect("loaded");
    let back = reloaded.schema("Parcel").expect("a schema");
    assert_eq!(back.fields().len(), schema.fields().len());
    for (a, b) in back.fields().iter().zip(schema.fields()) {
        assert_eq!(a.name(), b.name());
        assert_eq!(a.data_type(), b.data_type(), "{}", a.name());
    }

    // The file is JSON a person can edit.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"format_version\""), "{text}");
    assert!(text.contains("\"Utf8View\""), "{text}");
}

#[test]
fn a_column_can_name_the_xml_path_it_comes_from() {
    let settings = load(
        r#"{
          "format_version": 1,
          "layers": { "Parcel": { "postcode": { "type": "Utf8View", "path": "kodPocztowy" } } }
        }"#,
    );
    let spec = &settings.layers["Parcel"]["postcode"];
    assert_eq!(
        spec,
        &ColumnSpec::Detailed {
            data_type: "Utf8View".into(),
            path: Some("kodPocztowy".into())
        }
    );
    let schema = settings.schema("Parcel").expect("a schema");
    assert_eq!(schema.field(0).name(), "postcode");
    assert_eq!(
        schema
            .field(0)
            .metadata()
            .get(xeibe_schema::rules::meta::PATH)
            .map(String::as_str),
        Some("kodPocztowy")
    );
}

#[test]
fn geometry_columns_accept_a_kind_and_dimensions() {
    let settings = load(
        r#"{
          "format_version": 1,
          "layers": { "Parcel": {
            "wkb": "Geometry",
            "point": "Geometry(Point)",
            "solid": "Geometry(MultiPolygon, XYZ)"
          } }
        }"#,
    );
    let schema = settings.schema("Parcel").expect("a schema");
    let extension = |index: usize| {
        schema
            .field(index)
            .metadata()
            .get("ARROW:extension:name")
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(extension(0), "geoarrow.wkb", "bare Geometry is WKB");
    assert_eq!(extension(1), "geoarrow.point");
    assert_eq!(extension(2), "geoarrow.multipolygon");
}

#[test]
fn a_column_spec_round_trips_through_a_field() {
    let field = Field::new("count", DataType::Int64, true);
    let spec = ColumnSpec::from_field(&field).expect("a spec");
    assert_eq!(spec, ColumnSpec::Type("Int64".into()));
    let back = spec.to_field("count").expect("a field");
    assert_eq!(back.data_type(), &DataType::Int64);
    assert!(back.is_nullable());
}

#[test]
fn an_unknown_type_string_is_an_error_naming_the_column() {
    let settings = load(
        r#"{ "format_version": 1, "layers": { "Parcel": { "area": "NotAType" } } }"#,
    );
    let error = settings.schema("Parcel").expect_err("an invalid type");
    let message = error.to_string();
    assert!(message.contains("area"), "{message}");
    assert!(message.contains("NotAType"), "{message}");
}

#[test]
fn an_unknown_layer_is_an_error() {
    assert!(load(EXAMPLE).schema("Nope").is_err());
}

#[test]
fn options_and_layers_are_both_optional() {
    let only_options = load(r#"{ "format_version": 1, "options": { "batch_size": 1024 } }"#);
    assert_eq!(only_options.options.batch_size, 1024);
    assert!(only_options.layers.is_empty());

    let only_layers = load(r#"{ "format_version": 1, "layers": { "Parcel": { "a": "Int64" } } }"#);
    assert_eq!(
        only_layers.options.batch_size,
        ReadOptions::default().batch_size,
        "missing options fall back to the defaults"
    );
}

#[test]
fn the_axis_order_is_written_as_a_plain_mode() {
    // `xeibe scan` writes one mode; overrides only for inputs that mix
    // srsNames decided differently.
    let settings = load(
        r#"{ "format_version": 1, "options": { "inference": { "geometry": { "axis": "YX" } } } }"#,
    );
    assert_eq!(
        settings.options.inference.geometry.axis.mode,
        AxisOrderMode::YX
    );
    assert!(settings.options.inference.geometry.axis.overrides.is_empty());

    let with_override = load(
        r#"{ "format_version": 1, "options": { "inference": { "geometry": { "axis":
            { "mode": "Auto", "overrides": [[{ "srs_name": "EPSG:4326" }, "YX"]] } } } } }"#,
    );
    let axis = &with_override.options.inference.geometry.axis;
    assert_eq!(axis.mode, AxisOrderMode::Auto);
    assert_eq!(axis.overrides.len(), 1);
}
