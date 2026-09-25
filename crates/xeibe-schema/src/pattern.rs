use serde::{Deserialize, Serialize};

/// Glob over local names, optionally namespace-qualified:
/// `AD_PunktAdresowy/idIIP`, `*/area`, `**/@uom`, `{uri}*/**`.
///
/// The first step is the layer, the rest the path from the feature root.
/// In a step, `*` matches any run of characters and `?` one character; a
/// whole step of `**` matches any number of steps, including none. A step
/// without `{uri}` matches names in any namespace. Attributes are steps that
/// start with `@`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathPattern {
    pub raw: String,
}

/// One step of a pattern or a path: optional namespace and a local part.
struct Step<'a> {
    attribute: bool,
    ns: Option<&'a str>,
    local: &'a str,
}

impl<'a> Step<'a> {
    /// `name`, `@name`, `{uri}name`, `@{uri}name`. `None` if a brace is unclosed.
    fn parse(step: &'a str) -> Option<Self> {
        let (attribute, rest) = match step.strip_prefix('@') {
            Some(rest) => (true, rest),
            None => (false, step),
        };
        match rest.strip_prefix('{') {
            Some(qualified) => {
                let (ns, local) = qualified.split_once('}')?;
                Some(Step { attribute, ns: Some(ns), local })
            }
            None => Some(Step { attribute, ns: None, local: rest }),
        }
    }
}

impl PathPattern {
    pub fn parse(raw: &str) -> crate::Result<Self> {
        let error = |message: &str| crate::Error::Pattern {
            pattern: raw.to_string(),
            message: message.to_string(),
        };
        let steps = split_steps(raw).ok_or_else(|| error("unclosed `{`"))?;
        if steps.is_empty() {
            return Err(error("empty pattern"));
        }
        for step in &steps {
            if step.is_empty() {
                return Err(error("empty step"));
            }
            if *step == "**" {
                continue;
            }
            let parsed = Step::parse(step).ok_or_else(|| error("unclosed `{`"))?;
            if parsed.local.is_empty() {
                return Err(error("a step has no local name"));
            }
            if parsed.local.contains(['{', '}']) {
                return Err(error("unbalanced braces"));
            }
        }
        Ok(PathPattern { raw: raw.to_string() })
    }

    /// `path` is the element path from the feature root, e.g. `["idIIP", "lokalnyId"]`.
    /// Names may be local (`idIIP`, `@uom`) or in Clark notation (`{uri}idIIP`).
    pub fn matches(&self, layer: &str, path: &[&str]) -> bool {
        let Some(steps) = split_steps(&self.raw) else {
            return false;
        };
        let mut names = Vec::with_capacity(path.len() + 1);
        names.push(layer);
        names.extend_from_slice(path);
        match_steps(&steps, &names)
    }

    /// Higher = more specific (used to pick the winning override).
    ///
    /// Per step: a literal name 10 (+ 2 with a namespace), a partial glob 5,
    /// `*` 2, `**` 0.
    pub fn specificity(&self) -> u32 {
        let Some(steps) = split_steps(&self.raw) else {
            return 0;
        };
        steps
            .iter()
            .map(|step| {
                if *step == "**" {
                    return 0;
                }
                let Some(parsed) = Step::parse(step) else {
                    return 0;
                };
                let ns_bonus = if parsed.ns.is_some() { 2 } else { 0 };
                let base = if parsed.local == "*" {
                    2
                } else if parsed.local.contains(['*', '?']) {
                    5
                } else {
                    10
                };
                base + ns_bonus
            })
            .sum()
    }
}

/// Split at `/` outside `{…}` (namespace URIs contain slashes).
fn split_steps(raw: &str) -> Option<Vec<&str>> {
    if raw.is_empty() {
        return Some(Vec::new());
    }
    let mut steps = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in raw.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.checked_sub(1)?,
            '/' if depth == 0 => {
                steps.push(&raw[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    steps.push(&raw[start..]);
    Some(steps)
}

fn match_steps(steps: &[&str], names: &[&str]) -> bool {
    match steps.split_first() {
        None => names.is_empty(),
        Some((&"**", rest)) => (0..=names.len()).any(|skip| match_steps(rest, &names[skip..])),
        Some((step, rest)) => match names.split_first() {
            Some((name, names)) => match_step(step, name) && match_steps(rest, names),
            None => false,
        },
    }
}

fn match_step(pattern: &str, name: &str) -> bool {
    let (Some(pattern), Some(name)) = (Step::parse(pattern), Step::parse(name)) else {
        return false;
    };
    if pattern.attribute != name.attribute {
        return false;
    }
    if let Some(ns) = pattern.ns
        && name.ns != Some(ns) {
            return false;
        }
    wildcard(pattern.local.as_bytes(), name.local.as_bytes())
}

/// `*` and `?` glob over bytes (names are matched whole).
pub(crate) fn wildcard(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(b'*') => {
                backtrack = Some((p, t));
                p += 1;
            }
            Some(&c) if c == b'?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match backtrack {
                Some((bp, bt)) => {
                    p = bp + 1;
                    t = bt + 1;
                    backtrack = Some((bp, bt + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}
