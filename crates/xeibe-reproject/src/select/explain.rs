//! Why one coordinate goes through the operation it goes through
//! ([`crate::Transform::explain_point`]): the per-point choice of
//! `run_with` and `suggest`, told as it is made.
//!
//! The choice is generic over a [`Recorder`]. Converting passes `()`, whose
//! methods are empty and whose [`Recorder::DETAILED`] is false, so the
//! compiler makes a copy of the choice with the recording gone: the
//! conversion pays nothing for it. [`Explain`] keeps what it is told, for
//! one coordinate, and the two can't disagree, as they run the same code.
//! The step loop (`run`) is generic over it too, for
//! [`super::trace::Tracer`], whose [`Recorder::TRACE`] is true.

use crate::crs::Crs;
use crate::error::CoordinateError;

use super::trace::Event;

/// What the per-point choice tells about itself, by index into the
/// prepared candidates.
pub(crate) trait Recorder {
    /// Whether the recorder keeps anything: the work only it needs (each
    /// candidate's area test, the reasons) is skipped otherwise.
    const DETAILED: bool = false;
    /// The candidate's area of use contains the point, or doesn't.
    fn inside(&mut self, _candidate: usize, _inside: bool) {}
    /// Its area contains the point, but `by` is preferred to it.
    fn passed(&mut self, _candidate: usize, _by: usize, _reason: PassReason) {}
    /// It was run: `None` when it converted the point.
    fn tried(&mut self, _candidate: usize, _error: Option<CoordinateError>) {}
    /// No candidate whose area contains the point converted it: this one,
    /// the first without grids, is used whatever its area.
    fn fallback(&mut self, _candidate: usize) {}
    /// The choice starts looking again, without the candidates tried.
    fn round(&mut self) {}
    /// Whether the recorder follows the steps ([`Recorder::step`]): the
    /// calls are skipped otherwise, with what they would be given.
    const TRACE: bool = false;
    /// A candidate's steps start running.
    fn start_run(&mut self) {}
    /// A step was run: the coordinates are now `coords`, in `crs`'s
    /// internal form. `ellipsoidal`: whether the height is ellipsoidal,
    /// where the step loop knows.
    fn step(
        &mut self,
        _event: Event<'_>,
        _crs: &Crs,
        _coords: [f64; 3],
        _ellipsoidal: Option<bool>,
    ) {
    }
}

/// Converting: nothing kept.
impl Recorder for () {}

/// Why a candidate whose area contains the point wasn't used: it was
/// passed over for another, by PROJ's rule (`pj_get_suggested_operation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassReason {
    /// The other is more accurate.
    LessAccurate,
    /// Its accuracy is unknown; the other's is known.
    UnknownAccuracy,
    /// As accurate, and the other's area is smaller: more specific.
    LargerArea,
    /// As accurate, and its area has no name; the other's has one.
    UnknownAreaName,
    /// Its area is offshore.
    Offshore,
    /// The other is one of the operations PROJ always prefers among equals
    /// (a few NAD83(HARN), GDA94 and GDA2020 ones).
    Priority,
    /// The other ranks earlier, and the rule doesn't displace it: an
    /// earlier operation of unknown accuracy, for one.
    Earlier,
}

/// What became of one candidate operation for one coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Its area contains the point and it was chosen: it converted it.
    Chosen,
    /// No candidate whose area contains the point converted it (or none
    /// contains it), so this one, the first without grids, did, whatever
    /// its area, as PROJ falls back.
    Fallback,
    /// The only operation there is (or the one forced): used for every
    /// point, whatever its area, as PROJ does.
    Only,
    /// Its area contains the point, but operation `by` (an index into
    /// [`crate::Transform::operation_info`]) was preferred.
    Passed { by: usize, reason: PassReason },
    /// Its area doesn't contain the point.
    Outside,
    /// It was chosen and run, and couldn't convert the point.
    Failed(CoordinateError),
    /// Its area contains the point, but the choice stopped before it: it
    /// tries three operations at most, then falls back.
    NotReached,
}

/// One candidate operation, for one coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Explained {
    /// Whether its area of use contains the point, tested as the choice
    /// tests it: the area converted into the source CRS, in its internal
    /// coordinates (degrees on its ellipsoid for a geocentric source).
    /// `None` for an operation used whatever its area, whose area isn't
    /// tested, or when the point couldn't be placed.
    pub inside: Option<bool>,
    pub outcome: Outcome,
}

/// How the operation for one coordinate was chosen
/// ([`crate::Transform::explain_point`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Explanation {
    /// The operation that converted the point, as an index into
    /// [`crate::Transform::operation_info`]; `None` when none could.
    pub used: Option<usize>,
    /// Why the point couldn't be converted.
    pub error: Option<CoordinateError>,
    /// Every operation of [`crate::Transform::operation_info`], in order.
    pub candidates: Vec<Explained>,
}

/// The recorder of [`crate::Transform::explain_point`], by prepared
/// candidate.
#[derive(Debug, Default)]
pub(crate) struct Explain {
    pub(crate) inside: Vec<Option<bool>>,
    pub(crate) passed: Vec<Option<(usize, PassReason)>>,
    pub(crate) tried: Vec<Option<Option<CoordinateError>>>,
    pub(crate) fallback: Option<usize>,
    pub(crate) rounds: usize,
}

impl Explain {
    pub(crate) fn new(candidates: usize) -> Explain {
        Explain {
            inside: vec![None; candidates],
            passed: vec![None; candidates],
            tried: vec![None; candidates],
            ..Explain::default()
        }
    }
}

impl Recorder for Explain {
    const DETAILED: bool = true;

    fn inside(&mut self, candidate: usize, inside: bool) {
        self.inside[candidate] = Some(inside);
    }

    fn passed(&mut self, candidate: usize, by: usize, reason: PassReason) {
        self.passed[candidate] = Some((by, reason));
    }

    fn tried(&mut self, candidate: usize, error: Option<CoordinateError>) {
        self.tried[candidate] = Some(error);
    }

    fn fallback(&mut self, candidate: usize) {
        self.fallback = Some(candidate);
    }

    fn round(&mut self) {
        self.rounds += 1;
    }
}

impl Outcome {
    /// Which of two prepared parts of one operation (one across the
    /// antimeridian is prepared twice) says more.
    pub(crate) fn weight(&self) -> u8 {
        match self {
            Outcome::Chosen | Outcome::Fallback | Outcome::Only => 5,
            Outcome::Failed(_) => 4,
            Outcome::Passed { .. } => 3,
            Outcome::NotReached => 2,
            Outcome::Outside => 1,
        }
    }
}
