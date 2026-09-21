//! Full geometry parser: GML element subtree → [`Geometry`] in output axis order.

mod aggregates;
mod assemble;
mod coords;
mod curves;
mod envelope;
mod primitives;
mod surfaces;

pub use coords::{CoordinatesFormat, parse_coordinates, parse_pos_list};
pub use envelope::parse_envelope;

use xeibe_core::{Dialect, reader::GmlReader};

use crate::axis::AxisDecision;
use crate::model::{Envelope, GeomKind, Geometry};
use crate::options::GeometryOptions;

/// Values inherited from enclosing elements.
#[derive(Debug, Clone, Default)]
pub struct ParseContext {
    /// Nearest srsName: collection `boundedBy` → feature `boundedBy` → geometry → …
    pub srs_name: Option<String>,
    pub srs_dimension: Option<u8>,
    /// Applied axis decision for the current key; resolved lazily by the caller.
    pub axis: Option<AxisDecision>,
}

#[derive(Debug, Clone)]
pub struct ParsedGeometry {
    pub geometry: Option<Geometry>,
    /// Kind of the outermost source element.
    pub source_kind: GeomKind,
    pub srs_name: Option<String>,
    pub dialect: Dialect,
    /// Raw XML, when requested (`RawXml` policies).
    pub raw_xml: Option<String>,
    pub warnings: Vec<String>,
}

/// Resolves the axis decision for a (srsName, dialect) pair; implemented by
/// the dataset reader, which holds the per-key decisions (settings overrides or sampled evidence).
pub trait AxisResolver {
    fn resolve(&self, srs_name: Option<&str>, dialect: Dialect) -> AxisDecision;
}

pub struct GeometryParser<'o> {
    options: &'o GeometryOptions,
}

impl<'o> GeometryParser<'o> {
    pub fn new(options: &'o GeometryOptions) -> Self {
        GeometryParser { options }
    }

    /// Parse the geometry element the reader is positioned on (inside a
    /// geometry property). Consumes the element.
    pub fn parse(
        &self,
        reader: &mut GmlReader<'_>,
        context: &ParseContext,
        axis: &dyn AxisResolver,
    ) -> crate::Result<ParsedGeometry> {
        todo!()
    }

    /// Parse a `boundedBy` envelope (for srsName inheritance and bbox columns).
    pub fn parse_bounded_by(
        &self,
        reader: &mut GmlReader<'_>,
        context: &ParseContext,
    ) -> crate::Result<Option<Envelope>> {
        todo!()
    }
}
