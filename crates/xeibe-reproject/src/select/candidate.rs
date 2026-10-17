//! Candidate operations as PROJ builds them: single operations from the
//! registry, their inverses, synthetic conversions on one datum, and
//! concatenations, each with PROJ's name, accuracy and area of use.
//!
//! Ported from PROJ 9.8.1 `src/iso19111/operation/` (MIT, see
//! PROJ-NOTICE.md): `oputils.cpp` (`getAccuracy`, `getExtent`, naming of
//! inverses) and `concatenatedoperation.cpp` (`createComputeMetadata`). The
//! names matter: PROJ's last tie-breaks compare them.

use super::crs::{CrsCache, CrsKind, CrsRef};
use crate::extent::{Extent, world};
use crate::registry::{Table, registry};

pub(crate) const INVERSE_OF: &str = "Inverse of ";
pub(crate) const NULL_GEOGRAPHIC_OFFSET: &str = "Null geographic offset";
pub(crate) const NULL_GEOCENTRIC_TRANSLATION: &str = "Null geocentric translation";
pub(crate) const BALLPARK_GEOCENTRIC_TRANSLATION: &str = "Ballpark geocentric translation";
pub(crate) const BALLPARK_GEOGRAPHIC_OFFSET: &str = "Ballpark geographic offset";
pub(crate) const ENSEMBLE_MEMBER: &str = "Ensemble member identity";

/// What a single step does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SingleKind {
    /// A registry transformation, forward or inverse.
    Registry { code: u32, inverse: bool },
    /// A projected CRS's deriving conversion, forward or inverse.
    Projection { crs: u32, inverse: bool },
    /// A change between CRSs on one datum: 2D ↔ 3D, geographic ↔
    /// geocentric. PROJ names these "Null geographic offset", "Conversion
    /// from … to …" or "Null geocentric translation".
    SameDatum,
    /// Between a datum ensemble and one of its members: coordinates pass
    /// unchanged, good to the ensemble's accuracy (`datum.rs`).
    EnsembleMember,
}

/// One step of a candidate, between two CRSs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Single {
    pub(crate) kind: SingleKind,
    pub(crate) src: CrsRef,
    pub(crate) dst: CrsRef,
    pub(crate) name: String,
    /// -1 when unknown.
    pub(crate) accuracy: f64,
    pub(crate) extent: Option<Extent>,
    /// PROJ's conversions (not transformations): accuracy 0, area the world.
    pub(crate) is_conversion: bool,
    /// Has an identifier (an EPSG code, or the inverse of one).
    pub(crate) identified: bool,
}

/// A candidate operation from the source to the target CRS.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) name: String,
    pub(crate) src: CrsRef,
    pub(crate) dst: CrsRef,
    pub(crate) steps: Vec<Single>,
    /// The candidate itself has an identifier: a registry operation (single
    /// or concatenated) or a conversion, or the inverse of one. A
    /// concatenation built during the search has none.
    pub(crate) identified: bool,
    /// The registry operation this candidate is, with its direction, when it
    /// is one (single or concatenated).
    pub(crate) registry: Option<(u32, bool)>,
    /// -1 when unknown.
    pub(crate) accuracy: f64,
    pub(crate) extent: Option<Extent>,
    /// EPSG's remarks say "Replaced by".
    pub(crate) replaced: bool,
    /// A step's method is time-dependent.
    pub(crate) requires_time: bool,
}

impl Candidate {
    pub(crate) fn single(step: Single) -> Candidate {
        Candidate {
            name: step.name.clone(),
            src: step.src,
            dst: step.dst,
            identified: step.identified,
            registry: match step.kind {
                SingleKind::Registry { code, inverse } => Some((code, inverse)),
                _ => None,
            },
            accuracy: step.accuracy,
            extent: step.extent.clone(),
            replaced: match step.kind {
                SingleKind::Registry { code, .. } => registry().ops[&code].replaced,
                _ => false,
            },
            requires_time: match step.kind {
                SingleKind::Registry { code, .. } => {
                    is_time_dependent(registry().ops[&code].method)
                }
                _ => false,
            },
            steps: vec![step],
        }
    }

    /// Uses a grid-based operation.
    pub(crate) fn has_grids(&self) -> bool {
        self.steps.iter().any(|s| match s.kind {
            SingleKind::Registry { code, .. } => registry().ops[&code].table == Table::Grid,
            _ => false,
        })
    }

    /// The inverse candidate, named as PROJ names inverses.
    pub(crate) fn inverse(&self, crs: &mut CrsCache) -> Candidate {
        let steps: Vec<Single> = self.steps.iter().rev().map(|s| s.inverse(crs)).collect();
        let name = if self.steps.len() == 1 {
            steps[0].name.clone()
        } else if let Some((code, inverse)) = self.registry {
            // A registered concatenation: "Inverse of X", and back to X.
            if inverse {
                registry().ops[&code].name.clone()
            } else {
                invert_name(&self.name, false)
            }
        } else {
            concatenated_name(&steps)
        };
        Candidate {
            name,
            src: self.dst,
            dst: self.src,
            steps,
            identified: self.identified,
            registry: self.registry.map(|(c, inv)| (c, !inv)),
            accuracy: self.accuracy,
            extent: self.extent.clone(),
            replaced: self.replaced,
            requires_time: self.requires_time,
        }
    }
}

/// PROJ's name for the inverse of a named, non-synthetic operation.
fn invert_name(name: &str, is_conversion: bool) -> String {
    if let Some(rest) = name.strip_prefix(INVERSE_OF)
        && is_conversion
    {
        return rest.to_owned();
    }
    if name.contains(" + ") {
        format!("{INVERSE_OF}'{name}'")
    } else {
        format!("{INVERSE_OF}{name}")
    }
}

impl Single {
    pub(crate) fn inverse(&self, crs: &mut CrsCache) -> Single {
        let kind = match &self.kind {
            SingleKind::Registry { code, inverse } => SingleKind::Registry {
                code: *code,
                inverse: !inverse,
            },
            SingleKind::Projection { crs, inverse } => SingleKind::Projection {
                crs: *crs,
                inverse: !inverse,
            },
            SingleKind::SameDatum => SingleKind::SameDatum,
            SingleKind::EnsembleMember => SingleKind::EnsembleMember,
        };
        let name = match &self.kind {
            // Synthetic operations are named after their CRSs.
            SingleKind::SameDatum => same_datum_name(crs, self.dst, self.src),
            SingleKind::EnsembleMember => op_name(crs, ENSEMBLE_MEMBER, self.dst, self.src),
            SingleKind::Registry { code, inverse }
            | SingleKind::Projection { crs: code, inverse } => {
                if *inverse {
                    // Back to the forward operation.
                    match &self.kind {
                        SingleKind::Registry { .. } => registry().ops[code].name.clone(),
                        _ => self
                            .name
                            .strip_prefix(INVERSE_OF)
                            .unwrap_or(&self.name)
                            .to_owned(),
                    }
                } else {
                    invert_name(&self.name, false)
                }
            }
        };
        Single {
            kind,
            src: self.dst,
            dst: self.src,
            name,
            accuracy: self.accuracy,
            extent: self.extent.clone(),
            is_conversion: self.is_conversion,
            identified: self.identified,
        }
    }
}

/// Time-dependent methods (rates applied from a reference epoch). Without a
/// coordinate epoch PROJ applies them at their reference epoch.
pub(crate) fn is_time_dependent(method: u32) -> bool {
    matches!(method, 1053..=1058 | 1143)
}

/// A registry operation as a single step (not concatenated ones).
pub(crate) fn registry_single(code: u32, inverse: bool) -> Single {
    let op = &registry().ops[&code];
    let extent = op.area.clone();
    let (src, dst) = (
        CrsRef::Epsg(op.src.unwrap_or(0)),
        CrsRef::Epsg(op.dst.unwrap_or(0)),
    );
    let is_conversion = op.table == Table::Conversion;
    let forward = Single {
        kind: SingleKind::Registry {
            code,
            inverse: false,
        },
        src,
        dst,
        name: op.name.clone(),
        accuracy: if is_conversion {
            0.0
        } else {
            op.accuracy.unwrap_or(-1.0)
        },
        extent: if is_conversion { Some(world()) } else { extent },
        is_conversion,
        identified: true,
    };
    if !inverse {
        return forward;
    }
    Single {
        kind: SingleKind::Registry {
            code,
            inverse: true,
        },
        src: dst,
        dst: src,
        name: invert_name(&op.name, is_conversion),
        ..forward
    }
}

/// A registry operation, single or concatenated, as a candidate.
pub(crate) fn registry_candidate(
    code: u32,
    inverse: bool,
    crs: &mut CrsCache,
) -> Option<Candidate> {
    let op = &registry().ops[&code];
    if op.table != Table::Concatenated {
        return Some(Candidate::single(registry_single(code, inverse)));
    }
    // The steps' directions follow from chaining their CRSs, as PROJ does.
    let mut steps = Vec::new();
    let mut cur = CrsRef::Epsg(op.src?);
    for &step in &op.steps {
        let s = &registry().ops[&step];
        let (a, b) = (CrsRef::Epsg(s.src?), CrsRef::Epsg(s.dst?));
        let forward = if a == cur {
            true
        } else if b == cur {
            false
        } else {
            // Different CRSs on one datum (2D vs 3D, geographic vs geocentric).
            let d = crs.datum(cur);
            if crs.datum(a) == d {
                true
            } else if crs.datum(b) == d {
                false
            } else {
                return None;
            }
        };
        let single = registry_single(step, !forward);
        if single.src != cur {
            steps.push(same_datum(crs, cur, single.src)?);
        }
        cur = single.dst;
        steps.push(single);
    }
    let dst = CrsRef::Epsg(op.dst?);
    if cur != dst {
        steps.push(same_datum(crs, cur, dst)?);
    }
    let extent = op.area.clone();
    let accuracy = match op.accuracy {
        Some(a) => a,
        None => steps_accuracy(&steps, crs),
    };
    let forward = Candidate {
        name: op.name.clone(),
        src: CrsRef::Epsg(op.src?),
        dst,
        identified: true,
        registry: Some((code, false)),
        accuracy,
        extent,
        replaced: op.replaced,
        requires_time: steps.iter().any(|s| match s.kind {
            SingleKind::Registry { code, .. } => is_time_dependent(registry().ops[&code].method),
            _ => false,
        }),
        steps,
    };
    Some(if inverse {
        forward.inverse(crs)
    } else {
        forward
    })
}

/// The deriving conversion of a projected CRS, forward (base to projected)
/// or inverse.
pub(crate) fn projection(crs: &mut CrsCache, projected: u32, inverse: bool) -> Option<Single> {
    let facts = crs.get(CrsRef::Epsg(projected))?.clone();
    let (name, _) = facts.conversion?;
    let base = CrsRef::Epsg(facts.base?);
    let p = CrsRef::Epsg(projected);
    let (src, dst, name) = if inverse {
        (p, base, format!("{INVERSE_OF}{name}"))
    } else {
        (base, p, name)
    };
    Some(Single {
        kind: SingleKind::Projection {
            crs: projected,
            inverse,
        },
        src,
        dst,
        name,
        accuracy: 0.0,
        extent: Some(world()),
        is_conversion: true,
        identified: true,
    })
}

/// PROJ's `buildOpName`: qualifiers only when both CRSs share a name.
pub(crate) fn op_name(crs: &mut CrsCache, op_type: &str, src: CrsRef, dst: CrsRef) -> String {
    let (a, b) = (crs.name(src), crs.name(dst));
    let (mut qa, mut qb) = ("", "");
    if a == b {
        qa = qualifier(crs.kind(src));
        qb = qualifier(crs.kind(dst));
        if qa == qb {
            qa = "";
            qb = "";
        }
    }
    format!("{op_type} from {a}{qa} to {b}{qb}")
}

fn qualifier(kind: CrsKind) -> &'static str {
    match kind {
        CrsKind::Geocentric => " (geocentric)",
        CrsKind::Geographic2d => " (geog2D)",
        CrsKind::Geographic3d => " (geog3D)",
        _ => "",
    }
}

fn same_datum_name(crs: &mut CrsCache, src: CrsRef, dst: CrsRef) -> String {
    let (ks, kd) = (crs.kind(src), crs.kind(dst));
    let op_type = match (ks, kd) {
        (CrsKind::Geocentric, CrsKind::Geocentric) => NULL_GEOCENTRIC_TRANSLATION,
        (CrsKind::Geocentric, _) | (_, CrsKind::Geocentric) => "Conversion",
        _ => NULL_GEOGRAPHIC_OFFSET,
    };
    op_name(crs, op_type, src, dst)
}

/// The operation PROJ creates between two geodetic CRSs on one datum
/// without its database: a null geographic offset (2D ↔ 3D), a
/// geographic ↔ geocentric conversion or a null geocentric translation.
pub(crate) fn same_datum(crs: &mut CrsCache, src: CrsRef, dst: CrsRef) -> Option<Single> {
    let (ks, kd) = (crs.kind(src), crs.kind(dst));
    if !ks.is_geodetic() || !kd.is_geodetic() {
        return None;
    }
    let name = same_datum_name(crs, src, dst);
    let geocentric = ks == CrsKind::Geocentric || kd == CrsKind::Geocentric;
    let conversion = geocentric && ks != kd;
    // Null offsets and translations take the source CRS's area when both
    // CRSs share it, else the world.
    let extent = if conversion {
        world()
    } else {
        match (crs.extent(src), crs.extent(dst)) {
            (Some(a), Some(b)) if a == b => a,
            _ => world(),
        }
    };
    Some(Single {
        kind: SingleKind::SameDatum,
        src,
        dst,
        name,
        accuracy: 0.0,
        extent: Some(extent),
        is_conversion: conversion,
        identified: false,
    })
}

/// Between a datum ensemble and one of its members (`datum.rs`), over the
/// area both CRSs cover.
pub(crate) fn ensemble_member(crs: &mut CrsCache, src: CrsRef, dst: CrsRef) -> Option<Single> {
    if !crs.kind(src).is_geodetic() || !crs.kind(dst).is_geodetic() {
        return None;
    }
    let accuracy = crate::datum::ensemble_member(crs.datum(src)?, crs.datum(dst)?)?;
    let extent = match (crs.extent(src), crs.extent(dst)) {
        (Some(a), Some(b)) => a.intersection(&b)?,
        (a, b) => a.or(b).unwrap_or_else(world),
    };
    Some(Single {
        kind: SingleKind::EnsembleMember,
        src,
        dst,
        name: op_name(crs, ENSEMBLE_MEMBER, src, dst),
        accuracy,
        extent: Some(extent),
        is_conversion: false,
        identified: false,
    })
}

/// PROJ's `computeConcatenatedName`.
pub(crate) fn concatenated_name(steps: &[Single]) -> String {
    steps
        .iter()
        .map(|s| {
            if s.name.is_empty() {
                "unnamed"
            } else {
                s.name.as_str()
            }
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Whether a step is PROJ's `+proj=noop`: a geocentric translation of zero
/// between CRSs on one ellipsoid.
fn is_noop(step: &Single, crs: &mut CrsCache) -> bool {
    let SingleKind::Registry { code, .. } = step.kind else {
        return false;
    };
    let op = &registry().ops[&code];
    if !matches!(op.method, 9603 | 1031 | 1035)
        || op
            .params
            .iter()
            .any(|(_, v)| *v != crate::registry::ParamValue::Number(0.0))
    {
        return false;
    }
    let ellipsoid = |c: CrsRef, crs: &mut CrsCache| {
        crs.datum(c)
            .and_then(|d| registry().datums.get(&d))
            .map(|d| d.ellipsoid)
    };
    ellipsoid(step.src, crs) == ellipsoid(step.dst, crs)
}

/// PROJ's `getAccuracy` of a list of operations: the sum, unknown if any
/// step's is, with its ETRS89-realization rule.
pub(crate) fn steps_accuracy(steps: &[Single], crs: &mut CrsCache) -> f64 {
    const ACCURACY_ENSEMBLE_ETRS89: f64 = 0.1;
    let mut accuracy = -1.0;
    let mut next_accuracy = -1.0;
    let mut i = 0;
    while i < steps.len() {
        let sub = if next_accuracy >= 0.0 {
            next_accuracy
        } else {
            steps[i].accuracy
        };
        if sub < 0.0 {
            return -1.0;
        }
        if accuracy < 0.0 {
            accuracy = 0.0;
        }
        next_accuracy = -1.0;
        if sub > 0.0 && i + 1 < steps.len() {
            next_accuracy = steps[i + 1].accuracy;
            if next_accuracy > 0.0 {
                let crs1 = crs.name(steps[i].src);
                let middle = crs.name(steps[i].dst);
                let crs2 = crs.name(steps[i + 1].dst);
                if sub.max(next_accuracy) <= ACCURACY_ENSEMBLE_ETRS89
                    && (middle == "ETRS89" || middle.starts_with("ETRF"))
                    && crs1.starts_with("ETRS89-")
                    && crs2.starts_with("ETRS89-")
                    && (is_noop(&steps[i], crs) || is_noop(&steps[i + 1], crs))
                {
                    accuracy += sub.max(next_accuracy);
                    next_accuracy = -1.0;
                    i += 2;
                    continue;
                }
            }
        }
        accuracy += sub;
        i += 1;
    }
    accuracy
}

/// Two steps undo each other.
fn cancels(a: &Single, b: &Single) -> bool {
    a.src == b.dst
        && a.dst == b.src
        && match (&a.kind, &b.kind) {
            (
                SingleKind::Registry {
                    code: c1,
                    inverse: i1,
                },
                SingleKind::Registry {
                    code: c2,
                    inverse: i2,
                },
            ) => c1 == c2 && i1 != i2,
            (
                SingleKind::Projection {
                    crs: c1,
                    inverse: i1,
                },
                SingleKind::Projection {
                    crs: c2,
                    inverse: i2,
                },
            ) => c1 == c2 && i1 != i2,
            (SingleKind::SameDatum, SingleKind::SameDatum)
            | (SingleKind::EnsembleMember, SingleKind::EnsembleMember) => true,
            _ => false,
        }
}

/// PROJ's `ConcatenatedOperation::createComputeMetadata`: flattens nested
/// concatenations, drops pairs of steps that undo each other, and computes
/// the name, area of use (intersection) and accuracy. `None` when
/// `check_extent` is set and the areas don't intersect.
pub(crate) fn concatenate(
    parts: Vec<Candidate>,
    check_extent: bool,
    crs: &mut CrsCache,
) -> Option<Candidate> {
    if parts.len() == 1 {
        return parts.into_iter().next();
    }
    let src = parts.first()?.src;
    let dst = parts.last()?.dst;
    let requires_time = parts.iter().any(|p| p.requires_time);
    // Flatten: a concatenated part contributes its steps. A single-step part
    // with its own name and extent stays as that step.
    let mut flat: Vec<(Single, Option<Extent>)> = Vec::new();
    for part in parts {
        if part.steps.len() == 1 {
            let mut step = part.steps[0].clone();
            step.name = part.name.clone();
            flat.push((step, part.extent.clone()));
        } else {
            for step in part.steps {
                let extent = step.extent.clone();
                flat.push((step, extent));
            }
        }
    }
    if flat.len() > 2 {
        loop {
            let mut changed = false;
            let mut i = 0;
            while i + 1 < flat.len() {
                if cancels(&flat[i].0, &flat[i + 1].0) {
                    flat.drain(i..i + 2);
                    changed = true;
                    break;
                }
                i += 1;
            }
            if !changed || flat.len() <= 2 {
                break;
            }
        }
    }
    if flat.len() == 1 {
        let (step, _) = flat.pop()?;
        return Some(Candidate::single(step));
    }
    let steps: Vec<Single> = flat.iter().map(|(s, _)| s.clone()).collect();
    // getExtent of a list: intersection of the steps' areas, skipping
    // steps without one.
    let mut extent: Option<Extent> = None;
    let mut empty = false;
    for (_, e) in &flat {
        let Some(e) = e else { continue };
        extent = match extent {
            None => Some(e.clone()),
            Some(acc) => match acc.intersection(e) {
                Some(x) => Some(x),
                None => {
                    empty = true;
                    None
                }
            },
        };
        if empty {
            break;
        }
    }
    if check_extent && empty {
        return None;
    }
    let accuracy = steps_accuracy(&steps, crs);
    Some(Candidate {
        name: concatenated_name(&steps),
        src,
        dst,
        steps,
        identified: false,
        registry: None,
        accuracy,
        extent: if empty { None } else { extent },
        replaced: false,
        requires_time,
    })
}
