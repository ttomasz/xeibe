//! `Envelope` (`lowerCorner`/`upperCorner`, legacy `pos` pair/`coordinates`) and GML 2 `Box`.

use xeibe_core::reader::GmlReader;

use crate::model::Envelope;

pub fn parse_envelope(reader: &mut GmlReader<'_>, swap: bool) -> crate::Result<Envelope> {
    todo!()
}
