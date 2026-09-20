//! An independent ISO WKB reader, used to check what `xeibe-geom` writes and
//! what ends up in `geoarrow.wkb` columns.
//!
//! ISO codes: `type = dimension * 1000 + base`, with dimension 0 = XY, 1 = Z,
//! 2 = M, 3 = ZM, and the curve types 8…12. EWKB flag bits are rejected: the
//! project writes ISO WKB (see `docs/geometry.md`).

use crate::wkt::{Coord, G};

/// Decode one WKB value. The whole input must be consumed.
pub fn decode(bytes: &[u8]) -> Result<G, String> {
    let mut reader = Reader { bytes, pos: 0 };
    let geometry = reader.geometry()?;
    if reader.pos != bytes.len() {
        return Err(format!(
            "{} trailing bytes after the geometry",
            bytes.len() - reader.pos
        ));
    }
    Ok(geometry)
}

/// The ISO type code of the outermost geometry (e.g. 1001 for `POINT Z`).
pub fn type_code(bytes: &[u8]) -> Result<u32, String> {
    let mut reader = Reader { bytes, pos: 0 };
    let big_endian = reader.byte_order()?;
    reader.u32(big_endian)
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte_order(&mut self) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(true),
            1 => Ok(false),
            other => Err(format!("invalid byte order {other}")),
        }
    }

    fn u8(&mut self) -> Result<u8, String> {
        let byte = *self.bytes.get(self.pos).ok_or("unexpected end of WKB")?;
        self.pos += 1;
        Ok(byte)
    }

    fn u32(&mut self, big_endian: bool) -> Result<u32, String> {
        let end = self.pos + 4;
        let slice = self.bytes.get(self.pos..end).ok_or("unexpected end of WKB")?;
        let array: [u8; 4] = slice.try_into().unwrap();
        self.pos = end;
        Ok(if big_endian {
            u32::from_be_bytes(array)
        } else {
            u32::from_le_bytes(array)
        })
    }

    fn f64(&mut self, big_endian: bool) -> Result<f64, String> {
        let end = self.pos + 8;
        let slice = self.bytes.get(self.pos..end).ok_or("unexpected end of WKB")?;
        let array: [u8; 8] = slice.try_into().unwrap();
        self.pos = end;
        Ok(if big_endian {
            f64::from_be_bytes(array)
        } else {
            f64::from_le_bytes(array)
        })
    }

    fn coord(&mut self, big_endian: bool, ordinates: usize) -> Result<Coord, String> {
        (0..ordinates).map(|_| self.f64(big_endian)).collect()
    }

    fn coords(&mut self, big_endian: bool, ordinates: usize) -> Result<Vec<Coord>, String> {
        let count = self.u32(big_endian)? as usize;
        (0..count).map(|_| self.coord(big_endian, ordinates)).collect()
    }

    fn members(&mut self, big_endian: bool) -> Result<Vec<G>, String> {
        let count = self.u32(big_endian)? as usize;
        (0..count).map(|_| self.geometry()).collect()
    }

    fn geometry(&mut self) -> Result<G, String> {
        let big_endian = self.byte_order()?;
        let code = self.u32(big_endian)?;
        if code & 0xE000_0000 != 0 {
            return Err(format!("EWKB flags set in type code {code:#x}; expected ISO WKB"));
        }
        let base = code % 1000;
        let ordinates = match code / 1000 {
            0 => 2,
            1 => 3,             // Z
            2 => return Err("M geometries are not produced by this project".into()),
            3 => return Err("ZM geometries are not produced by this project".into()),
            other => return Err(format!("unknown dimension {other} in type code {code}")),
        };
        Ok(match base {
            1 => {
                let coord = self.coord(big_endian, ordinates)?;
                if coord.iter().all(|v| v.is_nan()) {
                    G::Point(None)
                } else {
                    G::Point(Some(coord))
                }
            }
            2 => G::LineString(self.coords(big_endian, ordinates)?),
            8 => G::CircularString(self.coords(big_endian, ordinates)?),
            3 => {
                let rings = self.u32(big_endian)? as usize;
                let mut out = Vec::with_capacity(rings);
                for _ in 0..rings {
                    out.push(self.coords(big_endian, ordinates)?);
                }
                G::Polygon(out)
            }
            9 => G::CompoundCurve(self.members(big_endian)?),
            10 => G::CurvePolygon(self.members(big_endian)?),
            4 => G::MultiPoint(self.members(big_endian)?),
            5 => G::MultiLineString(self.members(big_endian)?),
            6 => G::MultiPolygon(self.members(big_endian)?),
            7 => G::GeometryCollection(self.members(big_endian)?),
            11 => G::MultiCurve(self.members(big_endian)?),
            12 => G::MultiSurface(self.members(big_endian)?),
            other => return Err(format!("unsupported WKB geometry type {other}")),
        })
    }
}
