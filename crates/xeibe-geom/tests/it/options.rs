//! `GeometryOptions`: the only geometry options, all of them read parameters
//! (`docs/geometry.md`, "Options").
//!
//! Everything that used to be an option is fixed behaviour now, tested where
//! the behaviour lives: unclosed rings (`parse_primitives`), unsupported
//! geometry (`parse_surfaces`), axis overrides (`axis`).

use xeibe_geom::options::CurveMode;
use xeibe_geom::{AxisOrderMode, GeometryOptions};

#[test]
fn the_documented_options_parse_from_the_settings_file() {
    let options: GeometryOptions = serde_json::from_str(
        r#"{
            "axis": { "mode": "Auto", "overrides": { "EPSG:4326": "YX" } },
            "crs_override": "EPSG:2180",
            "curves": { "Linearize": { "max_angle_step_deg": 2, "max_gap": 0.5 } },
            "primary": "position"
        }"#,
    )
    .expect("the documented keys");
    assert_eq!(options.axis.mode, AxisOrderMode::Auto);
    assert_eq!(options.crs_override.as_deref(), Some("EPSG:2180"));
    assert!(matches!(options.curves, CurveMode::Linearize(_)));
    assert_eq!(options.primary.as_deref(), Some("position"));
}

#[test]
fn the_defaults_match_the_documented_ones() {
    let options = GeometryOptions::default();
    assert_eq!(options.axis.mode, AxisOrderMode::Auto);
    assert!(options.axis.overrides.is_empty());
    assert!(options.crs_override.is_none());
    assert!(matches!(options.curves, CurveMode::Preserve));
    assert!(options.primary.is_none(), "the first geometry column is primary");
}

#[test]
fn removed_options_are_not_part_of_the_format() {
    // Dimensions come from the column type, coordinates are always separated,
    // mixed CRSs and unsupported or degenerate geometry are errors, unclosed
    // rings are closed, the join tolerance is fixed, geodesic arcs use the
    // `Linearize` step, and the encoding is a scan option
    // (`InferenceOptions.geometry_encoding`).
    for removed in [
        r#"{"dimension": "Force2D"}"#,
        r#"{"interleaved": true}"#,
        r#"{"mixed_crs": "SplitColumns"}"#,
        r#"{"unsupported_geometry": "Null"}"#,
        r#"{"raw_xml_for_computed_arcs": true}"#,
        r#"{"arc_step_degrees": 2.0}"#,
        r#"{"join_tolerance": 0.001}"#,
        r#"{"close_rings": true}"#,
        r#"{"lenient_degenerate": true}"#,
        r#"{"encoding": "Wkb"}"#,
    ] {
        assert!(
            serde_json::from_str::<GeometryOptions>(removed).is_err(),
            "{removed} is still accepted"
        );
    }
}
