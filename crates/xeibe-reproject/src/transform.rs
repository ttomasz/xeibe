//! Conversion between two CRSs on the same datum.

use std::sync::Arc;

use crate::crs::{Axes, Bound, Crs, Datum, Kind};
use crate::error::{CoordinateError, Error};
use crate::geocentric::{cartesian, geodetic};
use crate::grid::Grids;
use crate::methods::{LAT_LIMIT, adjlon};
use crate::operation::Method;
use crate::select::crs::CrsRef;
use crate::select::explain::{Explanation, Recorder};
use crate::select::trace::{Event, Trace, Tracer};
use crate::select::{Candidates, Motion, OperationInfo, Selection};

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
    selection: Selection,
    epochs: Epochs,
    plan: Plan,
    /// The plan before a point motion operation was added to it for two
    /// different epochs; `None` when `plan` has none.
    unmoved: Option<Plan>,
}

/// The coordinate epochs declared for a transform's CRSs
/// ([`Transform::with_epochs`]), as decimal years.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Epochs {
    source: Option<f64>,
    target: Option<f64>,
}

impl Epochs {
    /// The epoch every coordinate starts at. With one of the two PROJ
    /// takes it for both; with two that differ, a point motion operation
    /// moves the coordinates from the source's to the target's on the way.
    fn fixed(self) -> Option<f64> {
        self.source.or(self.target)
    }

    /// The two epochs, when they differ: the coordinates are to be moved
    /// in time.
    fn change(self) -> Option<(f64, f64)> {
        match (self.source, self.target) {
            (Some(from), Some(to)) if from != to => Some((from, to)),
            _ => None,
        }
    }

    fn swapped(self) -> Epochs {
        Epochs {
            source: self.target,
            target: self.source,
        }
    }
}

/// PROJ's `getRoundedEpochInDecimalYear`: an epoch within a millionth of a
/// year of a thousandth is that thousandth.
fn rounded_epoch(year: f64) -> f64 {
    let thousandths = 1000.0 * year;
    if (thousandths - thousandths.round()).abs() <= 1e-3 {
        thousandths.round() / 1000.0
    } else {
        year
    }
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
    /// From or to a Bound CRS (WKT1's `TOWGS84`): its own transformation
    /// to the hub, and on from there, as PROJ does.
    Bound(Arc<BoundPlan>),
}

/// The route through a Bound CRS's hub: the source's transformation to its
/// hub, a transform between the hubs (or a hub and the other CRS), and the
/// target's transformation back from its hub.
#[derive(Debug)]
struct BoundPlan {
    first: Option<Leg>,
    middle: Transform,
    last: Option<Leg>,
    /// The operations' names: the legs' and each of the middle's, joined
    /// as PROJ names a concatenated operation.
    names: Vec<String>,
}

/// A Bound CRS's transformation, between its base CRS's internal
/// coordinates and the hub's.
#[derive(Debug, Clone)]
struct Leg {
    base: Crs,
    /// The geodetic CRS the transformation starts from: the base's
    /// geographic CRS, or the base itself if geocentric.
    via: Crs,
    hub: Crs,
    method: Arc<dyn Method>,
    name: String,
}

impl Leg {
    fn new(base: &Crs, bound: &Bound) -> Result<Leg, Error> {
        Leg::to(base, bound, &bound.hub, bound.operation.name.to_string())
    }

    /// The bound transformation, landing on `hub` instead of the bound
    /// CRS's own, under `name`.
    fn to(base: &Crs, bound: &Bound, hub: &Crs, name: String) -> Result<Leg, Error> {
        // The transformation runs between geodetic CRSs: the base's own if
        // it is geocentric, else the geographic CRS on its datum.
        let via = match base.kind {
            Kind::Geocentric { .. } => base.clone(),
            _ => base.geographic_of().ok_or_else(|| Error::Unsupported {
                crs: base.label(),
                what: "a bound CRS without a geodetic datum".into(),
            })?,
        };
        let method = crate::operation::build(&bound.operation, &via, hub).map_err(|e| {
            Error::Unsupported {
                crs: base.label(),
                what: format!("its bound transformation: {e:?}"),
            }
        })?;
        Ok(Leg {
            base: base.clone(),
            via,
            hub: hub.clone(),
            method,
            name,
        })
    }

    fn geocentric(&self) -> bool {
        matches!(self.base.kind, Kind::Geocentric { .. })
    }

    /// With `flow`, the height the transformation produces goes on (see
    /// [`BoundPlan::run`]).
    fn to_hub<R: Recorder>(
        &self,
        internal: [f64; 3],
        flow: bool,
        rec: &mut R,
    ) -> Result<[f64; 3], CoordinateError> {
        let via = if self.geocentric() {
            internal
        } else {
            to_geographic(&self.base, internal)?
        };
        if R::TRACE {
            rec.step(Event::Convert, &self.via, via, None);
        }
        let hub = if flow {
            self.method.forward_3d(via)?
        } else {
            self.method.forward(via)?
        };
        if R::TRACE {
            let event = Event::Bound {
                name: &self.name,
                inverse: false,
            };
            rec.step(event, &self.hub, hub, None);
        }
        Ok(hub)
    }

    fn back_from_hub<R: Recorder>(
        &self,
        hub: [f64; 3],
        flow: bool,
        rec: &mut R,
    ) -> Result<[f64; 3], CoordinateError> {
        let via = if flow {
            self.method.inverse_3d(hub)?
        } else {
            self.method.inverse(hub)?
        };
        if R::TRACE {
            let event = Event::Bound {
                name: &self.name,
                inverse: true,
            };
            rec.step(event, &self.via, via, None);
        }
        let out = if self.geocentric() {
            via
        } else {
            from_geographic(&self.base, via)?
        };
        if R::TRACE {
            rec.step(Event::Convert, &self.base, out, None);
        }
        Ok(out)
    }
}

impl BoundPlan {
    /// Between two Bound CRSs on one hub, two geog2D-domain
    /// transformations meet with nothing between them; PROJ's pipeline
    /// then drops the `pop`/`push` of the height between them, and the
    /// height the first produces flows into the second (as between EPSG's
    /// operations in the selection).
    ///
    /// `epoch` is for the operations after the hub; the bound
    /// transformations have no rates.
    fn run<R: Recorder>(
        &self,
        internal: [f64; 3],
        epoch: Option<f64>,
        rec: &mut R,
    ) -> Result<[f64; 3], CoordinateError> {
        let flow = matches!(self.middle.plan, Plan::Conversion)
            && self
                .first
                .as_ref()
                .zip(self.last.as_ref())
                .is_some_and(|(a, b)| a.method.keeps_height() && b.method.keeps_height());
        let hub = match &self.first {
            Some(leg) => leg.to_hub(internal, flow, rec)?,
            None => internal,
        };
        let other = self.middle.run_recorded(hub, epoch, rec)?;
        match &self.last {
            Some(leg) => leg.back_from_hub(other, flow, rec),
            None => Ok(other),
        }
    }
}

impl Transform {
    /// Set up the conversion from `src` to `dst`. On one datum this is a
    /// conversion; between datums the transformation is chosen per
    /// coordinate as PROJ chooses it, among EPSG's operations, the
    /// grid-based ones included (`docs/reprojection.md`, levels 2 and 3).
    /// A Bound CRS (WKT1's `TOWGS84`) goes through its own transformation
    /// to WGS 84 instead, as in PROJ (`docs/crs-wkt.md`).
    /// Grids must be loaded before converting coordinates that need them:
    /// see [`Transform::grid_requests`].
    pub fn new(src: &Crs, dst: &Crs, order: AxisOrder) -> Result<Self, Error> {
        Self::with_grids(src, dst, order, Grids::default())
    }

    /// [`Transform::new`] with a choice of which grids the selection may
    /// use: [`Grids::None`] keeps to operations without grids.
    pub fn with_grids(src: &Crs, dst: &Crs, order: AxisOrder, grids: Grids) -> Result<Self, Error> {
        Self::with_selection(src, dst, order, grids, Selection::default())
    }

    /// [`Transform::with_grids`] with the search for operations limited as
    /// `selection` says: to an area of interest, which also ranks them, and
    /// to a minimum accuracy (pyproj's `area_of_interest` and `accuracy`).
    pub fn with_selection(
        src: &Crs,
        dst: &Crs,
        order: AxisOrder,
        grids: Grids,
        selection: Selection,
    ) -> Result<Self, Error> {
        if src.bound.is_some() || dst.bound.is_some() {
            return Self::through_hubs(src, dst, order, grids, selection);
        }
        for crs in [src, dst] {
            if crs.horizontal().bound.is_some() {
                return Err(Error::Unsupported {
                    crs: crs.label(),
                    what: "a compound CRS whose horizontal part is a bound CRS".into(),
                });
            }
        }
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
                return Err(unknown_datum(src, dst).unwrap_or(Error::NoTransformation {
                    src: src.label(),
                    dst: dst.label(),
                }));
            };
            Plan::Operations(Arc::new(
                Candidates::new(src, dst, sc, dc, &grids, &selection).map_err(|e| match e {
                    Error::NoTransformation { .. } => unknown_datum(src, dst).unwrap_or(e),
                    e => e,
                })?,
            ))
        };
        Ok(Transform {
            src: src.clone(),
            dst: dst.clone(),
            order,
            grids,
            selection,
            epochs: Epochs::default(),
            plan,
            unmoved: None,
        })
    }

    /// From or to a Bound CRS, as PROJ's `createOperationsBoundToGeog` and
    /// `createOperationsBoundToBound` route it. A bound transformation
    /// isn't used between CRSs on one datum (unless both are bound, with
    /// different transformations); otherwise coordinates go to the hub
    /// with it, and from there on as between any CRSs: a conversion on the
    /// hub's datum, else EPSG's operations from it, chosen per coordinate
    /// on the hub's coordinates.
    fn through_hubs(
        src: &Crs,
        dst: &Crs,
        order: AxisOrder,
        grids: Grids,
        selection: Selection,
    ) -> Result<Self, Error> {
        let (src_base, dst_base) = (src.without_bound(), dst.without_bound());
        let ignore = same_datums(&src_base, &dst_base)
            && match (&src.bound, &dst.bound) {
                (Some(a), Some(b)) => a.same(b),
                _ => true,
            };
        let plan = if ignore {
            Plan::Conversion
        } else if let Some(plan) = nad27_to_nad83(src, dst)? {
            plan
        } else {
            let first = src
                .bound
                .as_deref()
                .map(|b| Leg::new(&src_base, b))
                .transpose()?;
            let last = dst
                .bound
                .as_deref()
                .map(|b| Leg::new(&dst_base, b))
                .transpose()?;
            let from = src.bound.as_ref().map_or(src_base, |b| b.hub.clone());
            let to = dst.bound.as_ref().map_or(dst_base, |b| b.hub.clone());
            let middle =
                Transform::with_selection(&from, &to, order, grids.clone(), selection.clone())?;
            let join = |middle: Option<&str>| {
                let first = first.as_ref().map(|l| l.name.clone());
                let last = last.as_ref().map(|l| format!("Inverse of {}", l.name));
                [first, middle.map(str::to_owned), last]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" + ")
            };
            let mut names: Vec<String> = middle
                .operations()
                .into_iter()
                .map(|m| join(Some(m)))
                .collect();
            if names.is_empty() {
                names.push(join(None));
            }
            Plan::Bound(Arc::new(BoundPlan {
                first,
                middle,
                last,
                names,
            }))
        };
        Ok(Transform {
            src: src.clone(),
            dst: dst.clone(),
            order,
            grids,
            selection,
            epochs: Epochs::default(),
            plan,
            unmoved: None,
        })
    }

    /// The transformation through one EPSG coordinate operation, instead of
    /// the automatic choice: the operation must join the two CRSs' datums
    /// (in either direction) and is used for every coordinate.
    pub fn via(src: &Crs, dst: &Crs, operation: u32, order: AxisOrder) -> Result<Self, Error> {
        for crs in [src, dst] {
            if crs.horizontal().bound.is_some() {
                return Err(Error::Unsupported {
                    crs: crs.label(),
                    what: "a forced operation from or to a bound CRS".into(),
                });
            }
        }
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
            selection: Selection::default(),
            epochs: Epochs::default(),
            plan: Plan::Via(
                operation,
                Arc::new(Candidates::with_operation(src, dst, sc, dc, operation)?),
            ),
            unmoved: None,
        })
    }

    /// The candidates between datums, if any.
    pub(crate) fn candidates(&self) -> Option<&Candidates> {
        match &self.plan {
            Plan::Conversion => None,
            Plan::Operations(c) | Plan::Via(_, c) | Plan::Only(c) => Some(c),
            // Those after the hub: the bound transformation is fixed.
            Plan::Bound(b) => b.middle.candidates(),
        }
    }

    /// Names of the candidate operations, best first; empty for a
    /// conversion on one datum.
    pub fn operations(&self) -> Vec<&str> {
        match &self.plan {
            Plan::Conversion => Vec::new(),
            Plan::Operations(c) | Plan::Via(_, c) | Plan::Only(c) => c.names(),
            Plan::Bound(b) => b.names.iter().map(String::as_str).collect(),
        }
    }

    /// The name of the operation one coordinate goes through, or `None` for
    /// a conversion on one datum or a coordinate that can't be converted.
    pub fn operation_for(&self, x: f64, y: f64, z: Option<f64>) -> Option<&str> {
        let internal = to_internal(&self.src.axes, self.order, [x, y, z.unwrap_or(0.0)]).ok()?;
        match &self.plan {
            Plan::Conversion => None,
            Plan::Operations(c) | Plan::Via(_, c) | Plan::Only(c) => {
                c.used(&self.src, &self.dst, internal, self.epochs.fixed())
            }
            Plan::Bound(b) => {
                let index = if b.middle.candidates().is_some() {
                    self.index_for(internal, self.epochs.fixed())?
                } else {
                    0
                };
                b.names.get(index).map(String::as_str)
            }
        }
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
        let internal = to_internal(&self.src.axes, self.order, [x, y, z.unwrap_or(0.0)]).ok()?;
        self.index_for(internal, self.epochs.fixed())
    }

    /// How the operation for one coordinate is chosen: for every
    /// operation of [`Transform::operation_info`], whether its area of use
    /// contains the point, tested as the choice tests it, and what became
    /// of it (chosen, passed over for another and why, outside, failed, the
    /// fallback). It runs the same choice as [`Transform::convert`], which
    /// pays nothing for it. `None` for a conversion on one datum, and for
    /// a coordinate that can't be placed (not finite, out of range). A
    /// Bound CRS's transform answers for its hub, as
    /// [`Transform::operation_index_for`] does.
    pub fn explain_point(&self, x: f64, y: f64, z: Option<f64>) -> Option<Explanation> {
        let internal = to_internal(&self.src.axes, self.order, [x, y, z.unwrap_or(0.0)]).ok()?;
        self.explain_internal(internal, self.epochs.fixed())
    }

    /// Each step one coordinate goes through, with the CRS and the
    /// coordinates after it, from the coordinate as given to the result
    /// (`docs/web-reproject.md`, piece 9.2): the projections, the
    /// operations between datums, the horizontal position kept and put
    /// back around an operation on heights, a Bound CRS's transformation
    /// to its hub. The steps of the operation [`Transform::convert`] uses,
    /// run by the same code, so the last step's coordinates are its
    /// result; on an error, the steps up to where the last operation tried
    /// failed. A grid's shift is the difference across its step.
    /// [`Transform::operation_info`]'s `chain` describes each operation's
    /// steps (methods, parameters). `None` for a coordinate that can't be
    /// placed (not finite).
    pub fn trace_point(&self, x: f64, y: f64, z: Option<f64>) -> Option<Trace> {
        let given = [x, y, z.unwrap_or(0.0)];
        let internal = to_internal(&self.src.axes, self.order, given).ok()?;
        let mut tracer = Tracer::new(&self.src, self.order, given, internal);
        let epoch = self.epochs.fixed();
        let result = self.run_recorded(internal, epoch, &mut tracer);
        let used = match self.plan {
            Plan::Conversion => None,
            _ => self.index_for(internal, epoch),
        };
        Some(tracer.finish(result, used))
    }

    fn explain_internal(&self, internal: [f64; 3], epoch: Option<f64>) -> Option<Explanation> {
        match &self.plan {
            Plan::Conversion => None,
            Plan::Operations(c) | Plan::Via(_, c) | Plan::Only(c) => {
                Some(c.explain(&self.src, &self.dst, internal, epoch))
            }
            Plan::Bound(b) => {
                let hub = match &b.first {
                    Some(leg) => leg.to_hub(internal, false, &mut ()).ok()?,
                    None => internal,
                };
                b.middle.explain_internal(hub, epoch)
            }
        }
    }

    fn index_for(&self, internal: [f64; 3], epoch: Option<f64>) -> Option<usize> {
        match &self.plan {
            Plan::Bound(b) => {
                let hub = match &b.first {
                    Some(leg) => leg.to_hub(internal, false, &mut ()).ok()?,
                    None => internal,
                };
                b.middle.index_for(hub, epoch)
            }
            _ => self
                .candidates()?
                .used_index(&self.src, &self.dst, internal, epoch),
        }
    }

    /// The transformation through operation `index` of
    /// [`Transform::operation_info`] only, for every coordinate whatever its
    /// area, as each member of PROJ's `TransformerGroup` is. `None` when
    /// there is no such operation. Its inverse runs the same operation the
    /// other way.
    pub fn with_operation_index(&self, index: usize) -> Option<Result<Transform, Error>> {
        if let Plan::Bound(b) = &self.plan {
            let middle = match b.middle.with_operation_index(index)? {
                Ok(m) => m,
                Err(e) => return Some(Err(e)),
            };
            let names = vec![b.names.get(index)?.clone()];
            return Some(Ok(Transform {
                src: self.src.clone(),
                dst: self.dst.clone(),
                order: self.order,
                grids: self.grids.clone(),
                selection: self.selection.clone(),
                epochs: self.epochs,
                plan: Plan::Bound(Arc::new(BoundPlan {
                    first: b.first.clone(),
                    middle,
                    last: b.last.clone(),
                    names,
                })),
                unmoved: None,
            }));
        }
        // The operation itself; the change of epoch is added to it again.
        let candidates = match self.unmoved.as_ref().unwrap_or(&self.plan) {
            Plan::Operations(c) | Plan::Via(_, c) | Plan::Only(c) => c,
            Plan::Conversion | Plan::Bound(_) => return None,
        };
        let only = candidates.only(index, &self.src, &self.dst)?;
        Some(only.and_then(|c| {
            Transform {
                src: self.src.clone(),
                dst: self.dst.clone(),
                order: self.order,
                grids: self.grids.clone(),
                selection: self.selection.clone(),
                epochs: Epochs::default(),
                plan: Plan::Only(Arc::new(c)),
                unmoved: None,
            }
            .at_epochs(self.epochs)
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
    ///
    /// The epochs of [`Transform::with_epochs`] go with their CRSs: the
    /// source's becomes the target's.
    pub fn inverse(&self) -> Result<Transform, Error> {
        let inverse = match self.unmoved.as_ref().unwrap_or(&self.plan) {
            Plan::Conversion => Transform {
                src: self.dst.clone(),
                dst: self.src.clone(),
                order: self.order,
                grids: self.grids.clone(),
                selection: self.selection.clone(),
                epochs: Epochs::default(),
                plan: Plan::Conversion,
                unmoved: None,
            },
            Plan::Operations(_) | Plan::Bound(_) => Transform::with_selection(
                &self.dst,
                &self.src,
                self.order,
                self.grids.clone(),
                self.selection.clone(),
            )?,
            Plan::Via(code, _) => Transform::via(&self.dst, &self.src, *code, self.order)?,
            Plan::Only(c) => Transform {
                src: self.dst.clone(),
                dst: self.src.clone(),
                order: self.order,
                grids: self.grids.clone(),
                selection: self.selection.clone(),
                epochs: Epochs::default(),
                plan: Plan::Only(Arc::new(c.inverse_of_first(&self.dst, &self.src)?)),
                unmoved: None,
            },
        };
        // A change of epoch goes the other way too: its point motion
        // operation with the epochs exchanged.
        inverse.at_epochs(self.epochs.swapped())
    }

    /// The transform with coordinate epochs for its CRSs, as decimal years
    /// (2024.5): what PROJ takes as a CRS with an epoch
    /// (`ITRF2014@2024.5`, a WKT `COORDINATEMETADATA`). `source` says
    /// when the coordinates given are valid, `target` when those wanted
    /// are; `None` for a CRS without one. Every coordinate is then
    /// converted at that epoch: a time-dependent operation (see
    /// [`OperationInfo::time_dependent`]) applies its rates from its
    /// reference epoch to it, where a transform without epochs stays at
    /// the reference epoch. Which operations there are doesn't depend on
    /// the epoch, and which one a coordinate takes only where a
    /// time-specific Helmert holds at it
    /// ([`OperationInfo::only_at_epoch`]).
    ///
    /// As in PROJ:
    ///
    /// * only a CRS whose coordinates change with time takes an epoch
    ///   ([`Crs::is_dynamic`]), or one EPSG has a point motion operation
    ///   for (NAD83(CSRS)); for any other it is an error;
    /// * with one epoch, both CRSs are at it: ITRF2014 at 2030 to GDA2020
    ///   and GDA2020 to ITRF2014 at 2030 both run at 2030;
    /// * the transform's epoch replaces a coordinate's own
    ///   ([`Transform::convert_at`]), which [`Transform::overrides_epoch`]
    ///   tells.
    ///
    /// Two different epochs ask for the coordinates to be moved in time,
    /// as the ground moved between the two: that takes a point motion
    /// operation, a velocity grid for one of the two CRSs. EPSG has them
    /// for few CRSs (the realizations of NAD83(CSRS), ETRF2014 in the
    /// Nordic and Baltic countries), and they read a grid
    /// ([`Transform::grid_requests`]). The source CRS's is used if it has
    /// one: the coordinates are moved to the target's epoch first, and the
    /// operations to the target CRS run at that epoch. Else the target
    /// CRS's: the operations run at the source's epoch, and the coordinates
    /// are moved last. Without one for either CRS it is an error, where
    /// PROJ leaves the coordinates at the source's epoch.
    ///
    /// ```
    /// use xeibe_reproject::{AxisOrder, Transform};
    ///
    /// // ITRF2014 at 2030.0 to GDA2020 (a plate-fixed, static CRS).
    /// let t = Transform::from_epsg(7912, 7843, AxisOrder::EastFirst)
    ///     .unwrap()
    ///     .with_epochs(Some(2030.0), None)
    ///     .unwrap();
    /// let (lon, lat, _) = t.convert(135.0, -25.0, Some(0.0)).unwrap();
    /// // PROJ 9.8.1 gives 134.999996938, -25.000005301.
    /// assert!((lon - 134.999_996_938).abs() < 1e-9 && (lat + 25.000_005_301).abs() < 1e-9);
    /// ```
    pub fn with_epochs(&self, source: Option<f64>, target: Option<f64>) -> Result<Self, Error> {
        let checked = |crs: &Crs, epoch: Option<f64>| -> Result<Option<f64>, Error> {
            let Some(epoch) = epoch else {
                return Ok(None);
            };
            let fail = |message: String| Error::Epoch {
                crs: crs.label(),
                message,
            };
            if !epoch.is_finite() || epoch <= 0.0 {
                return Err(fail(format!(
                    "the coordinate epoch {epoch} is not a decimal year"
                )));
            }
            // One xeibe can't run counts too: the error is then about it.
            let moves = || {
                epsg_datum(crs).is_some_and(|datum| {
                    crate::operation::point_motion_for(datum).is_some()
                        || crate::operation::unsupported_point_motion_for(datum).is_some()
                })
            };
            if !crs.is_dynamic() && !moves() {
                return Err(fail(
                    "a coordinate epoch should not be given for a static CRS".into(),
                ));
            }
            Ok(Some(rounded_epoch(epoch)))
        };
        let epochs = Epochs {
            source: checked(&self.src, source)?,
            target: checked(&self.dst, target)?,
        };
        // From the plan without a change of epoch, if it has one now.
        Transform {
            epochs: Epochs::default(),
            plan: self.unmoved.clone().unwrap_or_else(|| self.plan.clone()),
            unmoved: None,
            ..self.clone()
        }
        .at_epochs(epochs)
    }

    /// The transform, set up without epochs, at `epochs` (checked). Two
    /// that differ add a point motion operation to the plan.
    fn at_epochs(self, epochs: Epochs) -> Result<Transform, Error> {
        let Some((from, to)) = epochs.change() else {
            return Ok(Transform { epochs, ..self });
        };
        let fail = |message: String| Error::Epoch {
            crs: self.src.label(),
            message,
        };
        let moving = format!(
            "moving coordinates from epoch {from} to {to} (in {})",
            self.dst.label()
        );
        let Some(motion) = Motion::between(epsg_datum(&self.src), epsg_datum(&self.dst), from, to)
        else {
            let unsupported = [epsg_datum(&self.src), epsg_datum(&self.dst)]
                .into_iter()
                .flatten()
                .find_map(crate::operation::unsupported_point_motion_for);
            return Err(fail(match unsupported {
                Some(op) => format!(
                    "{moving} takes {} (EPSG:{}), whose method xeibe does not implement",
                    op.name, op.code
                ),
                None => format!(
                    "{moving} takes a point motion operation, and there is none for either CRS"
                ),
            }));
        };
        let op = &crate::registry::registry().ops[&motion.operation];
        if let Some(grid) = op
            .grid_names()
            .into_iter()
            .find(|name| !self.grids.has(name, &op.name))
        {
            return Err(fail(format!(
                "{moving} takes {} (EPSG:{}), whose grid {grid} is not available",
                op.name, op.code
            )));
        }
        // A CRS the selection has no code for stands at the operation's.
        let frame = CrsRef::Epsg(op.src.unwrap_or(0));
        let sc = crs_ref(&self.src).unwrap_or(frame);
        let dc = crs_ref(&self.dst).unwrap_or(frame);
        let moved = |c: &Candidates| c.moved(&self.src, &self.dst, sc, dc, &motion).map(Arc::new);
        let plan = match &self.plan {
            Plan::Conversion => Plan::Operations(Arc::new(Candidates::motion_only(
                &self.src, &self.dst, sc, dc, &motion,
            )?)),
            Plan::Operations(c) => Plan::Operations(moved(c)?),
            Plan::Via(code, c) => Plan::Via(*code, moved(c)?),
            Plan::Only(c) => Plan::Only(moved(c)?),
            Plan::Bound(_) => {
                return Err(fail(format!(
                    "{moving} from or to a bound CRS is not supported"
                )));
            }
        };
        Ok(Transform {
            epochs,
            unmoved: Some(self.plan.clone()),
            plan,
            ..self
        })
    }

    /// The source CRS's coordinate epoch ([`Transform::with_epochs`]).
    pub fn source_epoch(&self) -> Option<f64> {
        self.epochs.source
    }

    /// The target CRS's coordinate epoch ([`Transform::with_epochs`]).
    pub fn target_epoch(&self) -> Option<f64> {
        self.epochs.target
    }

    /// The epoch every coordinate starts at, when the transform has one:
    /// the source's, else the target's ([`Transform::with_epochs`]). With
    /// two that differ the coordinates end at the target's.
    pub fn epoch(&self) -> Option<f64> {
        self.epochs.fixed()
    }

    /// Whether the transform's own epoch replaces this one of a
    /// coordinate in [`Transform::convert_at`]: it has one, and `epoch` is
    /// another. PROJ's rule, which it applies silently; a caller that takes
    /// both kinds of epoch can warn with this.
    pub fn overrides_epoch(&self, epoch: f64) -> bool {
        epoch.is_finite() && self.epochs.fixed().is_some_and(|own| own != epoch)
    }

    /// Convert one coordinate. `z` is the third value when the data has one:
    /// the height for a 3D CRS, carried through unchanged for a 2D one.
    ///
    /// A coordinate with a NaN in x or y (GeoArrow's empty point) comes back
    /// unchanged.
    ///
    /// A time-dependent operation runs at the transform's epoch
    /// ([`Transform::with_epochs`]), else at its reference epoch.
    pub fn convert(
        &self,
        x: f64,
        y: f64,
        z: Option<f64>,
    ) -> Result<(f64, f64, Option<f64>), CoordinateError> {
        self.convert_with(x, y, z, self.epochs.fixed())
    }

    /// [`Transform::convert`] for a coordinate with an epoch of its own, a
    /// decimal year (2024.5): PROJ's fourth value, `t`, and pyproj's `tt`.
    /// A time-dependent operation applies its rates from its reference
    /// epoch to that one. An epoch that isn't finite is none (PROJ's
    /// `HUGE_VAL`): the coordinate is converted as [`Transform::convert`]
    /// converts it.
    ///
    /// The transform's own epoch, when it has one, is used instead, as in
    /// PROJ: see [`Transform::overrides_epoch`].
    pub fn convert_at(
        &self,
        x: f64,
        y: f64,
        z: Option<f64>,
        epoch: f64,
    ) -> Result<(f64, f64, Option<f64>), CoordinateError> {
        self.convert_with(x, y, z, self.epoch_of(epoch))
    }

    /// The epoch a coordinate with `epoch` of its own is converted at.
    fn epoch_of(&self, epoch: f64) -> Option<f64> {
        self.epochs
            .fixed()
            .or_else(|| epoch.is_finite().then_some(epoch))
    }

    fn convert_with(
        &self,
        x: f64,
        y: f64,
        z: Option<f64>,
        epoch: Option<f64>,
    ) -> Result<(f64, f64, Option<f64>), CoordinateError> {
        if x.is_nan() || y.is_nan() {
            return Ok((x, y, z));
        }
        let internal = to_internal(&self.src.axes, self.order, [x, y, z.unwrap_or(0.0)])?;
        let out = self.run(internal, epoch)?;
        let [x, y, h] = from_internal(&self.dst.axes, self.order, out);
        // A 2D target has no height of its own: hand back what came in.
        let z = z.map(|z| if self.dst.axes.dimension() == 3 { h } else { z });
        Ok((x, y, z))
    }

    /// Internal coordinates of the source CRS to the target's:
    /// [`Transform::run_recorded`] with `()`, written out, and kept out of
    /// line as it was when it called itself for a Bound CRS's middle
    /// transform. Inlined into `convert`, or as `run_recorded::<()>`, a
    /// conversion on one datum measured 2 to 4% slower (2026-11-08,
    /// `cargo bench -- convert_xy`, interleaved with the code before).
    #[inline(never)]
    fn run(&self, internal: [f64; 3], epoch: Option<f64>) -> Result<[f64; 3], CoordinateError> {
        match &self.plan {
            Plan::Conversion => on_one_datum(&self.src, &self.dst, internal),
            Plan::Operations(candidates) | Plan::Via(_, candidates) | Plan::Only(candidates) => {
                candidates.run(&self.src, &self.dst, internal, epoch)
            }
            Plan::Bound(b) => b.run(internal, epoch, &mut ()),
        }
    }

    /// [`Transform::run`], telling a tracing `rec` the steps.
    fn run_recorded<R: Recorder>(
        &self,
        internal: [f64; 3],
        epoch: Option<f64>,
        rec: &mut R,
    ) -> Result<[f64; 3], CoordinateError> {
        match &self.plan {
            Plan::Conversion => {
                let c = on_one_datum(&self.src, &self.dst, internal)?;
                if R::TRACE {
                    rec.step(Event::Convert, &self.dst, c, None);
                }
                Ok(c)
            }
            Plan::Operations(candidates) | Plan::Via(_, candidates) | Plan::Only(candidates) => {
                if R::TRACE {
                    return candidates
                        .choose(&self.src, &self.dst, internal, epoch, rec)
                        .map(|(out, _)| out);
                }
                candidates.run(&self.src, &self.dst, internal, epoch)
            }
            Plan::Bound(b) => b.run(internal, epoch, rec),
        }
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

    /// [`Transform::convert_xy`] for coordinates with epochs of their own,
    /// decimal years in `t`: each as [`Transform::convert_at`] converts it.
    pub fn convert_xyt(&self, x: &mut [f64], y: &mut [f64], t: &[f64]) -> Result<(), Error> {
        assert!(
            x.len() == y.len() && y.len() == t.len(),
            "x, y and t buffers differ in length"
        );
        for (index, ((x, y), t)) in x.iter_mut().zip(y.iter_mut()).zip(t).enumerate() {
            let (cx, cy, _) = self
                .convert_at(*x, *y, None, *t)
                .map_err(|reason| coordinate_error(index, *x, *y, reason))?;
            (*x, *y) = (cx, cy);
        }
        Ok(())
    }

    /// [`Transform::convert_xyz`] for coordinates with epochs of their own,
    /// decimal years in `t`: each as [`Transform::convert_at`] converts it.
    pub fn convert_xyzt(
        &self,
        x: &mut [f64],
        y: &mut [f64],
        z: &mut [f64],
        t: &[f64],
    ) -> Result<(), Error> {
        assert!(
            x.len() == y.len() && y.len() == z.len() && z.len() == t.len(),
            "x, y, z and t buffers differ in length"
        );
        for (index, (((x, y), z), t)) in x
            .iter_mut()
            .zip(y.iter_mut())
            .zip(z.iter_mut())
            .zip(t)
            .enumerate()
        {
            let (cx, cy, cz) = self
                .convert_at(*x, *y, Some(*z), *t)
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

pub(crate) fn coordinate_error(index: usize, x: f64, y: f64, reason: CoordinateError) -> Error {
    Error::Coordinate {
        index,
        x,
        y,
        reason,
    }
}

/// The EPSG code of a CRS's geodetic datum.
fn epsg_datum(crs: &Crs) -> Option<u32> {
    crs.datum()?.key.strip_prefix("EPSG:")?.parse().ok()
}

/// The CRS as the selection knows it: its EPSG code, or a compound CRS's
/// two codes. A CRS without one (WKT without ids) has the code of the EPSG
/// CRS it is equivalent to ([`Crs::identify`]).
fn crs_ref(crs: &Crs) -> Option<CrsRef> {
    if let Some(code) = crs.epsg_code() {
        return Some(CrsRef::Epsg(code));
    }
    match &crs.kind {
        Kind::Compound {
            horizontal,
            vertical,
        } => Some(CrsRef::Compound(
            horizontal.epsg_code()?,
            vertical.epsg_code()?,
        )),
        // A CRS EPSG doesn't have, on a datum it does (a projection of
        // one's own on WGS 84, from WKT or a PROJ string): from the EPSG
        // geographic CRS on the datum, as PROJ starts from a derived CRS's
        // base. The conversion onto it is the first step.
        Kind::Geographic { datum } | Kind::Geocentric { datum } | Kind::Projected { datum, .. } => {
            geographic_on(datum, crs.dimension() == 3).map(CrsRef::Epsg)
        }
        _ => None,
    }
}

/// The EPSG geographic CRS (3D or 2D) on an EPSG datum: the lowest code
/// not deprecated, else the lowest deprecated one.
fn geographic_on(datum: &Datum, three_d: bool) -> Option<u32> {
    let code: u32 = datum.key.strip_prefix("EPSG:")?.parse().ok()?;
    let kind = if three_d {
        xeibe_crs::CrsKind::Geographic3d
    } else {
        xeibe_crs::CrsKind::Geographic2d
    };
    xeibe_crs::CRS
        .iter()
        .filter(|r| r.kind == kind && r.datum == Some(code))
        .min_by_key(|r| (r.deprecated, r.code))
        .map(|r| r.code)
}

/// The datum of `src` or `dst` that nothing locates (decision 8 of
/// `docs/pyproj-phase3.md`): one without an EPSG id that no EPSG datum is
/// identified with, or one of EPSG's "Not specified (based on … ellipsoid)"
/// datums. Converting it to another datum would take a shift of zero,
/// which isn't assumed.
fn unknown_datum(src: &Crs, dst: &Crs) -> Option<Error> {
    [src, dst].into_iter().find_map(|crs| {
        let datum = crs.horizontal().datum()?;
        let unknown = datum.name.starts_with("Not specified (based on")
            || (!datum.key.starts_with("EPSG:") && crs_ref(crs).is_none());
        unknown.then(|| Error::UnknownDatum {
            crs: crs.label(),
            datum: datum.name.clone(),
        })
    })
}

/// PROJ's special case (`createOperationsBoundToGeog`) for a Bound CRS on
/// the Clarke 1866 ellipsoid (NAD27 and its kin) to a CRS on NAD83: WGS 84
/// and NAD83 count as one there, so the bound transformation lands on NAD83
/// directly, named "Transformation from … to NAD83".
fn nad27_to_nad83(src: &Crs, dst: &Crs) -> Result<Option<Plan>, Error> {
    let (bound, base, other, forward) = match (&src.bound, &dst.bound) {
        (Some(b), None) => (b, src.without_bound(), dst, true),
        (None, Some(b)) => (b, dst.without_bound(), src, false),
        _ => return Ok(None),
    };
    let clarke_1866 = base.datum().is_some_and(|d| {
        d.ellipsoid.a == 6_378_206.4 && (d.ellipsoid.b - 6_356_583.8).abs() < 1e-4
    });
    let hub_wgs84 = bound.hub.datum().is_some_and(|d| d.key == "EPSG:6326");
    let nad83 = other.datum().is_some_and(|d| d.key == "EPSG:6269");
    if !(clarke_1866 && hub_wgs84 && nad83) {
        return Ok(None);
    }
    let Some(landing) = other.geographic_of() else {
        return Ok(None);
    };
    // The other CRS's geographic CRS names the target, as in PROJ.
    let target_name = match &other.kind {
        Kind::Geographic { .. } => other.name.clone(),
        _ => other
            .code()
            .and_then(xeibe_crs::projjson)
            .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
            .and_then(|v| v["base_crs"]["name"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "NAD83".to_owned()),
    };
    // "Transformation from X to WGS84" names the base's geographic CRS.
    let base_name = bound
        .operation
        .name
        .strip_prefix("Transformation from ")
        .and_then(|n| n.strip_suffix(" to WGS84"))
        .map_or_else(|| base.name.clone(), str::to_owned);
    let name = format!("Transformation from {base_name} to {target_name}");
    let leg = Leg::to(&base, bound, &landing, name.clone())?;
    let (from, to) = if forward {
        (landing.clone(), other.clone())
    } else {
        (other.clone(), landing.clone())
    };
    let middle = Transform {
        src: from,
        dst: to,
        order: AxisOrder::EastFirst,
        grids: Grids::None,
        selection: Selection::default(),
        epochs: Epochs::default(),
        plan: Plan::Conversion,
        unmoved: None,
    };
    let (first, last) = if forward {
        (Some(leg), None)
    } else {
        (None, Some(leg))
    };
    let names = vec![if forward {
        name
    } else {
        format!("Inverse of {name}")
    }];
    Ok(Some(Plan::Bound(Arc::new(BoundPlan {
        first,
        middle,
        last,
        names,
    }))))
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
    // The ellipsoid within PROJ's 1e-10, as WKT may give it by inverse
    // flattening where EPSG has the semi-minor axis.
    let close = |x: f64, y: f64| x == y || (x - y).abs() <= 1e-10 * x.abs().max(y.abs());
    let same_ellipsoid = close(a.ellipsoid.a, b.ellipsoid.a) && close(a.ellipsoid.b, b.ellipsoid.b);
    if !(same && same_ellipsoid && close(a.prime_meridian, b.prime_meridian)) {
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
pub(crate) fn from_internal(axes: &Axes, order: AxisOrder, v: [f64; 3]) -> [f64; 3] {
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
