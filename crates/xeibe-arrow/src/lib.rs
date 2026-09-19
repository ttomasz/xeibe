//! GML features → Arrow `RecordBatch`es with GeoArrow geometry columns.
//!
//! Entry points: [`scan`] and [`read`]. See `docs/architecture.md`.

// Skeleton phase: signatures only, bodies are `todo!()`.
#![allow(dead_code, unused_variables)]

pub mod api;
pub mod axis;
pub mod builders;
pub mod error;
pub mod feature;
pub mod geometry_column;
pub mod options;
pub mod overflow;
pub mod pipeline;
pub mod reader;
pub mod report;
pub mod settings;

pub use api::{LayerInfo, ScanResult, read, scan};
pub use error::{Error, Result};
pub use options::{OnFeatureError, ReadOptions};
pub use reader::LayerReader;
pub use report::ReadReport;
pub use settings::{ColumnSpec, Settings};
