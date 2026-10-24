//! The search for candidate operations between two CRSs.
//!
//! Ported from PROJ 9.8.1 (MIT, see PROJ-NOTICE.md):
//! `coordinateoperationfactory.cpp` (`createOperations`,
//! `createOperationsFromDatabase`, `findOpsInRegistryDirect`,
//! `findsOpsInRegistryWithIntermediate`, `createOperationsWithDatumPivot`,
//! `createOperationsGeodToGeod`, `createOperationsDerivedTo`,
//! `hasTooSmallAreas`) and `factory.cpp`
//! (`createFromCoordinateReferenceSystemCodes`,
//! `createFromCRSCodesWithIntermediates`,
//! `createBetweenGeodeticCRSWithDatumBasedIntermediates`), with the EPSG
//! registry in place of PROJ's SQLite database, for EPSG operations only,
//! ballpark operations disallowed, and grids available when PROJ-data
//! publishes them (PROJ's `KNOWN_AVAILABLE`, its choice with network access)
//! or never ([`Grids::None`], PROJ without grids).
//!
//! PROJ's SQL queries return rows in an order SQLite leaves open where their
//! `ORDER BY` ties; here ties keep registry order (operation code, then
//! usage). The selection differential shows whether that ever matters.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::Selection;
use super::candidate::{
    Candidate, Single, SingleKind, concatenate, ensemble_member, projection, registry_candidate,
    same_datum,
};
use super::crs::{CrsCache, CrsKind, CrsRef};
use super::filter::{Areas, filter_out, is_exportable, is_null_transformation};
use crate::extent::Extent;
use crate::grid::Grids;
use crate::registry::{Operation, PROJ_CODES, registry};

mod vertical;

/// PROJ's recursion limit for `createOperations`.
const MAX_RECURSION: usize = 10;

/// PROJ's `useOnlyIntermediateCRSThroughDatumEnsembleNullOp`: every
/// operation is a concatenation with a step through one of PROJ's null
/// operations between a datum ensemble and a member. PROJ then also looks
/// through other datums' CRSs, which may hold a better route: WGS 84 (G1674)
/// to GDA94 through ITRF2008 (0.04 m) rather than through WGS 84 (5 m).
fn only_through_ensemble_operations(ops: &[Candidate]) -> bool {
    ops.iter()
        .all(|op| op.steps.len() > 1 && through_ensemble_operation(op))
}

/// Whether a step is one of PROJ's null operations between a datum
/// ensemble and a member.
fn through_ensemble_operation(op: &Candidate) -> bool {
    op.leaves()
        .iter()
        .any(|s| matches!(s.kind, SingleKind::Registry { code, .. } if code >= PROJ_CODES))
}

/// The one of PROJ's null operations between a datum ensemble and a member
/// that joins the two CRSs (`crate::registry`), and whether it runs
/// inverted from `a` to `b`.
fn proj_ensemble_operation(a: CrsRef, b: CrsRef) -> Option<(u32, bool)> {
    let (CrsRef::Epsg(a), CrsRef::Epsg(b)) = (a, b) else {
        return None;
    };
    let r = registry();
    r.by_crs.get(&a)?.iter().find_map(|&code| {
        let op = &r.ops[&code];
        (code >= PROJ_CODES && (op.dst == Some(b) || op.src == Some(b)))
            .then_some((code, op.src == Some(b)))
    })
}

pub(crate) struct Search {
    pub(crate) crs: CrsCache,
    pub(crate) areas: Areas,
    recursion: usize,
    in_datum_pivot: bool,
    grids: Grids,
    /// PROJ's guards against the vertical searches calling themselves.
    in_geog_to_vert_intermediate: bool,
    in_vert_to_vert_intermediate: bool,
    in_geog_to_vert_alternative: bool,
}

impl Search {
    pub(crate) fn new(
        mut crs: CrsCache,
        src: CrsRef,
        dst: CrsRef,
        grids: Grids,
        selection: &Selection,
    ) -> Search {
        let extent1 = crs.extent(src);
        let extent2 = crs.extent(dst);
        Search {
            crs,
            areas: Areas::new(extent1, extent2, selection),
            recursion: 0,
            in_datum_pivot: false,
            grids,
            in_geog_to_vert_intermediate: false,
            in_vert_to_vert_intermediate: false,
            in_geog_to_vert_alternative: false,
        }
    }

    fn base(&mut self, c: CrsRef) -> Option<CrsRef> {
        self.crs.get(c).and_then(|f| f.base).map(CrsRef::Epsg)
    }

    /// PROJ's `isSameGeodeticDatum`.
    fn same_datum(&mut self, a: CrsRef, b: CrsRef) -> bool {
        match (self.crs.datum(a), self.crs.datum(b)) {
            (Some(da), Some(db)) => crate::datum::same(da, db),
            _ => false,
        }
    }

    fn inverse_all(&mut self, list: Vec<Candidate>) -> Vec<Candidate> {
        list.iter().map(|c| c.inverse(&mut self.crs)).collect()
    }

    /// `Private::createOperations`.
    pub(crate) fn create_operations(&mut self, s: CrsRef, t: CrsRef) -> Vec<Candidate> {
        if self.recursion == MAX_RECURSION {
            return Vec::new();
        }
        self.recursion += 1;
        let res = self.create_operations_inner(s, t);
        self.recursion -= 1;
        res
    }

    fn create_operations_inner(&mut self, s: CrsRef, t: CrsRef) -> Vec<Candidate> {
        let (ks, kt) = (self.crs.kind(s), self.crs.kind(t));
        let derived_src = ks == CrsKind::Projected;
        let derived_dst = kt == CrsKind::Projected;
        let mut res = Vec::new();
        let base_s = if derived_src { self.base(s) } else { None };
        let base_t = if derived_dst { self.base(t) } else { None };
        if (!derived_src || base_s != Some(t))
            && (!derived_dst || base_t != Some(s))
            && self.query_database(s, t, &mut res)
        {
            return res;
        }
        if ks.is_geodetic() && kt.is_geodetic() {
            self.geod_to_geod(s, t, &mut res);
            return res;
        }
        if derived_src {
            self.derived_to(s, t, &mut res);
            return res;
        }
        if derived_dst {
            let reverse = self.create_operations(t, s);
            return self.inverse_all(reverse);
        }
        let (vs, vt) = (ks == CrsKind::Vertical, kt == CrsKind::Vertical);
        let (cs, ct) = (ks == CrsKind::Compound, kt == CrsKind::Compound);
        if vs && vt {
            self.vert_to_vert(s, t, &mut res);
        } else if vs && kt.is_geographic() {
            self.vert_to_geog(s, t, &mut res);
        } else if vt && ks.is_geographic() {
            let reverse = self.create_operations(t, s);
            return self.inverse_all(reverse);
        } else if cs && kt.is_geographic() {
            self.compound_to_geog(s, t, &mut res);
        } else if cs && kt == CrsKind::Geocentric {
            self.compound_to_geocentric(s, t, &mut res);
        } else if ks.is_geodetic() && ct {
            let reverse = self.create_operations(t, s);
            return self.inverse_all(reverse);
        } else if cs && ct {
            self.compound_to_compound(s, t, &mut res);
        }
        res
    }

    /// `createOperationsFromDatabase` for geodetic and projected CRSs, plus
    /// xeibe's identity between a datum ensemble and one of its members, and
    /// routes through a member (`via_realizations`). They compete with what
    /// EPSG publishes.
    fn query_database(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) -> bool {
        let done = self.query_registry(s, t, res);
        // Not where PROJ's own null operation joins the two CRSs: the same
        // coordinates, under PROJ's name.
        if proj_ensemble_operation(s, t).is_none()
            && let Some(step) = ensemble_member(&mut self.crs, s, t)
        {
            res.push(Candidate::single(step));
        }
        res.extend(self.via_realizations(s, t));
        done
    }

    /// Routes through a national realization: when one side is on a datum
    /// ensemble, an EPSG operation between the other side and a CRS on one of
    /// the ensemble's members, then the member identity. EPSG has replaced
    /// many operations to ETRS89 with ones to a country's realization of it
    /// ("RT90 to ETRS89-SWE [SWEREF 99] (1)", 0.1 m, replaces "RT90 to
    /// ETRS89 (1)", 0.5 m); PROJ doesn't look for these routes while a direct
    /// operation exists.
    fn via_realizations(&mut self, s: CrsRef, t: CrsRef) -> Vec<Candidate> {
        let mut out = Vec::new();
        if self.in_datum_pivot || !self.crs.kind(s).is_geodetic() || !self.crs.kind(t).is_geodetic()
        {
            return out;
        }
        let (Some(ds), Some(dt)) = (self.crs.datum(s), self.crs.datum(t)) else {
            return out;
        };
        let r = registry();
        // The member CRSs of `ensemble`, other than those on `other`'s datum.
        let member_crss = |ensemble: u32, other: u32| -> Vec<CrsRef> {
            let members = r.datums.get(&ensemble).and_then(|d| d.members.as_ref());
            members
                .into_iter()
                .flatten()
                .filter(|&&m| m != other)
                .flat_map(|m| r.crs_by_datum.get(m).into_iter().flatten())
                .map(|&c| CrsRef::Epsg(c))
                .collect()
        };
        // EPSG's operations only: one of PROJ's null operations to the
        // member would come straight back.
        let mut non_empty = false;
        for mid in member_crss(dt, ds) {
            for first in self.find_ops_direct(s, mid, &mut non_empty) {
                if !through_ensemble_operation(&first)
                    && let Some(id) = self.ensemble_step(mid, t)
                    && let Some(c) = concatenate(vec![first, id], true, &mut self.crs)
                {
                    out.push(c);
                }
            }
        }
        for mid in member_crss(ds, dt) {
            for second in self.find_ops_direct(mid, t, &mut non_empty) {
                if !through_ensemble_operation(&second)
                    && let Some(id) = self.ensemble_step(s, mid)
                    && let Some(c) = concatenate(vec![id, second], true, &mut self.crs)
                {
                    out.push(c);
                }
            }
        }
        out
    }

    /// The step between a CRS on a datum ensemble and one on a member: PROJ's
    /// null operation between the two where there is one, else xeibe's
    /// identity. The same coordinates either way.
    fn ensemble_step(&mut self, a: CrsRef, b: CrsRef) -> Option<Candidate> {
        match proj_ensemble_operation(a, b) {
            Some((code, inverse)) => registry_candidate(code, inverse, &mut self.crs),
            None => ensemble_member(&mut self.crs, a, b).map(Candidate::single),
        }
    }

    fn query_registry(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) -> bool {
        self.vertical_prelude(s, t, res);
        if !res.is_empty() {
            return true;
        }
        let geod = self.crs.kind(s).is_geodetic() && self.crs.kind(t).is_geodetic();
        let vertical =
            self.crs.kind(s) == CrsKind::Vertical || self.crs.kind(t) == CrsKind::Vertical;
        let mut non_empty_before = false;
        *res = self.find_ops_direct(s, t, &mut non_empty_before);
        if self.has_perfect_accuracy(res) {
            return true;
        }
        let mut do_filter = false;
        let mut same = false;
        if vertical {
            self.vertical_database(s, t, res, &mut non_empty_before);
        } else if geod {
            same = self.same_datum(s, t);
            if res.is_empty() && !same && !self.in_datum_pivot {
                self.with_datum_pivot(res, s, t);
                do_filter = !res.is_empty();
            }
        }
        let found_instantiable = match res.len() {
            0 => false,
            1 => is_exportable(&res[0]),
            _ => true,
        };
        if res.len() == 1 && !found_instantiable {
            non_empty_before = false;
        }
        if !same && (res.is_empty() || !found_instantiable) && !non_empty_before {
            let more = self.find_ops_with_intermediate(s, t, false);
            res.extend(more);
            do_filter = !res.is_empty();
        }
        if !self.in_datum_pivot && geod && !same {
            let only_replaced = non_empty_before && res.iter().all(|op| op.replaced);
            if !non_empty_before || only_replaced {
                let too_small = self.has_too_small_areas(res);
                let all_need_time = res.iter().all(|op| op.requires_time);
                let dynamic = |c: CrsRef, search: &mut Search| {
                    search
                        .crs
                        .datum(c)
                        .and_then(|d| registry().datums.get(&d))
                        .is_some_and(|d| d.dynamic)
                };
                let neither_dynamic = !dynamic(s, self) && !dynamic(t, self);
                if res.is_empty()
                    || only_replaced
                    || too_small
                    || (all_need_time && neither_dynamic)
                    || only_through_ensemble_operations(res)
                {
                    let names: HashSet<String> = if too_small {
                        res.iter().map(|op| op.name.clone()).collect()
                    } else {
                        HashSet::new()
                    };
                    let more = self.find_ops_with_intermediate(s, t, true);
                    if too_small && !res.is_empty() {
                        res.extend(more.into_iter().filter(|op| !names.contains(&op.name)));
                    } else {
                        res.extend(more);
                    }
                    do_filter = !res.is_empty();
                }
            }
        }
        do_filter && self.has_perfect_accuracy(res)
    }

    /// `hasPerfectAccuracyResult`.
    fn has_perfect_accuracy(&self, res: &[Candidate]) -> bool {
        filter_out(res, &self.areas, true)
            .iter()
            .any(|op| op.accuracy == 0.0)
    }

    /// `hasTooSmallAreas`.
    fn has_too_small_areas(&self, ops: &[Candidate]) -> bool {
        if ops.is_empty() || ops.len() >= 10 {
            return false;
        }
        let pa = |e: Option<Extent>| e.map_or(0.0, |e| e.pseudo_area());
        if self.areas.user_interest
            && let Some(interest) = &self.areas.interest
        {
            let target = interest.pseudo_area();
            if target <= 0.0 {
                return false;
            }
            return ops.iter().all(|op| {
                let area = op
                    .extent
                    .as_ref()
                    .map_or(0.0, |x| pa(x.intersection(interest)));
                area <= 0.5 * target
            });
        }
        let (e1, e2) = (&self.areas.extent1, &self.areas.extent2);
        let target = match (e1, e2) {
            (Some(a), Some(b)) => pa(a.intersection(b)),
            (Some(a), None) | (None, Some(a)) => a.pseudo_area(),
            (None, None) => 0.0,
        };
        if target <= 0.0 {
            return false;
        }
        for op in ops {
            let area = match &op.extent {
                None => 0.0,
                Some(x) => match (e1, e2) {
                    (Some(a), Some(b)) => {
                        let (ia, ib) = (x.intersection(a), x.intersection(b));
                        let both = match (&ia, &ib) {
                            (Some(p), Some(q)) => pa(p.intersection(q)),
                            _ => 0.0,
                        };
                        pa(ia) + pa(ib) - both
                    }
                    (Some(a), None) | (None, Some(a)) => pa(x.intersection(a)),
                    (None, None) => x.pseudo_area(),
                },
            };
            if area > 0.5 * target {
                return false;
            }
        }
        true
    }

    /// `findOpsInRegistryDirect`: registry operations between the two CRSs,
    /// either way round, filtered (not sorted).
    fn find_ops_direct(
        &mut self,
        s: CrsRef,
        t: CrsRef,
        non_empty_before: &mut bool,
    ) -> Vec<Candidate> {
        *non_empty_before = false;
        let (CrsRef::Epsg(sc), CrsRef::Epsg(tc)) = (s, t) else {
            return Vec::new();
        };
        let list = self.crs_codes(sc, tc);
        if list.is_empty() {
            return Vec::new();
        }
        *non_empty_before = true;
        filter_out(&list, &self.areas, false)
    }

    /// The replacement of `op` (same CRSs, same table) present in `set`.
    /// PROJ ignores a replacement whose grid it doesn't know.
    fn superseded_in(op: &Operation, set: &HashSet<u32>) -> bool {
        let r = registry();
        op.superseded_by.iter().any(|(code, same, table)| {
            *same
                && *table == op.table
                && set.contains(code)
                && r.ops.get(code).is_none_or(|new| {
                    new.table != crate::registry::Table::Grid
                        || new
                            .grid_name()
                            .is_some_and(|n| crate::grid::lookup(n, &new.name).is_some())
                })
        })
    }

    fn intersects_crs_areas(&self, x: &Extent) -> bool {
        [&self.areas.extent1, &self.areas.extent2]
            .into_iter()
            .flatten()
            .all(|e| x.intersects(e))
    }

    /// `createFromCoordinateReferenceSystemCodes` with `tryReverseOrder`.
    fn crs_codes(&mut self, s: u32, t: u32) -> Vec<Candidate> {
        // A projected target whose base is the source: its conversion.
        if self.crs.kind(CrsRef::Epsg(t)) == CrsKind::Projected
            && self.base(CrsRef::Epsg(t)) == Some(CrsRef::Epsg(s))
            && let Some(conv) = projection(&mut self.crs, t, false)
        {
            return vec![Candidate::single(conv)];
        }
        let r = registry();
        struct Row {
            code: u32,
            extent: Extent,
            accuracy: Option<f64>,
        }
        let mut rows = Vec::new();
        let mut codes: Vec<u32> = r.by_crs.get(&s).cloned().unwrap_or_default();
        codes.sort_unstable();
        for code in codes {
            let op = &r.ops[&code];
            if op.deprecated || op.table == crate::registry::Table::Conversion {
                continue;
            }
            let forward = op.src == Some(s) && op.dst == Some(t);
            let backward = op.src == Some(t) && op.dst == Some(s);
            if !forward && !backward {
                continue;
            }
            if let Some(extent) = &op.area {
                rows.push(Row {
                    code,
                    extent: extent.clone(),
                    accuracy: op.accuracy,
                });
            }
        }
        rows.sort_by(|a, b| {
            b.extent
                .pseudo_area()
                .total_cmp(&a.extent.pseudo_area())
                .then(a.accuracy.is_none().cmp(&b.accuracy.is_none()))
                .then(
                    a.accuracy
                        .unwrap_or(0.0)
                        .total_cmp(&b.accuracy.unwrap_or(0.0)),
                )
        });
        let set: HashSet<u32> = rows.iter().map(|row| row.code).collect();
        let keep: Vec<&Row> = rows
            .iter()
            .filter(|row| !Self::superseded_in(&r.ops[&row.code], &set))
            .collect();
        let intersecting: Vec<bool> = keep
            .iter()
            .map(|row| self.intersects_crs_areas(&row.extent))
            .collect();
        let any = intersecting.iter().any(|&b| b);
        let mut out = Vec::new();
        for (row, ok) in keep.iter().zip(intersecting) {
            if any && !ok {
                continue;
            }
            let inverse = r.ops[&row.code].src != Some(s);
            if let Some(c) = registry_candidate(row.code, inverse, &mut self.crs)
                && c.grids_available(&self.grids)
            {
                out.push(c);
            }
        }
        out
    }

    /// `findsOpsInRegistryWithIntermediate`.
    fn find_ops_with_intermediate(
        &mut self,
        s: CrsRef,
        t: CrsRef,
        datum_based: bool,
    ) -> Vec<Candidate> {
        let (CrsRef::Epsg(sc), CrsRef::Epsg(tc)) = (s, t) else {
            return Vec::new();
        };
        let res = if datum_based {
            self.with_datum_based_intermediates(sc, tc)
        } else {
            let geographic_only =
                self.crs.kind(s).is_geographic() && self.crs.kind(t).is_geographic();
            self.with_intermediates(sc, tc, geographic_only)
        };
        if res.is_empty() {
            return res;
        }
        filter_out(&res, &self.areas, false)
    }

    /// PROJ's `useIrrelevantPivot`: a concatenated operation that passes
    /// through the source or target CRS on the way.
    fn irrelevant_pivot(code: u32, s: u32, t: u32) -> bool {
        let r = registry();
        let op = &r.ops[&code];
        if op.steps.len() < 2 {
            return false;
        }
        op.steps[..op.steps.len() - 1].iter().any(|step| {
            let st = &r.ops[step];
            // The step's target as PROJ resolved the chain; either end of a
            // step touching the source or target counts in practice.
            st.dst == Some(s) || st.dst == Some(t)
        })
    }

    fn check_pivot(&mut self, x: u32, s: u32, t: u32) -> bool {
        let (sn, tn, xn) = (
            self.crs.name(CrsRef::Epsg(s)),
            self.crs.name(CrsRef::Epsg(t)),
            self.crs.name(CrsRef::Epsg(x)),
        );
        if sn.starts_with("ETRF") && tn.starts_with("ETRF") && xn.starts_with("ITRF") {
            let date = |name: &str, prefix: &str| -> i32 {
                let digits: String = name[prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                let v: i32 = digits.parse().unwrap_or(0);
                if (80..=99).contains(&v) { v + 1900 } else { v }
            };
            let (sd, td, xd) = (date(&sn, "ETRF"), date(&tn, "ETRF"), date(&xn, "ITRF"));
            if sd > 0 && td > 0 && xd > 0 && (xd < sd.min(td) || xd > sd.max(td)) {
                return false;
            }
        }
        if sn.starts_with("NAD83(CSRS)")
            && tn.starts_with("NAD83(CSRS)")
            && (xn == "NAD83" || xn == "WGS 84")
        {
            return false;
        }
        true
    }

    /// `createFromCRSCodesWithIntermediates` with no user-given intermediate.
    fn with_intermediates(&mut self, s: u32, t: u32, geographic_only: bool) -> Vec<Candidate> {
        let r = registry();
        if s == t || !r.by_crs.contains_key(&s) || !r.by_crs.contains_key(&t) {
            return Vec::new();
        }
        // Datums no older than the older of the two, when both have dates.
        let min_date = if geographic_only {
            let date = |c: u32, search: &mut Search| {
                search
                    .crs
                    .datum(CrsRef::Epsg(c))
                    .and_then(|d| r.datums.get(&d))
                    .and_then(|d| d.publication_date.clone())
            };
            match (date(s, self), date(t, self)) {
                (Some(a), Some(b)) => Some(a.min(b)),
                _ => None,
            }
        } else {
            None
        };
        let intermediate_ok = |x: u32, search: &mut Search| -> bool {
            if !geographic_only {
                return true;
            }
            let Some(g) = r.geodetic.get(&x) else {
                return false;
            };
            if g.kind == crate::registry::GeodeticKind::Geocentric {
                return false;
            }
            match &min_date {
                None => true,
                Some(min) => {
                    let _ = search;
                    r.datums
                        .get(&g.datum)
                        .and_then(|d| d.publication_date.as_ref())
                        .is_none_or(|d| d >= min)
                }
            }
        };
        let live = |c: u32| -> Vec<u32> {
            let mut v: Vec<u32> = r
                .by_crs
                .get(&c)
                .into_iter()
                .flatten()
                .copied()
                .filter(|code| {
                    !r.ops[code].deprecated
                        && r.ops[code].table != crate::registry::Table::Conversion
                })
                .collect();
            v.sort_unstable();
            v
        };
        let ops_s = live(s);
        let ops_t = live(t);
        // The four ways two operations can meet at an intermediate CRS:
        // (first op runs from S, second op runs to T), each either way round.
        let mut out = Vec::new();
        for (inv1, inv2) in [(false, false), (false, true), (true, false), (true, true)] {
            struct Row {
                op1: u32,
                op2: u32,
                acc: (u32, f64),
            }
            let mut rows = Vec::new();
            for &op1 in &ops_s {
                let o1 = &r.ops[&op1];
                // op1 goes S -> X (forward) or X -> S (taken inverse).
                let (from, x) = if inv1 {
                    (o1.dst, o1.src)
                } else {
                    (o1.src, o1.dst)
                };
                if from != Some(s) {
                    continue;
                }
                let Some(x) = x else { continue };
                if x == s || x == t || !intermediate_ok(x, self) {
                    continue;
                }
                for &op2 in &ops_t {
                    let o2 = &r.ops[&op2];
                    let (via, to) = if inv2 {
                        (o2.dst, o2.src)
                    } else {
                        (o2.src, o2.dst)
                    };
                    if via != Some(x) || to != Some(t) || op1 == op2 {
                        continue;
                    }
                    let (Some(a1), Some(a2)) = (&o1.area, &o2.area) else {
                        continue;
                    };
                    if !a1.intersects(a2) {
                        continue;
                    }
                    let extra_ok = [&self.areas.extent1, &self.areas.extent2]
                        .into_iter()
                        .flatten()
                        .filter(|e| !e.is_world())
                        .all(|e| a1.intersects(e) && a2.intersects(e));
                    if !extra_ok {
                        continue;
                    }
                    let nulls = o1.accuracy.is_none() as u32 + o2.accuracy.is_none() as u32;
                    let sum = o1.accuracy.unwrap_or(0.0) + o2.accuracy.unwrap_or(0.0);
                    rows.push(Row {
                        op1,
                        op2,
                        acc: (nulls, sum),
                    });
                }
            }
            rows.sort_by(|a, b| a.acc.0.cmp(&b.acc.0).then(a.acc.1.total_cmp(&b.acc.1)));
            // Superseded operations whose replacement is among the rows.
            let set1: HashSet<u32> = rows.iter().map(|row| row.op1).collect();
            let set2: HashSet<u32> = rows.iter().map(|row| row.op2).collect();
            for row in rows {
                if Self::superseded_in(&r.ops[&row.op1], &set1)
                    || Self::superseded_in(&r.ops[&row.op2], &set2)
                {
                    continue;
                }
                if Self::irrelevant_pivot(row.op1, s, t) {
                    continue;
                }
                let o1 = &r.ops[&row.op1];
                let x = if inv1 { o1.src } else { o1.dst }.unwrap_or(0);
                if !self.check_pivot(x, s, t) {
                    continue;
                }
                if Self::irrelevant_pivot(row.op2, s, t) {
                    continue;
                }
                let (Some(c1), Some(c2)) = (
                    registry_candidate(row.op1, inv1, &mut self.crs),
                    registry_candidate(row.op2, inv2, &mut self.crs),
                ) else {
                    continue;
                };
                if let Some(c) = concatenate(vec![c1, c2], false, &mut self.crs)
                    && c.grids_available(&self.grids)
                {
                    out.push(c);
                }
            }
        }
        out
    }

    /// `createBetweenGeodeticCRSWithDatumBasedIntermediates`.
    fn with_datum_based_intermediates(&mut self, s: u32, t: u32) -> Vec<Candidate> {
        let r = registry();
        let (src, dst) = (CrsRef::Epsg(s), CrsRef::Epsg(t));
        if s == t || !self.crs.kind(src).is_geodetic() || !self.crs.kind(dst).is_geodetic() {
            return Vec::new();
        }
        let nad83_csrs = self.crs.name(src).starts_with("NAD83(CSRS)")
            && self.crs.name(dst).starts_with("NAD83(CSRS)");
        let same_datum_list = |c: u32, search: &mut Search| -> Vec<u32> {
            let datum = search.crs.datum(CrsRef::Epsg(c));
            match datum.and_then(|d| r.crs_by_datum.get(&d)) {
                Some(list) if !list.is_empty() => list.clone(),
                _ => vec![c],
            }
        };
        let list_s = same_datum_list(s, self);
        let list_t = same_datum_list(t, self);
        #[derive(Clone)]
        struct Trfm {
            code: u32,
            /// The registry operation runs towards the src/dst side.
            toward: bool,
            extent: Extent,
        }
        // Datum of the CRS at the far end -> operations.
        let mut map_s: BTreeMap<u32, Vec<Trfm>> = BTreeMap::new();
        let mut map_t: BTreeMap<u32, Vec<Trfm>> = BTreeMap::new();
        let mut all_codes: Vec<u32> = r.ops.keys().copied().collect();
        all_codes.sort_unstable();
        for (is_source, select_on_target) in
            [(true, true), (false, true), (true, false), (false, false)]
        {
            let list = if is_source { &list_s } else { &list_t };
            for &code in &all_codes {
                let op = &r.ops[&code];
                if op.deprecated || op.table == crate::registry::Table::Conversion {
                    continue;
                }
                // select_on_target: the op ends on the list, its source is the far end.
                let (near, far) = if select_on_target {
                    (op.dst, op.src)
                } else {
                    (op.src, op.dst)
                };
                let (Some(near), Some(far)) = (near, far) else {
                    continue;
                };
                if !list.contains(&near) {
                    continue;
                }
                let Some(g) = r.geodetic.get(&far) else {
                    continue;
                };
                if nad83_csrs && (g.name == "NAD83" || g.name == "WGS 84") {
                    continue;
                }
                let key = g.datum;
                if let Some(extent) = &op.area {
                    let trfm = Trfm {
                        code,
                        toward: select_on_target,
                        extent: extent.clone(),
                    };
                    if is_source {
                        map_s.entry(key).or_default().push(trfm);
                    } else {
                        map_t.entry(key).or_default().push(trfm);
                    }
                }
            }
        }
        let extra: Vec<Extent> = [&self.areas.extent1, &self.areas.extent2]
            .into_iter()
            .flatten()
            .filter(|e| !e.is_world())
            .cloned()
            .collect();
        let mut candidates: Vec<(Trfm, Trfm)> = Vec::new();
        let mut set: Vec<(u32, Trfm)> = Vec::new();
        let mut irrelevant: HashMap<u32, bool> = HashMap::new();
        for (key, list_src) in &map_s {
            let Some(list_tgt) = map_t.get(key) else {
                continue;
            };
            for a in list_src {
                if !extra.iter().all(|e| e.intersects(&a.extent)) {
                    continue;
                }
                for b in list_tgt {
                    if !a.extent.intersects(&b.extent)
                        || !extra.iter().all(|e| e.intersects(&b.extent))
                    {
                        continue;
                    }
                    let bad_a = *irrelevant
                        .entry(a.code)
                        .or_insert_with(|| Self::irrelevant_pivot(a.code, s, t));
                    if bad_a {
                        continue;
                    }
                    let bad_b = *irrelevant
                        .entry(b.code)
                        .or_insert_with(|| Self::irrelevant_pivot(b.code, s, t));
                    if bad_b {
                        continue;
                    }
                    candidates.push((a.clone(), b.clone()));
                    set.push((a.code, a.clone()));
                    set.push((b.code, b.clone()));
                }
            }
        }
        let codes: HashSet<u32> = set.iter().map(|(c, _)| *c).collect();
        let superseded: HashSet<u32> = codes
            .iter()
            .copied()
            .filter(|c| Self::superseded_in(&r.ops[c], &codes))
            .collect();
        let date = |c: CrsRef, search: &mut Search| {
            search
                .crs
                .datum(c)
                .and_then(|d| r.datums.get(&d))
                .and_then(|d| d.publication_date.clone())
        };
        let (ds, dt) = (date(src, self), date(dst, self));
        let most_ancient = match (ds, dt) {
            (Some(a), Some(b)) => Some(if b < a { b } else { a }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        const CUT_OFF_DATE: &str = "1900-01-01";
        let mut out = Vec::new();
        for (a, b) in candidates {
            if superseded.contains(&a.code) || superseded.contains(&b.code) {
                continue;
            }
            // op1 goes from the src side to the shared datum: inverse when it
            // ends on src's datum. op2 goes from the shared datum to the dst
            // side: inverse when it starts on dst's datum.
            let (Some(op1), Some(op2)) = (
                registry_candidate(a.code, a.toward, &mut self.crs),
                registry_candidate(b.code, !b.toward, &mut self.crs),
            ) else {
                continue;
            };
            if let Some(min) = &most_ancient {
                let mut older = false;
                for c in [op1.src, op1.dst, op2.src, op2.dst] {
                    if let Some(d) = date(c, self)
                        && d.as_str() < CUT_OFF_DATE
                        && min.as_str() > CUT_OFF_DATE
                    {
                        older = true;
                    }
                }
                if older {
                    continue;
                }
            }
            let mut steps = Vec::new();
            if op1.src != src {
                match self.conversion_without_database(src, op1.src) {
                    Some(c) => steps.push(c),
                    None => continue,
                }
            }
            let (m1, m2) = (op1.dst, op2.src);
            steps.push(op1);
            if m1 != m2 {
                match self.conversion_without_database(m1, m2) {
                    Some(c) => steps.push(c),
                    None => continue,
                }
            }
            let last = op2.dst;
            steps.push(op2);
            if last != dst {
                match self.conversion_without_database(last, dst) {
                    Some(c) => steps.push(c),
                    None => continue,
                }
            }
            if let Some(c) = concatenate(steps, false, &mut self.crs)
                && c.grids_available(&self.grids)
            {
                out.push(c);
            }
        }
        out
    }

    /// `CoordinateOperationFactory::createOperation` without a database,
    /// between CRSs on one datum: the first of PROJ's synthetic operations.
    fn conversion_without_database(&mut self, a: CrsRef, b: CrsRef) -> Option<Candidate> {
        if !self.same_datum(a, b) {
            return None;
        }
        let mut res = Vec::new();
        self.geod_to_geod(a, b, &mut res);
        res.into_iter().next()
    }

    /// `createOperationsGeodToGeod` for two geodetic CRSs, without the
    /// ballpark operations PROJ adds between different datums.
    fn geod_to_geod(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) {
        if !self.same_datum(s, t) {
            return;
        }
        let (ks, kt) = (self.crs.kind(s), self.crs.kind(t));
        if ks.is_geographic() && kt.is_geographic() {
            // Same datum: an axis order change, or a null geographic offset.
            let lat_first = |c: CrsRef| {
                xeibe_crs::get(c.code()).map(|r| r.first_axis == xeibe_crs::FirstAxis::NorthOrLat)
            };
            if lat_first(s) != lat_first(t) {
                let three = ks == CrsKind::Geographic3d || kt == CrsKind::Geographic3d;
                let name = if three {
                    "axis order change (geographic3D horizontal)"
                } else {
                    "axis order change (2D)"
                };
                res.push(Candidate::single(Single {
                    kind: SingleKind::SameDatum,
                    src: s,
                    dst: t,
                    name: name.to_owned(),
                    accuracy: 0.0,
                    extent: Some(crate::extent::world()),
                    is_conversion: true,
                    identified: true,
                    ballpark: false,
                }));
                return;
            }
        }
        if let Some(step) = same_datum(&mut self.crs, s, t) {
            res.push(Candidate::single(step));
        }
    }

    /// `createOperationsDerivedTo` for a projected source CRS.
    fn derived_to(&mut self, s: CrsRef, t: CrsRef, res: &mut Vec<Candidate>) {
        let CrsRef::Epsg(code) = s else { return };
        let Some(first) = projection(&mut self.crs, code, true) else {
            return;
        };
        let base = first.dst;
        let first = Candidate::single(first);
        if base == t {
            res.push(first);
            return;
        }
        for second in self.create_operations(base, t) {
            if let Some(c) = concatenate(vec![first.clone(), second], true, &mut self.crs) {
                res.push(c);
            }
        }
    }

    /// `createOperationsWithDatumPivot`.
    fn with_datum_pivot(&mut self, res: &mut Vec<Candidate>, s: CrsRef, t: CrsRef) {
        self.in_datum_pivot = true;
        self.with_datum_pivot_inner(res, s, t);
        self.in_datum_pivot = false;
    }

    fn with_datum_pivot_inner(&mut self, res: &mut Vec<Candidate>, s: CrsRef, t: CrsRef) {
        let r = registry();
        let (Some(ds), Some(dt)) = (self.crs.datum(s), self.crs.datum(t)) else {
            return;
        };
        let cand_s: Vec<CrsRef> = r
            .crs_by_datum
            .get(&ds)
            .into_iter()
            .flatten()
            .map(|c| CrsRef::Epsg(*c))
            .collect();
        let cand_t: Vec<CrsRef> = r
            .crs_by_datum
            .get(&dt)
            .into_iter()
            .flatten()
            .map(|c| CrsRef::Epsg(*c))
            .collect();
        let three_d = |k: CrsKind| matches!(k, CrsKind::Geographic3d | CrsKind::Geocentric);
        let both_3d = three_d(self.crs.kind(s)) && three_d(self.crs.kind(t));
        let type_of = |k: CrsKind| match k {
            CrsKind::Geographic2d => 0,
            CrsKind::Geographic3d => 1,
            _ => 2,
        };
        let (name_s, name_t) = (self.crs.name(s), self.crs.name(t));
        for iter in 0..3 {
            let only_direct = iter == 0;
            for &cs in &cand_s {
                if self.crs.name(cs) != name_s {
                    continue;
                }
                let type_source = type_of(self.crs.kind(cs));
                let src_mod = if both_3d {
                    self.crs.promote_to_3d(cs)
                } else {
                    cs
                };
                for &ct in &cand_t {
                    if self.crs.name(ct) != name_t {
                        continue;
                    }
                    let type_target = type_of(self.crs.kind(ct));
                    if iter == 1 && type_source != type_target {
                        continue;
                    }
                    if iter == 2 && type_source == type_target {
                        continue;
                    }
                    let first = self.create_operations(s, src_mod);
                    let Some(first) = first.into_iter().next() else {
                        continue;
                    };
                    self.pivot_transformations(res, s, t, cs, ct, first, only_direct, both_3d);
                    if !res.is_empty() {
                        return;
                    }
                }
            }
            for &cs in &cand_s {
                let same_src_name = self.crs.name(cs) == name_s;
                let src_mod = if both_3d {
                    self.crs.promote_to_3d(cs)
                } else {
                    cs
                };
                let first = self.create_operations(s, src_mod);
                let Some(first) = first.into_iter().next() else {
                    continue;
                };
                for &ct in &cand_t {
                    if same_src_name && self.crs.name(ct) == name_t {
                        continue;
                    }
                    self.pivot_transformations(
                        res,
                        s,
                        t,
                        cs,
                        ct,
                        first.clone(),
                        only_direct,
                        both_3d,
                    );
                    if !res.is_empty() {
                        return;
                    }
                }
            }
        }
    }

    /// The `createTransformations` lambda of `createOperationsWithDatumPivot`.
    #[allow(clippy::too_many_arguments)]
    fn pivot_transformations(
        &mut self,
        res: &mut Vec<Candidate>,
        s: CrsRef,
        t: CrsRef,
        cand_s: CrsRef,
        cand_t: CrsRef,
        first: Candidate,
        only_direct: bool,
        both_3d: bool,
    ) {
        let is_null_first = is_null_transformation(&first.name);
        let seconds = if only_direct {
            let mut dummy = false;
            self.find_ops_direct(cand_s, cand_t, &mut dummy)
        } else {
            self.create_operations(cand_s, cand_t)
        };
        let third_from = if both_3d {
            self.crs.promote_to_3d(cand_t)
        } else {
            cand_t
        };
        let thirds = self.create_operations(third_from, t);
        let Some(third) = thirds.into_iter().next() else {
            return;
        };
        let is_null_third = is_null_transformation(&third.name);
        for second in seconds {
            let identified = second.identified || second.steps.iter().any(|s| s.identified);
            if !identified {
                continue;
            }
            if second.steps.len() == 1 && second.name.starts_with("axis order change") {
                continue;
            }
            let mut second = second;
            if both_3d {
                for step in &mut second.steps {
                    step.src = self.crs.promote_to_3d(step.src);
                    step.dst = self.crs.promote_to_3d(step.dst);
                }
                second.src = self.crs.promote_to_3d(second.src);
                second.dst = self.crs.promote_to_3d(second.dst);
            }
            let mut parts = Vec::new();
            if !is_null_first {
                parts.push(first.clone());
            }
            parts.push(second);
            if !is_null_third {
                parts.push(third.clone());
            }
            // PROJ gives the first part the source CRS and the last the
            // target CRS (`setCRSs`): a single operation found between 3D
            // CRSs for 2D ones becomes 2D, which keeps the height.
            let head = parts.remove(0);
            let head_dst = head.dst;
            parts.insert(0, head.with_crs(s, head_dst));
            let tail = parts.pop().expect("a part");
            let tail_src = tail.src;
            parts.push(tail.with_crs(tail_src, t));
            if let Some(mut c) = concatenate(parts, true, &mut self.crs) {
                c.src = s;
                c.dst = t;
                res.push(c);
            }
        }
    }
}

/// `CoordinateOperationFactory::createOperations`: all candidates from `src`
/// to `dst`, filtered and sorted as PROJ would.
pub(crate) fn create_operations(
    src: CrsRef,
    dst: CrsRef,
    grids: Grids,
    selection: &Selection,
) -> (Vec<Candidate>, CrsCache) {
    let mut search = Search::new(CrsCache::default(), src, dst, grids, selection);
    let res = search.create_operations(src, dst);
    let areas = search.areas.clone();
    let mut crs = search.crs;
    let sorted = super::filter::filter_and_sort(&res, &areas, &mut crs);
    (sorted, crs)
}
