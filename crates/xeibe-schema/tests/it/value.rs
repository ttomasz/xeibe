//! Value statistics: what a text value can be parsed as, at each level of
//! losslessness (`docs/schema-inference.md` §2.3).
//!
//! The table in the docs is the specification; this module is that table.

use xeibe_schema::value::{TemporalShape, TzShape, classify};
use xeibe_schema::{Merge, TypeSet, ValueStats};

#[track_caller]
fn assert_exact_value(value: &str, expected: TypeSet) {
    assert_eq!(
        classify(value).exact_value,
        expected,
        "exact_value of {value:?}"
    );
}

#[test]
fn string_is_always_possible() {
    for value in ["", "0012", "abc", "2021-03-04", "1", "true"] {
        let candidates = classify(value);
        assert!(candidates.exact_text.contains(TypeSet::STRING), "{value:?}");
        assert!(candidates.exact_value.contains(TypeSet::STRING), "{value:?}");
        assert!(candidates.lossy.contains(TypeSet::STRING), "{value:?}");
    }
}

#[test]
fn integers_and_floats() {
    assert_exact_value("1", TypeSet::INT | TypeSet::FLOAT | TypeSet::STRING);
    assert_exact_value("-42", TypeSet::INT | TypeSet::FLOAT | TypeSet::STRING);
    // Trailing zeros after the decimal point are allowed: the value survives.
    assert_exact_value("1523.40", TypeSet::FLOAT | TypeSet::STRING);
    assert_eq!(classify("1523.40").exact_text, TypeSet::STRING);
    // Too many digits for an f64 round trip.
    assert_exact_value("12345678901234567.89", TypeSet::STRING);
    // Beyond i64, but exact as a float.
    assert!(classify("99999999999999999999.0").lossy.contains(TypeSet::FLOAT));
}

#[test]
fn leading_zeros_mean_an_identifier() {
    // Postcodes, TERYT codes and the like are never numbers.
    assert_exact_value("0012", TypeSet::STRING);
    assert_exact_value("-012", TypeSet::STRING);
    assert!(classify("0012").lossy.contains(TypeSet::INT));
    // A single zero, or a zero before the decimal point, is fine.
    assert!(classify("0").exact_value.contains(TypeSet::INT));
    assert!(classify("0.5").exact_value.contains(TypeSet::FLOAT));
}

#[test]
fn booleans_are_only_true_and_false_unless_lossy() {
    assert!(classify("true").exact_value.contains(TypeSet::BOOL));
    assert!(classify("false").exact_text.contains(TypeSet::BOOL));
    assert!(!classify("1").exact_value.contains(TypeSet::BOOL));
    assert!(classify("1").lossy.contains(TypeSet::BOOL));
    assert!(!classify("TRUE").exact_text.contains(TypeSet::BOOL));
}

#[test]
fn temporal_values() {
    assert!(classify("2021-03-04").exact_text.contains(TypeSet::DATE));
    // A date with a time zone can't be printed back from a Date32, but the
    // value survives.
    assert!(!classify("2021-03-04Z").exact_text.contains(TypeSet::DATE));
    assert!(classify("2021-03-04Z").exact_value.contains(TypeSet::DATE));
    assert!(
        classify("2017-04-05T14:53:55+02:00")
            .exact_value
            .contains(TypeSet::DATETIME)
    );
    assert!(
        classify("2017-04-05T14:53:55")
            .exact_value
            .contains(TypeSet::DATETIME)
    );
    assert!(classify("14:30:00").exact_value.contains(TypeSet::TIME));
    // More than microsecond precision is not exact at the default unit.
    assert!(
        !classify("2017-04-05T14:53:55.1234567")
            .exact_value
            .contains(TypeSet::DATETIME)
    );
    // An ISO duration stays a string (no Interval type).
    assert_exact_value("P1Y2M3DT4H", TypeSet::STRING);
}

#[test]
fn statistics_intersect_over_the_values() {
    let mut stats = ValueStats::default();
    stats.observe("1");
    stats.observe("2");
    assert!(stats.exact_value.contains(TypeSet::INT));
    assert_eq!(stats.int_range, Some((1, 2)));
    assert_eq!(stats.count, 2);

    // One value that isn't an integer removes INT for the whole column.
    stats.observe("27a");
    assert!(!stats.exact_value.contains(TypeSet::INT));
    assert!(stats.exact_value.contains(TypeSet::STRING));
    assert_eq!(stats.count, 3);
    assert_eq!(stats.max_len, 3);
}

#[test]
fn float_statistics_keep_the_scale_for_metadata() {
    let mut stats = ValueStats::default();
    stats.observe("1.5");
    stats.observe("1523.40");
    let shape = stats.float_shape.expect("a float shape");
    assert_eq!(shape.max_scale, 2, "gml:max_scale");
    assert!(shape.max_significant_digits >= 6);
}

#[test]
fn time_zone_shapes_are_recorded() {
    let mut all_utc = ValueStats::default();
    all_utc.observe("2021-03-04T10:00:00+02:00");
    all_utc.observe("2021-05-04T10:00:00+02:00");
    assert_eq!(
        all_utc.temporal,
        Some(TemporalShape {
            tz: TzShape::Fixed(120),
            max_fraction_digits: 0
        })
    );

    let mut mixed = ValueStats::default();
    mixed.observe("2021-01-04T10:00:00+01:00");
    mixed.observe("2021-07-04T10:00:00+02:00");
    assert_eq!(mixed.temporal.expect("a shape").tz, TzShape::Mixed);

    // Some values with a time zone and some without can't share a column.
    let mut inconsistent = ValueStats::default();
    inconsistent.observe("2021-01-04T10:00:00+01:00");
    inconsistent.observe("2021-07-04T10:00:00");
    assert_eq!(
        inconsistent.temporal.expect("a shape").tz,
        TzShape::Inconsistent
    );

    let mut naive = ValueStats::default();
    naive.observe("2021-01-04T10:00:00");
    assert_eq!(naive.temporal.expect("a shape").tz, TzShape::Absent);
}

#[test]
fn distinct_values_are_kept_up_to_the_limit() {
    let mut stats = ValueStats::default();
    for value in ["a", "b", "a", "c"] {
        stats.observe(value);
    }
    assert!(!stats.distinct.overflowed);
    assert_eq!(stats.distinct.values.len(), 3, "a set, not a list");

    let mut many = ValueStats::default();
    for i in 0..1000 {
        many.observe(&i.to_string());
    }
    assert!(many.distinct.overflowed);
    assert!(
        many.distinct.values.is_empty(),
        "the set is dropped once it overflows"
    );
}

#[test]
fn merging_statistics_is_commutative() {
    let value_sets = [["1", "2"], ["3", "4"]];
    let make = |values: [&str; 2]| {
        let mut stats = ValueStats::default();
        for value in values {
            stats.observe(value);
        }
        stats
    };

    let mut forwards = make(value_sets[0]);
    forwards.merge(make(value_sets[1]));
    let mut backwards = make(value_sets[1]);
    backwards.merge(make(value_sets[0]));

    assert_eq!(forwards.count, 4);
    assert_eq!(forwards.count, backwards.count);
    assert_eq!(forwards.int_range, Some((1, 4)));
    assert_eq!(forwards.int_range, backwards.int_range);
    assert_eq!(forwards.exact_value, backwards.exact_value);
}

#[test]
fn merging_intersects_the_type_sets() {
    let mut numbers = ValueStats::default();
    numbers.observe("1");
    let mut words = ValueStats::default();
    words.observe("abc");
    numbers.merge(words);
    assert!(!numbers.exact_value.contains(TypeSet::INT));
    assert!(numbers.exact_value.contains(TypeSet::STRING));
}

#[test]
fn a_column_of_only_strings_stops_parsing_but_keeps_counting() {
    // Speed rule: once only STRING is left, later values are not parsed.
    let mut stats = ValueStats::default();
    stats.observe("abc");
    stats.observe("2021-03-04");
    assert_eq!(stats.exact_value, TypeSet::STRING);
    assert_eq!(stats.count, 2);
    assert_eq!(stats.max_len, 10);
}
