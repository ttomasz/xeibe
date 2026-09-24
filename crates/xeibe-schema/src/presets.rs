//! Presets (`docs/schema-inference.md` §3.6): starting points that differ from
//! [`InferenceOptions::default`] only where their summary says so. Schemas
//! are always flat, so presets only choose types.

use crate::InferenceOptions;
use crate::TypeSet;

impl InferenceOptions {
    /// Every scalar as `text`.
    pub fn strings() -> Self {
        let mut options = InferenceOptions::default();
        options.types.enabled = TypeSet::STRING;
        options
    }
}
