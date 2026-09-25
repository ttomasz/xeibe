//! Coordinate carriers: `pos`, `posList`, `coordinates`, `coord`.
//!
//! The functions here work on the element text alone. Their errors carry a
//! placeholder location, which the element parsers replace with
//! [`crate::Error::at`].

use crate::error::Error;
use crate::model::{Coords, Dim};

/// `gml:coordinates` separators (GML 2.1.2 §4.3.1 defaults: `.`, `,`, space).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatesFormat {
    pub decimal: String,
    pub cs: String,
    pub ts: String,
}

impl Default for CoordinatesFormat {
    fn default() -> Self {
        CoordinatesFormat { decimal: ".".into(), cs: ",".into(), ts: " ".into() }
    }
}

impl CoordinatesFormat {
    /// **[GDAL]** Each separator must be a single character that is not a
    /// digit; anything else is an error rather than a guess.
    fn validate(&self) -> crate::Result<(char, char, char)> {
        let single = |value: &str, attribute: &str| {
            let mut chars = value.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_ascii_digit() => Ok(c),
                _ => Err(Error::invalid_coordinates(format!(
                    "wrong value {value:?} for the {attribute} attribute of <coordinates>"
                ))),
            }
        };
        Ok((single(&self.decimal, "decimal")?, single(&self.cs, "cs")?, single(&self.ts, "ts")?))
    }
}

/// Parse `posList`/`pos` text. `dimension` from `srsDimension` (or the 3.0
/// `dimension` attribute), the CRS, `count`, or 2 — in that order.
///
/// `count`, when given, must match the number of positions. An empty text is
/// an empty sequence.
pub fn parse_pos_list(text: &str, dimension: usize, count: Option<usize>) -> crate::Result<Coords> {
    let dim = Dim::from_size(dimension).ok_or_else(|| {
        Error::invalid_coordinates(format!("unsupported dimension {dimension} (2 or 3 expected)"))
    })?;
    let mut values = Vec::new();
    for token in text.split_ascii_whitespace() {
        values.push(parse_number(token)?);
    }
    if values.len() % dimension != 0 {
        return Err(Error::invalid_coordinates(format!(
            "{} values are not a whole number of {dimension}D positions",
            values.len()
        )));
    }
    let positions = values.len() / dimension;
    if let Some(count) = count.filter(|count| *count != positions) {
        return Err(Error::invalid_coordinates(format!(
            "count=\"{count}\" but {positions} positions given"
        )));
    }
    Ok(Coords { dim: Some(dim), values })
}

/// Parse `gml:coordinates` text: tuples separated by `ts`, ordinates by `cs`,
/// with `decimal` as the decimal mark.
///
/// Lenient where GDAL is: when `ts` is whitespace, any run of whitespace
/// separates tuples. With the default separators and no comma at all, the
/// text is read as whitespace-separated numbers: 3 values are one 3D
/// position, otherwise pairs. **[GDAL]** A mix of 2D and 3D tuples becomes 3D,
/// with Z = 0 for the 2D ones.
pub fn parse_coordinates(text: &str, format: &CoordinatesFormat) -> crate::Result<Coords> {
    let (decimal, cs, ts) = format.validate()?;
    let text = text.trim();
    if text.is_empty() {
        return Ok(Coords::default());
    }

    if cs == ',' && ts.is_whitespace() && decimal == '.' && !text.contains(',') {
        let mut values = Vec::new();
        parse_numbers(text, &mut values)?;
        let dimension = if values.len() == 3 { 3 } else { 2 };
        return parse_values(values, dimension);
    }

    // One pass straight into the output: tuples stay 2D until the first 3D
    // one, which pads everything before it (and any later 2D tuple) with Z = 0.
    let mut coords = Coords::new(Dim::Xy);
    if ts.is_whitespace() {
        for tuple in text.split_whitespace() {
            push_tuple(&mut coords, tuple, cs, decimal)?;
        }
    } else {
        for tuple in text.split(ts).map(str::trim).filter(|tuple| !tuple.is_empty()) {
            push_tuple(&mut coords, tuple, cs, decimal)?;
        }
    }
    Ok(coords)
}

/// Parse one `coordinates` tuple onto `coords` (see [`parse_coordinates`]).
fn push_tuple(coords: &mut Coords, tuple: &str, cs: char, decimal: char) -> crate::Result<()> {
    let mut row = [0.0; 3];
    let mut n = 0;
    let mut parse = |ordinate: &str| -> crate::Result<()> {
        if n < 3 {
            row[n] = if decimal == '.' {
                parse_number(ordinate)?
            } else {
                parse_number(&ordinate.replace(decimal, "."))?
            };
        }
        n += 1;
        Ok(())
    };
    if cs.is_whitespace() {
        tuple.split_whitespace().try_for_each(&mut parse)?;
    } else {
        tuple.split(cs).map(str::trim).try_for_each(&mut parse)?;
    }
    if !(2..=3).contains(&n) {
        return Err(Error::invalid_coordinates(format!(
            "corrupt <coordinates> value: tuple {tuple:?} has {n} ordinates"
        )));
    }
    if n == 3 && coords.dim == Some(Dim::Xy) {
        let mut padded = Vec::with_capacity(coords.values.len() / 2 * 3 + 3);
        for position in coords.values.as_chunks::<2>().0 {
            padded.extend([position[0], position[1], 0.0]);
        }
        coords.values = padded;
        coords.dim = Some(Dim::Xyz);
    }
    coords.values.extend_from_slice(&row[..n]);
    if n < coords.size() {
        coords.values.push(0.0);
    }
    Ok(())
}

/// Swap the first two ordinates of every position in place.
pub fn swap_xy(coords: &mut Coords) {
    let size = coords.size();
    for position in coords.values.chunks_exact_mut(size) {
        position.swap(0, 1);
    }
}

/// Append the whitespace-separated numbers of `text` to `out`.
pub(crate) fn parse_numbers(text: &str, out: &mut Vec<f64>) -> crate::Result<()> {
    for token in text.split_ascii_whitespace() {
        out.push(parse_number(token)?);
    }
    Ok(())
}

/// One number, in any form the XML Schema `double` type allows, plus `+`.
pub(crate) fn parse_number(token: &str) -> crate::Result<f64> {
    fast_float2::parse::<f64, _>(token)
        .map_err(|_| Error::invalid_coordinates(format!("{token:?} is not a number")))
}

fn parse_values(values: Vec<f64>, dimension: usize) -> crate::Result<Coords> {
    if values.len() < 2 || !values.len().is_multiple_of(dimension) {
        return Err(Error::invalid_coordinates(format!(
            "corrupt <coordinates> value: {} ordinates",
            values.len()
        )));
    }
    Ok(Coords { dim: Dim::from_size(dimension), values })
}
