//! Presets (`docs/schema-inference.md` §3.6): starting points that differ from
//! [`InferenceOptions::default`] only where their summary says so.

use crate::InferenceOptions;
use crate::TypeSet;
use crate::options::{AttrSelect, Lossless, Nesting, SimpleContent};

/// Depth up to which `gdal_like` flattens; deeper subtrees become raw XML.
const GDAL_FLATTEN_DEPTH: u16 = 8;

impl InferenceOptions {
    /// Flat columns where lossless: `FlattenSingleOnly` + `Split`.
    pub fn flat() -> Self {
        let mut options = InferenceOptions::default();
        options.structure.nesting = Nesting::FlattenSingleOnly;
        options.structure.simple_with_attrs = SimpleContent::Split;
        options
    }

    /// `Flatten`, `Split`, lists of scalars only, `Lossy` types, attributes dropped.
    pub fn gdal_like() -> Self {
        let mut options = InferenceOptions::default();
        options.structure.nesting = Nesting::Flatten { max_depth: GDAL_FLATTEN_DEPTH };
        options.structure.simple_with_attrs = SimpleContent::Split;
        options.structure.xml_attributes = AttrSelect::None;
        options.types.lossless = Lossless::Lossy;
        options
    }

    /// `_` attribute prefix, `_VALUE` text field, `Struct` nesting.
    pub fn spark_xml_like() -> Self {
        let mut options = InferenceOptions::default();
        options.naming.attribute_prefix = "_".to_string();
        options.naming.text_field = "_VALUE".to_string();
        options.structure.nesting = Nesting::Struct;
        options
    }

    /// Every scalar as `Utf8View`.
    pub fn strings() -> Self {
        let mut options = InferenceOptions::default();
        options.types.enabled = TypeSet::STRING;
        options
    }
}
