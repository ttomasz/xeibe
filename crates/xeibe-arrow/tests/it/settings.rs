//! The settings file: read options and per-layer schemas as JSON
//! (`docs/architecture.md`, "Settings file").
//!
//! A column is `"name": "type"` or `"name": { "type": …, "path": … }`; without
//! a path, the name is the path. Types are Arrow `DataType` strings or
//! PostgreSQL/DuckDB-style aliases, which the scan writes.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, TimeUnit};
use xeibe_arrow::{ColumnSpec, ReadOptions, Settings};
use xeibe_geom::AxisOrderMode;
use xeibe_schema::rules::meta;

use crate::support::temp_dir;

const EXAMPLE: &str = r#"{
  "format_version": 1,
  "options": {
    "geometry": { "axis": "XY" }
  },
  "layers": {
    "prgad:AD_PunktAdresowy": {
      "@id": "text",
      "lokalnyId": { "type": "text", "path": "idIIP/*/lokalnyId" },
      "przestrzenNazw": { "type": "text", "path": "idIIP/*/przestrzenNazw" },
      "wersjaId": { "type": "timestamptz", "path": "idIIP/*/wersjaId" },
      "poczatekWersjiObiektu": "timestamp",
      "numerPorzadkowy": "text",
      "georeferencja": "geometry(Point)",
      "kodPocztowy": "text",
      "dataNadania": "date",
      "miejscowosc": { "type": "text", "path": "miejscowosc/@href" }
    },
    "xplan:BP_Plan": {
      "@id": "text",
      "referenzName": { "type": "text[]", "path": "externeReferenz[]/*/referenzName" },
      "referenzURL": { "type": "text[]", "path": "externeReferenz[]/*/referenzURL" },
      "datum": { "type": "date[]", "path": "externeReferenz[]/*/datum" },
      "raeumlicherGeltungsbereich": "geometry(MultiPolygon)"
    }
  }
}"#;

fn load(json: &str) -> Settings {
    let dir = temp_dir("settings");
    let path = dir.join("settings.json");
    std::fs::write(&path, json).unwrap();
    Settings::load(&path).expect("the settings file loads")
}

/// The Arrow type of a one-column layer `{"c": <type>}`.
fn type_of(type_string: &str) -> xeibe_arrow::Result<Field> {
    let settings = load(&format!(
        r#"{{ "format_version": 1, "layers": {{ "L": {{ "c": {type_string:?} }} }} }}"#
    ));
    settings.schema("L").map(|schema| schema.field(0).clone())
}

fn list_of(item: DataType) -> DataType {
    DataType::List(Arc::new(Field::new("item", item, true)))
}

fn extension(field: &Field) -> Option<&str> {
    field.metadata().get("ARROW:extension:name").map(String::as_str)
}

fn path_of(field: &Field) -> Option<&str> {
    field.metadata().get(meta::PATH).map(String::as_str)
}

#[test]
fn loads_the_documented_example() {
    let settings = load(EXAMPLE);
    assert_eq!(settings.format_version, Settings::FORMAT_VERSION);
    assert_eq!(settings.options.geometry.axis.mode, AxisOrderMode::XY);
    assert_eq!(settings.layers.len(), 2);
    let layer = &settings.layers["prgad:AD_PunktAdresowy"];
    assert_eq!(
        layer.keys().collect::<Vec<_>>(),
        [
            "@id",
            "lokalnyId",
            "przestrzenNazw",
            "wersjaId",
            "poczatekWersjiObiektu",
            "numerPorzadkowy",
            "georeferencja",
            "kodPocztowy",
            "dataNadania",
            "miejscowosc",
        ],
        "columns keep their order"
    );
}

#[test]
fn turns_column_specs_into_a_flat_arrow_schema() {
    let settings = load(EXAMPLE);
    let schema = settings.schema("prgad:AD_PunktAdresowy").expect("a schema");
    let field = |name: &str| schema.field_with_name(name).expect("a column").clone();
    assert_eq!(field("@id").data_type(), &DataType::Utf8View);
    assert_eq!(field("lokalnyId").data_type(), &DataType::Utf8View);
    assert_eq!(path_of(&field("lokalnyId")), Some("idIIP/*/lokalnyId"));
    assert_eq!(
        field("wersjaId").data_type(),
        &DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
    );
    assert_eq!(field("dataNadania").data_type(), &DataType::Date32);
    assert_eq!(path_of(&field("miejscowosc")), Some("miejscowosc/@href"));
    assert!(
        schema.fields().iter().all(|f| f.is_nullable()),
        "every column is nullable"
    );
    assert_eq!(extension(&field("georeferencja")), Some("geoarrow.point"));

    let plan = settings.schema("xplan:BP_Plan").expect("a schema");
    let datum = plan.field_with_name("datum").expect("a column");
    assert_eq!(datum.data_type(), &list_of(DataType::Date32));
    assert_eq!(path_of(datum), Some("externeReferenz[]/*/datum"));
}

#[test]
fn aliases_follow_postgresql_and_duckdb() {
    let cases: &[(&str, DataType)] = &[
        ("text", DataType::Utf8View),
        ("varchar", DataType::Utf8View),
        ("string", DataType::Utf8View),
        ("boolean", DataType::Boolean),
        ("bool", DataType::Boolean),
        ("smallint", DataType::Int16),
        ("int2", DataType::Int16),
        ("integer", DataType::Int32),
        ("int", DataType::Int32),
        ("int4", DataType::Int32),
        ("bigint", DataType::Int64),
        ("int8", DataType::Int64),
        ("real", DataType::Float32),
        ("float4", DataType::Float32),
        ("double", DataType::Float64),
        ("double precision", DataType::Float64),
        ("float8", DataType::Float64),
        ("date", DataType::Date32),
        ("timestamp", DataType::Timestamp(TimeUnit::Microsecond, None)),
        (
            "timestamptz",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        ),
        ("time", DataType::Time64(TimeUnit::Microsecond)),
        ("bytea", DataType::Binary),
        ("blob", DataType::Binary),
        ("text[]", list_of(DataType::Utf8View)),
        ("date[]", list_of(DataType::Date32)),
        ("bigint[]", list_of(DataType::Int64)),
    ];
    for (alias, expected) in cases {
        let field = type_of(alias).unwrap_or_else(|e| panic!("{alias}: {e}"));
        assert_eq!(field.data_type(), expected, "{alias}");
    }
}

#[test]
fn aliases_are_case_insensitive() {
    assert_eq!(type_of("TEXT").unwrap().data_type(), &DataType::Utf8View);
    assert_eq!(type_of("BigInt").unwrap().data_type(), &DataType::Int64);
    assert_eq!(
        extension(&type_of("Geometry(Point)").unwrap()),
        Some("geoarrow.point")
    );
}

#[test]
fn map_is_a_string_to_string_map() {
    let field = type_of("map").expect("the map alias");
    match field.data_type() {
        DataType::Map(entries, _) => match entries.data_type() {
            DataType::Struct(fields) => {
                assert_eq!(fields[0].data_type(), &DataType::Utf8View, "keys");
                assert_eq!(fields[1].data_type(), &DataType::Utf8View, "values");
            }
            other => panic!("map entries are {other}"),
        },
        other => panic!("map is {other}"),
    }
}

#[test]
fn arrow_type_strings_are_accepted_too() {
    assert_eq!(type_of("Utf8View").unwrap().data_type(), &DataType::Utf8View);
    assert_eq!(type_of("Int64").unwrap().data_type(), &DataType::Int64);
    assert_eq!(
        type_of("List(Utf8View)").unwrap().data_type(),
        &list_of(DataType::Utf8View)
    );
}

#[test]
fn numeric_and_decimal_are_rejected_with_a_hint() {
    // No Decimal128 (`docs/type-mapping.md`).
    for alias in ["numeric", "decimal"] {
        let message = type_of(alias).expect_err(alias).to_string();
        assert!(
            message.contains("double") && message.contains("text"),
            "{alias}: {message}"
        );
    }
}

#[test]
fn geometry_columns_accept_a_kind_and_dimensions() {
    assert_eq!(extension(&type_of("geometry").unwrap()), Some("geoarrow.wkb"), "bare geometry is WKB");
    assert_eq!(extension(&type_of("geometry(Point)").unwrap()), Some("geoarrow.point"));
    assert_eq!(
        extension(&type_of("geometry(MultiPolygon, XYZ)").unwrap()),
        Some("geoarrow.multipolygon")
    );
}

#[test]
fn a_geometry_list_is_a_list_of_wkb() {
    // For a geometry below a repeated element.
    let field = type_of("geometry[]").expect("a geometry list");
    match field.data_type() {
        DataType::List(item) => assert_eq!(extension(item), Some("geoarrow.wkb")),
        other => panic!("geometry[] is {other}"),
    }
}

#[test]
fn a_column_can_name_the_xml_path_it_comes_from() {
    let settings = load(
        r#"{
          "format_version": 1,
          "layers": { "Parcel": { "postcode": { "type": "text", "path": "kodPocztowy" } } }
        }"#,
    );
    let spec = &settings.layers["Parcel"]["postcode"];
    assert_eq!(
        spec,
        &ColumnSpec::Detailed {
            data_type: "text".into(),
            path: Some("kodPocztowy".into())
        }
    );
    let schema = settings.schema("Parcel").expect("a schema");
    assert_eq!(schema.field(0).name(), "postcode");
    assert_eq!(path_of(schema.field(0)), Some("kodPocztowy"));
}

#[test]
fn prefixed_path_steps_use_the_top_level_namespaces() {
    let settings = load(
        r#"{
          "format_version": 1,
          "namespaces": { "x": "http://example.com/x" },
          "layers": { "Parcel": { "x:code": "text" } }
        }"#,
    );
    let schema = settings.schema("Parcel").expect("a schema");
    let namespaces = schema.metadata().get(meta::NS).cloned().unwrap_or_default();
    assert!(
        namespaces.contains("http://example.com/x"),
        "the prefixes reach the schema metadata: {namespaces:?}"
    );
}

#[test]
fn the_scan_writes_aliases_and_paths_only_where_needed() {
    let with_path = |name: &str, data_type: DataType, path: &str| {
        Field::new(name, data_type, true)
            .with_metadata([(meta::PATH.to_string(), path.to_string())].into())
    };
    let schema = Schema::new(vec![
        with_path("@id", DataType::Utf8View, "@id"),
        with_path("count", DataType::Int64, "count"),
        with_path(
            "t",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            "t",
        ),
        with_path("tags", list_of(DataType::Utf8View), "tags[]"),
        with_path("lokalnyId", DataType::Utf8View, "idIIP/*/lokalnyId"),
    ]);
    let mut settings = Settings {
        format_version: Settings::FORMAT_VERSION,
        options: ReadOptions::default(),
        layers: Default::default(),
    };
    settings.set_schema("Parcel", &schema).expect("stored");
    let layer = &settings.layers["Parcel"];
    assert_eq!(layer["@id"], ColumnSpec::Type("text".into()), "name = path");
    assert_eq!(layer["count"], ColumnSpec::Type("bigint".into()));
    assert_eq!(layer["t"], ColumnSpec::Type("timestamptz".into()));
    assert_eq!(
        layer["tags"],
        ColumnSpec::Detailed {
            data_type: "text[]".into(),
            path: Some("tags[]".into())
        },
        "the scan always writes the anchor of a list"
    );
    assert_eq!(
        layer["lokalnyId"],
        ColumnSpec::Detailed {
            data_type: "text".into(),
            path: Some("idIIP/*/lokalnyId".into())
        }
    );

    let dir = temp_dir("settings-round-trip");
    let path = dir.join("out.json");
    settings.save(&path).expect("saved");
    let reloaded = Settings::load(&path).expect("loaded");
    let back = reloaded.schema("Parcel").expect("a schema");
    for (a, b) in back.fields().iter().zip(schema.fields()) {
        assert_eq!(a.name(), b.name());
        assert_eq!(a.data_type(), b.data_type(), "{}", a.name());
    }
}

#[test]
fn a_column_spec_round_trips_through_a_field() {
    let field = Field::new("count", DataType::Int64, true);
    let spec = ColumnSpec::from_field(&field).expect("a spec");
    assert_eq!(spec, ColumnSpec::Type("bigint".into()));
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

    let only_layers = load(r#"{ "format_version": 1, "layers": { "Parcel": { "a": "bigint" } } }"#);
    assert_eq!(
        only_layers.options.batch_size,
        ReadOptions::default().batch_size,
        "missing options fall back to the defaults"
    );
}

#[test]
fn the_axis_order_is_written_as_a_plain_mode() {
    // `xeibe scan` writes one mode; overrides only for inputs that mix
    // srsNames decided differently. Geometry options are read options, under
    // `options.geometry`, not `options.inference`.
    let settings = load(r#"{ "format_version": 1, "options": { "geometry": { "axis": "YX" } } }"#);
    assert_eq!(settings.options.geometry.axis.mode, AxisOrderMode::YX);
    assert!(settings.options.geometry.axis.overrides.is_empty());

    let with_override = load(
        r#"{ "format_version": 1, "options": { "geometry": { "axis":
            { "mode": "Auto", "overrides": { "EPSG:4326": "YX" } } } } }"#,
    );
    let axis = &with_override.options.geometry.axis;
    assert_eq!(axis.mode, AxisOrderMode::Auto);
    assert_eq!(axis.overrides.len(), 1);
}
