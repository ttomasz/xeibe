//! A geometry value for test assertions, with a WKT reader and writer.
//!
//! [`G`] covers the simple-feature types and the ISO curve types, which is what
//! `xeibe-geom` produces. Tests build a `G` from the library's geometry (or from
//! WKB, see [`crate::wkb`]) and compare it with a `G` parsed from a WKT string.
//!
//! Two comparisons are offered:
//!
//! - [`assert_wkt`] compares the structure exactly (with a tolerance on the
//!   numbers). Use it to pin down which type the parser is expected to produce.
//! - [`assert_wkt_canonical`] compares [`G::canonical`] forms, which merge
//!   adjacent parts of a compound curve and turn curve-free curve types into
//!   their simple-feature equivalents. Use it against GDAL's output, where those
//!   choices differ from ours by design (see `docs/geometry.md`).

use std::fmt;

/// A coordinate: 2 or 3 ordinates (x, y[, z]).
pub type Coord = Vec<f64>;

/// A ring: a closed sequence of coordinates.
pub type Ring = Vec<Coord>;

#[derive(Debug, Clone, PartialEq)]
pub enum G {
    /// `None` is `POINT EMPTY`.
    Point(Option<Coord>),
    LineString(Vec<Coord>),
    CircularString(Vec<Coord>),
    /// Parts are `LineString` and `CircularString`.
    CompoundCurve(Vec<G>),
    Polygon(Vec<Ring>),
    /// Rings are `LineString`, `CircularString` or `CompoundCurve`.
    CurvePolygon(Vec<G>),
    MultiPoint(Vec<G>),
    MultiLineString(Vec<G>),
    MultiPolygon(Vec<G>),
    MultiCurve(Vec<G>),
    MultiSurface(Vec<G>),
    GeometryCollection(Vec<G>),
    /// A type we don't model (`POLYHEDRALSURFACE`, `TIN`, …): only the tag is kept.
    Other(String),
}

impl G {
    /// Parse WKT. `TRIANGLE` is read as a `Polygon`, as it is only a ring.
    pub fn parse(wkt: &str) -> Result<G, String> {
        let tokens = tokenize(wkt)?;
        let mut parser = Parser { tokens, pos: 0 };
        let geometry = parser.geometry()?;
        if parser.pos != parser.tokens.len() {
            return Err(format!("trailing tokens at {} in {wkt:?}", parser.pos));
        }
        Ok(geometry)
    }

    /// The WKT tag, as GDAL prints it.
    pub fn tag(&self) -> &str {
        match self {
            G::Point(_) => "POINT",
            G::LineString(_) => "LINESTRING",
            G::CircularString(_) => "CIRCULARSTRING",
            G::CompoundCurve(_) => "COMPOUNDCURVE",
            G::Polygon(_) => "POLYGON",
            G::CurvePolygon(_) => "CURVEPOLYGON",
            G::MultiPoint(_) => "MULTIPOINT",
            G::MultiLineString(_) => "MULTILINESTRING",
            G::MultiPolygon(_) => "MULTIPOLYGON",
            G::MultiCurve(_) => "MULTICURVE",
            G::MultiSurface(_) => "MULTISURFACE",
            G::GeometryCollection(_) => "GEOMETRYCOLLECTION",
            G::Other(tag) => tag,
        }
    }

    /// Every coordinate, in document order.
    pub fn vertices(&self) -> Vec<Coord> {
        let mut out = Vec::new();
        self.collect_vertices(&mut out);
        out
    }

    fn collect_vertices(&self, out: &mut Vec<Coord>) {
        match self {
            G::Point(Some(c)) => out.push(c.clone()),
            G::Point(None) | G::Other(_) => {}
            G::LineString(cs) | G::CircularString(cs) => out.extend(cs.iter().cloned()),
            G::Polygon(rings) => {
                for ring in rings {
                    out.extend(ring.iter().cloned());
                }
            }
            G::CompoundCurve(parts)
            | G::CurvePolygon(parts)
            | G::MultiPoint(parts)
            | G::MultiLineString(parts)
            | G::MultiPolygon(parts)
            | G::MultiCurve(parts)
            | G::MultiSurface(parts)
            | G::GeometryCollection(parts) => {
                for part in parts {
                    part.collect_vertices(out);
                }
            }
        }
    }

    /// First coordinate in document order, if any.
    pub fn first_vertex(&self) -> Option<Coord> {
        self.vertices().into_iter().next()
    }

    /// `true` if any coordinate has a Z ordinate.
    pub fn has_z(&self) -> bool {
        self.vertices().iter().any(|c| c.len() > 2)
    }

    /// Swap the first two ordinates of every coordinate.
    pub fn swapped_xy(&self) -> G {
        let mut geometry = self.clone();
        geometry.swap_xy();
        geometry
    }

    fn swap_xy(&mut self) {
        fn swap(coords: &mut [Coord]) {
            for c in coords {
                if c.len() >= 2 {
                    c.swap(0, 1);
                }
            }
        }
        match self {
            G::Point(Some(c)) => {
                if c.len() >= 2 {
                    c.swap(0, 1);
                }
            }
            G::Point(None) | G::Other(_) => {}
            G::LineString(cs) | G::CircularString(cs) => swap(cs),
            G::Polygon(rings) => rings.iter_mut().for_each(|r| swap(r)),
            G::CompoundCurve(parts)
            | G::CurvePolygon(parts)
            | G::MultiPoint(parts)
            | G::MultiLineString(parts)
            | G::MultiPolygon(parts)
            | G::MultiCurve(parts)
            | G::MultiSurface(parts)
            | G::GeometryCollection(parts) => parts.iter_mut().for_each(|p| p.swap_xy()),
        }
    }

    /// The form used when comparing with GDAL, which represents the same
    /// geometry differently in places where `docs/geometry.md` makes another
    /// choice:
    ///
    /// - adjacent parts of a compound curve with the same interpolation are
    ///   merged, and a compound curve with one part becomes that part;
    /// - a curve-free `CurvePolygon`/`MultiCurve`/`MultiSurface` becomes
    ///   `Polygon`/`MultiLineString`/`MultiPolygon` (as `Geometry::simplify_types`).
    pub fn canonical(&self) -> G {
        match self {
            G::CompoundCurve(parts) => {
                let merged = merge_parts(parts.iter().map(|p| p.canonical()).collect());
                if merged.len() == 1 {
                    merged.into_iter().next().unwrap()
                } else {
                    G::CompoundCurve(merged)
                }
            }
            G::CurvePolygon(rings) => {
                let rings: Vec<G> = rings.iter().map(|r| r.canonical()).collect();
                if rings.iter().all(|r| matches!(r, G::LineString(_))) {
                    G::Polygon(
                        rings
                            .into_iter()
                            .map(|r| match r {
                                G::LineString(cs) => cs,
                                _ => unreachable!(),
                            })
                            .collect(),
                    )
                } else {
                    G::CurvePolygon(rings)
                }
            }
            G::MultiCurve(members) => {
                let members: Vec<G> = members.iter().map(|m| m.canonical()).collect();
                if members.iter().all(|m| matches!(m, G::LineString(_))) {
                    G::MultiLineString(members)
                } else {
                    G::MultiCurve(members)
                }
            }
            G::MultiSurface(members) => {
                let members: Vec<G> = members.iter().map(|m| m.canonical()).collect();
                if members.iter().all(|m| matches!(m, G::Polygon(_))) {
                    G::MultiPolygon(members)
                } else {
                    G::MultiSurface(members)
                }
            }
            G::MultiPoint(members) => G::MultiPoint(members.iter().map(|m| m.canonical()).collect()),
            G::MultiLineString(members) => {
                G::MultiLineString(members.iter().map(|m| m.canonical()).collect())
            }
            G::MultiPolygon(members) => {
                G::MultiPolygon(members.iter().map(|m| m.canonical()).collect())
            }
            G::GeometryCollection(members) => {
                G::GeometryCollection(members.iter().map(|m| m.canonical()).collect())
            }
            other => other.clone(),
        }
    }
}

/// Merge adjacent parts with the same interpolation, dropping a repeated
/// joining coordinate.
fn merge_parts(parts: Vec<G>) -> Vec<G> {
    let mut out: Vec<G> = Vec::new();
    for part in parts {
        match (out.last_mut(), &part) {
            (Some(G::LineString(prev)), G::LineString(next))
            | (Some(G::CircularString(prev)), G::CircularString(next)) => {
                let mut rest = next.clone();
                if prev.last() == rest.first() && !rest.is_empty() {
                    rest.remove(0);
                }
                prev.extend(rest);
            }
            _ => out.push(part),
        }
    }
    out
}

// ---------------------------------------------------------------- comparison

/// Tolerance for comparing coordinates: `|a - b| <= abs + rel * max(|a|, |b|)`.
#[derive(Debug, Clone, Copy)]
pub struct Tol {
    pub abs: f64,
    pub rel: f64,
}

impl Default for Tol {
    fn default() -> Self {
        Tol { abs: 1e-9, rel: 1e-9 }
    }
}

impl Tol {
    pub fn exact() -> Self {
        Tol { abs: 0.0, rel: 0.0 }
    }

    pub fn abs(abs: f64) -> Self {
        Tol { abs, rel: 0.0 }
    }

    pub fn matches(&self, a: f64, b: f64) -> bool {
        if a.is_nan() && b.is_nan() {
            return true;
        }
        (a - b).abs() <= self.abs + self.rel * a.abs().max(b.abs())
    }
}

/// First structural or numeric difference, as a message, or `None` if equal.
pub fn diff(actual: &G, expected: &G, tol: Tol) -> Option<String> {
    diff_at("", actual, expected, tol)
}

fn diff_at(path: &str, actual: &G, expected: &G, tol: Tol) -> Option<String> {
    if actual.tag() != expected.tag() {
        return Some(format!(
            "{path}: type {} != expected {}",
            actual.tag(),
            expected.tag()
        ));
    }
    match (actual, expected) {
        (G::Point(a), G::Point(b)) => match (a, b) {
            (None, None) => None,
            (Some(a), Some(b)) => diff_coord(path, a, b, tol),
            _ => Some(format!("{path}: empty point mismatch")),
        },
        (G::LineString(a), G::LineString(b)) | (G::CircularString(a), G::CircularString(b)) => {
            diff_coords(path, a, b, tol)
        }
        (G::Polygon(a), G::Polygon(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: {} rings != expected {}", a.len(), b.len()));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (a, b))| diff_coords(&format!("{path}/ring[{i}]"), a, b, tol))
        }
        (G::Other(_), G::Other(_)) => None,
        _ => {
            let (a, b) = (members(actual), members(expected));
            if a.len() != b.len() {
                return Some(format!("{path}: {} parts != expected {}", a.len(), b.len()));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (a, b))| diff_at(&format!("{path}/[{i}]"), a, b, tol))
        }
    }
}

fn members(geometry: &G) -> &[G] {
    match geometry {
        G::CompoundCurve(parts)
        | G::CurvePolygon(parts)
        | G::MultiPoint(parts)
        | G::MultiLineString(parts)
        | G::MultiPolygon(parts)
        | G::MultiCurve(parts)
        | G::MultiSurface(parts)
        | G::GeometryCollection(parts) => parts,
        _ => &[],
    }
}

fn diff_coords(path: &str, actual: &[Coord], expected: &[Coord], tol: Tol) -> Option<String> {
    if actual.len() != expected.len() {
        return Some(format!(
            "{path}: {} coordinates != expected {}",
            actual.len(),
            expected.len()
        ));
    }
    actual
        .iter()
        .zip(expected)
        .enumerate()
        .find_map(|(i, (a, b))| diff_coord(&format!("{path}/coord[{i}]"), a, b, tol))
}

fn diff_coord(path: &str, actual: &Coord, expected: &Coord, tol: Tol) -> Option<String> {
    if actual.len() != expected.len() {
        return Some(format!(
            "{path}: {} ordinates ({}) != expected {} ({})",
            actual.len(),
            fmt_coord(actual),
            expected.len(),
            fmt_coord(expected)
        ));
    }
    if actual
        .iter()
        .zip(expected)
        .all(|(a, b)| tol.matches(*a, *b))
    {
        None
    } else {
        Some(format!(
            "{path}: ({}) != expected ({})",
            fmt_coord(actual),
            fmt_coord(expected)
        ))
    }
}

/// Assert exact structure, with the default numeric tolerance.
#[track_caller]
pub fn assert_wkt(actual: &G, expected_wkt: &str) {
    assert_wkt_tol(actual, expected_wkt, Tol::default());
}

#[track_caller]
pub fn assert_wkt_tol(actual: &G, expected_wkt: &str, tol: Tol) {
    let expected = G::parse(expected_wkt).expect("expected WKT parses");
    if let Some(message) = diff(actual, &expected, tol) {
        panic!("{message}\n  actual:   {actual}\n  expected: {expected}");
    }
}

/// Assert equality of the [`G::canonical`] forms (used against GDAL output).
#[track_caller]
pub fn assert_wkt_canonical(actual: &G, expected_wkt: &str, tol: Tol) {
    let expected = G::parse(expected_wkt).expect("expected WKT parses");
    if let Some(message) = diff(&actual.canonical(), &expected.canonical(), tol) {
        panic!("{message}\n  actual:   {actual}\n  expected: {expected}");
    }
}

// ------------------------------------------------------------------- writing

fn fmt_coord(coord: &Coord) -> String {
    coord
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn fmt_coords(coords: &[Coord]) -> String {
    coords.iter().map(fmt_coord).collect::<Vec<_>>().join(",")
}

impl fmt::Display for G {
    /// GDAL-style WKT, so failure messages can be compared with `*.gdal.txt`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let z = if self.has_z() { " Z" } else { "" };
        match self {
            G::Point(None) => write!(f, "POINT EMPTY"),
            G::Point(Some(c)) => write!(f, "POINT{z} ({})", fmt_coord(c)),
            G::LineString(cs) | G::CircularString(cs) => {
                if cs.is_empty() {
                    write!(f, "{}{z} EMPTY", self.tag())
                } else {
                    write!(f, "{}{z} ({})", self.tag(), fmt_coords(cs))
                }
            }
            G::Polygon(rings) => {
                if rings.is_empty() {
                    return write!(f, "POLYGON{z} EMPTY");
                }
                let rings: Vec<String> = rings.iter().map(|r| format!("({})", fmt_coords(r))).collect();
                write!(f, "POLYGON{z} ({})", rings.join(","))
            }
            G::Other(tag) => write!(f, "{tag}"),
            _ => {
                let parts = members(self);
                if parts.is_empty() {
                    return write!(f, "{}{z} EMPTY", self.tag());
                }
                // Members of the expected type are written without their tag,
                // as GDAL and the OGC WKT grammar do.
                let untagged = match self {
                    G::MultiPoint(_) => Untagged::Point,
                    G::CompoundCurve(_) | G::CurvePolygon(_) | G::MultiCurve(_)
                    | G::MultiLineString(_) => Untagged::LineString,
                    G::MultiPolygon(_) | G::MultiSurface(_) => Untagged::Polygon,
                    _ => Untagged::None,
                };
                let parts: Vec<String> = parts
                    .iter()
                    .map(|part| match (untagged, part) {
                        (Untagged::Point, G::Point(Some(c))) => format!("({})", fmt_coord(c)),
                        (Untagged::LineString, G::LineString(cs)) if !cs.is_empty() => {
                            format!("({})", fmt_coords(cs))
                        }
                        (Untagged::Polygon, G::Polygon(rings)) if !rings.is_empty() => {
                            let rings: Vec<String> =
                                rings.iter().map(|r| format!("({})", fmt_coords(r))).collect();
                            format!("({})", rings.join(","))
                        }
                        _ => part.to_string(),
                    })
                    .collect();
                write!(f, "{}{z} ({})", self.tag(), parts.join(","))
            }
        }
    }
}

// ------------------------------------------------------------------- parsing

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Number(f64),
    Open,
    Close,
    Comma,
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            _ => {
                let start = i;
                while i < chars.len()
                    && !chars[i].is_whitespace()
                    && !matches!(chars[i], '(' | ')' | ',')
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let upper = word.to_ascii_uppercase();
                if word.starts_with(|c: char| c.is_ascii_alphabetic())
                    && !matches!(upper.as_str(), "NAN" | "INF" | "INFINITY" | "-INF")
                {
                    tokens.push(Token::Word(upper));
                } else {
                    let value = word
                        .parse::<f64>()
                        .map_err(|_| format!("not a number: {word:?}"))?;
                    tokens.push(Token::Number(value));
                }
            }
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn expect(&mut self, token: Token) -> Result<(), String> {
        match self.next() {
            Some(found) if found == token => Ok(()),
            other => Err(format!("expected {token:?}, found {other:?}")),
        }
    }

    /// `EMPTY` after the tag (and the optional `Z`/`M`/`ZM`).
    fn empty_or_open(&mut self) -> Result<bool, String> {
        if let Some(Token::Word(word)) = self.peek()
            && matches!(word.as_str(), "Z" | "M" | "ZM")
        {
            self.pos += 1;
        }
        match self.next() {
            Some(Token::Word(word)) if word == "EMPTY" => Ok(true),
            Some(Token::Open) => Ok(false),
            other => Err(format!("expected EMPTY or '(', found {other:?}")),
        }
    }

    fn geometry(&mut self) -> Result<G, String> {
        let tag = match self.next() {
            Some(Token::Word(word)) => word,
            other => return Err(format!("expected a WKT tag, found {other:?}")),
        };
        let empty = self.empty_or_open()?;
        let geometry = match tag.as_str() {
            "POINT" => {
                if empty {
                    return Ok(G::Point(None));
                }
                let coord = self.coord()?;
                self.expect(Token::Close)?;
                return Ok(G::Point(Some(coord)));
            }
            "LINESTRING" | "CIRCULARSTRING" => {
                let coords = if empty { Vec::new() } else { self.coord_list()? };
                if tag == "LINESTRING" {
                    G::LineString(coords)
                } else {
                    G::CircularString(coords)
                }
            }
            "POLYGON" | "TRIANGLE" => {
                G::Polygon(if empty { Vec::new() } else { self.ring_list()? })
            }
            "COMPOUNDCURVE" => G::CompoundCurve(if empty {
                Vec::new()
            } else {
                self.part_list(Untagged::LineString)?
            }),
            "CURVEPOLYGON" => G::CurvePolygon(if empty {
                Vec::new()
            } else {
                self.part_list(Untagged::LineString)?
            }),
            "MULTIPOINT" => G::MultiPoint(if empty {
                Vec::new()
            } else {
                self.part_list(Untagged::Point)?
            }),
            "MULTILINESTRING" => G::MultiLineString(if empty {
                Vec::new()
            } else {
                self.part_list(Untagged::LineString)?
            }),
            "MULTICURVE" => G::MultiCurve(if empty {
                Vec::new()
            } else {
                self.part_list(Untagged::LineString)?
            }),
            "MULTIPOLYGON" => G::MultiPolygon(if empty {
                Vec::new()
            } else {
                self.part_list(Untagged::Polygon)?
            }),
            "MULTISURFACE" => G::MultiSurface(if empty {
                Vec::new()
            } else {
                self.part_list(Untagged::Polygon)?
            }),
            "GEOMETRYCOLLECTION" => {
                let mut members = Vec::new();
                if !empty {
                    loop {
                        members.push(self.geometry()?);
                        match self.next() {
                            Some(Token::Comma) => continue,
                            Some(Token::Close) => break,
                            other => return Err(format!("expected ',' or ')', found {other:?}")),
                        }
                    }
                }
                G::GeometryCollection(members)
            }
            other => {
                // A type we don't model: skip its (balanced) body.
                if !empty {
                    let mut depth = 1;
                    while depth > 0 {
                        match self.next() {
                            Some(Token::Open) => depth += 1,
                            Some(Token::Close) => depth -= 1,
                            Some(_) => {}
                            None => return Err(format!("unbalanced body of {other}")),
                        }
                    }
                }
                return Ok(G::Other(other.to_string()));
            }
        };
        Ok(geometry)
    }

    /// One coordinate: every number until the next `,` or `)`.
    fn coord(&mut self) -> Result<Coord, String> {
        let mut coord = Vec::new();
        while let Some(Token::Number(value)) = self.peek() {
            coord.push(*value);
            self.pos += 1;
        }
        if coord.len() < 2 {
            return Err(format!("coordinate with {} ordinates", coord.len()));
        }
        Ok(coord)
    }

    /// `c, c, …)` — the closing parenthesis is consumed.
    fn coord_list(&mut self) -> Result<Vec<Coord>, String> {
        let mut coords = Vec::new();
        loop {
            coords.push(self.coord()?);
            match self.next() {
                Some(Token::Comma) => continue,
                Some(Token::Close) => break,
                other => return Err(format!("expected ',' or ')', found {other:?}")),
            }
        }
        Ok(coords)
    }

    /// `(c, c, …), (…))` — the closing parenthesis is consumed.
    fn ring_list(&mut self) -> Result<Vec<Ring>, String> {
        let mut rings = Vec::new();
        loop {
            self.expect(Token::Open)?;
            rings.push(self.coord_list()?);
            match self.next() {
                Some(Token::Comma) => continue,
                Some(Token::Close) => break,
                other => return Err(format!("expected ',' or ')', found {other:?}")),
            }
        }
        Ok(rings)
    }

    /// Members of a multi/compound geometry: either tagged (`CIRCULARSTRING (…)`)
    /// or untagged (`(…)`), in which case `untagged` says what they are.
    fn part_list(&mut self, untagged: Untagged) -> Result<Vec<G>, String> {
        let mut parts = Vec::new();
        loop {
            let part = match self.peek() {
                Some(Token::Word(word)) if word == "EMPTY" => {
                    self.pos += 1;
                    match untagged {
                        Untagged::Point => G::Point(None),
                        Untagged::LineString => G::LineString(Vec::new()),
                        Untagged::Polygon | Untagged::None => G::Polygon(Vec::new()),
                    }
                }
                Some(Token::Word(_)) => self.geometry()?,
                Some(Token::Number(_)) if untagged == Untagged::Point => {
                    // `MULTIPOINT (1 2, 3 4)`
                    G::Point(Some(self.coord()?))
                }
                _ => {
                    self.expect(Token::Open)?;
                    match untagged {
                        Untagged::Point => {
                            let coord = self.coord()?;
                            self.expect(Token::Close)?;
                            G::Point(Some(coord))
                        }
                        Untagged::LineString => G::LineString(self.coord_list()?),
                        Untagged::Polygon => G::Polygon(self.ring_list()?),
                        Untagged::None => return Err("untagged member".into()),
                    }
                }
            };
            parts.push(part);
            match self.next() {
                Some(Token::Comma) => continue,
                Some(Token::Close) => break,
                other => return Err(format!("expected ',' or ')', found {other:?}")),
            }
        }
        Ok(parts)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Untagged {
    Point,
    LineString,
    Polygon,
    /// Members always carry their tag (a geometry collection).
    None,
}
