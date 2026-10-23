//! When two datums count as the same, and when a datum ensemble stands in for
//! one of its members.
//!
//! Datums are compared by EPSG code. PROJ compares names instead, through
//! EPSG's aliases and its own, so a datum once named like another is the same
//! to it (EUREF-FIN, once "European Terrestrial Reference System 1989", is
//! ETRS89). xeibe uses EPSG's ensembles for that: an ensemble is defined as
//! its members, which it doesn't tell apart beyond its stated accuracy, so
//! coordinates pass between a member and its ensemble unchanged, good to that
//! accuracy (`docs/reprojection.md`, level 2).

use crate::registry::registry;

/// Whether two datums (or ensembles), by EPSG code, are the same: two CRSs
/// on them differ by a conversion, not a transformation.
pub(crate) fn same(a: u32, b: u32) -> bool {
    a == b
}

/// When one datum is an ensemble and the other one of its members: the
/// ensemble's accuracy in metres.
pub(crate) fn ensemble_member(a: u32, b: u32) -> Option<f64> {
    let datums = &registry().datums;
    let member_of = |ensemble: u32, member: u32| {
        let d = datums.get(&ensemble)?;
        d.members.as_ref()?.contains(&member).then_some(())?;
        d.ensemble_accuracy
    };
    member_of(a, b).or_else(|| member_of(b, a))
}

/// The members of an ensemble, and the ensembles a datum is a member of.
pub(crate) fn relatives(code: u32) -> Vec<u32> {
    let datums = &registry().datums;
    let mut out: Vec<u32> = datums
        .get(&code)
        .and_then(|d| d.members.clone())
        .unwrap_or_default();
    out.extend(
        datums
            .iter()
            .filter(|(_, d)| d.members.as_ref().is_some_and(|m| m.contains(&code)))
            .map(|(&c, _)| c),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensemble_members_pass_at_the_ensemble_accuracy() {
        // EUREF-FIN (1391) and ETRF2000-PL (1305) are members of ETRS89
        // (6258), good to 0.1 m; WGS 84 (G2139) (1309) of WGS 84 (6326), to 2 m.
        assert_eq!(ensemble_member(1391, 6258), Some(0.1));
        assert_eq!(ensemble_member(6258, 1305), Some(0.1));
        assert_eq!(ensemble_member(1309, 6326), Some(2.0));
        assert_eq!(ensemble_member(1391, 1305), None);
        assert_eq!(ensemble_member(6258, 6326), None);
        assert!(same(6258, 6258) && !same(1391, 6258));
    }
}
