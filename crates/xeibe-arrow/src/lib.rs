//! GML features → Arrow `RecordBatch`es with GeoArrow geometry columns.
//!
//! <div class="warning">
//!
//! **Experimental, not for production use.** This crate is AI-generated and
//! has not been thoroughly tested yet. Its interfaces are not stable and will
//! change in the near future, in breaking ways.
//!
//! </div>
//!
//! Entry points: [`scan`] lists the layers of the input with their inferred
//! schemas, [`read`] streams one layer as record batches. See the
//! [architecture document](https://github.com/ttomasz/xeibe/blob/main/docs/architecture.md).
//!
//! ```no_run
//! use xeibe_arrow::{ReadOptions, read, scan};
//! use xeibe_core::Source;
//! use xeibe_schema::ScanExtent;
//!
//! let options = ReadOptions::default();
//!
//! let result = scan(Source::file("parcels.gml")?, ScanExtent::Full, &options)?;
//! for layer in result.layers() {
//!     println!("{}: {} features", layer.name, layer.feature_count);
//! }
//!
//! for batch in read(Source::file("parcels.gml")?, "Parcel", None, &options)? {
//!     let batch = batch?;
//!     println!("{} rows", batch.num_rows());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

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
pub use geometry_column::to_wkb;
pub use options::{OnFeatureError, ReadOptions};
pub use reader::LayerReader;
pub use report::ReadReport;
pub use settings::{ColumnSpec, Settings};
