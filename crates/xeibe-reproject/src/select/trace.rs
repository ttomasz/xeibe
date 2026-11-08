//! The steps one coordinate goes through, with the coordinates after each
//! ([`crate::Transform::trace_point`]): the recorder the step loop tells
//! what it does, run by the same code as [`crate::Transform::convert`]
//! (`docs/web-reproject.md`, piece 9.2).

use crate::crs::{Crs, Kind};
use crate::error::CoordinateError;
use crate::registry::{PROJ_CODES, registry};
use crate::transform::{AxisOrder, from_internal};

use super::StepKind;
use super::explain::Recorder;

/// The steps one coordinate went through ([`crate::Transform::trace_point`]).
#[derive(Debug, Clone)]
pub struct Trace {
    /// The operation it went through, as an index into
    /// [`crate::Transform::operation_info`]; `None` for a conversion on one
    /// datum, and when it couldn't be converted.
    pub used: Option<usize>,
    /// From the coordinate as given ([`TraceStep::Start`]) to the result:
    /// the last step's coordinates are [`crate::Transform::convert`]'s,
    /// in the target CRS. Conversions that change nothing (between two
    /// geographic CRSs on one datum, say) are left out, as the engine
    /// leaves them out; the last step is always in the target CRS.
    pub steps: Vec<Traced>,
    /// Why the coordinate couldn't be converted. The steps then end where
    /// the last operation tried failed.
    pub error: Option<CoordinateError>,
}

/// One step of a [`Trace`].
#[derive(Debug, Clone)]
pub struct Traced {
    pub step: TraceStep,
    /// The CRS of the coordinates after the step. After an operation into
    /// a vertical CRS, the horizontal position's: the height is then in
    /// the vertical CRS ([`TracedHeight::GravityRelated`]).
    pub crs: Crs,
    /// The coordinates after the step, in `crs`'s units and axes, in the
    /// transform's axis order (easting or longitude first with
    /// [`AxisOrder::EastFirst`]), as [`crate::Transform::convert`] gives
    /// them in the target CRS. With a 2D `crs`, the third value is the
    /// height (or the value given) in metres.
    pub coords: [f64; 3],
    pub height: TracedHeight,
}

/// What a traced coordinate's third value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TracedHeight {
    /// No height: from a 2D source the third value is the one given (0
    /// without), carried through.
    None,
    /// An ellipsoidal height, on the ellipsoid of the CRS's datum; Z for
    /// a geocentric CRS.
    Ellipsoidal,
    /// A gravity-related height.
    GravityRelated,
}

/// What a step of a [`Trace`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceStep {
    /// The coordinate as given, in the source CRS.
    Start,
    /// Into another CRS on the same datum: a map projection or its
    /// inverse, between geographic and geocentric coordinates, other axes
    /// or units.
    Convert,
    /// An operation between datums ([`StepKind::Epsg`] or
    /// [`StepKind::EnsembleIdentity`]), with PROJ's name for it.
    /// `height_kept`: the height comes back as it went in, as around a 2D
    /// transformation between 3D CRSs (PROJ's `push`/`pop +v_3`).
    Operation {
        kind: StepKind,
        name: String,
        height_kept: bool,
    },
    /// The horizontal position kept, before an operation on heights
    /// (PROJ's `push +v_1 +v_2`); the coordinates are unchanged.
    SaveHorizontal,
    /// The horizontal position put back after it (`pop +v_1 +v_2`), in the
    /// CRS it was kept in, with the height the operations since gave.
    RestoreHorizontal,
    /// A Bound CRS's own transformation (WKT1's `TOWGS84`) to its hub, or
    /// back from it.
    Bound { name: String, inverse: bool },
}

/// What the step loop tells a recorder it did ([`Recorder::step`]).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Event<'a> {
    Convert,
    /// Registry operation `code`.
    Apply {
        code: u32,
        inverse: bool,
        keeps_height: bool,
    },
    Save,
    Restore,
    Bound {
        name: &'a str,
        inverse: bool,
    },
}

/// The recorder of [`crate::Transform::trace_point`].
#[derive(Debug)]
pub(crate) struct Tracer {
    order: AxisOrder,
    /// The coordinate as given, which the start keeps as it is (back from
    /// radians it could be a bit off).
    given: [f64; 3],
    /// In the CRSs' internal coordinates, as recorded.
    steps: Vec<(TraceStep, Crs, [f64; 3], TracedHeight)>,
    /// Where the operation being tried started.
    mark: usize,
    /// The steps of the last operation that failed.
    failed: Vec<(TraceStep, Crs, [f64; 3], TracedHeight)>,
}

impl Tracer {
    pub(crate) fn new(src: &Crs, order: AxisOrder, given: [f64; 3], internal: [f64; 3]) -> Tracer {
        let height = height_of(src, TracedHeight::None);
        Tracer {
            order,
            given,
            steps: vec![(TraceStep::Start, src.clone(), internal, height)],
            mark: 0,
            failed: Vec::new(),
        }
    }

    pub(crate) fn finish(
        mut self,
        result: Result<[f64; 3], CoordinateError>,
        used: Option<usize>,
    ) -> Trace {
        let error = result.err();
        if error.is_some() {
            self.steps.append(&mut self.failed);
        }
        // A conversion that left the internal coordinates as they were
        // only renamed them; the last step stays, in the target CRS,
        // unless the step before is in it already.
        let last = self.steps.len().saturating_sub(1);
        let mut previous: Option<[f64; 3]> = None;
        let mut steps: Vec<Traced> = Vec::new();
        for (i, (step, crs, c, height)) in self.steps.into_iter().enumerate() {
            let same = previous == Some(c);
            previous = Some(c);
            let renamed = i != last
                || error.is_some()
                || steps.last().is_some_and(|s| s.crs.equivalent(&crs, false));
            if step == TraceStep::Convert && same && renamed {
                continue;
            }
            let coords = if i == 0 {
                self.given
            } else {
                from_internal(&crs.axes, self.order, c)
            };
            steps.push(Traced {
                step,
                crs,
                coords,
                height,
            });
        }
        Trace {
            used: error.is_none().then_some(used).flatten(),
            steps,
            error,
        }
    }

    fn last_height(&self) -> TracedHeight {
        self.steps
            .last()
            .map_or(TracedHeight::None, |(_, _, _, h)| *h)
    }
}

/// The height in `crs` after a step that doesn't say, following one of
/// height `previous`.
fn height_of(crs: &Crs, previous: TracedHeight) -> TracedHeight {
    match &crs.kind {
        Kind::Compound { .. } | Kind::Vertical { .. } => TracedHeight::GravityRelated,
        Kind::Geocentric { .. } => TracedHeight::Ellipsoidal,
        Kind::Geographic { .. } if crs.dimension() == 3 => TracedHeight::Ellipsoidal,
        _ => previous,
    }
}

impl Recorder for Tracer {
    const TRACE: bool = true;

    fn tried(&mut self, _candidate: usize, error: Option<CoordinateError>) {
        if error.is_some() {
            self.failed = self.steps.split_off(self.mark);
        }
    }

    fn start_run(&mut self) {
        self.mark = self.steps.len();
    }

    fn step(&mut self, event: Event<'_>, crs: &Crs, coords: [f64; 3], ellipsoidal: Option<bool>) {
        let previous = self.last_height();
        let height = match ellipsoidal {
            Some(true) => TracedHeight::Ellipsoidal,
            Some(false) if previous == TracedHeight::None => TracedHeight::None,
            Some(false) => TracedHeight::GravityRelated,
            None => height_of(crs, previous),
        };
        let step = match event {
            Event::Convert => TraceStep::Convert,
            Event::Save => TraceStep::SaveHorizontal,
            Event::Restore => TraceStep::RestoreHorizontal,
            Event::Bound { name, inverse } => TraceStep::Bound {
                name: name.to_owned(),
                inverse,
            },
            Event::Apply {
                code,
                inverse,
                keeps_height,
            } => {
                let op = &registry().ops[&code];
                let kind = if code >= PROJ_CODES {
                    StepKind::EnsembleIdentity {
                        method: op.method,
                        inverse,
                    }
                } else {
                    StepKind::Epsg { code, inverse }
                };
                let name = if inverse {
                    format!("Inverse of {}", op.name)
                } else {
                    op.name.to_string()
                };
                TraceStep::Operation {
                    kind,
                    name,
                    height_kept: keeps_height,
                }
            }
        };
        self.steps.push((step, crs.clone(), coords, height));
    }
}
