//! Presets and per-layer option patches (`docs/schema-inference.md` §3.6).
//!
//! Only two presets are left: schemas are always flat, so there is nothing
//! for `flat`, `gdal_like` or `spark_xml_like` to choose.

use xeibe_schema::options::Lossless;
use xeibe_schema::{InferenceOptions, TypeSet};

#[test]
fn default_is_lossless_by_value() {
    let default = InferenceOptions::default();
    assert_eq!(default.types.lossless, Lossless::Value);
    assert!(default.structure.collapse_type_wrappers);
}

#[test]
fn strings_turns_every_scalar_into_text() {
    let strings = InferenceOptions::strings();
    assert_eq!(strings.types.enabled, TypeSet::STRING);
}

#[test]
fn per_layer_patches_are_applied_on_top_of_the_global_options() {
    let mut options = InferenceOptions::default();
    options.layers.push((
        "AD_PunktAdresowy".to_string(),
        xeibe_schema::options::InferenceOptionsPatch {
            types: Some(xeibe_schema::options::TypeOptions {
                enabled: TypeSet::STRING,
                ..InferenceOptions::default().types
            }),
            ..Default::default()
        },
    ));

    assert_eq!(
        options.for_layer("AD_PunktAdresowy").types.enabled,
        TypeSet::STRING
    );
    assert_eq!(
        options.for_layer("AD_Miejscowosc").types.enabled,
        InferenceOptions::default().types.enabled,
        "other layers keep the global options"
    );
}

#[test]
fn later_patches_win() {
    let patch = |lossless| xeibe_schema::options::InferenceOptionsPatch {
        types: Some(xeibe_schema::options::TypeOptions {
            lossless,
            ..InferenceOptions::default().types
        }),
        ..Default::default()
    };
    let mut options = InferenceOptions::default();
    options.layers.push(("Parcel".into(), patch(Lossless::Lossy)));
    options.layers.push(("Parcel".into(), patch(Lossless::Text)));
    assert_eq!(options.for_layer("Parcel").types.lossless, Lossless::Text);
}
