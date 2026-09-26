//! `Envelope` (`lowerCorner`/`upperCorner`, legacy `pos` pair/`coordinates`) and GML 2 `Box`.

use xeibe_core::reader::GmlReader;

use super::primitives::append;
use super::{CrsDimensions, Elem, ParseContext, Parser, Scope, current_element};
use crate::model::{Coords, Envelope};
use crate::options::GeometryOptions;

/// Parse the `Envelope`/`Box` whose `Start` the reader has just returned.
/// Consumes the element. `swap` applies an axis decision to the corners.
pub fn parse_envelope(reader: &mut GmlReader<'_>, swap: bool) -> crate::Result<Envelope> {
    let elem = current_element(reader)?;
    let options = GeometryOptions::default();
    let dimensions = CrsDimensions::default();
    let mut parser = Parser::new(&options, None, &ParseContext::default(), &dimensions);
    match parser.envelope(reader, &elem, Scope::default()) {
        Ok(mut envelope) => {
            if swap {
                swap_corners(&mut envelope);
            }
            Ok(envelope)
        }
        Err(error) => {
            let _ = parser.recover(reader);
            Err(error)
        }
    }
}

/// Swap the first two ordinates of both corners.
pub(super) fn swap_corners(envelope: &mut Envelope) {
    for corner in [&mut envelope.lower, &mut envelope.upper] {
        if corner.len() >= 2 {
            corner.swap(0, 1);
        }
    }
}

impl Parser<'_> {
    /// The corners as written: `lowerCorner`/`upperCorner`, or the
    /// deprecated two positions in `pos`, `coordinates` or `coord`
    /// (07-036 §10.1.4.6).
    pub(super) fn envelope(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<Envelope> {
        let scope = self.enter(elem, scope);
        let mut lower = None;
        let mut upper = None;
        let mut positions = Coords::default();
        while let Some(child) = self.next_child(reader)? {
            if child.is("lowerCorner") || child.is("upperCorner") {
                let corner = self.pos_list(reader, &child, scope, true)?;
                let corner = corner.first().map(<[f64]>::to_vec);
                if child.local() == "lowerCorner" {
                    lower = corner;
                } else {
                    upper = corner;
                }
            } else if let Some(carried) = self.carrier(reader, &child, scope)? {
                append(&mut positions, carried);
            } else {
                self.skip(reader)?;
            }
        }
        if lower.is_none() && upper.is_none() && positions.len() == 2 {
            lower = Some(positions.get(0).to_vec());
            upper = Some(positions.get(1).to_vec());
        }
        match (lower, upper) {
            (Some(lower), Some(upper)) => {
                Ok(Envelope { lower, upper, srs_name: elem.attrs.srs_name.clone() })
            }
            _ => Err(self.invalid(reader, format!("{} needs a lower and an upper corner", elem.local()))),
        }
    }
}
