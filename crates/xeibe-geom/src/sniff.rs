//! Lightweight geometry scan used by scans and read samples: element kinds, curve
//! presence, srsName/srsDimension/axisLabels, dialect, and the first position
//! (for the axis-order range check). Does not build geometries.

use xeibe_core::reader::{GmlReader, XmlEvent};
use xeibe_core::{Dialect, QName};

use crate::dialect::DialectTracker;
use crate::epsg::CrsTable;
use crate::model::GeomKind;
use crate::parse::assemble::{DimensionInputs, crs_dimension, effective_dimension};
use crate::parse::coords::{CoordinatesFormat, parse_coordinates, parse_number};
use crate::parse::{ARC_SEGMENTS, Attrs, UNSUPPORTED, current_element, geom_kind};

#[derive(Debug, Clone, Default)]
pub struct GeometrySniff {
    /// Kind of the outermost geometry element.
    pub kinds: Vec<GeomKind>,
    pub has_curves: bool,
    pub has_unsupported: bool,
    /// The geometry's srsName, else the inherited one, else the first one inside.
    pub srs_name: Option<String>,
    /// The geometry's `srsDimension`, else the first one inside.
    pub srs_dimension: Option<u8>,
    pub axis_labels: Option<String>,
    pub dialect: Option<Dialect>,
    /// First position, as written (no axis decision applied).
    pub first_position: Option<Vec<f64>>,
    pub by_reference: bool,
    /// No position at all.
    pub empty: bool,
}

/// Called right after the reader returned a geometry's start element (as for
/// [`crate::GeometryParser::parse`]); consumes the element.
///
/// Only the first position's text is parsed; the rest of the coordinates are
/// skipped over as text.
pub fn sniff_geometry(
    reader: &mut GmlReader<'_>,
    inherited_srs: Option<&str>,
) -> crate::Result<GeometrySniff> {
    let root = current_element(reader)?;
    let table = CrsTable::builtin();
    let kind = geom_kind(&root.name);
    let mut sniff = GeometrySniff {
        kinds: vec![kind],
        has_unsupported: kind == GeomKind::Unsupported,
        ..GeometrySniff::default()
    };
    let mut dialect = DialectTracker::default();
    dialect.observe(&root.name);
    let mut first_srs: Option<String> = None;
    // (srsDimension, CRS dimension) in scope, per open element.
    let mut scopes = Vec::new();
    let scope = observe(&mut sniff, &mut first_srs, &root.attrs, (None, inherited_srs.and_then(|s| crs_dimension(s, &table))), &table);
    scopes.push(scope);
    let root_srs = root.attrs.srs_name.clone();

    while !scopes.is_empty() {
        let (name, attrs) = match reader.next_event()? {
            XmlEvent::Start { name, attrs } => (name, Attrs::read(&attrs)),
            XmlEvent::End { .. } => {
                scopes.pop();
                continue;
            }
            XmlEvent::Text(_) => continue,
            XmlEvent::Eof => {
                return Err(crate::Error::Core(xeibe_core::Error::Xml {
                    location: reader.location(),
                    message: "unexpected end of input inside a geometry".into(),
                }));
            }
        };
        dialect.observe(&name);
        let parent = *scopes.last().expect("inside the geometry");
        let scope = observe(&mut sniff, &mut first_srs, &attrs, parent, &table);
        if name.is_gml() {
            let local = &*name.local;
            if ARC_SEGMENTS.contains(&local) {
                sniff.has_curves = true;
            } else if UNSUPPORTED.contains(&local) {
                sniff.has_unsupported = true;
            }
            if sniff.first_position.is_none() && is_carrier(&name) {
                // The carrier's text (or, for `coord`, its children) ends here.
                sniff.first_position = first_position(reader, &name, &attrs, scope)?;
                continue;
            }
        }
        scopes.push(scope);
    }

    sniff.srs_name = root_srs.or_else(|| inherited_srs.map(str::to_string)).or(first_srs);
    sniff.dialect = Some(dialect.result());
    sniff.empty = sniff.first_position.is_none();
    Ok(sniff)
}

/// Record an element's attributes; returns the scope of its content.
fn observe(
    sniff: &mut GeometrySniff,
    first_srs: &mut Option<String>,
    attrs: &Attrs,
    parent: (Option<u8>, Option<u8>),
    table: &CrsTable,
) -> (Option<u8>, Option<u8>) {
    let (mut srs_dimension, mut crs) = parent;
    if let Some(srs) = &attrs.srs_name {
        crs = crs_dimension(srs, table);
        if first_srs.is_none() {
            *first_srs = Some(srs.clone());
        }
    }
    if let Some(dimension) = attrs.srs_dimension {
        srs_dimension = Some(dimension);
        sniff.srs_dimension.get_or_insert(dimension);
    }
    if sniff.axis_labels.is_none() {
        sniff.axis_labels.clone_from(&attrs.axis_labels);
    }
    sniff.by_reference |= attrs.href;
    (srs_dimension, crs)
}

fn is_carrier(name: &QName) -> bool {
    matches!(&*name.local, "pos" | "posList" | "coordinates" | "coord" | "lowerCorner")
}

/// Read a carrier up to its end tag and return its first position.
fn first_position(
    reader: &mut GmlReader<'_>,
    name: &QName,
    attrs: &Attrs,
    (srs_dimension, crs): (Option<u8>, Option<u8>),
) -> crate::Result<Option<Vec<f64>>> {
    if &*name.local == "coord" {
        return coord_position(reader);
    }
    let text = element_text(reader)?;
    let position = if &*name.local == "coordinates" {
        let format = CoordinatesFormat {
            decimal: attrs.decimal.clone().unwrap_or_else(|| ".".into()),
            cs: attrs.cs.clone().unwrap_or_else(|| ",".into()),
            ts: attrs.ts.clone().unwrap_or_else(|| " ".into()),
        };
        // Only the first tuple, unless it can't be told apart.
        let first = match format.ts.chars().next() {
            Some(ts) if !ts.is_whitespace() => text.trim().split(ts).next().unwrap_or_default(),
            _ if text.contains(',') => text.split_whitespace().next().unwrap_or_default(),
            _ => text.as_str(),
        };
        parse_coordinates(first, &format).ok().and_then(|coords| coords.first().map(<[f64]>::to_vec))
    } else {
        let single = &*name.local != "posList";
        let mut tokens = text.split_ascii_whitespace();
        let inputs = DimensionInputs {
            own: attrs.srs_dimension.or(attrs.dimension),
            inherited: srs_dimension,
            crs,
            count: attrs.count,
            single_position: single,
        };
        // The number of values only matters for rule 4 (a `pos`, or `count`).
        let values = if inputs.own.or(inputs.inherited).or(inputs.crs).is_none() && (single || inputs.count.is_some()) {
            tokens.clone().count()
        } else {
            0
        };
        let (dimension, _) = effective_dimension(inputs, values);
        let position: Option<Vec<f64>> =
            tokens.by_ref().take(dimension).map(|token| parse_number(token).ok()).collect();
        position.filter(|position| position.len() == dimension)
    };
    Ok(position)
}

/// The text of the current element, up to its end tag.
fn element_text(reader: &mut GmlReader<'_>) -> crate::Result<String> {
    let mut text = String::new();
    loop {
        match reader.next_event()? {
            XmlEvent::Text(more) => text.push_str(&more),
            XmlEvent::Start { .. } => reader.skip_element()?,
            XmlEvent::End { .. } | XmlEvent::Eof => return Ok(text),
        }
    }
}

/// GML 2 `coord`: `X`, `Y`, optional `Z`.
fn coord_position(reader: &mut GmlReader<'_>) -> crate::Result<Option<Vec<f64>>> {
    let mut xyz = [None; 3];
    loop {
        let axis = match reader.next_event()? {
            XmlEvent::Start { name, .. } => match &*name.local {
                "X" => Some(0),
                "Y" => Some(1),
                "Z" => Some(2),
                _ => None,
            },
            XmlEvent::End { .. } | XmlEvent::Eof => break,
            XmlEvent::Text(_) => continue,
        };
        match axis {
            Some(axis) => xyz[axis] = parse_number(element_text(reader)?.trim()).ok(),
            None => reader.skip_element()?,
        }
    }
    Ok(match xyz {
        [Some(x), Some(y), z] => Some([x, y].into_iter().chain(z).collect()),
        _ => None,
    })
}
