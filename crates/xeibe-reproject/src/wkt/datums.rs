//! EPSG datums by name, for WKT that gives a datum without an id
//! (`docs/crs-wkt.md`, decision 4). Names match as PROJ matches them
//! ([`crate::name::equivalent`]), against EPSG's names and aliases.

use std::collections::HashMap;
use std::sync::OnceLock;

use xeibe_crs::{DATUM_ALIASES, DATUMS, DatumKind, DatumName};

use crate::name::canonical;

/// Traditional names of three ensembles, which PROJ writes for them where
/// ensembles can't be written (WKT1, WKT2 2015).
const TRADITIONAL: &[(&str, u32)] = &[
    ("World Geodetic System 1984", 6326),
    ("European Terrestrial Reference System 1989", 6258),
    ("Greenland 1996", 1421),
];

fn index() -> &'static HashMap<String, Vec<u32>> {
    static INDEX: OnceLock<HashMap<String, Vec<u32>>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index: HashMap<String, Vec<u32>> = HashMap::new();
        let names = DATUMS
            .iter()
            .map(|d| (d.code, d.name))
            .chain(DATUM_ALIASES.iter().copied())
            .chain(TRADITIONAL.iter().map(|&(n, c)| (c, n)));
        for (code, name) in names {
            let codes = index.entry(canonical(name, true)).or_default();
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
        index
    })
}

/// The EPSG datum (or ensemble) of this kind that `name` names: the only
/// one, or the only one not deprecated.
pub(crate) fn by_name(name: &str, kind: DatumKind) -> Option<&'static DatumName> {
    let found = all_by_name(name, kind);
    match found[..] {
        [one] => Some(one),
        _ => {
            let live: Vec<_> = found.into_iter().filter(|d| !d.deprecated).collect();
            match live[..] {
                [one] => Some(one),
                _ => None,
            }
        }
    }
}

/// Every EPSG datum (or ensemble) of this kind that `name` names, by its
/// own name or an alias: a deprecated datum shares its name with the one
/// that replaced it (EPSG:6640 and EPSG:1047), or lives on as its alias.
pub(crate) fn all_by_name(name: &str, kind: DatumKind) -> Vec<&'static DatumName> {
    index()
        .get(&canonical(name, true))
        .into_iter()
        .flatten()
        .filter_map(|&c| xeibe_crs::datum(c))
        .filter(|d| d.kind == kind)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdal_and_traditional_names() {
        let code = |n| by_name(n, DatumKind::Geodetic).map(|d| d.code);
        assert_eq!(code("Ordnance_Survey_of_Great_Britain_1936"), Some(6277));
        assert_eq!(
            code("European_Terrestrial_Reference_System_1989"),
            Some(6258)
        );
        assert_eq!(code("World Geodetic System 1984"), Some(6326));
        // EPSG:6747 "Greenland 1996" is deprecated; the GR96 ensemble isn't.
        assert_eq!(code("Greenland 1996"), Some(1421));
        assert_eq!(code("not a datum"), None);
        assert_eq!(
            by_name("Mean Sea Level", DatumKind::Vertical).map(|d| d.code),
            Some(5100)
        );
    }
}
