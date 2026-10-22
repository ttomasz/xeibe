//! WKT's syntax, shared by every version: `KEYWORD[arg, …]` (or round
//! brackets), whose arguments are quoted strings, numbers, bare words
//! (`north`, `Cartesian`) and nested nodes.

use crate::error::Error;

/// How deep nodes may nest. EPSG's deepest CRS (a compound CRS over a
/// projected one) nests 6 levels; this only stops runaway input.
const MAX_DEPTH: usize = 64;

/// One `KEYWORD[…]`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    /// Upper-cased: keywords are case-insensitive.
    pub(crate) keyword: String,
    pub(crate) args: Vec<Arg>,
    /// Byte offset of the keyword in the text, for error messages.
    pub(crate) offset: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Arg {
    /// A quoted string, with `""` unescaped.
    Text(String),
    Number(f64),
    /// A bare word: an enumeration value such as `north` or `ellipsoidal`.
    Word(String),
    Node(Node),
}

/// Parses one WKT object, which must be all of `text` apart from white space.
pub(crate) fn parse(text: &str) -> Result<Node, Error> {
    let mut p = Parser { text, pos: 0 };
    p.skip_space();
    let node = p.node(0)?;
    p.skip_space();
    if p.pos < text.len() {
        return Err(p.error("text after the end of the WKT"));
    }
    Ok(node)
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn bytes(&self) -> &[u8] {
        self.text.as_bytes()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes().get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn error(&self, message: &str) -> Error {
        let near: String = self.text[self.pos..].chars().take(30).collect();
        Error::Wkt {
            offset: self.pos,
            message: if near.is_empty() {
                format!("{message}, at the end")
            } else {
                format!("{message}, at {near:?}")
            },
        }
    }

    fn word(&mut self) -> &str {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            self.pos += 1;
        }
        &self.text[start..self.pos]
    }

    /// A keyword and its bracketed arguments.
    fn node(&mut self, depth: usize) -> Result<Node, Error> {
        if depth > MAX_DEPTH {
            return Err(self.error("nodes nested too deeply"));
        }
        let offset = self.pos;
        if !self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
            return Err(self.error("expected a keyword"));
        }
        let keyword = self.word().to_ascii_uppercase();
        self.skip_space();
        let close = match self.peek() {
            Some(b'[') => b']',
            Some(b'(') => b')',
            _ => return Err(self.error(&format!("expected [ after {keyword}"))),
        };
        self.pos += 1;
        let mut args = Vec::new();
        self.skip_space();
        if self.peek() == Some(close) {
            self.pos += 1;
            return Ok(Node {
                keyword,
                args,
                offset,
            });
        }
        loop {
            self.skip_space();
            args.push(self.arg(depth)?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b) if b == close => {
                    self.pos += 1;
                    return Ok(Node {
                        keyword,
                        args,
                        offset,
                    });
                }
                _ => {
                    return Err(
                        self.error(&format!("expected , or {} in {keyword}", close as char))
                    );
                }
            }
        }
    }

    fn arg(&mut self, depth: usize) -> Result<Arg, Error> {
        match self.peek() {
            Some(b'"') => self.quoted().map(Arg::Text),
            Some(b) if b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.') => {
                self.number().map(Arg::Number)
            }
            Some(b) if b.is_ascii_alphabetic() => {
                let start = self.pos;
                let word = self.word().to_owned();
                self.skip_space();
                if matches!(self.peek(), Some(b'[' | b'(')) {
                    self.pos = start;
                    self.node(depth + 1).map(Arg::Node)
                } else {
                    Ok(Arg::Word(word))
                }
            }
            _ => Err(self.error("expected a value")),
        }
    }

    fn quoted(&mut self) -> Result<String, Error> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let rest = &self.text[self.pos..];
            let Some(at) = rest.find('"') else {
                return Err(self.error("unterminated string"));
            };
            out.push_str(&rest[..at]);
            self.pos += at + 1;
            if self.peek() == Some(b'"') {
                out.push('"');
                self.pos += 1;
            } else {
                return Ok(out);
            }
        }
    }

    fn number(&mut self) -> Result<f64, Error> {
        let start = self.pos;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.pos += 1;
        }
        while self.peek().is_some_and(|b| b.is_ascii_digit() || b == b'.') {
            self.pos += 1;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let token = &self.text[start..self.pos];
        token.parse().map_err(|_| {
            self.pos = start;
            self.error("not a number")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_strings_numbers_and_words() {
        let n = parse(r#" axis ( "Easting ""E""", east, ORDER[1] ) "#).unwrap();
        assert_eq!(n.keyword, "AXIS");
        assert_eq!(n.offset, 1);
        assert_eq!(n.args[0], Arg::Text("Easting \"E\"".into()));
        assert_eq!(n.args[1], Arg::Word("east".into()));
        let Arg::Node(order) = &n.args[2] else {
            panic!()
        };
        assert_eq!(order.args, [Arg::Number(1.0)]);
        let n = parse("BBOX[-90,-1.5E2,+9e1,.5]").unwrap();
        assert_eq!(n.args, [-90.0, -150.0, 90.0, 0.5].map(Arg::Number).to_vec());
    }

    #[test]
    fn errors_say_where() {
        let e = parse(r#"GEOGCRS["x",DATUM["y"]"#).unwrap_err();
        assert!(matches!(e, Error::Wkt { offset: 22, .. }), "{e}");
        let e = parse(r#"ID["EPSG",1]]"#).unwrap_err();
        assert!(e.to_string().contains("after the end"), "{e}");
        assert!(parse(r#"ID["EPSG",1)"#).is_err());
        assert!(parse(r#"ID["EPSG,1]"#).is_err());
        assert!(parse("ID[1-2]").is_err());
        let deep = "A[".repeat(100) + &"]".repeat(100);
        assert!(parse(&deep).is_err());
    }
}
