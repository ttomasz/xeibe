use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::{Session, TableProvider};
use datafusion::error::{DataFusionError, Result};
use datafusion::execution::runtime_env::RuntimeEnv;
use datafusion::logical_expr::{Expr, TableType};
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::empty::EmptyExec;
use datafusion::physical_plan::streaming::{PartitionStream, StreamingTableExec};
use tokio::runtime::Handle;
use xeibe_arrow::ReadOptions;

use crate::partition::SourcePartition;
use crate::sources::{external, resolve, resolve_async};

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
    /// DataFusion's CSV/JSON `schema_infer_max_records`. A given schema is
    /// used as it is: it is the projection.
    pub async fn try_new(
        state: &dyn Session,
        sources: Vec<String>,
        layer: &str,
        schema: Option<SchemaRef>,
        options: ReadOptions,
    ) -> Result<Self> {
        let schema = match schema {
            Some(schema) => schema,
            None => {
                let runtime = state.runtime_env().clone();
                let handle = Handle::current();
                let (inputs, layer, options) = (sources.clone(), layer.to_string(), options.clone());
                tokio::task::spawn_blocking(move || sample_schema(&runtime, &inputs, &layer, &options, Some(&handle)))
                    .await
                    .map_err(external)??
            }
        };
        Ok(Self::with_schema(sources, layer, schema, options))
    }

    /// A table with a known schema; nothing is read until a scan.
    pub fn with_schema(sources: Vec<String>, layer: &str, schema: SchemaRef, options: ReadOptions) -> Self {
        GmlTable { sources, layer: layer.to_string(), schema, options }
    }

    pub fn layer(&self) -> &str {
        &self.layer
    }
}

/// The schema of a read without one: inferred from the layer's sample.
/// Blocking.
pub(crate) fn sample_schema(
    runtime: &RuntimeEnv,
    inputs: &[String],
    layer: &str,
    options: &ReadOptions,
    handle: Option<&Handle>,
) -> Result<SchemaRef> {
    let sources = resolve(runtime, inputs, handle)?;
    let mut options = options.clone();
    options.projection = None;
    // The schema is known once `read` returns; dropping the reader stops it.
    let reader = xeibe_arrow::read(sources, layer, None, &options).map_err(external)?;
    Ok(datafusion::arrow::record_batch::RecordBatchReader::schema(&reader))
}

#[async_trait]
impl TableProvider for GmlTable {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    /// Filters are left to DataFusion (they cannot skip parsing); the limit is
    /// applied as batches arrive.
    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let schema = match projection {
            Some(indices) => Arc::new(self.schema.project(indices)?),
            None => self.schema.clone(),
        };
        let mut options = self.options.clone();
        options.projection = Some(schema.fields().iter().map(|field| field.name().clone()).collect());

        let sources = resolve_async(state.runtime_env().clone(), self.sources.clone()).await?;
        if sources.is_empty() {
            return Ok(Arc::new(EmptyExec::new(schema)));
        }
        let partitions = sources
            .into_iter()
            .map(|source| {
                Arc::new(SourcePartition::new(source, &self.layer, self.schema.clone(), options.clone()))
                    as Arc<dyn PartitionStream>
            })
            .collect();
        let exec = StreamingTableExec::try_new(schema, partitions, None, Vec::new(), false, limit)
            .map_err(|error| DataFusionError::Internal(format!("GML scan: {error}")))?;
        Ok(Arc::new(exec))
    }
}
