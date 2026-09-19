use crate::InferenceOptions;

impl InferenceOptions {
    /// Flat columns where lossless: `FlattenSingleOnly` + `Split`.
    pub fn flat() -> Self {
        todo!()
    }

    /// `Flatten`, `Split`, lists of scalars only, `Lossy` types, attributes dropped.
    pub fn gdal_like() -> Self {
        todo!()
    }

    /// `_` attribute prefix, `_VALUE` text field, `Struct` nesting.
    pub fn spark_xml_like() -> Self {
        todo!()
    }

    /// Every scalar as `Utf8View`.
    pub fn strings() -> Self {
        todo!()
    }
}
