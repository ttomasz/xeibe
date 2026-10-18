//! The search with heights: vertical and compound CRSs (`docs/reprojection.md`,
//! level 3), ported from PROJ 9.8.1
//! `src/iso19111/operation/coordinateoperationfactory.cpp` (MIT, see
//! PROJ-NOTICE.md): the vertical part of `createOperationsFromDatabase`,
//! `createOperationsFromDatabaseWithVertCRS` and its helpers,
//! `createOperationsVertToVert`, `createOperationsVertToGeogSynthetized`,
//! `createOperationsCompoundToGeog`, `createOperationsToGeod`,
//! `createOperationsCompoundToCompound`, and the operations PROJ builds from
//! a horizontal and a vertical one (`createHorizVerticalPROJBased`,
//! `createHorizVerticalHorizPROJBased`, `createHorizNullVerticalPROJBased`).
//!
//! Left out, as EPSG's CRSs never reach them: bound CRSs, geoid models
//! attached to a vertical CRS, coordinate epochs (and PROJ's NAD83(CSRS)v7
//! and CGVD2013 special cases that rely on them), PROJ strings.

use std::collections::{BTreeMap, HashMap};

use super::super::candidate::{
    BALLPARK_VERTICAL_NO_ELLIPSOID, BALLPARK_VERTICAL_TRANSFORMATION, Candidate,
    NULL_GEOGRAPHIC_OFFSET, Single, SingleKind, concatenate, conversion_name, registry_candidate,
    steps_accuracy,
};
use super::super::crs::{CrsKind, CrsRef};
use super::super::filter::filter_out;
use super::Search;
use crate::extent::{Extent, world};
use crate::registry::{Table, registry};

/// "EPSG code for Interpolation CRS", "EPSG code for Horizontal CRS".
const INTERPOLATION_CRS: u32 = 1048;
const HORIZONTAL_CRS: u32 = 1037;

impl Search {
    fn is_vertical(&mut self, c: CrsRef) -> bool {
        self.crs.kind(c) == CrsKind::Vertical
    }

    /// The vertical part of `createOperationsFromDatabase` before the
    /// registry: a geographic to vertical search runs the other way round.
    /// (EPSG's vertical CRSs carry no geoid model, so
    /// `createOperationsGeogToVertFromGeoid` finds nothing.)
    pub(super) fn vertical_prelude(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) {
        if self.crs.kind(s).is_geographic() && self.is_vertical(t) {
            let mut swapped = Vec::new();
            self.query_registry(t, s, &mut swapped);
            *res = self.inverse_all(swapped);
        }
    }

    /// The vertical branch of `createOperationsFromDatabase` after the
    /// direct registry search.
    pub(super) fn vertical_database(
        &mut self,
        s: CrsRef,
        t: CrsRef,
        res: &mut Vec<Candidate>,
        non_empty_before: &mut bool,
    ) {
        let (ks, kt) = (self.crs.kind(s), self.crs.kind(t));
        if res.is_empty() {
            // From a geographic 2D CRS: the operations from its 3D twin.
            if ks == CrsKind::Geographic2d && kt == CrsKind::Vertical {
                let p = self.crs.promote_to_3d(s);
                for op in self.find_ops_direct(p, t, non_empty_before) {
                    res.push(op.with_crs(s, t));
                }
            } else if kt == CrsKind::Geographic2d && ks == CrsKind::Vertical {
                let p = self.crs.promote_to_3d(t);
                for op in self.find_ops_direct(s, p, non_empty_before) {
                    res.push(op.with_crs(s, t));
                }
            }
        }
        if res.is_empty() {
            self.database_with_vertical(s, t, res);
        }
    }

    /// `createOperationsFromDatabaseWithVertCRS`.
    fn database_with_vertical(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) {
        let (ks, kt) = (self.crs.kind(s), self.crs.kind(t));
        let (geog_s, geog_t) = (ks.is_geographic(), kt.is_geographic());
        let (vert_s, vert_t) = (ks == CrsKind::Vertical, kt == CrsKind::Vertical);
        // "NAVD88 height (ftUS)" to a geographic CRS through the operations
        // of "NAVD88 height".
        if res.is_empty() && !self.in_geog_to_vert_intermediate && geog_s && vert_t {
            *res = self.geog_to_vert_with_intermediate_vert(s, t);
        } else if res.is_empty() && !self.in_geog_to_vert_intermediate && geog_t && vert_s {
            let found = self.geog_to_vert_with_intermediate_vert(t, s);
            *res = self.inverse_all(found);
        }
        // A geographic 3D CRS EPSG has the operations of in 2D.
        if self.geog3d_to_vert_through_geog2d(s, t, res) {
        } else if self.geog3d_to_vert_through_geog2d(t, s, res) {
            let found = std::mem::take(res);
            *res = self.inverse_all(found);
        }
        if res.is_empty() && !self.in_vert_to_vert_intermediate && vert_s && vert_t {
            *res = self.vert_to_vert_with_intermediate_vert(s, t);
        }
        // No direct operation from "NAVD88 height" to WGS 84: those to
        // another geographic CRS.
        if res.is_empty() && !self.in_geog_to_vert_alternative && geog_s && vert_t {
            *res = self.geog_to_vert_with_alternative_geog(s, t);
        } else if res.is_empty() && !self.in_geog_to_vert_alternative && geog_t && vert_s {
            let found = self.geog_to_vert_with_alternative_geog(t, s);
            *res = self.inverse_all(found);
        }
    }

    /// The `geog3DToVertTryThroughGeog2D` lambda: whether it applied.
    fn geog3d_to_vert_through_geog2d(
        &mut self,
        geog: CrsRef,
        vert: CrsRef,
        res: &mut Vec<Candidate>,
    ) -> bool {
        if !res.is_empty()
            || self.crs.kind(geog) != CrsKind::Geographic3d
            || !self.is_vertical(vert)
        {
            return false;
        }
        // `createGeodeticCRSFromDatum`: live geodetic CRSs on the datum, by
        // code; the first geographic 2D one.
        let r = registry();
        let candidates = self
            .crs
            .datum(geog)
            .and_then(|d| r.crs_by_datum.get(&d))
            .cloned()
            .unwrap_or_default();
        for c in candidates {
            if r.geodetic.get(&c).map(|g| g.kind)
                == Some(crate::registry::GeodeticKind::Geographic2d)
            {
                let mut ignored = false;
                *res = self.find_ops_direct(CrsRef::Epsg(c), vert, &mut ignored);
                break;
            }
        }
        true
    }

    /// `findCandidateVertCRSForDatum`: the live vertical CRSs on the datum.
    fn vertical_crs_on_datum(&mut self, c: CrsRef) -> Vec<CrsRef> {
        let r = registry();
        self.crs
            .vertical_datum(c)
            .and_then(|d| r.vertical_by_datum.get(&d))
            .map(|list| list.iter().map(|&code| CrsRef::Epsg(code)).collect())
            .unwrap_or_default()
    }

    /// `createOperationsGeogToVertWithIntermediateVert`.
    fn geog_to_vert_with_intermediate_vert(&mut self, s: CrsRef, t: CrsRef) -> Vec<Candidate> {
        self.in_geog_to_vert_intermediate = true;
        let mut res = Vec::new();
        for cand in self.vertical_crs_on_datum(t) {
            let first = self.create_operations(s, cand);
            if first.is_empty() {
                continue;
            }
            let second = self.create_operations(cand, t);
            if second.is_empty() {
                continue;
            }
            for f in first {
                if !f.identified {
                    continue;
                }
                if cand == t {
                    res.push(f);
                } else if let Some(c) = concatenate(vec![f, second[0].clone()], true, &mut self.crs)
                {
                    res.push(c);
                }
            }
            if !res.is_empty() {
                break;
            }
        }
        self.in_geog_to_vert_intermediate = false;
        res
    }

    /// `createOperationsVertToVertWithIntermediateVert`: through a vertical
    /// CRS on the target's datum (an operation to it, then a change of unit
    /// or direction), else on the source's.
    fn vert_to_vert_with_intermediate_vert(&mut self, s: CrsRef, t: CrsRef) -> Vec<Candidate> {
        self.in_vert_to_vert_intermediate = true;
        let mut res = Vec::new();
        for cand in self.vertical_crs_on_datum(t) {
            if cand == t {
                continue;
            }
            let first = self.create_operations(s, cand);
            if first.is_empty() {
                continue;
            }
            let second = self.create_operations(cand, t);
            if second.is_empty() {
                continue;
            }
            for f in first {
                if f.identified
                    && let Some(c) = concatenate(vec![f, second[0].clone()], true, &mut self.crs)
                {
                    res.push(c);
                }
            }
        }
        if res.is_empty() {
            for cand in self.vertical_crs_on_datum(s) {
                if cand == s {
                    continue;
                }
                let second = self.create_operations(cand, t);
                if second.is_empty() {
                    continue;
                }
                let first = self.create_operations(s, cand);
                if first.is_empty() {
                    continue;
                }
                for g in second {
                    if g.identified
                        && let Some(c) = concatenate(vec![first[0].clone(), g], true, &mut self.crs)
                    {
                        res.push(c);
                    }
                }
            }
        }
        self.in_vert_to_vert_intermediate = false;
        res
    }

    /// `createOperationsGeogToVertWithAlternativeGeog`: the registry's
    /// operations to the vertical CRS from any geographic CRS, used from the
    /// source's longitude and latitude as they are.
    fn geog_to_vert_with_alternative_geog(&mut self, s: CrsRef, t: CrsRef) -> Vec<Candidate> {
        let _ = s;
        self.in_geog_to_vert_alternative = true;
        let ops = self.find_ops_direct_to(t);
        let mut res = Vec::new();
        for op in ops {
            if self.crs.kind(op.src).is_geographic() {
                res.push(op);
            }
        }
        self.in_geog_to_vert_alternative = false;
        res
    }

    /// `findOpsInRegistryDirectTo`: the registry's operations ending at
    /// `t`, either way round, that meet both CRSs' areas, filtered.
    fn find_ops_direct_to(&mut self, t: CrsRef) -> Vec<Candidate> {
        let CrsRef::Epsg(tc) = t else {
            return Vec::new();
        };
        let r = registry();
        let mut rows: Vec<(u32, Extent, Option<f64>)> = Vec::new();
        let mut codes: Vec<u32> = r.by_crs.get(&tc).cloned().unwrap_or_default();
        codes.sort_unstable();
        for code in codes {
            let op = &r.ops[&code];
            if op.deprecated || op.table == Table::Conversion {
                continue;
            }
            if let Some(extent) = &op.area {
                rows.push((code, extent.clone(), op.accuracy));
            }
        }
        rows.sort_by(|a, b| {
            b.1.pseudo_area()
                .total_cmp(&a.1.pseudo_area())
                .then(a.2.is_none().cmp(&b.2.is_none()))
                .then(a.2.unwrap_or(0.0).total_cmp(&b.2.unwrap_or(0.0)))
        });
        let set: std::collections::HashSet<u32> = rows.iter().map(|row| row.0).collect();
        let mut out = Vec::new();
        for (code, extent, _) in rows {
            if Self::superseded_in(&r.ops[&code], &set) || !self.intersects_crs_areas(&extent) {
                continue;
            }
            let inverse = r.ops[&code].dst != Some(tc);
            if let Some(c) = registry_candidate(code, inverse, &mut self.crs)
                && c.grids_available(self.grids)
            {
                out.push(c);
            }
        }
        if out.is_empty() {
            return out;
        }
        filter_out(&out, &self.areas, false)
    }

    /// `createOperationsVertToVert`: a change of unit or direction on one
    /// datum, else PROJ's ballpark.
    pub(super) fn vert_to_vert(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) {
        let same = self.crs.vertical_datum(s).is_some()
            && self.crs.vertical_datum(s) == self.crs.vertical_datum(t);
        let extent = self.same_extent(s, t);
        let single = if same {
            Single {
                kind: SingleKind::VerticalConversion,
                src: s,
                dst: t,
                name: conversion_name(&mut self.crs, s, t),
                accuracy: 0.0,
                extent: Some(extent),
                is_conversion: true,
                identified: false,
                ballpark: false,
            }
        } else {
            let name = format!(
                "Transformation from {} to {} ({BALLPARK_VERTICAL_TRANSFORMATION})",
                self.crs.name(s),
                self.crs.name(t)
            );
            ballpark_vertical(s, t, name, extent)
        };
        res.push(Candidate::single(single));
    }

    /// The source CRS's area when the target has the same, else the world.
    fn same_extent(&mut self, s: CrsRef, t: CrsRef) -> Extent {
        match (self.crs.extent(s), self.crs.extent(t)) {
            (Some(a), Some(b)) if a == b => a,
            _ => world(),
        }
    }

    /// `createOperationsVertToGeogSynthetized`: a change of unit when the
    /// vertical datum has the geographic CRS's datum's name, else PROJ's
    /// ballpark (the gravity-related height taken as ellipsoidal).
    pub(super) fn vert_to_geog(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) {
        let r = registry();
        let name_of = |d: Option<u32>| d.and_then(|d| r.datums.get(&d)).map(|d| d.name.clone());
        let vertical_name = name_of(self.crs.vertical_datum(s));
        let geodetic_name = name_of(self.crs.datum(t));
        let same = vertical_name.is_some()
            && vertical_name.as_deref() != Some("unknown")
            && vertical_name == geodetic_name;
        let extent = self.same_extent(s, t);
        let single = if same {
            let factor = self.crs.vertical_axis(s).map_or(1.0, |(f, _)| f);
            let from = if factor != 1.0 { s } else { t };
            Single {
                kind: SingleKind::VerticalConversion,
                src: s,
                dst: t,
                name: conversion_name(&mut self.crs, from, t),
                accuracy: 0.0,
                extent: Some(extent),
                is_conversion: true,
                identified: false,
                ballpark: false,
            }
        } else {
            let name = format!(
                "Transformation from {} to {} ({BALLPARK_VERTICAL_NO_ELLIPSOID})",
                self.crs.name(s),
                self.crs.name(t)
            );
            ballpark_vertical(s, t, name, extent)
        };
        res.push(Candidate::single(single));
    }

    /// Whether a list has a registered operation, or a concatenation with
    /// a registered step (all its grids are available: the search keeps no
    /// other): PROJ's `hasAtLeastOneOpWithNonTrivialAndWithAllGrids`.
    fn has_registered(ops: &[Candidate]) -> bool {
        ops.iter()
            .any(|op| op.identified || op.steps.iter().any(|s| s.identified))
    }

    /// `createOperationsCompoundToGeog`.
    pub(super) fn compound_to_geog(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) {
        let Some((h, v)) = self.crs.components(s) else {
            return;
        };
        // A projected horizontal CRS: to its base first.
        if self.crs.kind(h) == CrsKind::Projected {
            let Some(base) = self.crs.get(h).and_then(|f| f.base) else {
                return;
            };
            let interm = CrsRef::Compound(base, v.code());
            let first = self.create_operations(s, interm);
            let Some(first) = first.into_iter().next() else {
                return;
            };
            for last in self.create_operations(interm, t) {
                if let Some(c) = concatenate(vec![first.clone(), last], true, &mut self.crs) {
                    res.push(c);
                }
            }
            return;
        }
        // Only a vertical transformation if the target is 3D.
        if self.crs.kind(t) != CrsKind::Geographic3d {
            for op in self.create_operations(h, t) {
                res.push(op.with_crs(s, t));
            }
            return;
        }
        let src_geog = self.crs.geographic(h);
        let horizontal = match src_geog {
            Some(_) => self.create_operations(h, t),
            None => Vec::new(),
        };
        let mut vertical = self.create_operations(v, t);
        let found = Self::has_registered(&vertical);
        if let Some(g) = src_geog
            && g != t
            && self.crs.promote_to_3d(g) != t
        {
            let g2 = self.crs.demote_to_2d(g);
            let g3 = self.crs.promote_to_3d(g2);
            let tmp = self.create_operations(v, g3);
            let found2 = Self::has_registered(&tmp);
            if found2 && !found {
                vertical = tmp;
            } else if found2 {
                vertical.extend(tmp);
            }
        }
        // Only ballpark vertical transformations: through a geographic CRS
        // a horizontal transformation passes, if there is a vertical one to
        // it (PROJ's issue 4618).
        if vertical.iter().all(Candidate::has_ballpark) {
            let mut intermediates: BTreeMap<String, CrsRef> = BTreeMap::new();
            for op in &horizontal {
                if op.has_ballpark() || op.steps.len() < 2 {
                    continue;
                }
                for step in &op.steps {
                    let g = self.crs.promote_to_3d(step.dst);
                    let geodetic = matches!(
                        self.crs.kind(g),
                        CrsKind::Geographic3d | CrsKind::Geocentric
                    );
                    if geodetic && matches!(g, CrsRef::Epsg(_)) && g != t {
                        let name = self.crs.name(g);
                        intermediates.entry(name).or_insert(g);
                    }
                }
            }
            for g in intermediates.into_values() {
                let tmp = self.create_operations(v, g);
                if tmp.iter().all(Candidate::has_ballpark) {
                    continue;
                }
                let ops1 = self.create_operations(s, g);
                let ops2 = self.create_operations(g, t);
                for o1 in ops1.iter().filter(|o| !o.has_ballpark()) {
                    for o2 in ops2.iter().filter(|o| !o.has_ballpark()) {
                        if let Some(c) =
                            concatenate(vec![o1.clone(), o2.clone()], true, &mut self.crs)
                        {
                            res.push(c);
                        }
                    }
                }
            }
        }
        // PROJ's horizontal list for a geographic CRS is never empty: it
        // falls back on a ballpark, which xeibe leaves out (the filter
        // drops it), so an empty list goes on to the interpolation CRS.
        if (horizontal.is_empty() && src_geog.is_none()) || vertical.is_empty() {
            *res = horizontal;
            return;
        }
        let mut cache: HashMap<CrsRef, (Vec<Candidate>, Vec<Candidate>)> = HashMap::new();
        let mut with_interpolation = false;
        for vt in &vertical {
            let Some(interp) = self.interpolation_geographic(vt) else {
                continue;
            };
            let (src_to_interp, interp_to_target) = match cache.get(&interp) {
                Some(pair) => pair.clone(),
                None => {
                    // To the interpolation CRS in 2D, which leaves the
                    // height alone; on to the target in 3D, which carries
                    // the ellipsoidal height across datums.
                    let a = self.create_operations(h, interp);
                    let interp2d = self.crs.demote_to_2d(interp);
                    let interp3d = self.crs.promote_to_3d(interp2d);
                    // Not a horizontal grid shift alone (PROJ's
                    // `HORIZONTAL_SHIFT_GTIFF`, what NTv2, NTv1 and NADCON
                    // become with PROJ-data's grids): it would take the
                    // interpolation CRS's ellipsoidal height for the
                    // target's.
                    let b: Vec<Candidate> = self
                        .create_operations(interp3d, t)
                        .into_iter()
                        .filter(|c| !horizontal_grid_only(c))
                        .collect();
                    cache.insert(interp, (a.clone(), b.clone()));
                    (a, b)
                }
            };
            let same_geographic = h == self.crs.demote_to_2d(t);
            let only_one = src_to_interp.len() == 1 && interp_to_target.len() == 1;
            for a in &src_to_interp {
                for b in &interp_to_target {
                    if !only_one
                        && ((same_geographic && self.datum_set(a) != self.datum_set(b))
                            || !self.compatible(a, b))
                    {
                        continue;
                    }
                    if let Some(op) = self.horiz_vertical_horiz(s, t, a, vt, b, true) {
                        res.push(op);
                        with_interpolation = true;
                    }
                }
            }
        }
        for vt in &vertical {
            if self.interpolation_geographic(vt).is_some()
                || (with_interpolation && !vt.has_ballpark())
            {
                continue;
            }
            for ht in &horizontal {
                if let Some(op) = self.horiz_vertical(s, t, ht, vt, true) {
                    res.push(op);
                }
            }
        }
    }

    /// `getInterpolationGeogCRS`: the interpolation CRS of the vertical
    /// transformation (its only transformation step, for a concatenation),
    /// or its geographic target, in 2D; none for a ballpark or a projected
    /// interpolation CRS.
    fn interpolation_geographic(&mut self, vt: &Candidate) -> Option<CrsRef> {
        let step = transformation_step(vt)?;
        if step.ballpark {
            return None;
        }
        let interp = match step_interpolation(step) {
            Some(code) => {
                let c = CrsRef::Epsg(code);
                if !self.crs.kind(c).is_geographic() {
                    return None;
                }
                c
            }
            None => {
                if !self.crs.kind(step.dst).is_geographic() {
                    return None;
                }
                step.dst
            }
        };
        Some(self.crs.demote_to_2d(interp))
    }

    /// The geodetic datums the transformations of an operation start and
    /// end on (`makeDatumSet`).
    fn datum_set(&mut self, op: &Candidate) -> std::collections::BTreeSet<String> {
        let r = registry();
        let mut set = std::collections::BTreeSet::new();
        let steps: Vec<&Single> = if op.steps.len() > 1 {
            op.steps.iter().collect()
        } else {
            vec![&op.steps[0]]
        };
        for step in steps {
            if !is_transformation(step) {
                continue;
            }
            for c in [step.src, step.dst] {
                if let Some(name) = self
                    .crs
                    .datum(c)
                    .and_then(|d| r.datums.get(&d))
                    .map(|d| d.name.clone())
                {
                    set.insert(name);
                }
            }
        }
        set
    }

    /// `useCompatibleTransformationsForSameSourceTarget`: where the two
    /// operations have a transformation between the same two CRSs (either
    /// way), it is the same one.
    fn compatible(&mut self, a: &Candidate, b: &Candidate) -> bool {
        let strip = |n: &str| {
            [
                " (2D)",
                " (geographic3D horizontal)",
                " (geog2D)",
                " (geog3D)",
            ]
            .iter()
            .fold(n.to_owned(), |acc, p| acc.replace(p, ""))
        };
        let subs = |c: &Candidate| -> Vec<Single> {
            if c.steps.len() > 1 {
                c.steps.clone()
            } else {
                vec![c.steps[0].clone()]
            }
        };
        for sa in subs(a) {
            if !is_transformation(&sa) {
                continue;
            }
            let a_name = strip(&sa.name);
            let (a_src, a_dst) = (self.crs.name(sa.src), self.crs.name(sa.dst));
            for sb in subs(b) {
                if !is_transformation(&sb) {
                    continue;
                }
                let (b_src, b_dst) = (self.crs.name(sb.src), self.crs.name(sb.dst));
                if a_src == b_src && a_dst == b_dst {
                    if a_name.starts_with(NULL_GEOGRAPHIC_OFFSET)
                        && sb.name.starts_with(NULL_GEOGRAPHIC_OFFSET)
                    {
                        continue;
                    }
                    if a_name != strip(&sb.name) {
                        return false;
                    }
                } else if a_src == b_dst && a_dst == b_src {
                    if a_name.starts_with(NULL_GEOGRAPHIC_OFFSET)
                        && sb.name.starts_with(NULL_GEOGRAPHIC_OFFSET)
                    {
                        continue;
                    }
                    let inverse = sb.inverse(&mut self.crs);
                    if a_name != strip(&inverse.name) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// `createHorizVerticalHorizPROJBased`: to the interpolation CRS, the
    /// vertical transformation there, on to the target; one operation.
    fn horiz_vertical_horiz(
        &mut self,
        s: CrsRef,
        t: CrsRef,
        a: &Candidate,
        vt: &Candidate,
        b: &Candidate,
        check_extent: bool,
    ) -> Option<Candidate> {
        let is_noop =
            |c: &Candidate| c.name.starts_with(NULL_GEOGRAPHIC_OFFSET) && !c.name.contains(" + ");
        let mut ops: Vec<&Candidate> = Vec::new();
        if !is_noop(a) {
            ops.push(a);
        }
        ops.push(vt);
        if !is_noop(b) {
            ops.push(b);
        }
        let b_inverse = b.inverse(&mut self.crs);
        let name = if ops.len() == 3 && equivalent(&b_inverse, a, &mut self.crs) {
            let horizontal = if !a.name.starts_with("Inverse of") {
                &a.name
            } else {
                &b.name
            };
            format!("{} using {horizontal}", vt.name)
        } else {
            ops.iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>()
                .join(" + ")
        };
        let (extent, empty) = intersect(ops.iter().map(|o| o.extent.clone()));
        if check_extent && empty {
            return None;
        }
        let accuracy = ops_accuracy(&ops, &mut self.crs);
        let save_horizontal = self.saves_horizontal(a, b);
        let mut parts = Vec::new();
        for o in [a, vt, b] {
            parts.extend(o.steps.iter().cloned());
        }
        Some(composite(
            s,
            t,
            name,
            parts,
            save_horizontal,
            accuracy,
            extent,
            ops.iter().any(|o| o.has_ballpark()),
        ))
    }

    /// Whether to keep the horizontal position across the vertical step:
    /// when the way to the interpolation CRS is one EPSG transformation and
    /// the route comes back to the source's horizontal CRS, the position is
    /// the source's, not the way there and back. (PROJ keeps it only after
    /// a Helmert, where the way back wouldn't cancel exactly; elsewhere the
    /// two agree to well below a millimetre.)
    fn saves_horizontal(&mut self, a: &Candidate, b: &Candidate) -> bool {
        let [step] = a.steps.as_slice() else {
            return false;
        };
        let SingleKind::Registry { code, .. } = step.kind else {
            return false;
        };
        registry().ops[&code].table != Table::Conversion && a.src == self.crs.demote_to_2d(b.dst)
    }

    /// `createHorizVerticalPROJBased`: the horizontal operation, then the
    /// vertical one in the target CRS.
    fn horiz_vertical(
        &mut self,
        s: CrsRef,
        t: CrsRef,
        ht: &Candidate,
        vt: &Candidate,
        check_extent: bool,
    ) -> Option<Candidate> {
        let mut parts: Vec<Single> = ht.steps.clone();
        parts.extend(vt.steps.iter().cloned());
        if ht.name.starts_with(NULL_GEOGRAPHIC_OFFSET) && !ht.name.contains(" + ") {
            let accuracy = if vt.accuracy < 0.0 && vt.steps.iter().all(|s| s.is_conversion) {
                0.0
            } else {
                vt.accuracy
            };
            return Some(composite(
                s,
                t,
                vt.name.clone(),
                parts,
                false,
                accuracy,
                vt.extent.clone(),
                vt.has_ballpark(),
            ));
        }
        let (extent, empty) = intersect([ht.extent.clone(), vt.extent.clone()].into_iter());
        if check_extent && empty {
            return None;
        }
        let accuracy = ops_accuracy(&[ht, vt], &mut self.crs);
        Some(composite(
            s,
            t,
            format!("{} + {}", ht.name, vt.name),
            parts,
            false,
            accuracy,
            extent,
            ht.has_ballpark() || vt.has_ballpark(),
        ))
    }

    /// `createOperationsToGeod`: through the geographic 3D CRS on the
    /// target's datum.
    pub(super) fn compound_to_geocentric(
        &mut self,
        s: CrsRef,
        t: CrsRef,
        res: &mut Vec<Candidate>,
    ) {
        let r = registry();
        let Some(interm) = self
            .crs
            .datum(t)
            .and_then(|d| r.crs_by_datum.get(&d))
            .and_then(|list| {
                list.iter().copied().find(|c| {
                    r.geodetic.get(c).map(|g| g.kind)
                        == Some(crate::registry::GeodeticKind::Geographic3d)
                })
            })
            .map(CrsRef::Epsg)
        else {
            return;
        };
        let first = self.create_operations(s, interm);
        let second = self.create_operations(interm, t);
        let Some(second) = second.into_iter().next() else {
            return;
        };
        for op in first {
            let op = op.with_crs(s, interm);
            if let Some(c) = concatenate(vec![op, second.clone()], true, &mut self.crs) {
                res.push(c);
            }
        }
    }

    /// `createOperationsCompoundToCompound`.
    pub(super) fn compound_to_compound(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) {
        let (Some((hs, vs)), Some((ht, vt))) = (self.crs.components(s), self.crs.components(t))
        else {
            return;
        };
        let (Some(src_geog), Some(dst_geog)) = (self.crs.geographic(hs), self.crs.geographic(ht))
        else {
            return;
        };
        let same_geographic = src_geog == dst_geog;
        let interm_src = self.crs.promote_to_3d(src_geog);
        let interm_dst = if same_geographic {
            interm_src
        } else {
            self.crs.promote_to_3d(dst_geog)
        };
        let geog_to_geog = self.create_operations(interm_src, interm_dst);
        let mut vertical: Vec<Candidate> = Vec::new();
        let mut tried_vertical = false;
        let mut through_geographic = false;
        if vs != vt {
            tried_vertical = true;
            vertical = self.create_operations(vs, vt);
            // Only a ballpark between the two vertical CRSs, or different
            // geographic CRSs with a known 3D version and a real
            // transformation between them ("GDA94 + AHD height" to "GDA2020
            // + AVWS height"): through geographic 3D CRSs.
            through_geographic = (vertical.len() == 1 && vertical[0].has_ballpark())
                || (!same_geographic
                    && matches!(interm_src, CrsRef::Epsg(_))
                    && matches!(interm_dst, CrsRef::Epsg(_))
                    && geog_to_geog.first().is_some_and(|op| !op.has_ballpark()));
        }
        if through_geographic {
            let src_to_geog = self.create_operations(s, interm_src);
            let geog_to_target = self.create_operations(interm_dst, t);
            let non_trivial = |ops: &[Candidate]| {
                ops.first()
                    .is_some_and(|op| !op.has_ballpark() || op.has_grids())
            };
            let (src_ok, target_ok) = (non_trivial(&src_to_geog), non_trivial(&geog_to_target));
            let mut best = (-1.0f64, 0usize);
            if src_ok && target_ok {
                for pass in 0..=1 {
                    if !res.is_empty() {
                        break;
                    }
                    for op1 in &src_to_geog {
                        if (pass == 0 && op1.has_ballpark())
                            || op1.name.contains(BALLPARK_VERTICAL_TRANSFORMATION)
                        {
                            continue;
                        }
                        for op2 in &geog_to_geog {
                            for op3 in &geog_to_target {
                                if (pass == 0 && op3.has_ballpark())
                                    || op3.name.contains(BALLPARK_VERTICAL_TRANSFORMATION)
                                {
                                    continue;
                                }
                                let parts = if same_geographic {
                                    vec![op1.clone(), op3.clone()]
                                } else {
                                    vec![op1.clone(), op2.clone(), op3.clone()]
                                };
                                if let Some(c) = concatenate(parts, true, &mut self.crs) {
                                    best = better(best, &c);
                                    res.push(c);
                                }
                            }
                        }
                    }
                }
            }
            if res.is_empty()
                || !matches!(interm_src, CrsRef::Epsg(_))
                || !matches!(interm_dst, CrsRef::Epsg(_))
            {
                if matches!(src_geog, CrsRef::Epsg(_))
                    && matches!(interm_dst, CrsRef::Epsg(_))
                    && target_ok
                {
                    let ops1 = self.create_operations(s, interm_dst);
                    if non_trivial(&ops1) {
                        self.two_steps(&ops1, &geog_to_target, res, &mut best);
                    }
                }
                if matches!(dst_geog, CrsRef::Epsg(_))
                    && matches!(interm_src, CrsRef::Epsg(_))
                    && src_ok
                {
                    let ops2 = self.create_operations(interm_src, t);
                    if non_trivial(&ops2) {
                        self.two_steps(&src_to_geog, &ops2, res, &mut best);
                    }
                }
            }
            if !res.is_empty() {
                return;
            }
            if !tried_vertical {
                vertical = self.create_operations(vs, vt);
            }
        }
        for vtr in &vertical {
            let interp = operation_interpolation(vtr)
                .map(CrsRef::Epsg)
                .unwrap_or(src_geog);
            let ops_src = self.create_operations(hs, interp);
            let ops_dst = self.create_operations(interp, ht);
            for a in &ops_src {
                for b in &ops_dst {
                    if let Some(op) = self.horiz_vertical_horiz(s, t, a, vtr, b, true) {
                        res.push(op);
                    }
                }
            }
            let ballpark_name = format!(
                "Transformation from {} to {} ({BALLPARK_VERTICAL_TRANSFORMATION})",
                self.crs.name(vs),
                self.crs.name(vt)
            );
            if vertical.len() == 1
                && vtr.has_ballpark()
                && self.crs.kind(hs) == CrsKind::Projected
                && self.crs.kind(ht) == CrsKind::Projected
                && vtr.name == ballpark_name
            {
                // "MGI 1901 / Slovene National Grid + SVS2000 height" to
                // "Slovenia 1996 / Slovene National Grid + SVS2010 height":
                // the registry's operations between the projected CRSs, with
                // a ballpark for the heights (so never kept here).
                let mut horizontal = Vec::new();
                self.query_registry(hs, ht, &mut horizontal);
                for oh in horizontal {
                    let mut parts = oh.steps.clone();
                    parts.extend(vtr.steps.iter().cloned());
                    let (extent, _) =
                        intersect([oh.extent.clone(), vtr.extent.clone()].into_iter());
                    res.push(composite(
                        s,
                        t,
                        format!("{} + {}", oh.name, vtr.name),
                        parts,
                        false,
                        -1.0,
                        extent,
                        true,
                    ));
                }
            }
        }
        if vertical.is_empty() {
            for op in self.create_operations(hs, ht) {
                res.push(op.with_crs(s, t));
            }
        }
    }

    /// The `createOpsInTwoSteps` lambda: replaces the results when its own
    /// best is better.
    fn two_steps(
        &mut self,
        ops1: &[Candidate],
        ops2: &[Candidate],
        res: &mut Vec<Candidate>,
        best: &mut (f64, usize),
    ) {
        let mut res2 = Vec::new();
        let mut best2 = (-1.0f64, 0usize);
        for pass in 0..=1 {
            if !res2.is_empty() {
                break;
            }
            for op1 in ops1 {
                if (pass == 0 && op1.has_ballpark())
                    || op1.name.contains(BALLPARK_VERTICAL_TRANSFORMATION)
                {
                    continue;
                }
                for op2 in ops2 {
                    if (pass == 0 && op2.has_ballpark())
                        || op2.name.contains(BALLPARK_VERTICAL_TRANSFORMATION)
                    {
                        continue;
                    }
                    if let Some(c) =
                        concatenate(vec![op1.clone(), op2.clone()], true, &mut self.crs)
                    {
                        best2 = better(best2, &c);
                        res2.push(c);
                    }
                }
            }
        }
        if best2.0 >= 0.0
            && (best.0 < 0.0 || best2.0 < best.0 || (best2.0 == best.0 && best2.1 < best.1))
        {
            *best = best2;
            *res = res2;
        }
    }
}

/// A single horizontal grid shift (NTv2, NTv1, NADCON) on a PROJ-data grid.
fn horizontal_grid_only(c: &Candidate) -> bool {
    let [step] = c.steps.as_slice() else {
        return false;
    };
    let SingleKind::Registry { code, .. } = step.kind else {
        return false;
    };
    let op = &registry().ops[&code];
    matches!(op.method, 9613..=9615)
        && op
            .grid_name()
            .is_some_and(|n| crate::grid::lookup(n, &op.name).is_some())
}

/// The better of the best so far and a new operation: known accuracy
/// first, then the lower, then fewer steps.
fn better(best: (f64, usize), c: &Candidate) -> (f64, usize) {
    let (accuracy, steps) = (c.accuracy, c.steps.len());
    if accuracy >= 0.0
        && (best.0 < 0.0 || accuracy < best.0 || (accuracy == best.0 && steps < best.1))
    {
        (accuracy, steps)
    } else {
        best
    }
}

/// PROJ's ballpark between vertical CRSs (or to a geographic CRS's
/// ellipsoidal height): unknown accuracy.
fn ballpark_vertical(s: CrsRef, t: CrsRef, name: String, extent: Extent) -> Single {
    Single {
        kind: SingleKind::BallparkVertical,
        src: s,
        dst: t,
        name,
        accuracy: -1.0,
        extent: Some(extent),
        is_conversion: false,
        identified: false,
        ballpark: true,
    }
}

/// A transformation (not a conversion): a registry transformation or a
/// ballpark.
fn is_transformation(s: &Single) -> bool {
    match s.kind {
        SingleKind::Registry { code, .. } => registry().ops[&code].table != Table::Conversion,
        SingleKind::BallparkVertical | SingleKind::EnsembleMember => true,
        _ => false,
    }
}

/// The only transformation of an operation: itself, or a concatenation's
/// single transformation step.
fn transformation_step(c: &Candidate) -> Option<&Single> {
    let mut found = None;
    for s in &c.steps {
        if is_transformation(s) {
            if found.is_some() {
                return None;
            }
            found = Some(s);
        }
    }
    found
}

/// A registry step's interpolation CRS.
fn step_interpolation(s: &Single) -> Option<u32> {
    let SingleKind::Registry { code, .. } = s.kind else {
        return None;
    };
    let op = &registry().ops[&code];
    op.param(INTERPOLATION_CRS)
        .or_else(|| op.param(HORIZONTAL_CRS))
        .map(|c| c as u32)
}

/// An operation's interpolation CRS: a single operation's, or the one a
/// concatenation's steps agree on (`ConcatenatedOperation`: the first that
/// has one sets it; a later step without it or with another clears it).
fn operation_interpolation(c: &Candidate) -> Option<u32> {
    if let [step] = c.steps.as_slice() {
        return step_interpolation(step);
    }
    let mut found: Option<u32> = None;
    for s in &c.steps {
        let own = step_interpolation(s);
        match (found, own) {
            (None, own) => found = own,
            (Some(f), Some(o)) if f == o => {}
            _ => return None,
        }
    }
    found
}

/// PROJ's `_isEquivalentTo` with its `EQUIVALENT` criterion, for the
/// operations it compares here: single operations with the same method and
/// parameters, or two Helmerts with the same seven parameters (in the
/// Position Vector convention). The CRSs don't take part.
fn equivalent(a: &Candidate, b: &Candidate, crs: &mut super::super::crs::CrsCache) -> bool {
    // Concatenations: the same number of steps, each equivalent.
    a.steps.len() == b.steps.len()
        && a.steps
            .iter()
            .zip(&b.steps)
            .all(|(x, y)| equivalent_steps(x, y, crs))
}

fn equivalent_steps(x: &Single, y: &Single, crs: &mut super::super::crs::CrsCache) -> bool {
    // A change on one datum is a method without parameters (or a zero
    // offset): the same kind of change is equivalent, whatever the CRSs'
    // dimensions.
    if x.kind == SingleKind::SameDatum && y.kind == SingleKind::SameDatum {
        let shape = |s: &Single, crs: &mut super::super::crs::CrsCache| {
            (
                crs.kind(s.src) == CrsKind::Geocentric,
                crs.kind(s.dst) == CrsKind::Geocentric,
            )
        };
        return shape(x, crs) == shape(y, crs);
    }
    let (
        SingleKind::Registry {
            code: cx,
            inverse: ix,
        },
        SingleKind::Registry {
            code: cy,
            inverse: iy,
        },
    ) = (&x.kind, &y.kind)
    else {
        return x.kind == y.kind && x.src == y.src && x.dst == y.dst;
    };
    let r = registry();
    let (ox, oy) = (&r.ops[cx], &r.ops[cy]);
    if let (Some(p), Some(q)) = (towgs84(ox, *ix), towgs84(oy, *iy)) {
        return p
            .iter()
            .zip(q)
            .all(|(p, q)| (p - q).abs() <= 1e-10 * p.abs());
    }
    ox.method == oy.method && ox.params == oy.params && ix == iy
}

/// `Transformation::getTOWGS84Parameters` of a Helmert (translations,
/// Position Vector, Coordinate Frame; geocentric, geog2D, geog3D domains):
/// translations in metres, rotations in the Position Vector convention,
/// scale difference; an inverse negates them all, as PROJ inverts them.
fn towgs84(op: &crate::registry::Operation, inverse: bool) -> Option<[f64; 7]> {
    let rotations = match op.method {
        1031 | 9603 | 1035 => 0.0,
        1033 | 9606 | 1037 => 1.0,
        1032 | 9607 | 1038 | 1149 => -1.0,
        _ => return None,
    };
    let p = |code: u32| op.param(code).unwrap_or(0.0);
    let mut v = [
        p(8605),
        p(8606),
        p(8607),
        rotations * p(8608),
        rotations * p(8609),
        rotations * p(8610),
        if rotations == 0.0 { 0.0 } else { p(8611) },
    ];
    if inverse {
        v.iter_mut().for_each(|x| *x = -*x);
    }
    Some(v)
}

/// The intersection of the areas, and whether it is empty (`getExtent` of a
/// list of operations).
fn intersect(extents: impl Iterator<Item = Option<Extent>>) -> (Option<Extent>, bool) {
    let mut acc: Option<Extent> = None;
    for e in extents.flatten() {
        acc = match acc {
            None => Some(e),
            Some(a) => match a.intersection(&e) {
                Some(x) => Some(x),
                None => return (None, true),
            },
        };
    }
    (acc, false)
}

/// `getAccuracy` of a list of operations.
fn ops_accuracy(ops: &[&Candidate], crs: &mut super::super::crs::CrsCache) -> f64 {
    let steps: Vec<Single> = ops
        .iter()
        .map(|o| {
            let mut step = o.steps[0].clone();
            step.accuracy = o.accuracy;
            step.name = o.name.clone();
            (step.src, step.dst) = (o.src, o.dst);
            step
        })
        .collect();
    steps_accuracy(&steps, crs)
}

/// One operation PROJ builds from others.
#[allow(clippy::too_many_arguments)]
fn composite(
    s: CrsRef,
    t: CrsRef,
    name: String,
    parts: Vec<Single>,
    save_horizontal: bool,
    accuracy: f64,
    extent: Option<Extent>,
    ballpark: bool,
) -> Candidate {
    Candidate::single(Single {
        kind: SingleKind::Composite {
            parts,
            save_horizontal,
        },
        src: s,
        dst: t,
        name,
        accuracy,
        extent,
        is_conversion: false,
        identified: false,
        ballpark,
    })
}
