//! Full geometry parser: GML element subtree → [`Geometry`] in output axis order.
//!
//! One recursive-descent [`Parser`] per geometry. It reads the element tree
//! through [`GmlReader`], builds the model in the order the coordinates are
//! written, and applies the axis decision once at the end: the decision key
//! includes the dialect, which is only known once the geometry's structure
//! has been seen. The element parsers live in the submodules, as `impl
//! Parser` blocks.

mod aggregates;
pub(crate) mod assemble;
pub(crate) mod coords;
mod curves;
mod envelope;
mod primitives;
mod surfaces;

use std::borrow::Cow;

pub use coords::{CoordinatesFormat, parse_coordinates, parse_pos_list, swap_xy};
pub use envelope::parse_envelope;

use xeibe_core::reader::{Attributes, GmlReader, XmlEvent};
use xeibe_core::{Dialect, QName, RawElement, ns};

use crate::axis::AxisDecision;
use crate::dialect::DialectTracker;
use crate::epsg::{CrsInfo, CrsTable};
use crate::error::Error;
use crate::linearize::linearize;
use crate::model::{Envelope, GeomKind, Geometry, Surface};
use crate::options::{CurveMode, GeometryOptions};
use crate::crs::SrsName;

/// Values inherited from enclosing elements.
#[derive(Debug, Clone, Default)]
pub struct ParseContext {
    /// Nearest srsName: collection `boundedBy` → feature `boundedBy` → geometry → …
    pub srs_name: Option<String>,
    pub srs_dimension: Option<u8>,
    /// Axis decision already resolved by the caller. When set it is applied
    /// as is and the [`AxisResolver`] is not asked.
    pub axis: Option<AxisDecision>,
}

#[derive(Debug, Clone)]
pub struct ParsedGeometry {
    /// Always `Some` from [`GeometryParser::parse`].
    pub geometry: Option<Geometry>,
    /// Kind of the outermost source element.
    pub source_kind: GeomKind,
    /// The geometry's srsName: its own, else the inherited one, else the
    /// first one declared inside it.
    pub srs_name: Option<String>,
    pub dialect: Dialect,
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

    /// Parse the geometry element whose `Start` event the reader has just
    /// returned (the caller took the event; the element is read back with
    /// [`GmlReader::current_start`]). Consumes the element, also when it
    /// fails: the reader is then after the element's end tag if the XML
    /// allows it, so the caller can go on with the next property.
    ///
    /// Unsupported geometry (and geometry given by `xlink:href`) is an error
    /// ([`Error::Unsupported`], [`Error::ByReference`]); the caller applies
    /// `OnFeatureError`. `curves = Linearize` is applied here.
    pub fn parse(
        &self,
        reader: &mut GmlReader<'_>,
        context: &ParseContext,
        axis: &dyn AxisResolver,
    ) -> crate::Result<ParsedGeometry> {
        let root = current_element(reader)?;
        let source_kind = geom_kind(&root.name);
        let mut parser = Parser::new(self.options, Some(axis), context);
        parser.srs_name = root.attrs.srs_name.clone().or_else(|| context.srs_name.clone());
        parser.dialect.observe(&root.name);
        let scope = parser.root_scope(context);

        match parser.geometry(reader, root, scope) {
            Ok(mut geometry) => {
                if parser.swap() {
                    swap_geometry(&mut geometry);
                }
                if let CurveMode::Linearize(linearize_options) = &self.options.curves {
                    geometry = linearize(geometry, linearize_options);
                }
                Ok(ParsedGeometry {
                    geometry: Some(geometry),
                    source_kind,
                    dialect: parser.dialect.result(),
                    srs_name: parser.srs_name,
                    warnings: parser.warnings,
                })
            }
            Err(error) => {
                // Leave the reader after the element if possible; the
                // original error is the one worth reporting.
                let _ = parser.recover(reader);
                Err(error)
            }
        }
    }

    /// Parse a `boundedBy` (whose `Start` has just been returned; an
    /// `Envelope` or `Box` start works too), for srsName inheritance and bbox
    /// columns. `gml:Null` gives `None`.
    ///
    /// The envelope's srsName is its own or the inherited one. Its corners
    /// are as written, unless `context.axis` holds a decision to apply.
    pub fn parse_bounded_by(
        &self,
        reader: &mut GmlReader<'_>,
        context: &ParseContext,
    ) -> crate::Result<Option<Envelope>> {
        Ok(self.parse_bounded_by_inherited(reader, context)?.0)
    }

    /// [`Self::parse_bounded_by`], and what the envelope hands down to the
    /// geometries it bounds: its srsName and srsDimension, else the ones of
    /// `context` (`docs/geometry.md`, "srsName inheritance"). The handed-down
    /// context holds no axis decision.
    pub fn parse_bounded_by_inherited(
        &self,
        reader: &mut GmlReader<'_>,
        context: &ParseContext,
    ) -> crate::Result<(Option<Envelope>, ParseContext)> {
        let first = current_element(reader)?;
        let mut parser = Parser::new(self.options, None, context);
        let scope = parser.root_scope(context);
        let swap = context.axis.as_ref().is_some_and(|decision| decision.swap);
        let result = (|| {
            let mut envelope = None;
            if is_envelope(&first.name) {
                envelope = Some((parser.envelope(reader, &first, scope)?, first.attrs.srs_dimension));
            } else {
                while let Some(child) = parser.next_child(reader)? {
                    if envelope.is_none() && is_envelope(&child.name) {
                        envelope = Some((parser.envelope(reader, &child, scope)?, child.attrs.srs_dimension));
                    } else {
                        parser.skip(reader)?;
                    }
                }
            }
            Ok(envelope)
        })();
        let envelope = match result {
            Ok(envelope) => envelope,
            Err(error) => {
                let _ = parser.recover(reader);
                return Err(error);
            }
        };
        let mut inherited = ParseContext {
            srs_name: context.srs_name.clone(),
            srs_dimension: context.srs_dimension,
            axis: None,
        };
        let envelope = envelope.map(|(mut envelope, srs_dimension)| {
            if swap {
                envelope::swap_corners(&mut envelope);
            }
            if envelope.srs_name.is_none() {
                envelope.srs_name = context.srs_name.clone();
            }
            inherited.srs_name.clone_from(&envelope.srs_name);
            inherited.srs_dimension = srs_dimension.or(context.srs_dimension);
            envelope
        });
        Ok((envelope, inherited))
    }
}

/// The `boundedBy` of a feature collection, as the splitter keeps it
/// ([`xeibe_core::FeatureChunk::collection_bounded_by`]): its envelope as
/// written, and what it hands down to every feature of the collection (its
/// srsName and srsDimension). An envelope whose corners can't be read still
/// hands down its srsName and srsDimension; `gml:Null` hands down nothing.
pub fn collection_bounded_by(raw: &RawElement) -> crate::Result<(Option<Envelope>, ParseContext)> {
    let mut reader = GmlReader::new(&raw.bytes, &raw.namespaces, raw.byte_offset).with_source(raw.source);
    let mut depth = 0;
    let attrs = loop {
        match reader.next_event()? {
            XmlEvent::Start { name, attrs } => {
                depth += 1;
                // The first element inside the `boundedBy`.
                if depth == 2 {
                    if !is_envelope(&name) {
                        return Ok((None, ParseContext::default()));
                    }
                    break Attrs::read(&attrs);
                }
            }
            XmlEvent::End { .. } | XmlEvent::Eof => return Ok((None, ParseContext::default())),
            XmlEvent::Text(_) => {}
        }
    };
    let context = ParseContext { srs_name: attrs.srs_name, srs_dimension: attrs.srs_dimension, axis: None };
    let options = GeometryOptions::default();
    let envelope = match GeometryParser::new(&options).parse_bounded_by(&mut reader, &ParseContext::default()) {
        Ok(envelope) => envelope,
        Err(Error::Core(error)) => return Err(Error::Core(error)),
        Err(_) => None,
    };
    Ok((envelope, context))
}

fn is_envelope(name: &QName) -> bool {
    name.is_gml_named("Envelope") || name.is_gml_named("EnvelopeWithTimePeriod") || name.is_gml_named("Box")
}

/// The source kind of a geometry element.
pub(crate) fn geom_kind(name: &QName) -> GeomKind {
    if !name.is_gml() {
        return GeomKind::Unsupported;
    }
    match &*name.local {
        "Point" => GeomKind::Point,
        "LineString" => GeomKind::LineString,
        "LinearRing" => GeomKind::LinearRing,
        "Polygon" => GeomKind::Polygon,
        "Curve" => GeomKind::Curve,
        "OrientableCurve" => GeomKind::OrientableCurve,
        "CompositeCurve" => GeomKind::CompositeCurve,
        "Ring" => GeomKind::Ring,
        "Surface" => GeomKind::Surface,
        "OrientableSurface" => GeomKind::OrientableSurface,
        "CompositeSurface" => GeomKind::CompositeSurface,
        "MultiPoint" => GeomKind::MultiPoint,
        "MultiLineString" => GeomKind::MultiLineString,
        "MultiCurve" => GeomKind::MultiCurve,
        "MultiPolygon" => GeomKind::MultiPolygon,
        "MultiSurface" => GeomKind::MultiSurface,
        "MultiGeometry" => GeomKind::MultiGeometry,
        "Envelope" | "EnvelopeWithTimePeriod" => GeomKind::Envelope,
        "Box" => GeomKind::Box,
        _ => GeomKind::Unsupported,
    }
}

/// GML's array properties (`gml:PointArrayPropertyType` and its siblings),
/// which hold several geometries of one kind: read as one Multi geometry
/// ([`crate::Geometry::from_parts`]). Any other property holds one geometry.
pub fn is_array_property(name: &QName) -> bool {
    ["pointArrayProperty", "curveArrayProperty", "surfaceArrayProperty", "solidArrayProperty"]
        .iter()
        .any(|local| name.is_gml_named(local))
}

/// Curve segments given by points or by parameters (the ones we read).
pub(crate) const ARC_SEGMENTS: &[&str] = &[
    "Arc",
    "ArcString",
    "Circle",
    "ArcByCenterPoint",
    "CircleByCenterPoint",
    "ArcByBulge",
    "ArcStringByBulge",
];

/// Curve segments we read: `LineStringSegment` and the arcs.
pub(crate) const SEGMENTS: &[&str] = &[
    "LineStringSegment",
    "Arc",
    "ArcString",
    "Circle",
    "ArcByCenterPoint",
    "CircleByCenterPoint",
    "ArcByBulge",
    "ArcStringByBulge",
];

/// Geometry, segment and patch elements that are out of scope (support
/// matrix §5): they are geometry errors.
pub(crate) const UNSUPPORTED: &[&str] = &[
    "Solid",
    "Shell",
    "CompositeSolid",
    "MultiSolid",
    "PolyhedralSurface",
    "TriangulatedSurface",
    "Tin",
    "Cone",
    "Cylinder",
    "Sphere",
    "Grid",
    "RectifiedGrid",
    "GeometricComplex",
    "GeodesicString",
    "Geodesic",
    "CubicSpline",
    "BSpline",
    "Bezier",
    "Clothoid",
    "OffsetCurve",
    "LineStringSegment3D",
];

/// A start element with the attributes the geometry elements use, copied
/// out of the reader in one pass.
#[derive(Debug)]
pub(crate) struct Elem {
    pub name: QName,
    pub attrs: Attrs,
}

impl Elem {
    pub fn local(&self) -> &str {
        &self.name.local
    }

    /// `local` in one of the GML namespaces.
    pub fn is(&self, local: &str) -> bool {
        self.name.is_gml_named(local)
    }
}

#[derive(Debug, Default)]
pub(crate) struct Attrs {
    pub srs_name: Option<String>,
    pub srs_dimension: Option<u8>,
    /// The GML 3.0 `posList dimension` (a synonym of `srsDimension`).
    pub dimension: Option<u8>,
    pub count: Option<usize>,
    pub num_arc: Option<usize>,
    /// `orientation="-"`.
    pub reversed: bool,
    pub href: bool,
    pub interpolation: Option<String>,
    pub uom: Option<String>,
    pub axis_labels: Option<String>,
    /// `decimal`, `cs`, `ts` of `coordinates`.
    pub decimal: Option<String>,
    pub cs: Option<String>,
    pub ts: Option<String>,
}

impl Attrs {
    pub fn read(attrs: &Attributes<'_>) -> Self {
        let mut out = Attrs::default();
        for (namespace, local, value) in attrs.iter_raw() {
            match (namespace, local) {
                (None, "srsName") => out.srs_name = Some(value.into_owned()),
                (None, "srsDimension") => out.srs_dimension = value.trim().parse().ok(),
                (None, "dimension") => out.dimension = value.trim().parse().ok(),
                (None, "count") => out.count = value.trim().parse().ok(),
                (None, "numArc") => out.num_arc = value.trim().parse().ok(),
                (None, "orientation") => out.reversed = value.trim() == "-",
                (Some(ns::XLINK), "href") => out.href = true,
                (None, "interpolation") => out.interpolation = Some(value.into_owned()),
                (None, "uom") => out.uom = Some(value.into_owned()),
                (None, "axisLabels") => out.axis_labels = Some(value.into_owned()),
                (None, "decimal") => out.decimal = Some(value.into_owned()),
                (None, "cs") => out.cs = Some(value.into_owned()),
                (None, "ts") => out.ts = Some(value.into_owned()),
                _ => {}
            }
        }
        out
    }
}

/// The element whose `Start` the reader returned last.
pub(crate) fn current_element(reader: &GmlReader<'_>) -> crate::Result<Elem> {
    let (name, attrs) = reader.current_start().ok_or_else(|| {
        Error::Core(xeibe_core::Error::Xml {
            location: reader.location(),
            message: "the geometry parser must be called right after a start element".into(),
        })
    })?;
    Ok(Elem { attrs: Attrs::read(&attrs), name })
}

/// Inherited values for the dimension rule (`docs/geometry.md`, "Dimension").
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Scope {
    /// `srsDimension` of the nearest ancestor that has one.
    pub srs_dimension: Option<u8>,
    /// Dimension of the CRS named by the nearest srsName.
    pub crs_dimension: Option<u8>,
}

/// State of one geometry parse.
pub(crate) struct Parser<'p> {
    pub table: Cow<'p, CrsTable>,
    axis: Option<&'p dyn AxisResolver>,
    fixed: Option<bool>,
    /// See [`ParsedGeometry::srs_name`].
    pub srs_name: Option<String>,
    decisions: Vec<(Option<String>, Dialect, bool)>,
    pub dialect: DialectTracker,
    pub warnings: Vec<String>,
    /// Elements opened below (and including) the root and not yet closed.
    depth: usize,
    warned_srs: bool,
}

impl<'p> Parser<'p> {
    pub fn new(
        options: &'p GeometryOptions,
        axis: Option<&'p dyn AxisResolver>,
        context: &ParseContext,
    ) -> Self {
        let table = match &options.axis.crs_table {
            Some(table) => Cow::Borrowed(table),
            None => Cow::Owned(CrsTable::builtin()),
        };
        Parser {
            table,
            axis,
            fixed: context.axis.as_ref().map(|decision| decision.swap),
            srs_name: None,
            decisions: Vec::new(),
            dialect: DialectTracker::default(),
            warnings: Vec::new(),
            depth: 1,
            warned_srs: false,
        }
    }

    fn root_scope(&self, context: &ParseContext) -> Scope {
        Scope {
            srs_dimension: context.srs_dimension,
            crs_dimension: context
                .srs_name
                .as_deref()
                .and_then(|srs| assemble::crs_dimension(srs, &self.table)),
        }
    }

    /// Take an element's srsName and srsDimension into the scope of its
    /// content, and note the srsName for the geometry.
    pub fn enter(&mut self, elem: &Elem, scope: Scope) -> Scope {
        let mut scope = scope;
        if let Some(srs) = &elem.attrs.srs_name {
            scope.crs_dimension = assemble::crs_dimension(srs, &self.table);
            match &self.srs_name {
                None => self.srs_name = Some(srs.clone()),
                Some(current) if current != srs && !self.warned_srs => {
                    self.warned_srs = true;
                    self.warnings.push(format!(
                        "srsName {srs:?} inside a geometry in {current:?}; the geometry's srsName is used"
                    ));
                }
                Some(_) => {}
            }
        }
        if let Some(dimension) = elem.attrs.srs_dimension {
            scope.srs_dimension = Some(dimension);
        }
        scope
    }

    /// Whether to swap, for the geometry's srsName and the dialect seen so
    /// far. Asked once per key.
    pub fn swap(&mut self) -> bool {
        if let Some(swap) = self.fixed {
            return swap;
        }
        let Some(axis) = self.axis else {
            return false;
        };
        let dialect = self.dialect.result();
        if let Some((_, _, swap)) =
            self.decisions.iter().find(|(srs, d, _)| *srs == self.srs_name && *d == dialect)
        {
            return *swap;
        }
        let swap = axis.resolve(self.srs_name.as_deref(), dialect).swap;
        self.decisions.push((self.srs_name.clone(), dialect, swap));
        swap
    }

    /// Facts about the geometry's CRS, if it is in the table.
    pub fn crs_info(&self) -> Option<CrsInfo> {
        let srs = SrsName::parse(self.srs_name.as_deref()?);
        match srs.crs?.horizontal() {
            crate::crs::CrsRef::Code { authority, code } => self.table.get(authority, code),
            crate::crs::CrsRef::Compound(_) | crate::crs::CrsRef::Unresolved(_) => None,
        }
    }

    // ------------------------------------------------------------ events

    /// The next child element of the current element, or `None` at its end
    /// tag. Text between child elements is ignored.
    pub fn next_child(&mut self, reader: &mut GmlReader<'_>) -> crate::Result<Option<Elem>> {
        loop {
            match reader.next_event()? {
                XmlEvent::Start { name, attrs } => {
                    let attrs = Attrs::read(&attrs);
                    self.depth += 1;
                    self.dialect.observe(&name);
                    return Ok(Some(Elem { name, attrs }));
                }
                XmlEvent::End { .. } => {
                    self.depth = self.depth.saturating_sub(1);
                    return Ok(None);
                }
                XmlEvent::Text(_) => {}
                XmlEvent::Eof => return Err(unexpected_eof(reader)),
            }
        }
    }

    /// Skip the element whose start [`Self::next_child`] just returned.
    pub fn skip(&mut self, reader: &mut GmlReader<'_>) -> crate::Result<()> {
        reader.skip_element()?;
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    /// After an error: close every element still open, up to the root's end.
    fn recover(&mut self, reader: &mut GmlReader<'_>) -> crate::Result<()> {
        while self.depth > 0 {
            self.skip(reader)?;
        }
        Ok(())
    }

    /// Append the numbers in the current element's text to `out`, up to its
    /// end tag. Child elements are skipped.
    pub fn read_numbers(&mut self, reader: &mut GmlReader<'_>, out: &mut Vec<f64>) -> crate::Result<()> {
        loop {
            let result = match reader.next_event()? {
                XmlEvent::Text(text) => coords::parse_numbers(&text, out),
                XmlEvent::Start { .. } => {
                    self.depth += 1;
                    self.skip(reader)?;
                    Ok(())
                }
                XmlEvent::End { .. } => {
                    self.depth = self.depth.saturating_sub(1);
                    return Ok(());
                }
                XmlEvent::Eof => return Err(unexpected_eof(reader)),
            };
            result.map_err(|error| error.at(reader.location()))?;
        }
    }

    /// The current element's text, up to its end tag. Child elements are skipped.
    pub fn read_text(&mut self, reader: &mut GmlReader<'_>) -> crate::Result<String> {
        let mut out = String::new();
        loop {
            match reader.next_event()? {
                XmlEvent::Text(text) => out.push_str(&text),
                XmlEvent::Start { .. } => {
                    self.depth += 1;
                    self.skip(reader)?;
                }
                XmlEvent::End { .. } => {
                    self.depth = self.depth.saturating_sub(1);
                    return Ok(out);
                }
                XmlEvent::Eof => return Err(unexpected_eof(reader)),
            }
        }
    }

    /// Call `member` for every child element of a member property
    /// (`curveMember`, `baseSurface`, `pointMembers`, …), which must consume
    /// it. Content outside GML is passed on too: it is an unknown geometry.
    /// A property with no inline content but an `xlink:href` is a geometry
    /// by reference. Returns the number of members.
    pub fn members(
        &mut self,
        reader: &mut GmlReader<'_>,
        property: &Elem,
        scope: Scope,
        mut member: impl FnMut(&mut Self, &mut GmlReader<'_>, Elem, Scope) -> crate::Result<()>,
    ) -> crate::Result<usize> {
        let scope = self.enter(property, scope);
        let mut count = 0;
        while let Some(child) = self.next_child(reader)? {
            member(self, reader, child, scope)?;
            count += 1;
        }
        if count == 0 && property.attrs.href {
            return Err(Error::ByReference { location: reader.location() });
        }
        Ok(count)
    }

    // ------------------------------------------------------------ errors

    pub fn unsupported(&self, reader: &GmlReader<'_>, element: impl Into<String>) -> Error {
        Error::Unsupported { element: element.into(), location: reader.location() }
    }

    pub fn invalid(&self, reader: &GmlReader<'_>, message: impl Into<String>) -> Error {
        Error::InvalidGeometry { message: message.into(), location: reader.location() }
    }

    pub fn position_count(
        &self,
        reader: &GmlReader<'_>,
        element: &'static str,
        found: usize,
        expected: impl Into<String>,
    ) -> Error {
        Error::PositionCount { element, location: reader.location(), found, expected: expected.into() }
    }

    /// The error for an element that is not a geometry of the expected kind:
    /// unsupported if it is out of scope (or not GML), else invalid.
    pub fn wrong_kind(&self, reader: &GmlReader<'_>, elem: &Elem, expected: &str) -> Error {
        if !elem.name.is_gml() {
            self.unsupported(reader, elem.name.to_clark())
        } else if UNSUPPORTED.contains(&elem.local()) {
            self.unsupported(reader, elem.local())
        } else {
            self.invalid(reader, format!("{} is not {expected}", elem.local()))
        }
    }

    // ------------------------------------------------------------ dispatch

    /// Any geometry element, consumed up to its end tag.
    pub fn geometry(&mut self, reader: &mut GmlReader<'_>, elem: Elem, scope: Scope) -> crate::Result<Geometry> {
        if !elem.name.is_gml() {
            return Err(self.unsupported(reader, elem.name.to_clark()));
        }
        let href = elem.attrs.href;
        let geometry = match elem.local() {
            "Point" => Geometry::Point(self.point(reader, &elem, scope)?),
            "LineString" => Geometry::LineString(self.line_string(reader, &elem, scope)?),
            "LinearRing" => Geometry::LineString(self.linear_ring(reader, &elem, scope)?),
            "Curve" | "OrientableCurve" | "CompositeCurve" | "Ring" => {
                assemble::curve_to_geometry(self.curve(reader, elem, scope)?)
            }
            "Polygon" | "Surface" | "OrientableSurface" => {
                assemble::surfaces_to_geometry(self.surfaces(reader, elem, scope)?)
            }
            "CompositeSurface" => {
                let surfaces = self.surfaces(reader, elem, scope)?;
                Geometry::MultiSurface(crate::model::MultiSurface(surfaces)).simplify_types()
            }
            "MultiPoint" | "MultiLineString" | "MultiCurve" | "MultiPolygon" | "MultiSurface"
            | "MultiGeometry" => self.aggregate(reader, &elem, scope)?,
            "Envelope" | "EnvelopeWithTimePeriod" | "Box" => {
                Geometry::Polygon(self.envelope(reader, &elem, scope)?.to_polygon())
            }
            other => return Err(self.unsupported(reader, other)),
        };
        if href && geometry.dim().is_none() {
            return Err(Error::ByReference { location: reader.location() });
        }
        Ok(geometry)
    }

    /// A surface element (`Polygon`, `Surface`, `OrientableSurface`,
    /// `CompositeSurface`) as its surfaces.
    pub fn surfaces(&mut self, reader: &mut GmlReader<'_>, elem: Elem, scope: Scope) -> crate::Result<Vec<Surface>> {
        let surfaces = match elem.local() {
            "Polygon" if elem.name.is_gml() => vec![self.polygon(reader, &elem, scope)?],
            "Surface" if elem.name.is_gml() => self.surface(reader, &elem, scope)?,
            "OrientableSurface" if elem.name.is_gml() => self.orientable_surface(reader, &elem, scope)?,
            "CompositeSurface" if elem.name.is_gml() => self.composite_surface(reader, &elem, scope)?,
            _ => return Err(self.wrong_kind(reader, &elem, "a surface")),
        };
        if elem.attrs.href && surfaces.iter().all(|s| s.dim().is_none()) {
            return Err(Error::ByReference { location: reader.location() });
        }
        Ok(surfaces)
    }
}

fn unexpected_eof(reader: &GmlReader<'_>) -> Error {
    Error::Core(xeibe_core::Error::Xml {
        location: reader.location(),
        message: "unexpected end of input inside a geometry".into(),
    })
}

/// Apply a swap decision to every position (only the first two ordinates).
pub(crate) fn swap_geometry(geometry: &mut Geometry) {
    use crate::model::{Curve, CurvePart, Point};
    fn point(point: &mut Point) {
        if let Some(coord) = point.coord.as_mut().filter(|c| c.len() >= 2) {
            coord.swap(0, 1);
        }
    }
    fn curve(curve: &mut Curve) {
        match curve {
            Curve::Linear(line) => swap_xy(&mut line.coords),
            Curve::Circular(arc) => swap_xy(&mut arc.coords),
            Curve::Compound(compound) => {
                for part in &mut compound.parts {
                    match part {
                        CurvePart::Linear(line) => swap_xy(&mut line.coords),
                        CurvePart::Circular(arc) => swap_xy(&mut arc.coords),
                    }
                }
            }
        }
    }
    fn polygon(polygon: &mut crate::model::Polygon) {
        for ring in polygon.exterior.iter_mut().chain(&mut polygon.interiors) {
            swap_xy(&mut ring.coords);
        }
    }
    fn surface(surface: &mut Surface) {
        match surface {
            Surface::Polygon(p) => polygon(p),
            Surface::CurvePolygon(p) => p.exterior.iter_mut().chain(&mut p.interiors).for_each(curve),
        }
    }
    match geometry {
        Geometry::Point(p) => point(p),
        Geometry::LineString(line) => swap_xy(&mut line.coords),
        Geometry::Polygon(p) => polygon(p),
        Geometry::MultiPoint(points) => points.0.iter_mut().for_each(point),
        Geometry::MultiLineString(lines) => lines.0.iter_mut().for_each(|l| swap_xy(&mut l.coords)),
        Geometry::MultiPolygon(polygons) => polygons.0.iter_mut().for_each(polygon),
        Geometry::GeometryCollection(members) => members.0.iter_mut().for_each(swap_geometry),
        Geometry::CircularString(arc) => swap_xy(&mut arc.coords),
        Geometry::CompoundCurve(compound) => {
            let mut whole = Curve::Compound(std::mem::take(compound));
            curve(&mut whole);
            if let Curve::Compound(swapped) = whole {
                *compound = swapped;
            }
        }
        Geometry::CurvePolygon(p) => p.exterior.iter_mut().chain(&mut p.interiors).for_each(curve),
        Geometry::MultiCurve(curves) => curves.0.iter_mut().for_each(curve),
        Geometry::MultiSurface(surfaces) => surfaces.0.iter_mut().for_each(surface),
    }
}
