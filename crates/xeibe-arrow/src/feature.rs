//! Parses one feature element and routes its values into the layer builder.

use xeibe_core::reader::GmlReader;
use xeibe_geom::{AxisResolver, GeometryParser, ParseContext};
use xeibe_schema::LayerSchema;

use crate::builders::LayerBatchBuilder;
use crate::overflow::OverflowCollector;
use crate::report::ReadReport;

pub struct FeatureReader<'a> {
    schema: &'a LayerSchema,
    geometry: GeometryParser<'a>,
    axis: &'a dyn AxisResolver,
}

impl<'a> FeatureReader<'a> {
    pub fn new(
        schema: &'a LayerSchema,
        geometry: GeometryParser<'a>,
        axis: &'a dyn AxisResolver,
    ) -> Self {
        todo!()
    }

    /// Reader positioned on the feature start element; consumes it and appends one row.
    pub fn read_feature(
        &self,
        reader: &mut GmlReader<'_>,
        context: &ParseContext,
        out: &mut LayerBatchBuilder,
        overflow: &mut OverflowCollector,
        report: &mut ReadReport,
    ) -> crate::Result<()> {
        todo!()
    }
}
