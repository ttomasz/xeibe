use std::sync::Arc;

use datafusion::catalog::{TableFunctionArgs, TableFunctionImpl, TableProvider};
use datafusion::error::Result;

/// `read_gml('path/*.gml', layer => 'Name' [, settings => 'file.json'] [, preset => 'flat'] [, axis_order => 'auto'])`.
/// Without a schema for the layer in `settings`, the schema is sampled.
#[derive(Debug, Default)]
pub struct ReadGmlFunction;

impl TableFunctionImpl for ReadGmlFunction {
    fn call_with_args(&self, args: TableFunctionArgs) -> Result<Arc<dyn TableProvider>> {
        todo!()
    }
}
