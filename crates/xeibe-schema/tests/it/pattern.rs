//! `PathPattern`: globs over element paths, used by overrides and the
//! `force_list`/`force_scalar` options (`docs/schema-inference.md` §3.5).

use xeibe_schema::PathPattern;

fn pattern(raw: &str) -> PathPattern {
    PathPattern::parse(raw).unwrap_or_else(|e| panic!("parsing {raw:?}: {e}"))
}

#[test]
fn matches_a_path_below_a_layer() {
    let p = pattern("AD_PunktAdresowy/idIIP");
    assert!(p.matches("AD_PunktAdresowy", &["idIIP"]));
    assert!(!p.matches("AD_PunktAdresowy", &["idIIP", "lokalnyId"]));
    assert!(!p.matches("AD_UlicaPlac", &["idIIP"]));
}

#[test]
fn a_star_matches_one_step_and_a_double_star_matches_any() {
    let one_step = pattern("*/area");
    assert!(one_step.matches("Parcel", &["area"]));
    assert!(!one_step.matches("Parcel", &["owner", "area"]));

    let any = pattern("**/@uom");
    assert!(any.matches("Parcel", &["@uom"]));
    assert!(any.matches("Parcel", &["area", "@uom"]));
    assert!(any.matches("Parcel", &["owner", "area", "@uom"]));
    assert!(!any.matches("Parcel", &["area"]));
}

#[test]
fn patterns_may_be_namespace_qualified() {
    let p = pattern("{https://geoportal.gov.pl/schemas/prgad/1.0}*/**");
    assert!(p.matches("{https://geoportal.gov.pl/schemas/prgad/1.0}AD_PunktAdresowy", &["idIIP"]));
    assert!(!p.matches("{http://example.com/app}Parcel", &["idIIP"]));
}

#[test]
fn more_specific_patterns_win() {
    // The winning override is the most specific match.
    let specific = pattern("Parcel/area");
    let wildcard = pattern("*/area");
    let anything = pattern("**");
    assert!(specific.specificity() > wildcard.specificity());
    assert!(wildcard.specificity() > anything.specificity());
}

#[test]
fn an_invalid_pattern_is_an_error() {
    assert!(PathPattern::parse("{unclosed/area").is_err());
    assert!(PathPattern::parse("").is_err());
}
