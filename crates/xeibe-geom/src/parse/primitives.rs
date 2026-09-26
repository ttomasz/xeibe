//! `Point`, `LineString`, `LinearRing`, and the coordinate carriers in
//! context: `pos`, `posList`, `coordinates`, `coord`, `pointProperty`/`pointRep`.

use std::sync::LazyLock;

use xeibe_core::reader::GmlReader;

use super::assemble::{DimensionInputs, check_line_string, check_linear_ring, effective_dimension};
use super::coords::{CoordinatesFormat, parse_coordinates, parse_number};
use super::{Elem, Parser, Scope};
use crate::error::Error;
use crate::model::{Coords, Dim, LineString, Point};

/// `coordinates` without `decimal`/`cs`/`ts`, shared so the common case
/// allocates nothing.
static DEFAULT_FORMAT: LazyLock<CoordinatesFormat> = LazyLock::new(CoordinatesFormat::default);

impl Parser<'_> {
    pub(super) fn point(&mut self, reader: &mut GmlReader<'_>, elem: &Elem, scope: Scope) -> crate::Result<Point> {
        let scope = self.enter(elem, scope);
        let coords = self.positions(reader, scope, false)?;
        match coords.len() {
            0 => Ok(Point { coord: None }),
            1 => Ok(Point { coord: Some(coords.values) }),
            n => Err(self.position_count(reader, "Point", n, "1")),
        }
    }

    pub(super) fn line_string(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        scope: Scope,
    ) -> crate::Result<LineString> {
        let scope = self.enter(elem, scope);
        let coords = self.positions(reader, scope, true)?;
        self.check_line_string(reader, "LineString", &coords)?;
        Ok(LineString { coords })
    }

    pub(super) fn check_line_string(
        &self,
        reader: &GmlReader<'_>,
        element: &'static str,
        coords: &Coords,
    ) -> crate::Result<()> {
        check_line_string(coords).map_err(|_| self.position_count(reader, element, coords.len(), "at least 2"))
    }

    pub(super) fn linear_ring(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        scope: Scope,
    ) -> crate::Result<LineString> {
        let scope = self.enter(elem, scope);
        let mut coords = self.positions(reader, scope, true)?;
        match check_linear_ring(&mut coords) {
            Ok(warnings) => self.warnings.extend(warnings),
            Err(_) => return Err(self.position_count(reader, "LinearRing", coords.len(), "at least 4")),
        }
        Ok(LineString { coords })
    }

    /// The positions of the current element, from all its coordinate
    /// carriers (mixing them is allowed). `points` also accepts inline
    /// `pointProperty`/`pointRep`. Other children are skipped.
    pub(super) fn positions(
        &mut self,
        reader: &mut GmlReader<'_>,
        scope: Scope,
        points: bool,
    ) -> crate::Result<Coords> {
        let mut coords = Coords::default();
        while let Some(child) = self.next_child(reader)? {
            if let Some(carried) = self.carrier(reader, &child, scope)? {
                append(&mut coords, carried);
            } else if points && (child.is("pointProperty") || child.is("pointRep")) {
                let mut found = None;
                self.members(reader, &child, scope, |parser, reader, member, scope| {
                    if !member.is("Point") {
                        return Err(parser.wrong_kind(reader, &member, "a point"));
                    }
                    found = parser.point(reader, &member, scope)?.coord;
                    Ok(())
                })?;
                if let Some(coord) = found {
                    coords.push(&coord);
                }
            } else {
                self.skip(reader)?;
            }
        }
        Ok(coords)
    }

    /// Read `child` if it is a coordinate carrier (`None`, unread, otherwise).
    pub(super) fn carrier(
        &mut self,
        reader: &mut GmlReader<'_>,
        child: &Elem,
        scope: Scope,
    ) -> crate::Result<Option<Coords>> {
        if !child.name.is_gml() {
            return Ok(None);
        }
        let coords = match child.local() {
            "pos" => self.pos_list(reader, child, scope, true)?,
            "posList" => self.pos_list(reader, child, scope, false)?,
            "coordinates" => self.coordinates(reader, child)?,
            "coord" => self.coord(reader)?,
            _ => return Ok(None),
        };
        Ok(Some(coords))
    }

    /// `pos` (`single`) or `posList`, with the dimension rule.
    pub(super) fn pos_list(
        &mut self,
        reader: &mut GmlReader<'_>,
        elem: &Elem,
        scope: Scope,
        single: bool,
    ) -> crate::Result<Coords> {
        let crs = match &elem.attrs.srs_name {
            Some(srs) => self.crs_dimension(srs),
            None => scope.crs_dimension,
        };
        let mut values = Vec::new();
        self.read_numbers(reader, &mut values)?;
        if values.is_empty() {
            return Ok(Coords::default());
        }
        let inputs = DimensionInputs {
            own: elem.attrs.srs_dimension.or(elem.attrs.dimension),
            inherited: scope.srs_dimension,
            crs,
            count: elem.attrs.count,
            single_position: single,
        };
        let (dimension, warning) = effective_dimension(inputs, values.len());
        self.warnings.extend(warning);
        let element = if single { "pos" } else { "posList" };
        let invalid = |message: String| Error::InvalidCoordinates { location: reader.location(), message };
        let dim = Dim::from_size(dimension)
            .ok_or_else(|| invalid(format!("unsupported dimension {dimension} in {element} (2 or 3 expected)")))?;
        if values.len() % dimension != 0 {
            return Err(invalid(format!(
                "{} values in {element} are not a whole number of {dimension}D positions",
                values.len()
            )));
        }
        let positions = values.len() / dimension;
        if single && positions != 1 {
            return Err(self.position_count(reader, "pos", positions, "1"));
        }
        if let Some(count) = elem.attrs.count.filter(|&count| count != positions) {
            return Err(invalid(format!("count=\"{count}\" but {positions} positions in {element}")));
        }
        Ok(Coords { dim: Some(dim), values })
    }

    /// `coordinates` with its `decimal`/`cs`/`ts`.
    fn coordinates(&mut self, reader: &mut GmlReader<'_>, elem: &Elem) -> crate::Result<Coords> {
        let text = self.read_text(reader)?;
        let attrs = &elem.attrs;
        let custom;
        let format = if attrs.decimal.is_none() && attrs.cs.is_none() && attrs.ts.is_none() {
            &*DEFAULT_FORMAT
        } else {
            let default = &*DEFAULT_FORMAT;
            custom = CoordinatesFormat {
                decimal: attrs.decimal.clone().unwrap_or_else(|| default.decimal.clone()),
                cs: attrs.cs.clone().unwrap_or_else(|| default.cs.clone()),
                ts: attrs.ts.clone().unwrap_or_else(|| default.ts.clone()),
            };
            &custom
        };
        parse_coordinates(&text, format).map_err(|error| error.at(reader.location()))
    }

    /// GML 2 `coord` with `X`, `Y` and optional `Z`.
    fn coord(&mut self, reader: &mut GmlReader<'_>) -> crate::Result<Coords> {
        let mut xyz = [None; 3];
        while let Some(child) = self.next_child(reader)? {
            let axis = match child.local() {
                "X" => 0,
                "Y" => 1,
                "Z" => 2,
                _ => {
                    self.skip(reader)?;
                    continue;
                }
            };
            let text = self.read_text(reader)?;
            let value = parse_number(text.trim()).map_err(|error| error.at(reader.location()))?;
            xyz[axis] = Some(value);
        }
        let invalid = |message: &str| Error::InvalidCoordinates {
            location: reader.location(),
            message: message.into(),
        };
        let (Some(x), Some(y)) = (xyz[0], xyz[1]) else {
            return Err(invalid("a coord needs X and Y"));
        };
        let mut coords = match xyz[2] {
            Some(_) => Coords::new(Dim::Xyz),
            None => Coords::new(Dim::Xy),
        };
        coords.values.extend([x, y]);
        coords.values.extend(xyz[2]);
        Ok(coords)
    }
}

/// Append `other` to `coords`, taking it over whole when `coords` is empty.
pub(super) fn append(coords: &mut Coords, other: Coords) {
    if other.is_empty() {
        return;
    }
    if coords.is_empty() {
        *coords = other;
    } else if coords.size() == other.size() {
        coords.values.extend_from_slice(&other.values);
    } else {
        for position in other.positions() {
            coords.push(position);
        }
    }
}
