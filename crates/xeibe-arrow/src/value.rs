//! Text → typed values for the column builders (XML Schema lexical forms).
//!
//! A value that doesn't fit its column's type gives `None`: a feature error
//! (`docs/schema-inference.md` §6.3).

use arrow_schema::{DataType, TimeUnit};

/// A parsed value, in the physical form of its column type.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    Bool(bool),
    /// Signed integers, and timestamps / `Date64` / `Time64` in their unit.
    Int(i64),
    UInt(u64),
    Float(f64),
    /// `Date32` days, `Time32` values.
    Int32(i32),
    Str(String),
}

/// Parse `text` (already trimmed) as a value of `data_type`.
pub fn parse_scalar(data_type: &DataType, text: &str) -> Option<Scalar> {
    Some(match data_type {
        // Binary columns take the text's bytes as written.
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View | DataType::Binary | DataType::LargeBinary => {
            Scalar::Str(text.to_string())
        }
        DataType::Boolean => Scalar::Bool(match text {
            "true" | "1" => true,
            "false" | "0" => false,
            _ => return None,
        }),
        DataType::Int8 => Scalar::Int(int_in(text, i8::MIN as i64, i8::MAX as i64)?),
        DataType::Int16 => Scalar::Int(int_in(text, i16::MIN as i64, i16::MAX as i64)?),
        DataType::Int32 => Scalar::Int(int_in(text, i32::MIN as i64, i32::MAX as i64)?),
        DataType::Int64 => Scalar::Int(int_in(text, i64::MIN, i64::MAX)?),
        DataType::UInt8 => Scalar::UInt(uint_in(text, u8::MAX as u64)?),
        DataType::UInt16 => Scalar::UInt(uint_in(text, u16::MAX as u64)?),
        DataType::UInt32 => Scalar::UInt(uint_in(text, u32::MAX as u64)?),
        DataType::UInt64 => Scalar::UInt(uint_in(text, u64::MAX)?),
        DataType::Float16 | DataType::Float32 | DataType::Float64 => Scalar::Float(parse_float(text)?),
        DataType::Date32 => {
            let (days, rest) = parse_date(text)?;
            parse_tz(rest)?;
            Scalar::Int32(i32::try_from(days).ok()?)
        }
        DataType::Date64 => {
            let (days, rest) = parse_date(text)?;
            parse_tz(rest)?;
            Scalar::Int(days.checked_mul(86_400_000)?)
        }
        DataType::Timestamp(unit, tz) => Scalar::Int(parse_timestamp(text, *unit, tz.is_some())?),
        DataType::Time32(unit) => {
            let (value, _) = parse_time_of_day(text, *unit)?;
            Scalar::Int32(i32::try_from(value).ok()?)
        }
        DataType::Time64(unit) => Scalar::Int(parse_time_of_day(text, *unit)?.0),
        _ => return None,
    })
}

fn int_in(text: &str, min: i64, max: i64) -> Option<i64> {
    let digits = text.strip_prefix('+').unwrap_or(text);
    let value: i64 = digits.parse().ok()?;
    (min..=max).contains(&value).then_some(value)
}

fn uint_in(text: &str, max: u64) -> Option<u64> {
    let digits = text.strip_prefix('+').unwrap_or(text);
    let value: u64 = digits.parse().ok()?;
    (value <= max).then_some(value)
}

/// `xs:double`: decimal or exponent notation, `INF`, `-INF`, `NaN`.
fn parse_float(text: &str) -> Option<f64> {
    match text {
        "INF" | "+INF" => return Some(f64::INFINITY),
        "-INF" => return Some(f64::NEG_INFINITY),
        "NaN" => return Some(f64::NAN),
        _ => {}
    }
    // Rust also takes `inf`, `infinity` and `nan` in any case; XML doesn't.
    if text.bytes().any(|b| b.is_ascii_alphabetic() && b != b'e' && b != b'E') {
        return None;
    }
    text.parse().ok()
}

/// `[-]YYYY-MM-DD` → days since 1970-01-01, and what follows.
fn parse_date(text: &str) -> Option<(i64, &str)> {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let year_len = unsigned.bytes().take_while(u8::is_ascii_digit).count();
    if !(4..=9).contains(&year_len) {
        return None;
    }
    let mut year: i64 = unsigned[..year_len].parse().ok()?;
    if negative {
        year = -year;
    }
    let rest = unsigned[year_len..].strip_prefix('-')?;
    let month = two_digits(rest)?;
    let rest = rest[2..].strip_prefix('-')?;
    let day = two_digits(rest)?;
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > days_in_month {
        return None;
    }
    Some((days_from_civil(year, month, day), &rest[2..]))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = month as i64;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `hh:mm:ss[.f+]` → (seconds of the day, fraction digits, what follows).
fn parse_time(text: &str) -> Option<(i64, &str, &str)> {
    let hour = two_digits(text)?;
    let rest = text[2..].strip_prefix(':')?;
    let minute = two_digits(rest)?;
    let rest = rest[2..].strip_prefix(':')?;
    let second = two_digits(rest)?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let seconds = (hour * 3600 + minute * 60 + second.min(59)) as i64;
    let rest = &rest[2..];
    match rest.strip_prefix('.') {
        Some(fraction) => {
            let len = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if len == 0 {
                return None;
            }
            Some((seconds, &fraction[..len], &fraction[len..]))
        }
        None => Some((seconds, "", rest)),
    }
}

/// Empty (no time zone), `Z`, or `±hh:mm` → offset in minutes.
fn parse_tz(text: &str) -> Option<Option<i16>> {
    if text.is_empty() {
        return Some(None);
    }
    if text == "Z" {
        return Some(Some(0));
    }
    let (sign, rest) = match text.as_bytes()[0] {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
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

fn two_digits(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_digit() || !bytes[1].is_ascii_digit() {
        return None;
    }
    Some(((bytes[0] - b'0') * 10 + (bytes[1] - b'0')) as u32)
}

fn units_per_second(unit: TimeUnit) -> (i64, usize) {
    match unit {
        TimeUnit::Second => (1, 0),
        TimeUnit::Millisecond => (1_000, 3),
        TimeUnit::Microsecond => (1_000_000, 6),
        TimeUnit::Nanosecond => (1_000_000_000, 9),
    }
}

/// Fraction digits as a count of `unit`s; `None` if digits beyond the unit's
/// precision are not zero (the value would lose precision).
fn fraction_units(fraction: &str, unit: TimeUnit) -> Option<i64> {
    let (_, digits) = units_per_second(unit);
    let (kept, dropped) = fraction.split_at(fraction.len().min(digits));
    if dropped.bytes().any(|b| b != b'0') {
        return None;
    }
    let mut value: i64 = if kept.is_empty() { 0 } else { kept.parse().ok()? };
    for _ in kept.len()..digits {
        value *= 10;
    }
    Some(value)
}

/// `xs:dateTime` → count of `unit`s since the epoch. A column with a time zone
/// takes only values with one (normalized to UTC); a column without one takes
/// only values without one.
fn parse_timestamp(text: &str, unit: TimeUnit, with_tz: bool) -> Option<i64> {
    let (days, rest) = parse_date(text)?;
    let time = rest.strip_prefix('T')?;
    let (seconds, fraction, rest) = parse_time(time)?;
    let tz = parse_tz(rest)?;
    if tz.is_some() != with_tz {
        return None;
    }
    let offset_seconds = tz.unwrap_or(0) as i64 * 60;
    let (per_second, _) = units_per_second(unit);
    let whole = (days * 86_400 + seconds - offset_seconds).checked_mul(per_second)?;
    whole.checked_add(fraction_units(fraction, unit)?)
}

/// `xs:time` → count of `unit`s since midnight (the time zone, if any, is dropped).
fn parse_time_of_day(text: &str, unit: TimeUnit) -> Option<(i64, Option<i16>)> {
    let (seconds, fraction, rest) = parse_time(text)?;
    let tz = parse_tz(rest)?;
    let (per_second, _) = units_per_second(unit);
    Some((seconds * per_second + fraction_units(fraction, unit)?, tz))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_timestamps() {
        assert_eq!(parse_scalar(&DataType::Date32, "1970-01-02"), Some(Scalar::Int32(1)));
        assert_eq!(parse_scalar(&DataType::Date32, "2000-03-01Z"), Some(Scalar::Int32(11017)));
        let utc = DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
        assert_eq!(parse_scalar(&utc, "1970-01-01T00:00:01+00:00"), Some(Scalar::Int(1_000_000)));
        assert_eq!(parse_scalar(&utc, "1970-01-01T01:00:00.5+01:00"), Some(Scalar::Int(500_000)));
        assert_eq!(parse_scalar(&utc, "1970-01-01T00:00:01"), None);
        assert_eq!(parse_scalar(&utc, "1970-01-01T00:00:00.0000001Z"), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_scalar(&DataType::Int64, "0012"), Some(Scalar::Int(12)));
        assert_eq!(parse_scalar(&DataType::Int8, "300"), None);
        assert_eq!(parse_scalar(&DataType::Float64, "1e3"), Some(Scalar::Float(1000.0)));
        assert_eq!(parse_scalar(&DataType::Float64, "inf"), None);
        assert_eq!(parse_scalar(&DataType::Int64, "huge"), None);
    }
}
