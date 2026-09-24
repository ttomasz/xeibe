//! Schema inference: observation (path tree) + policy (`InferenceOptions`), and
//! binding a given schema to XML paths.
//!
//! See `docs/schema-inference.md` and `docs/type-mapping.md`.

pub mod bind;
pub mod error;
pub mod explain;
pub mod geometry_stats;
pub mod merge;
pub mod node;
pub mod observation;
pub mod options;
pub mod pattern;
pub mod presets;
pub mod rules;
pub mod scan;
pub mod value;

pub use error::{Error, Result};
pub use merge::Merge;
pub use node::ElementNode;
pub use bind::{ColumnPath, bind_schema};
pub use observation::{DatasetObservation, LayerObservation};
pub use options::{InferenceOptions, SampleOptions};
pub use pattern::PathPattern;
pub use rules::{FieldDecision, FieldRoute, LayerSchema, RouteValue, infer_schema};
pub use scan::{ScanExtent, ScanOptions, Scanner};
pub use value::{TypeSet, ValueStats};
