//! A PROJ string's tokens and numbers, as PROJ 9.8.1 reads them: the
//! steps and key-value pairs of `PROJStringSyntaxParser` (`io.cpp`) over
//! `pj_shrink`, `pj_trim_argc` and `pj_trim_argv` (`internal.cpp`), and
//! numbers and angles as `c_locale_stod` and `dmstor` read them.

use std::cell::Cell;

/// One `key=value` or bare `key`, and whether the parser has looked at it,
/// which tells it apart from what PROJ keeps only as an extension.
#[derive(Debug)]
pub(super) struct KeyValue {
    pub(super) key: String,
    /// Empty for a bare key, as in PROJ.
    pub(super) value: String,
    pub(super) used: Cell<bool>,
}

impl KeyValue {
    fn new(word: &str) -> Self {
        let (key, value) = word.split_once('=').unwrap_or((word, ""));
        Self {
            key: key.to_owned(),
            value: value.to_owned(),
            used: Cell::new(false),
        }
    }
}

/// A `+proj=` or `+init=` and its parameters.
#[derive(Debug, Default)]
pub(super) struct Step {
    pub(super) name: String,
    pub(super) is_init: bool,
    pub(super) inverted: bool,
    pub(super) params: Vec<KeyValue>,
}

/// A string's steps, the pairs before its first step, its `+title`, and
/// whether it is a `+proj=pipeline`.
#[derive(Debug, Default)]
pub(super) struct Syntax {
    pub(super) steps: Vec<Step>,
    pub(super) globals: Vec<KeyValue>,
    pub(super) title: String,
    pub(super) pipeline: bool,
}

/// `PROJStringSyntaxParser` for a string that isn't a pipeline; a pipeline
/// is only flagged, since it isn't read here.
pub(super) fn syntax(text: &str) -> Syntax {
    let mut text = text.to_owned();
    // "+title=several words +foo=bar"
    if text.starts_with("+title=")
        && text.len() > 7
        && text.as_bytes()[7] != b'"'
        && let (Some(plus), Some(space)) = (text[1..].find(" +").map(|i| i + 1), text.find(' '))
        && space < plus
    {
        text = format!(
            "+title={}{}",
            double_quote_if_needed(&text[7..plus]),
            &text[plus..]
        );
    }
    let tokens = arguments(&text);
    let mut out = Syntax::default();
    let has_proj = tokens.iter().any(|t| t.starts_with("proj="));
    let has_init = tokens.iter().any(|t| t.starts_with("init="));
    if tokens.iter().any(|t| t == "proj=pipeline") {
        out.pipeline = true;
        return out;
    }
    if has_proj || has_init {
        out.steps.push(Step::default());
    }
    for word in &tokens {
        if let Some(name) = word.strip_prefix("proj=")
            && !has_init
            && out.steps.last().is_some_and(|s| s.name.is_empty())
        {
            out.steps.last_mut().expect("a step").name = name.to_owned();
        } else if let Some(name) = word.strip_prefix("init=") {
            let step = out.steps.last_mut().expect("a step");
            step.name = name.to_owned();
            step.is_init = true;
        } else if word == "inv" {
            if let Some(step) = out.steps.last_mut() {
                step.inverted = true;
            }
        } else if let Some(title) = word.strip_prefix("title=") {
            out.title = title.to_owned();
        } else if word != "step" {
            let pair = KeyValue::new(word);
            match out.steps.last_mut() {
                Some(step) => step.params.push(pair),
                None => out.globals.push(pair),
            }
        }
    }
    out
}

/// PROJ's `pj_double_quote_string_param_if_needed`.
fn double_quote_if_needed(s: &str) -> String {
    if s.contains(' ') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

fn is_space(b: u8) -> bool {
    b.is_ascii_whitespace() || b == 0x0b
}

/// `pj_chomp`: without a `#` comment and leading and trailing whitespace
/// and semicolons.
fn chomp(s: &[u8]) -> &[u8] {
    let s = match s.iter().position(|&b| b == b'#') {
        Some(i) => &s[..i],
        None => s,
    };
    let start = s
        .iter()
        .position(|&b| !(is_space(b) || b == b';'))
        .unwrap_or(s.len());
    let mut end = s.len();
    while end > start + 1 && (is_space(s[end - 1]) || s[end - 1] == b';') {
        end -= 1;
    }
    &s[start..end]
}

/// `pj_shrink`: repeated whitespace collapsed, `+` and `;` between words
/// removed, `,` and `=` taking up the whitespace around them, quoted
/// values (`="…"`) left alone.
fn shrink(s: &[u8]) -> Vec<u8> {
    let c = chomp(s);
    let mut out: Vec<u8> = Vec::with_capacity(c.len());
    let mut ws = false;
    let mut in_string = false;
    let mut j = 0;
    while j < c.len() {
        let mut b = c[j];
        if in_string {
            if b == b'"' && c.get(j + 1) == Some(&b'"') {
                out.push(b);
                j += 1;
            } else if b == b'"' {
                in_string = false;
            }
            out.push(c[j]);
            j += 1;
            continue;
        }
        if b == b'+' && (out.is_empty() || ws) {
            b = b' ';
        }
        if b == b'"' && out.last() == Some(&b'=') {
            in_string = true;
            ws = false;
            out.push(b);
            j += 1;
            continue;
        }
        if is_space(b) || b == b';' {
            if !ws && !out.is_empty() {
                out.push(b' ');
            }
            ws = true;
        } else {
            ws = false;
            out.push(b);
        }
        j += 1;
    }
    let mut greedy: Vec<u8> = Vec::with_capacity(out.len());
    for &b in &out {
        if greedy.is_empty() {
            greedy.push(b);
            continue;
        }
        if b == b'=' || b == b',' {
            if greedy.last() == Some(&b' ') {
                *greedy.last_mut().expect("not empty") = b;
            } else {
                greedy.push(b);
            }
            continue;
        }
        if b == b' ' && matches!(greedy.last(), Some(b'=') | Some(b',')) {
            continue;
        }
        greedy.push(b);
    }
    greedy
}

/// `pj_trim_argc` and `pj_trim_argv`: the words of a shrunk string, with
/// quoted values unquoted.
fn arguments(text: &str) -> Vec<String> {
    let s = shrink(text.as_bytes());
    if s.is_empty() {
        return Vec::new();
    }
    let mut words: Vec<Vec<u8>> = vec![Vec::new()];
    let mut in_string = false;
    let mut i = 0;
    while i < s.len() {
        let b = s[i];
        let word = words.last_mut().expect("a word");
        if in_string {
            if b == b'"' && s.get(i + 1) == Some(&b'"') {
                word.push(b);
                i += 1;
            } else if b == b'"' {
                in_string = false;
            }
            word.push(s[i]);
        } else if b == b'=' && s.get(i + 1) == Some(&b'"') {
            word.extend_from_slice(b"=\"");
            i += 1;
            in_string = true;
        } else if b == b' ' {
            words.push(Vec::new());
        } else {
            word.push(b);
        }
        i += 1;
    }
    words
        .into_iter()
        .map(|w| unquote(&String::from_utf8_lossy(&w)))
        .collect()
}

/// `unquote_string`: `key="a ""b"""` is `key=a "b"`.
fn unquote(word: &str) -> String {
    let Some(eq) = word.find("=\"") else {
        return word.to_owned();
    };
    if !word.ends_with('"') || word.len() < eq + 3 {
        return word.to_owned();
    }
    let mut out = word[..=eq].to_owned();
    let mut chars = word[eq + 2..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                chars.next();
            } else {
                break;
            }
        }
        out.push(c);
    }
    out
}

/// `c_locale_stod`: the whole of `s` as a number.
pub(super) fn number(s: &str) -> Option<f64> {
    let t = s.strip_prefix('+').unwrap_or(s);
    if t.is_empty()
        || !t
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b'e' | b'E'))
    {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// PROJ's degree in radians and back, as `dmstor` and its callers use them
/// (`proj_internal.h`, written as PROJ writes them).
pub(super) const DEG_TO_RAD: f64 = 0.017453292519943296;
#[allow(clippy::excessive_precision)]
pub(super) const RAD_TO_DEG: f64 = 57.295779513082321;

/// The longest prefix of `s` that `strtod` reads as a number (no sign, as
/// `dmstor` strips it first), and its length.
fn strtod_prefix(s: &[u8]) -> Option<(f64, usize)> {
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < s.len() && s[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    let whole = digits(&mut i);
    let mut frac = false;
    if i < s.len() && s[i] == b'.' {
        i += 1;
        frac = digits(&mut i);
    }
    if !whole && !frac {
        return None;
    }
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        let mut j = i + 1;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            j += 1;
        }
        let start = j;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            i = j;
        }
    }
    let text = std::str::from_utf8(&s[..i]).ok()?;
    text.parse::<f64>().ok().map(|v| (v, i))
}

/// `dmstor`: an angle, in degrees, minutes and seconds (`49d30'15"N`), in
/// radians or as a plain number of degrees, converted to radians; `None`
/// unless all of `s` is read. Plain numbers go through `DEG_TO_RAD`, as in
/// PROJ, so that `RAD_TO_DEG` gives PROJ's degrees back bit for bit.
pub(super) fn dmstor(s: &str) -> Option<f64> {
    const SYM: &[u8] = b"NnEeSsWw";
    // PROJ's degree, minute and second in radians, as written in `dmstor.cpp`.
    #[allow(clippy::excessive_precision)]
    const VM: [f64; 3] = [DEG_TO_RAD, 0.0002908882086657216, 0.0000048481368110953599];
    let bytes = s.trim_start_matches(is_space_char).as_bytes();
    // `dmstor` copies printable characters (and the bytes of °) only.
    let len = bytes
        .iter()
        .position(|&b| !(b.is_ascii_graphic() || b == 0xc2 || b == 0xb0))
        .unwrap_or(bytes.len())
        .min(63);
    let work = &bytes[..len];
    let mut p = 0;
    let mut sign = b'+';
    if let Some(&b) = work.first()
        && (b == b'+' || b == b'-')
    {
        sign = b;
        p = 1;
    }
    let mut v = 0.0;
    let mut nl = 0usize;
    while nl < 3 {
        if p >= work.len() || !(work[p].is_ascii_digit() || work[p] == b'.') {
            break;
        }
        let (tv, n) = strtod_prefix(&work[p..])?;
        p += n;
        let c = work.get(p).copied().unwrap_or(0);
        let (unit, adv) = match c {
            b'D' | b'd' | 0xb0 => (0, 1),
            b'\'' => (1, 1),
            b'"' => (2, 1),
            0xc2 if work.get(p + 1) == Some(&0xb0) => (0, 2),
            b'r' | b'R' => {
                if nl != 0 {
                    return None;
                }
                p += 1;
                v = tv;
                nl = 5;
                continue;
            }
            _ => {
                v += tv * VM[nl];
                nl = 5;
                continue;
            }
        };
        if unit < nl {
            return None;
        }
        v += tv * VM[unit];
        p += adv;
        nl = unit + 1;
    }
    if let Some(&c) = work.get(p)
        && let Some(i) = SYM.iter().position(|&s| s == c)
    {
        sign = if i >= 4 { b'-' } else { b'+' };
        p += 1;
    }
    if sign == b'-' {
        v = -v;
    }
    // The caller requires the whole string to have been read.
    (p == work.len() && len == bytes.len()).then_some(v)
}

fn is_space_char(c: char) -> bool {
    c.is_ascii_whitespace()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        arguments(s)
    }

    #[test]
    fn tokens() {
        assert_eq!(
            words("+proj=utm +zone=33 +datum=WGS84"),
            ["proj=utm", "zone=33", "datum=WGS84"]
        );
        assert_eq!(words("  proj = utm ;  +zone =33"), ["proj=utm", "zone=33"]);
        assert_eq!(
            words("+towgs84=1, 2,3 +x_0=1e+08"),
            ["towgs84=1,2,3", "x_0=1e+08"]
        );
        assert_eq!(
            words(r#"+title="a ""b"" c" +proj=longlat"#),
            [r#"title=a "b" c"#, "proj=longlat"]
        );
        assert_eq!(words("+proj=longlat # a comment"), ["proj=longlat"]);
        let s = syntax("+title=two words +proj=merc +no_defs");
        assert_eq!(s.title, "two words");
        assert_eq!(s.steps[0].name, "merc");
        assert!(syntax("+proj=pipeline +step +proj=merc").pipeline);
        let s = syntax("+init=epsg:4326 +type=crs");
        assert!(s.steps[0].is_init);
        assert_eq!(s.steps[0].name, "epsg:4326");
    }

    #[test]
    fn angles() {
        let deg = |s| dmstor(s).map(|r| r * RAD_TO_DEG);
        assert_eq!(deg("19"), Some(19.0 * DEG_TO_RAD * RAD_TO_DEG));
        assert_eq!(deg("-90"), Some(-90.0));
        let paris = deg("2d20'14.025\"E").unwrap();
        assert!((paris - (2.0 + 20.0 / 60.0 + 14.025 / 3600.0)).abs() < 1e-12);
        assert!((deg("49d30'N").unwrap() - 49.5).abs() < 1e-12);
        assert!((deg("9d07'54.862\"W").unwrap() + 9.131906111111).abs() < 1e-9);
        assert_eq!(dmstor("1.5r"), Some(1.5));
        assert_eq!(dmstor("12x"), None);
        assert_eq!(dmstor("abc"), None);
        assert_eq!(number("0.9996"), Some(0.9996));
        assert_eq!(number("1e5"), Some(1e5));
        assert_eq!(number("inf"), None);
        assert_eq!(number("12x"), None);
    }
}
