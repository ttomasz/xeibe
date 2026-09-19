use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::{Session, TableProvider};
use datafusion::error::Result;
use datafusion::logical_expr::{Expr, TableType};
use datafusion::physical_plan::ExecutionPlan;
use xeibe_arrow::ReadOptions;

/// One layer as a table. Scans run the parallel reader with projection pushdown;
/// each source is one partition.
#[derive(Debug)]
pub struct GmlTable {
    /// Object-store URLs or local paths, resolved against the session's registry at scan time.
    sources: Vec<String>,
    layer: String,
    schema: SchemaRef,
    options: ReadOptions,
}

impl GmlTable {
    /// With `schema: None`, samples the layer now (in `spawn_blocking`), like
    /// DataFusion's CSV/JSON `schema_infer_max_records`.
    pub async fn try_new(
        state: &dyn Session,
        sources: Vec<String>,
        layer: &str,
        schema: Option<SchemaRef>,
        options: ReadOptions,
    ) -> Result<Self> {
        todo!()
    }
}

#[async_trait]
impl TableProvider for GmlTable {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        todo!()
    }
}
