//! Schema inference: observation (path tree) + policy (`InferenceOptions`), and
//! binding a given schema to XML paths.
//!
//! <div class="warning">
//!
//! **Experimental, not for production use.** This crate is AI-generated and
//! has not been thoroughly tested yet. Its interfaces are not stable and will
//! change in the near future, in breaking ways.
//!
//! </div>
//!
//! See the [schema inference](https://github.com/ttomasz/xeibe/blob/main/docs/schema-inference.md) and
//! [type mapping](https://github.com/ttomasz/xeibe/blob/main/docs/type-mapping.md) documents.

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

pub use bind::{ColumnPath, bind_schema};
pub use error::{Error, Result};
pub use merge::Merge;
pub use node::ElementNode;
pub use observation::{DatasetObservation, LayerObservation};
pub use options::{InferenceOptions, SampleOptions};
pub use pattern::PathPattern;
pub use rules::{FieldDecision, FieldRoute, LayerSchema, RouteValue, infer_schema};
pub use scan::{ScanExtent, ScanOptions, Scanner};
pub use value::{TypeSet, ValueStats};
