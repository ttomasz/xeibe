//! Geometry column builders: native GeoArrow types or WKB (with curves).

use std::sync::Arc;

use arrow_array::builder::BinaryBuilder;
use arrow_array::{ArrayRef, Float64Array, StructArray};
use arrow_buffer::NullBuffer;
use arrow_schema::{ArrowError, DataType};
use geoarrow_array::GeoArrowArray;
use geoarrow_array::builder::{
    GeometryBuilder, LineStringBuilder, MultiLineStringBuilder, MultiPointBuilder,
    MultiPolygonBuilder, PointBuilder, PolygonBuilder,
};
use geoarrow_schema::{Dimension, GeoArrowType};
use xeibe_geom::model::{Coords, Curve, CurvePart, Point, Polygon, Surface};
use xeibe_geom::wkb::{Endianness, write_wkb};
use xeibe_geom::model::Envelope;
use xeibe_geom::{Dim, Geometry};

/// What a geometry column holds, from its field's GeoArrow extension type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryKind {
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    /// `geoarrow.geometry`: any simple-feature geometry.
    Mixed,
    /// `geoarrow.wkb`: anything, curves included.
    Wkb,
}

/// The kind and the dimension a column's geometries are written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeometrySpec {
    pub kind: GeometryKind,
    /// `None`: each geometry keeps its own dimension.
    pub dim: Option<Dim>,
}

impl GeometrySpec {
    /// A native column has the dimension of its type; WKB and
    /// `geoarrow.geometry` columns keep what is written.
    pub fn for_type(geometry: &GeoArrowType) -> crate::Result<Self> {
        let native = |kind: GeometryKind, dimension: Dimension| -> crate::Result<Self> {
            let dim = match dimension {
                Dimension::XY => Dim::Xy,
                Dimension::XYZ => Dim::Xyz,
                other => {
                    return Err(ArrowError::InvalidArgumentError(format!(
                        "geometry dimension {other:?} is not supported (GML has no measures)"
                    ))
                    .into());
                }
            };
            Ok(GeometrySpec { kind, dim: Some(dim) })
        };
        match geometry {
            GeoArrowType::Point(t) => native(GeometryKind::Point, t.dimension()),
            GeoArrowType::LineString(t) => native(GeometryKind::LineString, t.dimension()),
            GeoArrowType::Polygon(t) => native(GeometryKind::Polygon, t.dimension()),
            GeoArrowType::MultiPoint(t) => native(GeometryKind::MultiPoint, t.dimension()),
            GeoArrowType::MultiLineString(t) => native(GeometryKind::MultiLineString, t.dimension()),
            GeoArrowType::MultiPolygon(t) => native(GeometryKind::MultiPolygon, t.dimension()),
            GeoArrowType::Geometry(_) => Ok(GeometrySpec { kind: GeometryKind::Mixed, dim: None }),
            GeoArrowType::Wkb(_) => Ok(GeometrySpec { kind: GeometryKind::Wkb, dim: None }),
            other => Err(ArrowError::InvalidArgumentError(format!(
                "geometry columns of type {other:?} are not supported"
            ))
            .into()),
        }
    }

    /// Bring a parsed geometry into the column's form: simple types for native
    /// columns, the column's dimension (a 2D value in an XYZ column gets a NaN
    /// Z). `Err` gives the geometry back when the column can't hold it: a
    /// curve or another kind in a native column, or a Z value in an XY one.
    pub fn prepare(&self, geometry: Geometry) -> Result<Geometry, Geometry> {
        let mut geometry = match self.kind {
            GeometryKind::Wkb => geometry,
            _ => {
                let geometry = geometry.simplify_types();
                if !geometry.is_simple() || !self.accepts_kind(&geometry) {
                    return Err(geometry);
                }
                geometry
            }
        };
        match self.dim {
            Some(Dim::Xy) if geometry.dim().is_some_and(|dim| dim != Dim::Xy) => return Err(geometry),
            // A no-op for parts that already have it; mixed 2D/3D parts differ.
            Some(dim) => force_dim(&mut geometry, dim),
            None => {}
        }
        Ok(geometry)
    }

    /// The column's type, for messages: `Polygon`, `MultiPolygon XYZ`, `WKB`.
    pub fn describe(&self) -> String {
        let dim = match self.dim {
            Some(Dim::Xyz) => " XYZ",
            _ => "",
        };
        format!("a {:?}{dim} column", self.kind)
    }

    fn accepts_kind(&self, geometry: &Geometry) -> bool {
        use Geometry as G;
        match (self.kind, geometry) {
            (GeometryKind::Wkb | GeometryKind::Mixed, _) => true,
            // A single column holds its kind only, not a one-part Multi form.
            (GeometryKind::Point, G::Point(_)) => true,
            (GeometryKind::LineString, G::LineString(_)) => true,
            (GeometryKind::Polygon, G::Polygon(_)) => true,
            (GeometryKind::MultiPoint, G::Point(_) | G::MultiPoint(_)) => true,
            (GeometryKind::MultiLineString, G::LineString(_) | G::MultiLineString(_)) => true,
            (GeometryKind::MultiPolygon, G::Polygon(_) | G::MultiPolygon(_)) => true,
            _ => false,
        }
    }
}

pub struct GeometryColumnBuilder {
    typ: GeoArrowType,
    inner: Inner,
    /// Values pushed since the last `finish`.
    len: usize,
    /// Scratch buffer for WKB.
    wkb: Vec<u8>,
}

enum Inner {
    Point(PointBuilder),
    LineString(LineStringBuilder),
    Polygon(PolygonBuilder),
    MultiPoint(MultiPointBuilder),
    MultiLineString(MultiLineStringBuilder),
    MultiPolygon(MultiPolygonBuilder),
    /// Boxed: it holds a builder per kind and dimension.
    Geometry(Box<GeometryBuilder>),
    Wkb(BinaryBuilder),
}

impl GeometryColumnBuilder {
    /// Chosen from the field's GeoArrow extension type (set by the rule engine).
    pub fn for_field(field: &arrow_schema::Field, capacity: usize) -> crate::Result<Self> {
        let typ = geoarrow_type(field)?;
        let inner = Inner::new(&typ, capacity)?;
        Ok(GeometryColumnBuilder { typ, inner, len: 0, wkb: Vec::new() })
    }

    /// Append a geometry already brought into the column's form
    /// ([`GeometrySpec::prepare`]), or a null.
    pub fn push(&mut self, geometry: Option<&Geometry>) -> crate::Result<()> {
        let error = |e: geoarrow_schema::error::GeoArrowError| ArrowError::ExternalError(Box::new(e));
        self.len += 1;
        match &mut self.inner {
            Inner::Wkb(builder) => match geometry {
                Some(geometry) => {
                    self.wkb.clear();
                    write_wkb(geometry, Endianness::Little, &mut self.wkb);
                    builder.append_value(&self.wkb);
                }
                None => builder.append_null(),
            },
            Inner::Point(builder) => builder.push_geometry(geometry).map_err(error)?,
            Inner::LineString(builder) => builder.push_geometry(geometry).map_err(error)?,
            Inner::Polygon(builder) => builder.push_geometry(geometry).map_err(error)?,
            Inner::MultiPoint(builder) => builder.push_geometry(geometry).map_err(error)?,
            Inner::MultiLineString(builder) => builder.push_geometry(geometry).map_err(error)?,
            Inner::MultiPolygon(builder) => builder.push_geometry(geometry).map_err(error)?,
            Inner::Geometry(builder) => builder.push_geometry(geometry).map_err(error)?,
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The column so far; the builder starts over.
    pub fn finish(&mut self) -> ArrayRef {
        self.len = 0;
        let fresh = Inner::new(&self.typ, 0).expect("the type was accepted before");
        match std::mem::replace(&mut self.inner, fresh) {
            Inner::Point(b) => b.finish().into_array_ref(),
            Inner::LineString(b) => b.finish().into_array_ref(),
            Inner::Polygon(b) => b.finish().into_array_ref(),
            Inner::MultiPoint(b) => b.finish().into_array_ref(),
            Inner::MultiLineString(b) => b.finish().into_array_ref(),
            Inner::MultiPolygon(b) => b.finish().into_array_ref(),
            Inner::Geometry(b) => b.finish().into_array_ref(),
            Inner::Wkb(mut b) => Arc::new(b.finish()),
        }
    }
}

impl Inner {
    fn new(typ: &GeoArrowType, capacity: usize) -> crate::Result<Self> {
        Ok(match typ {
            GeoArrowType::Point(t) => Inner::Point(PointBuilder::with_capacity(t.clone(), capacity)),
            GeoArrowType::LineString(t) => Inner::LineString(LineStringBuilder::new(t.clone())),
            GeoArrowType::Polygon(t) => Inner::Polygon(PolygonBuilder::new(t.clone())),
            GeoArrowType::MultiPoint(t) => Inner::MultiPoint(MultiPointBuilder::new(t.clone())),
            GeoArrowType::MultiLineString(t) => Inner::MultiLineString(MultiLineStringBuilder::new(t.clone())),
            GeoArrowType::MultiPolygon(t) => Inner::MultiPolygon(MultiPolygonBuilder::new(t.clone())),
            GeoArrowType::Geometry(t) => Inner::Geometry(Box::new(GeometryBuilder::new(t.clone()))),
            GeoArrowType::Wkb(_) => Inner::Wkb(BinaryBuilder::with_capacity(capacity, capacity * 32)),
            other => {
                return Err(ArrowError::InvalidArgumentError(format!(
                    "geometry columns of type {other:?} are not supported"
                ))
                .into());
            }
        })
    }
}

/// The GeoArrow type of a geometry field.
pub fn geoarrow_type(field: &arrow_schema::Field) -> crate::Result<GeoArrowType> {
    GeoArrowType::from_extension_field(field)
        .map_err(|e| ArrowError::ExternalError(Box::new(e)))?
        .ok_or_else(|| {
            ArrowError::InvalidArgumentError(format!("column {} has no GeoArrow type", field.name())).into()
        })
}

/// `geoarrow.box` columns: a struct of `xmin, ymin[, zmin], xmax, ymax[, zmax]`.
pub struct BoxColumnBuilder {
    fields: arrow_schema::Fields,
    values: Vec<Vec<f64>>,
    validity: Vec<bool>,
}

impl BoxColumnBuilder {
    pub fn for_field(field: &arrow_schema::Field) -> crate::Result<Self> {
        let DataType::Struct(fields) = field.data_type() else {
            return Err(ArrowError::InvalidArgumentError(format!(
                "box column {} is not a struct",
                field.name()
            ))
            .into());
        };
        Ok(BoxColumnBuilder {
            fields: fields.clone(),
            values: vec![Vec::new(); fields.len()],
            validity: Vec::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.validity.len()
    }

    pub fn is_empty(&self) -> bool {
        self.validity.is_empty()
    }

    pub fn push(&mut self, envelope: Option<&Envelope>) {
        let dims = self.fields.len() / 2;
        let corners = envelope.filter(|e| e.lower.len() >= 2 && e.upper.len() >= 2);
        for (i, column) in self.values.iter_mut().enumerate() {
            let value = corners.map_or(0.0, |e| {
                let corner = if i < dims { &e.lower } else { &e.upper };
                corner.get(i % dims).copied().unwrap_or(f64::NAN)
            });
            column.push(value);
        }
        self.validity.push(corners.is_some());
    }

    pub fn finish(&mut self) -> crate::Result<ArrayRef> {
        let columns: Vec<ArrayRef> = self
            .values
            .iter_mut()
            .map(|values| Arc::new(Float64Array::from(std::mem::take(values))) as ArrayRef)
            .collect();
        let validity = NullBuffer::from(std::mem::take(&mut self.validity));
        let nulls = (validity.null_count() > 0).then_some(validity);
        Ok(Arc::new(StructArray::try_new(self.fields.clone(), columns, nulls)?))
    }
}

/// Drop Z (`Xy`), or add a NaN Z to 2D positions (`Xyz`).
pub fn force_dim(geometry: &mut Geometry, dim: Dim) {
    visit_coords(
        geometry,
        &mut |coords: &mut Coords| {
            let Some(from) = coords.dim else {
                return;
            };
            if from == dim {
                return;
            }
            let (from_size, to_size) = (from.size(), dim.size());
            let mut values = Vec::with_capacity(coords.values.len() / from_size * to_size);
            for position in coords.values.chunks_exact(from_size) {
                for i in 0..to_size {
                    values.push(position.get(i).copied().unwrap_or(f64::NAN));
                }
            }
            coords.values = values;
            coords.dim = Some(dim);
        },
        &mut |point: &mut Point| {
            if let Some(coord) = point.coord.as_mut() {
                coord.resize(dim.size(), f64::NAN);
            }
        },
    );
}

/// Call `on_coords` for every coordinate sequence and `on_point` for every point.
fn visit_coords(
    geometry: &mut Geometry,
    on_coords: &mut dyn FnMut(&mut Coords),
    on_point: &mut dyn FnMut(&mut Point),
) {
    fn curve(curve: &mut Curve, on_coords: &mut dyn FnMut(&mut Coords)) {
        match curve {
            Curve::Linear(line) => on_coords(&mut line.coords),
            Curve::Circular(arc) => on_coords(&mut arc.coords),
            Curve::Compound(compound) => {
                for part in &mut compound.parts {
                    match part {
                        CurvePart::Linear(line) => on_coords(&mut line.coords),
                        CurvePart::Circular(arc) => on_coords(&mut arc.coords),
                    }
                }
            }
        }
    }
    fn polygon(polygon: &mut Polygon, on_coords: &mut dyn FnMut(&mut Coords)) {
        for ring in polygon.exterior.iter_mut().chain(&mut polygon.interiors) {
            on_coords(&mut ring.coords);
        }
    }
    match geometry {
        Geometry::Point(p) => on_point(p),
        Geometry::LineString(line) => on_coords(&mut line.coords),
        Geometry::Polygon(p) => polygon(p, on_coords),
        Geometry::MultiPoint(points) => points.0.iter_mut().for_each(on_point),
        Geometry::MultiLineString(lines) => lines.0.iter_mut().for_each(|l| on_coords(&mut l.coords)),
        Geometry::MultiPolygon(polygons) => polygons.0.iter_mut().for_each(|p| polygon(p, on_coords)),
        Geometry::GeometryCollection(members) => {
            for member in &mut members.0 {
                visit_coords(member, on_coords, on_point);
            }
        }
        Geometry::CircularString(arc) => on_coords(&mut arc.coords),
        Geometry::CompoundCurve(compound) => {
            for part in &mut compound.parts {
                on_coords(part.coords_mut());
            }
        }
        Geometry::CurvePolygon(p) => {
            for ring in p.exterior.iter_mut().chain(&mut p.interiors) {
                curve(ring, on_coords);
            }
        }
        Geometry::MultiCurve(curves) => curves.0.iter_mut().for_each(|c| curve(c, on_coords)),
        Geometry::MultiSurface(surfaces) => {
            for surface in &mut surfaces.0 {
                match surface {
                    Surface::Polygon(p) => polygon(p, on_coords),
                    Surface::CurvePolygon(p) => {
                        for ring in p.exterior.iter_mut().chain(&mut p.interiors) {
                            curve(ring, on_coords);
                        }
                    }
                }
            }
        }
    }
}
