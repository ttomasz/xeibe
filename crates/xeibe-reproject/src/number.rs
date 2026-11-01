//! Numbers as PROJ writes them in WKT and PROJ strings: its
//! `internal::toString(double)`, C's `%.15g`, or `%.14g` when that shows a
//! run of nines.

use std::fmt::Write;

/// PROJ's `internal::toString(double)`.
pub(crate) fn to_string(v: f64) -> String {
    let mut out = String::new();
    write(&mut out, v);
    out
}

/// `to_string`, onto `out`.
pub(crate) fn write(out: &mut String, v: f64) {
    let start = out.len();
    write_g(out, v, 15);
    if out[start..].contains("9999999999") {
        out.truncate(start);
        write_g(out, v, 14);
    }
}

/// C's `%.{precision}g`, for a precision of 1 to 17.
fn write_g(out: &mut String, v: f64, precision: usize) {
    debug_assert!((1..=17).contains(&precision));
    if v == 0.0 {
        out.push_str(if v.is_sign_negative() { "-0" } else { "0" });
        return;
    }
    if !v.is_finite() {
        out.push_str(if v.is_nan() {
            "nan"
        } else if v > 0.0 {
            "inf"
        } else {
            "-inf"
        });
        return;
    }
    // The shortest digits that read back as v. When there are no more than
    // the precision, rounding v to the precision gives the same digits: a
    // step in the 15th significant digit is wider than the gap between two
    // normal doubles. Otherwise v is rounded to the precision exactly.
    let mut buf = Buf::default();
    let _ = write!(buf, "{v:e}");
    let (digits, exp) = buf.digits();
    if precision <= 15 && digits.len() <= precision && v.is_normal() {
        place(out, v < 0.0, digits, exp, precision);
        return;
    }
    let mut buf = Buf::default();
    let _ = write!(buf, "{:.*e}", precision - 1, v);
    let (digits, exp) = buf.digits();
    let kept = digits.iter().rposition(|&d| d != b'0').map_or(1, |i| i + 1);
    place(out, v < 0.0, &digits[..kept], exp, precision);
}

/// `%g`'s layout of the significant digits `digits` (no trailing zeros)
/// times 10 to `exp`: positional from 1e-4 to below 10 to the precision,
/// else with an exponent of at least two digits.
fn place(out: &mut String, negative: bool, digits: &[u8], exp: i32, precision: usize) {
    let digits = std::str::from_utf8(digits).expect("ASCII digits");
    if negative {
        out.push('-');
    }
    if exp < -4 || exp >= precision as i32 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let _ = write!(out, "{:02}", exp.unsigned_abs());
    } else if exp < 0 {
        out.push_str("0.");
        for _ in 0..-exp - 1 {
            out.push('0');
        }
        out.push_str(digits);
    } else {
        let whole = exp as usize + 1;
        if digits.len() <= whole {
            out.push_str(digits);
            for _ in digits.len()..whole {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..whole]);
            out.push('.');
            out.push_str(&digits[whole..]);
        }
    }
}

/// Rust's `{:e}` of a finite double, on the stack: at most 17 digits, a
/// sign, a point and `e-324`.
#[derive(Default)]
struct Buf {
    bytes: [u8; 32],
    len: usize,
    digits: [u8; 17],
}

impl Buf {
    /// The significant digits and the exponent of what was written.
    fn digits(&mut self) -> (&[u8], i32) {
        let s = &self.bytes[..self.len];
        let e = s.iter().position(|&b| b == b'e').expect("an exponent");
        let mut n = 0;
        for &b in s[..e].iter().filter(|b| b.is_ascii_digit()) {
            self.digits[n] = b;
            n += 1;
        }
        let exp = std::str::from_utf8(&s[e + 1..])
            .ok()
            .and_then(|x| x.parse().ok())
            .expect("an integer exponent");
        (&self.digits[..n], exp)
    }
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len + s.len();
        self.bytes
            .get_mut(self.len..end)
            .ok_or(std::fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(v: f64, precision: usize) -> String {
        let mut out = String::new();
        write_g(&mut out, v, precision);
        out
    }

    /// The `%g` of Rust's exact formatting, as this module formatted
    /// numbers before it had the shortest digits to start from.
    fn exact_g(v: f64, p: usize) -> String {
        if v == 0.0 || !v.is_finite() {
            return g(v, p);
        }
        let e = format!("{:.*e}", p - 1, v);
        let (mantissa, exponent) = e.split_once('e').unwrap();
        let x: i32 = exponent.parse().unwrap();
        let trim = |s: &str| -> String {
            if s.contains('.') {
                s.trim_end_matches('0').trim_end_matches('.').to_owned()
            } else {
                s.to_owned()
            }
        };
        if x < -4 || x >= p as i32 {
            let sign = if x < 0 { '-' } else { '+' };
            format!("{}e{sign}{:02}", trim(mantissa), x.abs())
        } else {
            let decimals = (p as i32 - 1 - x).max(0) as usize;
            trim(&format!("{v:.decimals$}"))
        }
    }

    #[test]
    fn formats_as_c() {
        assert_eq!(g(0.3048006096012192, 15), "0.304800609601219");
        assert_eq!(g(1640416.666666667, 15), "1640416.66666667");
        assert_eq!(g(500000.0, 15), "500000");
        assert_eq!(g(1e-6, 15), "1e-06");
        assert_eq!(g(1e-5, 15), "1e-05");
        assert_eq!(g(2.5e-5, 15), "2.5e-05");
        assert_eq!(g(0.0001, 15), "0.0001");
        assert_eq!(g(1.5e20, 15), "1.5e+20");
        assert_eq!(g(1e15, 15), "1e+15");
        assert_eq!(g(123456789012345.0, 15), "123456789012345");
        assert_eq!(g(-0.0001, 15), "-0.0001");
        assert_eq!(g(123456789012345680.0, 15), "1.23456789012346e+17");
        assert_eq!(g(1e23_f64.next_down(), 15), "1e+23");
        assert_eq!(g(1e-300, 15), "1e-300");
        assert_eq!(g(-0.0, 15), "-0");
        assert_eq!(to_string(0.1 + 0.2), "0.3");
    }

    #[test]
    fn a_run_of_nines_is_rounded_once_more() {
        // PROJ's own output.
        assert_eq!(to_string(2.99999999999999), "3");
        assert_eq!(to_string(12.3999999999999), "12.4");
        assert_eq!(to_string(5.99999999999997e-7), "6e-07");
        assert_eq!(to_string(1.99999999999), "1.99999999999");
        assert_eq!(to_string(9.9999999999999e-05), "9.9999999999999e-05");
    }

    #[test]
    fn shortest_digits_agree_with_exact_rounding() {
        // Doubles from a fixed xorshift, over every exponent, and decimals
        // as EPSG writes them.
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut values = Vec::new();
        for _ in 0..200_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            values.push(f64::from_bits(x));
        }
        for i in 0..100_000_i64 {
            values.push(i as f64 / 1000.0);
            values.push((i * 7919) as f64 * 1e-9);
            values.push(1.0 + i as f64 * 1e-12);
        }
        for v in values.into_iter().filter(|v| v.is_finite()) {
            for p in [14, 15] {
                assert_eq!(g(v, p), exact_g(v, p), "{v:e} at {p}");
            }
        }
    }
}
