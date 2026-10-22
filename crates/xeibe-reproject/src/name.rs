//! Matching names the way PROJ matches them: `Identifier::canonicalizeName`
//! and `isEquivalentName`, ported from PROJ 9.8.1
//! `src/iso19111/metadata.cpp` (MIT, see PROJ-NOTICE.md).
//!
//! Two names match when they are equal after lower-casing, dropping
//! spaces, underscores and some punctuation, and a few rewrites: `1984`
//! and `84` are the same year, and (when bigger differences are allowed)
//! "zone" and "height" are dropped and "north"/"south" shortened. So GDAL's
//! `Ordnance_Survey_of_Great_Britain_1936` is EPSG's "Ordnance Survey of
//! Great Britain 1936".

/// Characters a name is compared without.
fn ignored(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'_' | b'-' | b'/' | b'(' | b')' | b'.' | b'&' | b','
    )
}

/// Whether `s` continues at `at` with `word`, ignoring ASCII case.
fn continues_with(s: &[u8], at: usize, word: &str) -> bool {
    s.get(at..at + word.len())
        .is_some_and(|w| w.eq_ignore_ascii_case(word.as_bytes()))
}

/// Accented letters PROJ reads as their base letter.
const ACCENTS: &[(&str, u8)] = &[
    ("á", b'a'),
    ("ä", b'a'),
    ("ě", b'e'),
    ("è", b'e'),
    ("é", b'e'),
    ("ë", b'e'),
    ("í", b'i'),
    ("ô", b'o'),
    ("ö", b'o'),
    ("ç", b'c'),
];

/// PROJ's canonical form of a name. With `bigger`, "zone" and "height"
/// after a separator are dropped and "north"/"south" after a separator or
/// a digit become "n"/"s".
pub(crate) fn canonical(name: &str, bigger: bool) -> String {
    let s = name.as_bytes();
    let digit = |i: usize| s.get(i).is_some_and(u8::is_ascii_digit);
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i].to_ascii_lowercase();
        // " + " between two names, as in compound CRS names.
        if c == b' ' && s.get(i + 1) == Some(&b'+') && s.get(i + 2) == Some(&b' ') {
            i += 3;
            continue;
        }
        // "19dd" as "dd", after a non-digit.
        if c == b'1'
            && out.last().is_some_and(|l| !l.is_ascii_digit())
            && s.get(i + 1) == Some(&b'9')
            && digit(i + 2)
            && digit(i + 3)
        {
            i += 2;
            continue;
        }
        if bigger && i > 0 {
            let after_separator = ignored(s[i - 1]);
            if after_separator
                && let Some(word) = ["zone", "height"].iter().find(|w| continues_with(s, i, w))
            {
                i += word.len();
                continue;
            }
            if (after_separator || s[i - 1].is_ascii_digit())
                && let Some(word) = ["north", "south"].iter().find(|w| continues_with(s, i, w))
            {
                out.push(c);
                i += word.len();
                continue;
            }
        }
        if c > 127
            && let Some((utf8, ascii)) = ACCENTS
                .iter()
                .find(|(u, _)| s[i..].starts_with(u.as_bytes()))
        {
            out.push(*ascii);
            i += utf8.len();
            continue;
        }
        if continues_with(s, i, "_IntlFeet") && i + "_IntlFeet".len() == s.len() {
            out.extend_from_slice(b"feet");
            break;
        }
        if !ignored(c) {
            out.push(c);
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// PROJ's `isEquivalentName`, with bigger differences allowed, as PROJ
/// compares the names of datums and other objects.
pub(crate) fn equivalent(a: &str, b: &str) -> bool {
    a == b || canonical(a, true) == canonical(b, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_proj_canonicalizes() {
        assert!(equivalent(
            "Ordnance_Survey_of_Great_Britain_1936",
            "Ordnance Survey of Great Britain 1936"
        ));
        assert!(equivalent("WGS 84", "WGS_1984"));
        assert!(equivalent(
            "Nouvelle Triangulation Francaise (Paris)",
            "Nouvelle_Triangulation_Francaise_Paris"
        ));
        assert!(equivalent(
            "Réseau Géodésique Français",
            "Reseau Geodesique Francais"
        ));
        assert_eq!(canonical("UTM zone 33N", true), "utm33n");
        assert_eq!(canonical("UTM zone 33 north", true), "utm33n");
        assert_eq!(canonical("EGM96 height", true), "egm96");
        assert_eq!(canonical("RD + NAP height", false), "rdnapheight");
        // A year only loses its "19" after a letter, not inside a number.
        assert_eq!(canonical("ED 1950", false), "ed50");
        assert_eq!(canonical("11950", false), "11950");
        assert!(!equivalent("ETRS89", "ETRF89"));
    }
}
