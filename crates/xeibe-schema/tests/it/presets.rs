//! Presets and per-layer option patches (`docs/schema-inference.md` §3.6).

use xeibe_schema::options::{Lossless, Nesting, SimpleContent};
use xeibe_schema::{InferenceOptions, TypeSet};

#[test]
fn flat_is_flat_wherever_flattening_loses_nothing() {
    let flat = InferenceOptions::flat();
    assert_eq!(flat.structure.nesting, Nesting::FlattenSingleOnly);
    assert_eq!(flat.structure.simple_with_attrs, SimpleContent::Split);
}

#[test]
fn gdal_like_is_lossy_and_drops_attributes() {
    let gdal = InferenceOptions::gdal_like();
    assert!(matches!(gdal.structure.nesting, Nesting::Flatten { .. }));
    assert_eq!(gdal.structure.simple_with_attrs, SimpleContent::Split);
    assert_eq!(gdal.types.lossless, Lossless::Lossy);
    assert_eq!(
        gdal.structure.xml_attributes,
        xeibe_schema::options::AttrSelect::None
    );
}

#[test]
fn spark_xml_like_uses_sparks_names() {
    let spark = InferenceOptions::spark_xml_like();
    assert_eq!(spark.naming.attribute_prefix, "_");
    assert_eq!(spark.naming.text_field, "_VALUE");
    assert_eq!(spark.structure.nesting, Nesting::Struct);
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
            structure: Some(xeibe_schema::options::StructureOptions {
                nesting: Nesting::FlattenSingleOnly,
                ..InferenceOptions::default().structure
            }),
            ..Default::default()
        },
    ));

    assert_eq!(
        options.for_layer("AD_PunktAdresowy").structure.nesting,
        Nesting::FlattenSingleOnly
    );
    assert_eq!(
        options.for_layer("AD_Miejscowosc").structure.nesting,
        Nesting::Struct,
        "other layers keep the global options"
    );
}

#[test]
fn later_patches_win() {
    let patch = |nesting| xeibe_schema::options::InferenceOptionsPatch {
        structure: Some(xeibe_schema::options::StructureOptions {
            nesting,
            ..InferenceOptions::default().structure
        }),
        ..Default::default()
    };
    let mut options = InferenceOptions::default();
    options.layers.push(("Parcel".into(), patch(Nesting::FlattenSingleOnly)));
    options.layers.push(("Parcel".into(), patch(Nesting::Struct)));
    assert_eq!(options.for_layer("Parcel").structure.nesting, Nesting::Struct);
}
