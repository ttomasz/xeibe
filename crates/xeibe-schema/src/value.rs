//! Value statistics: every type a text value can be parsed as, at three levels
//! of losslessness, intersected over a column (`docs/schema-inference.md` §2.3).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::Merge;

bitflags::bitflags! {
    /// Types a value can be parsed as. `STRING` is always set.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct TypeSet: u16 {
        const BOOL = 1 << 0;
        const INT = 1 << 1;
        const FLOAT = 1 << 2;
        const DATE = 1 << 3;
        const DATETIME = 1 << 4;
        const TIME = 1 << 5;
        const STRING = 1 << 6;
    }
}

/// Candidate types of one text value at each losslessness level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidates {
    pub exact_text: TypeSet,
    pub exact_value: TypeSet,
    pub lossy: TypeSet,
}

/// Fractional-second digits a value may have and still be exact at the
/// default timestamp unit (microseconds).
const EXACT_FRACTION_DIGITS: u8 = 6;

/// Values longer than this are not kept in [`BoundedSet`]: they are text, not
/// codes, and would make the set's memory depend on the data.
const MAX_DISTINCT_VALUE_LEN: usize = 1024;

/// Classify a single value (leading-zero rule, f64 round-trip, temporal forms).
pub fn classify(value: &str) -> Candidates {
    analyze(value).candidates
}

/// Everything one value tells the statistics.
struct Analysis {
    candidates: Candidates,
    /// The value as an integer, if it parses as one at all.
    int: Option<i64>,
    /// Significant digits and scale, if it parses as a number.
    float: Option<FloatShape>,
    /// Time-zone offset (minutes, `None` = no time zone) and fraction digits.
    temporal: Option<(Option<i16>, u8)>,
}

fn analyze(value: &str) -> Analysis {
    let mut analysis = Analysis {
        candidates: Candidates {
            exact_text: TypeSet::STRING,
            exact_value: TypeSet::STRING,
            lossy: TypeSet::STRING,
        },
        int: None,
        float: None,
        temporal: None,
    };
    let c = &mut analysis.candidates;

    match value {
        "true" | "false" => {
            c.exact_text |= TypeSet::BOOL;
            c.exact_value |= TypeSet::BOOL;
            c.lossy |= TypeSet::BOOL;
        }
        "1" | "0" => c.lossy |= TypeSet::BOOL,
        _ if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") => {
            c.lossy |= TypeSet::BOOL;
        }
        _ => {}
    }

    if let Some(number) = Number::parse(value) {
        analyze_number(value, &number, &mut analysis);
    } else if let Some(temporal) = Temporal::parse(value) {
        analyze_temporal(&temporal, &mut analysis);
    }
    analysis
}

fn analyze_number(value: &str, number: &Number<'_>, analysis: &mut Analysis) {
    let c = &mut analysis.candidates;
    // `0012`, `-012`: an identifier, never a number (but `0` and `0.5` are fine).
    let identifier = number.int_digits.len() > 1 && number.int_digits.starts_with('0');

    if number.is_integer() {
        if let Ok(int) = value.trim_start_matches('+').parse::<i64>() {
            analysis.int = Some(int);
            c.lossy |= TypeSet::INT;
            if !identifier {
                c.exact_value |= TypeSet::INT;
                if int.to_string() == value {
                    c.exact_text |= TypeSet::INT;
                }
            }
        }
    }

    let Ok(float) = value.parse::<f64>() else {
        return;
    };
    if !float.is_finite() {
        return;
    }
    c.lossy |= TypeSet::FLOAT;
    analysis.float = Some(FloatShape {
        max_significant_digits: number.significant_digits(),
        max_scale: number.scale(),
    });
    if !identifier && number.decimal() == Decimal::of_f64(float) {
        c.exact_value |= TypeSet::FLOAT;
        if format!("{float}") == value {
            c.exact_text |= TypeSet::FLOAT;
        }
    }
}

fn analyze_temporal(temporal: &Temporal, analysis: &mut Analysis) {
    let c = &mut analysis.candidates;
    let exact_fraction = temporal.fraction_digits <= EXACT_FRACTION_DIGITS;
    let trailing_zero = temporal.fraction_trailing_zero;
    analysis.temporal = Some((temporal.tz, temporal.fraction_digits));
    match temporal.kind {
        TemporalKind::Date => {
            c.lossy |= TypeSet::DATE | TypeSet::DATETIME;
            c.exact_value |= TypeSet::DATE;
            // A Date32 has nowhere to keep the time zone, so it can't print it back.
            if temporal.tz.is_none() {
                c.exact_text |= TypeSet::DATE;
            }
        }
        TemporalKind::DateTime => {
            c.lossy |= TypeSet::DATETIME;
            if exact_fraction {
                c.exact_value |= TypeSet::DATETIME;
                if !trailing_zero {
                    c.exact_text |= TypeSet::DATETIME;
                }
            }
        }
        TemporalKind::Time => {
            c.lossy |= TypeSet::TIME;
            if exact_fraction {
                c.exact_value |= TypeSet::TIME;
                if temporal.tz.is_none() && !trailing_zero {
                    c.exact_text |= TypeSet::TIME;
                }
            }
        }
    }
}

/// A number in `xs:decimal` / `xs:double` lexical form, split into parts.
/// `NaN` and `INF` are not numbers here: they are rare and never exact.
struct Number<'a> {
    negative: bool,
    int_digits: &'a str,
    frac_digits: &'a str,
    exponent: i32,
    has_exponent: bool,
    has_point: bool,
}

impl<'a> Number<'a> {
    fn parse(value: &'a str) -> Option<Self> {
        let (negative, rest) = match value.as_bytes().first()? {
            b'-' => (true, &value[1..]),
            b'+' => (false, &value[1..]),
            _ => (false, value),
        };
        let (mantissa, exponent) = match rest.find(['e', 'E']) {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let (int_digits, frac_digits, has_point) = match mantissa.split_once('.') {
            Some((int, frac)) => (int, frac, true),
            None => (mantissa, "", false),
        };
        let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if int_digits.is_empty() && frac_digits.is_empty()
            || !digits(int_digits)
            || !digits(frac_digits)
        {
            return None;
        }
        let exponent_value = match exponent {
            Some(e) => {
                let unsigned = e.strip_prefix(['+', '-']).unwrap_or(e);
                if unsigned.is_empty() || !digits(unsigned) {
                    return None;
                }
                e.parse::<i32>().ok()?
            }
            None => 0,
        };
        Some(Number {
            negative,
            int_digits,
            frac_digits,
            exponent: exponent_value,
            has_exponent: exponent.is_some(),
            has_point,
        })
    }

    fn is_integer(&self) -> bool {
        !self.has_exponent && !self.has_point
    }

    /// Digits after the first non-zero one, as written (trailing zeros count).
    fn significant_digits(&self) -> u8 {
        let all = self.int_digits.len() + self.frac_digits.len();
        let leading = self
            .int_digits
            .bytes()
            .chain(self.frac_digits.bytes())
            .take_while(|&b| b == b'0')
            .count();
        (all - leading).min(u8::MAX as usize) as u8
    }

    /// Fractional digits of the value as written, after the exponent.
    fn scale(&self) -> u8 {
        let scale = self.frac_digits.len() as i64 - self.exponent as i64;
        scale.clamp(0, u8::MAX as i64) as u8
    }

    fn decimal(&self) -> Decimal {
        let mut digits = String::with_capacity(self.int_digits.len() + self.frac_digits.len());
        digits.push_str(self.int_digits);
        digits.push_str(self.frac_digits);
        Decimal::new(
            self.negative,
            &digits,
            self.exponent as i64 - self.frac_digits.len() as i64,
        )
    }
}

/// A decimal number normalized as `±digits × 10^exponent`, without leading or
/// trailing zeros in `digits`. Zero has empty digits and no sign.
#[derive(Debug, PartialEq, Eq)]
struct Decimal {
    negative: bool,
    digits: String,
    exponent: i64,
}

impl Decimal {
    fn new(negative: bool, digits: &str, mut exponent: i64) -> Self {
        let digits = digits.trim_start_matches('0');
        let trimmed = digits.trim_end_matches('0');
        exponent += (digits.len() - trimmed.len()) as i64;
        if trimmed.is_empty() {
            return Decimal { negative: false, digits: String::new(), exponent: 0 };
        }
        Decimal { negative, digits: trimmed.to_string(), exponent }
    }

    /// The shortest decimal that round-trips to `value` (Rust's `{:e}`).
    fn of_f64(value: f64) -> Self {
        let text = format!("{value:e}");
        let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
        let (negative, mantissa) = match mantissa.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, mantissa),
        };
        let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let exponent = exponent.parse::<i64>().unwrap_or(0) - frac.len() as i64;
        Decimal::new(negative, &format!("{int}{frac}"), exponent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemporalKind {
    Date,
    DateTime,
    Time,
}

/// `xs:date`, `xs:dateTime` or `xs:time`, validated.
struct Temporal {
    kind: TemporalKind,
    /// Offset in minutes; `None` = no time zone.
    tz: Option<i16>,
    fraction_digits: u8,
    fraction_trailing_zero: bool,
}

impl Temporal {
    fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        // A date starts with a (possibly negative) year of 4+ digits and `-`;
        // a time with `hh:`.
        if bytes.len() >= 3 && bytes[2] == b':' {
            let (fraction, rest) = parse_time(value)?;
            let tz = parse_tz(rest)?;
            return Some(Temporal::with(TemporalKind::Time, tz, fraction));
        }
        let rest = parse_date(value)?;
        if let Some(time) = rest.strip_prefix('T') {
            let (fraction, rest) = parse_time(time)?;
            let tz = parse_tz(rest)?;
            return Some(Temporal::with(TemporalKind::DateTime, tz, fraction));
        }
        let tz = parse_tz(rest)?;
        Some(Temporal::with(TemporalKind::Date, tz, ""))
    }

    fn with(kind: TemporalKind, tz: Option<i16>, fraction: &str) -> Self {
        Temporal {
            kind,
            tz,
            fraction_digits: fraction.len().min(u8::MAX as usize) as u8,
            fraction_trailing_zero: fraction.ends_with('0'),
        }
    }
}

/// `[-]YYYY-MM-DD`; returns what follows.
fn parse_date(value: &str) -> Option<&str> {
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let year_len = unsigned.bytes().take_while(u8::is_ascii_digit).count();
    // More than four digits only without a leading zero (XML Schema 1.0).
    if year_len < 4 || (year_len > 4 && unsigned.starts_with('0')) || year_len > 9 {
        return None;
    }
    let year: i64 = unsigned[..year_len].parse().ok()?;
    let rest = unsigned[year_len..].strip_prefix('-')?;
    let month = two_digits(rest)?;
    let rest = rest[2..].strip_prefix('-')?;
    let day = two_digits(rest)?;
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > days {
        return None;
    }
    Some(&rest[2..])
}

/// `hh:mm:ss[.f+]`; returns the fraction digits and what follows.
fn parse_time(value: &str) -> Option<(&str, &str)> {
    let hour = two_digits(value)?;
    let rest = value[2..].strip_prefix(':')?;
    let minute = two_digits(rest)?;
    let rest = rest[2..].strip_prefix(':')?;
    let second = two_digits(rest)?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let rest = &rest[2..];
    match rest.strip_prefix('.') {
        Some(fraction) => {
            let len = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if len == 0 {
                return None;
            }
            Some((&fraction[..len], &fraction[len..]))
        }
        None => Some(("", rest)),
    }
}

/// Empty (no time zone), `Z`, or `±hh:mm`. `None` if malformed.
fn parse_tz(value: &str) -> Option<Option<i16>> {
    if value.is_empty() {
        return Some(None);
    }
    if value == "Z" {
        return Some(Some(0));
    }
    let (sign, rest) = match value.as_bytes()[0] {
        b'+' => (1, &value[1..]),
        b'-' => (-1, &value[1..]),
        _ => return None,
    };
    if rest.len() != 5 || rest.as_bytes()[2] != b':' {
        return None;
    }
    let hours = two_digits(rest)?;
    let minutes = two_digits(&rest[3..])?;
    if hours > 14 || minutes > 59 {
        return None;
    }
    Some(Some(sign * (hours as i16 * 60 + minutes as i16)))
}

fn two_digits(value: &str) -> Option<u32> {
    let bytes = value.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_digit() || !bytes[1].is_ascii_digit() {
        return None;
    }
    Some(((bytes[0] - b'0') * 10 + (bytes[1] - b'0')) as u32)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueStats {
    pub count: u64,
    /// ∩ over values: parses and prints back identically.
    pub exact_text: TypeSet,
    /// ∩ over values: the value survives a round trip through the Arrow type.
    pub exact_value: TypeSet,
    /// ∩ over values: parses at all.
    pub lossy: TypeSet,
    pub int_range: Option<(i64, i64)>,
    pub float_shape: Option<FloatShape>,
    pub temporal: Option<TemporalShape>,
    pub max_len: u32,
    pub distinct: BoundedSet,
}

/// The default capacity of [`BoundedSet`] (`Limits::distinct_values`).
pub const DEFAULT_DISTINCT_VALUES: u16 = 64;

impl Default for ValueStats {
    /// No values yet: every type is still possible (the identity of the
    /// intersection).
    fn default() -> Self {
        ValueStats::with_capacity(DEFAULT_DISTINCT_VALUES)
    }
}

impl ValueStats {
    /// Empty statistics keeping up to `distinct_values` distinct values.
    pub fn with_capacity(distinct_values: u16) -> Self {
        ValueStats {
            count: 0,
            exact_text: TypeSet::all(),
            exact_value: TypeSet::all(),
            lossy: TypeSet::all(),
            int_range: None,
            float_shape: None,
            temporal: None,
            max_len: 0,
            distinct: BoundedSet {
                values: Vec::new(),
                overflowed: false,
                capacity: distinct_values,
            },
        }
    }

    /// Update with one value. Skips parsing once only `STRING` remains.
    pub fn observe(&mut self, value: &str) {
        self.count += 1;
        let len = value.chars().count().min(u32::MAX as usize) as u32;
        self.max_len = self.max_len.max(len);
        self.distinct.insert(value);

        if (self.exact_text | self.exact_value | self.lossy) == TypeSet::STRING {
            return;
        }
        let analysis = analyze(value);
        let c = analysis.candidates;
        self.exact_text &= c.exact_text;
        self.exact_value &= c.exact_value;
        self.lossy &= c.lossy;
        if let Some(int) = analysis.int {
            self.int_range = Some(match self.int_range {
                Some((lo, hi)) => (lo.min(int), hi.max(int)),
                None => (int, int),
            });
        }
        if let Some(shape) = analysis.float {
            let current = self.float_shape.get_or_insert_with(FloatShape::default);
            current.merge(shape);
        }
        if let Some((tz, fraction)) = analysis.temporal {
            let shape = TemporalShape {
                tz: match tz {
                    Some(offset) => TzShape::Fixed(offset),
                    None => TzShape::Absent,
                },
                max_fraction_digits: fraction,
            };
            match &mut self.temporal {
                Some(current) => current.merge(shape),
                None => self.temporal = Some(shape),
            }
        }
    }

    /// The type set for one losslessness level.
    pub fn types(&self, lossless: crate::options::Lossless) -> TypeSet {
        match lossless {
            crate::options::Lossless::Text => self.exact_text,
            crate::options::Lossless::Value => self.exact_value,
            crate::options::Lossless::Lossy => self.lossy,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatShape {
    pub max_significant_digits: u8,
    /// Largest number of fractional digits seen → `gml:max_scale`.
    pub max_scale: u8,
}

impl Merge for FloatShape {
    fn merge(&mut self, other: Self) {
        self.max_significant_digits = self.max_significant_digits.max(other.max_significant_digits);
        self.max_scale = self.max_scale.max(other.max_scale);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TzShape {
    #[default]
    Absent,
    /// Offset in minutes, the same for every value.
    Fixed(i16),
    /// Different offsets (e.g. summer/winter time).
    Mixed,
    /// Some values with a time zone, some without → string.
    Inconsistent,
}

impl Merge for TzShape {
    fn merge(&mut self, other: Self) {
        use TzShape::*;
        *self = match (*self, other) {
            (Inconsistent, _) | (_, Inconsistent) => Inconsistent,
            (Absent, Absent) => Absent,
            (Absent, _) | (_, Absent) => Inconsistent,
            (Fixed(a), Fixed(b)) if a == b => Fixed(a),
            _ => Mixed,
        };
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporalShape {
    pub tz: TzShape,
    pub max_fraction_digits: u8,
}

impl Merge for TemporalShape {
    fn merge(&mut self, other: Self) {
        self.tz.merge(other.tz);
        self.max_fraction_digits = self.max_fraction_digits.max(other.max_fraction_digits);
    }
}

/// Up to `capacity` distinct values; cleared once it overflows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoundedSet {
    pub values: Vec<Arc<str>>,
    pub overflowed: bool,
    pub capacity: u16,
}

impl BoundedSet {
    pub fn insert(&mut self, value: &str) {
        if self.overflowed || self.values.iter().any(|v| &**v == value) {
            return;
        }
        if self.values.len() >= self.capacity as usize || value.len() > MAX_DISTINCT_VALUE_LEN {
            self.overflow();
            return;
        }
        self.values.push(Arc::from(value));
    }

    /// The only value, if exactly one was seen and the set never overflowed.
    pub fn single(&self) -> Option<&str> {
        match (self.overflowed, self.values.as_slice()) {
            (false, [value]) => Some(value),
            _ => None,
        }
    }

    fn overflow(&mut self) {
        self.overflowed = true;
        self.values = Vec::new();
    }
}

impl Merge for ValueStats {
    fn merge(&mut self, other: Self) {
        self.count += other.count;
        self.exact_text &= other.exact_text;
        self.exact_value &= other.exact_value;
        self.lossy &= other.lossy;
        self.int_range = match (self.int_range, other.int_range) {
            (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
            (a, b) => a.or(b),
        };
        self.float_shape = match (self.float_shape, other.float_shape) {
            (Some(mut a), Some(b)) => {
                a.merge(b);
                Some(a)
            }
            (a, b) => a.or(b),
        };
        self.temporal = match (self.temporal, other.temporal) {
            (Some(mut a), Some(b)) => {
                a.merge(b);
                Some(a)
            }
            (a, b) => a.or(b),
        };
        self.max_len = self.max_len.max(other.max_len);
        self.distinct.merge(other.distinct);
    }
}

impl Merge for BoundedSet {
    fn merge(&mut self, other: Self) {
        self.capacity = self.capacity.max(other.capacity);
        if other.overflowed {
            self.overflow();
        }
        for value in other.values {
            self.insert(&value);
        }
    }
}
