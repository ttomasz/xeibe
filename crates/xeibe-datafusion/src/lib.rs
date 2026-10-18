//! DataFusion integration (and, through it, SedonaDB).
//!
//! <div class="warning">
//!
//! **Experimental, not for production use.** This crate is AI-generated and
//! has not been thoroughly tested yet. Its interfaces are not stable and will
//! change in the near future, in breaking ways.
//!
//! </div>
//!
//! - [`GmlTable`]: a `TableProvider` for **one layer**, with a given schema or one
//!   sampled when the table is created. Each source is one partition.
//! - [`ReadGmlFunction`]: `read_gml('path/*.gml', 'Layer' [, 'settings=…'])`.
//!
//! Network I/O and any caching come from DataFusion's `object_store` registry; a
//! registered store is adapted to `ByteSource` and read with one streaming `get`.

pub mod function;
pub mod partition;
pub mod sources;
pub mod table;

pub use function::ReadGmlFunction;
pub use table::GmlTable;

/// Register `read_gml` on a session context.
pub fn register(ctx: &datafusion::prelude::SessionContext) {
    ctx.register_udtf("read_gml", std::sync::Arc::new(ReadGmlFunction));
}
