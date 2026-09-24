//! GML features → Arrow `RecordBatch`es with GeoArrow geometry columns.
//!
//! Entry points: [`scan`] and [`read`]. See `docs/architecture.md`.

pub mod api;
pub mod axis;
pub mod builders;
pub mod error;
pub mod feature;
pub mod geometry_column;
pub mod options;
pub mod pipeline;
pub mod reader;
pub mod report;
pub mod route;
pub mod settings;
pub mod value;

pub use api::{LayerInfo, ScanResult, read, scan};
pub use error::{Error, Result};
pub use options::{OnFeatureError, ReadOptions};
pub use reader::LayerReader;
pub use report::ReadReport;
pub use settings::{ColumnSpec, Settings};
