//! Conversion between two CRSs on the same datum.

use std::sync::Arc;

use crate::crs::{Axes, Crs, Kind};
use crate::error::{CoordinateError, Error};
use crate::geocentric::{cartesian, geodetic};
use crate::grid::Grids;
use crate::methods::{LAT_LIMIT, adjlon};
use crate::select::crs::CrsRef;
use crate::select::{Candidates, OperationInfo};

/// The order coordinates are given and returned in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisOrder {
    /// Easting or longitude first, as xeibe writes GeoArrow and GeoParquet:
    /// the first two values are swapped for a CRS whose first axis points
    /// north. Other axes stay as the CRS defines them, so a CRS with westing
    /// and southing axes keeps them.
    #[default]
    EastFirst,
    /// The CRS's own axis order, e.g. latitude first for EPSG:4326.
    Authority,
}

/// A conversion or transformation from one CRS to another, set up once and
/// applied to many coordinates.
#[derive(Debug, Clone)]
pub struct Transform {
    src: Crs,
    dst: Crs,
    order: AxisOrder,
    grids: Grids,
    plan: Plan,
}

#[derive(Debug, Clone)]
enum Plan {
    /// Both CRSs on one datum.
    Conversion,
    /// Between datums: candidate operations, chosen per coordinate.
    Operations(Arc<Candidates>),
    /// One operation forced with [`Transform::via`].
    Via(u32, Arc<Candidates>),
    /// One candidate of the selection, forced with
    /// [`Transform::with_operation_index`].
    Only(Arc<Candidates>),
}

impl Transform {
    /// Set up the conversion from `src` to `dst`. On one datum this is a
    /// conversion; between datums the transformation is chosen per
    /// coordinate as PROJ chooses it, among EPSG's operations, the
    /// grid-based ones included (`docs/reprojection.md`, levels 2 and 3).
    /// Grids must be loaded before converting coordinates that need them:
    /// see [`Transform::grid_requests`].
    pub fn new(src: &Crs, dst: &Crs, order: AxisOrder) -> Result<Self, Error> {
        Self::with_grids(src, dst, order, Grids::default())
    }

    /// [`Transform::new`] with a choice of which grids the selection may
    /// use: [`Grids::None`] keeps to operations without grids.
    pub fn with_grids(src: &Crs, dst: &Crs, order: AxisOrder, grids: Grids) -> Result<Self, Error> {
        for crs in [src, dst] {
            if matches!(crs.kind, Kind::Vertical { .. }) {
                return Err(Error::Unsupported {
                    crs: crs.label(),
                    what: "a vertical CRS on its own (a compound CRS gives its heights a position)"
                        .into(),
                });
            }
        }
        let plan = if same_datums(src, dst) {
            Plan::Conversion
        } else {
            let (Some(sc), Some(dc)) = (crs_ref(src), crs_ref(dst)) else {
                return Err(Error::NoTransformation {
                    src: src.label(),
                    dst: dst.label(),
                });
            };
            Plan::Operations(Arc::new(Candidates::new(src, dst, sc, dc, &grids)?))
        };
        Ok(Transform {
            src: src.clone(),
            dst: dst.clone(),
            order,
            grids,
            plan,
        })
    }

    /// The transformation through one EPSG coordinate operation, instead of
    /// the automatic choice: the operation must join the two CRSs' datums
    /// (in either direction) and is used for every coordinate.
    pub fn via(src: &Crs, dst: &Crs, operation: u32, order: AxisOrder) -> Result<Self, Error> {
        let (Some(sc), Some(dc)) = (crs_ref(src), crs_ref(dst)) else {
            return Err(Error::NoTransformation {
                src: src.label(),
                dst: dst.label(),
            });
        };
        Ok(Transform {
            src: src.clone(),
            dst: dst.clone(),
            order,
            grids: Grids::ProjData,
            plan: Plan::Via(
                operation,
                Arc::new(Candidates::with_operation(src, dst, sc, dc, operation)?),
            ),
        })
    }

    /// The candidates between datums, if any.
    pub(crate) fn candidates(&self) -> Option<&Candidates> {
        match &self.plan {
            Plan::Conversion => None,
            Plan::Operations(c) | Plan::Via(_, c) | Plan::Only(c) => Some(c),
        }
    }

    /// Names of the candidate operations, best first; empty for a
    /// conversion on one datum.
    pub fn operations(&self) -> Vec<&str> {
        match &self.plan {
            Plan::Conversion => Vec::new(),
            Plan::Operations(c) | Plan::Via(_, c) | Plan::Only(c) => c.names(),
        }
    }

    /// The name of the operation one coordinate goes through, or `None` for
    /// a conversion on one datum or a coordinate that can't be converted.
    pub fn operation_for(&self, x: f64, y: f64, z: Option<f64>) -> Option<&str> {
        let (Plan::Operations(c) | Plan::Via(_, c) | Plan::Only(c)) = &self.plan else {
            return None;
        };
        let internal = to_internal(&self.src.axes, self.order, [x, y, z.unwrap_or(0.0)]).ok()?;
        c.used(&self.src, &self.dst, internal)
    }

    /// The operations this transform chooses among, best first, with their
    /// EPSG codes, accuracy, area of use and grids; empty for a conversion
    /// on one datum. Indexes into this list are those of
    /// [`Transform::operation_index_for`] and
    /// [`Transform::with_operation_index`].
    pub fn operation_info(&self) -> Vec<OperationInfo> {
        self.candidates().map(Candidates::info).unwrap_or_default()
    }

    /// The index in [`Transform::operation_info`] of the operation one
    /// coordinate goes through, or `None` for a conversion on one datum or
    /// a coordinate that can't be converted.
    pub fn operation_index_for(&self, x: f64, y: f64, z: Option<f64>) -> Option<usize> {
        let c = self.candidates()?;
        let internal = to_internal(&self.src.axes, self.order, [x, y, z.unwrap_or(0.0)]).ok()?;
        c.used_index(&self.src, &self.dst, internal)
    }

    /// The transformation through operation `index` of
    /// [`Transform::operation_info`] only, for every coordinate whatever its
    /// area, as each member of PROJ's `TransformerGroup` is. `None` when
    /// there is no such operation. Its inverse runs the same operation the
    /// other way.
    pub fn with_operation_index(&self, index: usize) -> Option<Result<Transform, Error>> {
        let only = self.candidates()?.only(index, &self.src)?;
        Some(only.map(|c| Transform {
            src: self.src.clone(),
            dst: self.dst.clone(),
            order: self.order,
            grids: self.grids.clone(),
            plan: Plan::Only(Arc::new(c)),
        }))
    }

    /// Set up the conversion between two EPSG codes.
    pub fn from_epsg(src: u32, dst: u32, order: AxisOrder) -> Result<Self, Error> {
        Self::new(&Crs::from_epsg(src)?, &Crs::from_epsg(dst)?, order)
    }

    pub fn source(&self) -> &Crs {
        &self.src
    }

    pub fn target(&self) -> &Crs {
        &self.dst
    }

    /// The axis order coordinates are given and returned in.
    pub fn order(&self) -> AxisOrder {
        self.order
    }

    /// The conversion the other way. Between datums the operations are
    /// chosen again for that direction, so this can fail like [`Transform::new`].
    pub fn inverse(&self) -> Result<Transform, Error> {
        match self.plan {
            Plan::Conversion => Ok(Transform {
                src: self.dst.clone(),
                dst: self.src.clone(),
                order: self.order,
                grids: self.grids.clone(),
                plan: Plan::Conversion,
            }),
            Plan::Operations(_) => {
                Transform::with_grids(&self.dst, &self.src, self.order, self.grids.clone())
            }
            Plan::Via(code, _) => Transform::via(&self.dst, &self.src, code, self.order),
            Plan::Only(ref c) => Ok(Transform {
                src: self.dst.clone(),
                dst: self.src.clone(),
                order: self.order,
                grids: self.grids.clone(),
                plan: Plan::Only(Arc::new(c.inverse_of_first(&self.dst)?)),
            }),
        }
    }

    /// Convert one coordinate. `z` is the third value when the data has one:
    /// the height for a 3D CRS, carried through unchanged for a 2D one.
    ///
    /// A coordinate with a NaN in x or y (GeoArrow's empty point) comes back
    /// unchanged.
    pub fn convert(
        &self,
        x: f64,
        y: f64,
        z: Option<f64>,
    ) -> Result<(f64, f64, Option<f64>), CoordinateError> {
        if x.is_nan() || y.is_nan() {
            return Ok((x, y, z));
        }
        let internal = to_internal(&self.src.axes, self.order, [x, y, z.unwrap_or(0.0)])?;
        let out = match &self.plan {
            Plan::Conversion => on_one_datum(&self.src, &self.dst, internal)?,
            Plan::Operations(candidates) | Plan::Via(_, candidates) | Plan::Only(candidates) => {
                candidates.run(&self.src, &self.dst, internal)?
            }
        };
        let [x, y, h] = from_internal(&self.dst.axes, self.order, out);
        // A 2D target has no height of its own: hand back what came in.
        let z = z.map(|z| if self.dst.axes.dimension() == 3 { h } else { z });
        Ok((x, y, z))
    }

    /// Convert coordinates held as separate x and y buffers, in place. Stops
    /// at the first coordinate that fails, leaving the rest unconverted.
    pub fn convert_xy(&self, x: &mut [f64], y: &mut [f64]) -> Result<(), Error> {
        assert_eq!(x.len(), y.len(), "x and y buffers differ in length");
        for (index, (x, y)) in x.iter_mut().zip(y.iter_mut()).enumerate() {
            let (cx, cy, _) = self
                .convert(*x, *y, None)
                .map_err(|reason| coordinate_error(index, *x, *y, reason))?;
            (*x, *y) = (cx, cy);
        }
        Ok(())
    }

    /// Convert coordinates held as separate x, y and z buffers, in place.
    pub fn convert_xyz(&self, x: &mut [f64], y: &mut [f64], z: &mut [f64]) -> Result<(), Error> {
        assert!(
            x.len() == y.len() && y.len() == z.len(),
            "x, y and z buffers differ in length"
        );
        for (index, ((x, y), z)) in x.iter_mut().zip(y.iter_mut()).zip(z.iter_mut()).enumerate() {
            let (cx, cy, cz) = self
                .convert(*x, *y, Some(*z))
                .map_err(|reason| coordinate_error(index, *x, *y, reason))?;
            (*x, *y, *z) = (cx, cy, cz.unwrap_or(*z));
        }
        Ok(())
    }

    /// Convert interleaved coordinates (`x0 y0 x1 y1 …`, or with z when
    /// `dimension` is 3), in place. A fourth ordinate (M) is left alone.
    pub fn convert_interleaved(&self, coords: &mut [f64], dimension: usize) -> Result<(), Error> {
        assert!((2..=4).contains(&dimension), "dimension must be 2, 3 or 4");
        assert_eq!(
            coords.len() % dimension,
            0,
            "buffer is not a whole number of coordinates"
        );
        for (index, c) in coords.chunks_exact_mut(dimension).enumerate() {
            let z = (dimension >= 3).then(|| c[2]);
            let (x, y, z) = self
                .convert(c[0], c[1], z)
                .map_err(|reason| coordinate_error(index, c[0], c[1], reason))?;
            c[0] = x;
            c[1] = y;
            if let Some(z) = z {
                c[2] = z;
            }
        }
        Ok(())
    }
}

fn coordinate_error(index: usize, x: f64, y: f64, reason: CoordinateError) -> Error {
    Error::Coordinate {
        index,
        x,
        y,
        reason,
    }
}

/// The CRS as the selection knows it: its EPSG code, or a compound CRS's
/// two codes.
fn crs_ref(crs: &Crs) -> Option<CrsRef> {
    if let Some(code) = crs.code() {
        return Some(CrsRef::Epsg(code));
    }
    match &crs.kind {
        Kind::Compound {
            horizontal,
            vertical,
        } => Some(CrsRef::Compound(horizontal.code()?, vertical.code()?)),
        _ => None,
    }
}

/// Whether two CRSs differ only by a conversion: the same geodetic datum,
/// and heights that need no transformation. A gravity-related height
/// converts to one on the same vertical datum (another unit, or depth), and
/// to or from a CRS without heights, which drops it or carries the value
/// through (PROJ's horizontal-only operation); not to an ellipsoidal height.
fn same_datums(src: &Crs, dst: &Crs) -> bool {
    // An engineering datum has no tie to any other: only CRSs on it
    // convert, whatever their axes and units (PROJ's identity between
    // equivalent engineering CRSs).
    match (src.engineering_datum(), dst.engineering_datum()) {
        (Some(a), Some(b)) => return a.key == b.key,
        (None, None) => {}
        _ => return false,
    }
    let (Some(a), Some(b)) = (src.datum(), dst.datum()) else {
        return false;
    };
    let epsg = |key: &str| {
        key.strip_prefix("EPSG:")
            .and_then(|c| c.parse::<u32>().ok())
    };
    let same = match (epsg(&a.key), epsg(&b.key)) {
        // Same EPSG datum code; an ensemble and its member differ by a transformation.
        (Some(x), Some(y)) => crate::datum::same(x, y),
        _ => a.key == b.key,
    };
    if !(same && a.ellipsoid == b.ellipsoid && a.prime_meridian == b.prime_meridian) {
        return false;
    }
    let heightless = |c: &Crs| c.dimension() == 2;
    match (src.vertical_datum(), dst.vertical_datum()) {
        (None, None) => true,
        (Some(x), Some(y)) => x.key == y.key,
        (Some(_), None) => heightless(dst),
        (None, Some(_)) => heightless(src),
    }
}

/// Internal coordinates of one CRS to another on the same datum: through
/// geographic coordinates, or, on an engineering datum, the same position in
/// the grid's plane.
pub(crate) fn on_one_datum(
    src: &Crs,
    dst: &Crs,
    internal: [f64; 3],
) -> Result<[f64; 3], CoordinateError> {
    if src.engineering_datum().is_some() {
        return Ok(internal);
    }
    from_geographic(dst, to_geographic(src, internal)?)
}

/// From the caller's order and units to the internal order in radians and metres.
pub(crate) fn to_internal(
    axes: &Axes,
    order: AxisOrder,
    mut v: [f64; 3],
) -> Result<[f64; 3], CoordinateError> {
    if !v.iter().all(|v| v.is_finite()) {
        return Err(CoordinateError::NotFinite);
    }
    if order == AxisOrder::EastFirst && axes.first_is_northing() {
        v.swap(0, 1);
    }
    Ok(axes.to_internal(v))
}

/// The reverse of [`to_internal`].
fn from_internal(axes: &Axes, order: AxisOrder, v: [f64; 3]) -> [f64; 3] {
    let mut out = axes.to_crs(v);
    if order == AxisOrder::EastFirst && axes.first_is_northing() {
        out.swap(0, 1);
    }
    out
}

/// A CRS's internal coordinates to geographic ones on its datum: longitude
/// from the prime meridian, latitude, height.
pub(crate) fn to_geographic(crs: &Crs, internal: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
    Ok(match &crs.kind {
        Kind::Geographic { .. } => check_geographic(internal)?,
        Kind::Projected { projection, .. } => {
            let (lon, lat) = projection.inverse(internal[0], internal[1])?;
            if !(lon.is_finite() && lat.is_finite()) {
                return Err(CoordinateError::OutsideDomain);
            }
            [adjlon(lon), lat, internal[2]]
        }
        Kind::Geocentric { datum } => geodetic(internal, &datum.ellipsoid),
        // The height stays what it is: gravity-related, not ellipsoidal.
        Kind::Compound { horizontal, .. } => to_geographic(horizontal, internal)?,
        Kind::Vertical { .. } => internal,
        // A local grid has no position on the ellipsoid: only an operation
        // leaves it.
        Kind::Engineering { .. } => return Err(CoordinateError::OutsideDomain),
    })
}

/// The reverse of [`to_geographic`].
pub(crate) fn from_geographic(
    crs: &Crs,
    geographic: [f64; 3],
) -> Result<[f64; 3], CoordinateError> {
    Ok(match &crs.kind {
        Kind::Geographic { .. } => geographic,
        Kind::Projected { projection, .. } => {
            let [lon, lat, h] = check_geographic(geographic)?;
            let (e, n) = projection.forward(lon, lat)?;
            if !(e.is_finite() && n.is_finite()) {
                return Err(CoordinateError::OutsideDomain);
            }
            [e, n, h]
        }
        Kind::Geocentric { datum } => cartesian(check_geographic(geographic)?, &datum.ellipsoid),
        Kind::Compound { horizontal, .. } => from_geographic(horizontal, geographic)?,
        Kind::Vertical { .. } => geographic,
        Kind::Engineering { .. } => return Err(CoordinateError::OutsideDomain),
    })
}

/// Reject latitudes beyond the poles and longitudes beyond ±10 radians, as
/// PROJ does before projecting; clamp latitudes that overshoot by rounding.
fn check_geographic([lon, lat, h]: [f64; 3]) -> Result<[f64; 3], CoordinateError> {
    if lat.abs() > LAT_LIMIT || lon.abs() > 10.0 {
        return Err(CoordinateError::InvalidGeographic);
    }
    let lat = lat.clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
    Ok([lon, lat, h])
}
