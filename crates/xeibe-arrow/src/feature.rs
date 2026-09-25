//! Parses one feature element and puts its values into the layer builder.
//!
//! The feature is read into a row of [`Value`]s first and appended only when
//! it is complete: a feature error can then skip the feature, or null its
//! geometry, without leaving half a row in the builders.
//!
//! Only what the schema's paths lead to is read; any other element is skipped
//! unparsed. List columns follow their anchor: a counter per anchor goes up
//! when an anchor element starts, and a list is padded with nulls to one
//! entry per occurrence (`docs/schema-inference.md`, "Lists and alignment").

use xeibe_core::reader::{Attributes, GmlReader, XmlEvent};
use xeibe_core::{Dialect, Location, QName, ns};
use xeibe_geom::{AxisResolver, GeometryParser, ParseContext, SrsName};
use xeibe_schema::RouteValue;

use crate::OnFeatureError;
use crate::axis::AxisDecisions;
use crate::builders::{LayerBatchBuilder, Value};
use crate::pipeline::ReadPlan;
use crate::report::{ReadReport, Warning};
use crate::route::{Item, RouteNode, Target};
use crate::value::parse_scalar;

pub struct FeatureReader<'a> {
    plan: &'a ReadPlan,
    geometry: GeometryParser<'a>,
    /// One resolver per geometry column, for the chunk's source.
    axis: Vec<AxisDecisions>,
}

/// What became of one feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureOutcome {
    Row,
    /// Skipped by `OnFeatureError::Skip` (recorded in the report).
    Skipped,
}

/// The row being read.
struct RowState {
    row: Vec<Value>,
    /// Occurrences of each anchor so far.
    counts: Vec<u32>,
    /// The first value that doesn't fit its column; the rest of the feature
    /// is still read, so the next one starts in the right place.
    error: Option<String>,
    /// The first geometry error.
    geometry_error: Option<String>,
    /// srsName of the feature's `boundedBy`, inherited by its geometries.
    srs_name: Option<String>,
    location: Location,
    warnings: Vec<Warning>,
    /// Scratch space for attribute targets.
    targets: Vec<Target>,
}

impl RowState {
    fn fail(&mut self, message: String) {
        self.error.get_or_insert(message);
    }
}

impl<'a> FeatureReader<'a> {
    pub fn new(plan: &'a ReadPlan, geometry: GeometryParser<'a>, axis: Vec<AxisDecisions>) -> Self {
        FeatureReader { plan, geometry, axis }
    }

    /// Reader positioned on the feature start element; consumes it and appends
    /// one row (unless the feature is skipped).
    pub fn read_feature(
        &self,
        reader: &mut GmlReader<'_>,
        location: Location,
        out: &mut LayerBatchBuilder,
        report: &mut ReadReport,
    ) -> crate::Result<FeatureOutcome> {
        let routes = &self.plan.routes;
        let mut state = RowState {
            row: vec![Value::Null; routes.columns.len()],
            counts: vec![0; routes.anchors],
            error: None,
            geometry_error: None,
            srs_name: None,
            location,
            warnings: Vec::new(),
            targets: Vec::new(),
        };
        let nodes = [&routes.root];
        let nil = {
            let (_, attrs) = reader.current_start().ok_or_else(|| xeibe_core::Error::Xml {
                location: reader.location(),
                message: "no feature start".into(),
            })?;
            self.start(&nodes, &attrs, &mut state)
        };
        let content_start = reader.position();
        self.content(reader, &nodes, 0, nil, content_start, &mut state)?;
        self.commit(state, out, report)
    }

    fn commit(&self, mut state: RowState, out: &mut LayerBatchBuilder, report: &mut ReadReport) -> crate::Result<FeatureOutcome> {
        let plan = self.plan;
        for warning in state.warnings.drain(..) {
            report.warn(warning);
        }
        let location = state.location.clone();
        let skip = |report: &mut ReadReport, error: crate::Error| {
            report.skipped.push((location.clone(), error.to_string()));
            Ok(FeatureOutcome::Skipped)
        };
        if let Some(message) = state.error.take() {
            let error = crate::Error::Feature { location: location.clone(), message };
            // `NullGeometry` only helps with geometry errors.
            return match plan.on_feature_error {
                OnFeatureError::Skip => skip(report, error),
                OnFeatureError::Error | OnFeatureError::NullGeometry => Err(error),
            };
        }
        if let Some(message) = state.geometry_error.take() {
            let error = crate::Error::Feature { location: location.clone(), message };
            match plan.on_feature_error {
                OnFeatureError::Error => return Err(error),
                OnFeatureError::Skip => return skip(report, error),
                // The geometry was never put, so it is null.
                OnFeatureError::NullGeometry => {}
            }
        }
        for (column, value) in plan.routes.columns.iter().zip(state.row.iter_mut()) {
            if !column.list {
                continue;
            }
            // One entry per occurrence of the anchor; null if it never occurred.
            let count = state.counts[column.anchor] as usize;
            match value {
                Value::List(items) => items.resize(count.max(items.len()), Value::Null),
                Value::Null if count > 0 => *value = Value::List(vec![Value::Null; count]),
                _ => {}
            }
        }
        for (column, value) in plan.routes.columns.iter().zip(&state.row) {
            let missing = match value {
                Value::Null => !column.nullable,
                Value::List(items) => !column.item_nullable && items.iter().any(|item| matches!(item, Value::Null)),
                _ => false,
            };
            if missing {
                let error = crate::Error::MissingValue { location: location.clone(), column: column.name.clone() };
                return match plan.on_feature_error {
                    OnFeatureError::Skip => skip(report, error),
                    OnFeatureError::Error | OnFeatureError::NullGeometry => Err(error),
                };
            }
        }
        out.append_row(state.row)?;
        Ok(FeatureOutcome::Row)
    }

    // ---- elements -----------------------------------------------------------

    /// The start tag of an element that `nodes` match: count the anchors
    /// among them, then put the attributes. Returns whether it is `xsi:nil`.
    fn start(&self, nodes: &[&RouteNode], attrs: &Attributes<'_>, state: &mut RowState) -> bool {
        for node in nodes {
            if let Some(anchor) = node.anchor {
                state.counts[anchor] += 1;
            }
        }
        let mut nil = false;
        let mut targets = std::mem::take(&mut state.targets);
        for (namespace, local, value) in attrs.iter_raw() {
            if namespace == Some(ns::XSI) {
                if local == "nil" {
                    nil = matches!(value.trim(), "true" | "1");
                }
                continue;
            }
            targets.clear();
            for node in nodes {
                node.attribute_targets(namespace, local, &mut targets);
            }
            if targets.is_empty() {
                continue;
            }
            let mut value = value.trim();
            if namespace == Some(ns::XLINK) && local == "href" && self.plan.strip_local_href_hash {
                value = value.strip_prefix('#').unwrap_or(value);
            }
            for target in &targets {
                self.put_text(*target, value, state);
            }
        }
        state.targets = targets;
        nil
    }

    /// The content of an element that `nodes` match (at `depth`, the feature
    /// being 0), up to and including its end tag. `content_start`: where its
    /// content begins in the buffer, for raw XML.
    fn content(
        &self,
        reader: &mut GmlReader<'_>,
        nodes: &[&RouteNode],
        depth: usize,
        nil: bool,
        content_start: usize,
        state: &mut RowState,
    ) -> crate::Result<()> {
        let targets = || nodes.iter().flat_map(|node| node.targets.iter().copied());
        if nil {
            // Null: whatever it holds is not read.
            return reader.skip_element().map_err(Into::into);
        }
        let whole = targets().find(|target| {
            matches!(target.value, RouteValue::Map | RouteValue::InnerText | RouteValue::BoundingBox)
        });
        if let Some(target) = whole {
            return self.whole(reader, target, depth, state);
        }
        let wants_text = targets().any(|target| target.value == RouteValue::Text);
        let wants_geometry = targets().any(|target| target.value == RouteValue::Geometry);
        let has_children = nodes.iter().any(|node| !node.is_leaf());
        if !wants_text && !wants_geometry && !has_children {
            return reader.skip_element().map_err(Into::into);
        }

        let mut text = String::new();
        let mut had_children = false;
        let mut geometry_done = false;
        loop {
            match reader.next_event()? {
                XmlEvent::Start { name, attrs } => {
                    had_children = true;
                    if wants_geometry && !geometry_done && name.is_gml() {
                        geometry_done = true;
                        drop(attrs);
                        self.geometry(reader, nodes, state)?;
                        continue;
                    }
                    let mut matched = Vec::new();
                    for node in nodes {
                        node.matching_children(&name, &mut matched);
                    }
                    if matched.is_empty() {
                        drop(attrs);
                        if depth == 0 && name.is_gml_named("boundedBy") && self.plan.has_geometry {
                            // The feature's envelope gives its geometries their srsName.
                            self.bounded_by_srs(reader, state)?;
                        } else {
                            reader.skip_element()?;
                        }
                        continue;
                    }
                    let nil = self.start(&matched, &attrs, state);
                    drop(attrs);
                    let child_start = reader.position();
                    self.content(reader, &matched, depth + 1, nil, child_start, state)?;
                }
                XmlEvent::Text(t) => {
                    if wants_text {
                        text.push_str(&t);
                    }
                }
                XmlEvent::End { .. } => break,
                XmlEvent::Eof => return Err(unexpected_eof(reader).into()),
            }
        }
        if wants_text {
            let raw = if had_children { Some(inner_raw(&reader.raw_since(content_start))) } else { None };
            for target in targets().filter(|target| target.value == RouteValue::Text) {
                self.text(target, &text, raw.as_deref(), state);
            }
        }
        Ok(())
    }

    /// An element's text into a `Text` target. A string column at an element
    /// with child elements takes its raw XML (`raw`).
    fn text(&self, target: Target, text: &str, raw: Option<&str>, state: &mut RowState) {
        let Item::Scalar(data_type) = &self.plan.routes.columns[target.column].item else {
            return;
        };
        let string = is_string(data_type);
        let value = match raw {
            Some(raw) if string => raw.trim(),
            _ => text.trim(),
        };
        if value.is_empty() && (self.plan.empty_as_null || !string) {
            return;
        }
        self.put_text(target, value, state);
    }

    /// A target that takes the element whole: a map, the text without its
    /// markup, or a box.
    fn whole(&self, reader: &mut GmlReader<'_>, target: Target, depth: usize, state: &mut RowState) -> crate::Result<()> {
        match target.value {
            RouteValue::Map => {
                let pairs = subtree_map(reader)?;
                if !pairs.is_empty() {
                    self.put(target.column, Value::Map(pairs), state);
                }
                Ok(())
            }
            RouteValue::BoundingBox => self.bounding_box(reader, target, depth, state),
            _ => {
                let text = inner_text(reader)?;
                self.text(target, &text, None, state);
                Ok(())
            }
        }
    }

    // ---- geometry -----------------------------------------------------------

    /// The geometry element whose start the reader has just returned, inside
    /// a property that `nodes` match: into each of their geometry targets.
    fn geometry(&self, reader: &mut GmlReader<'_>, nodes: &[&RouteNode], state: &mut RowState) -> crate::Result<()> {
        let columns = &self.plan.routes.columns;
        let targets: Vec<Target> = nodes
            .iter()
            .flat_map(|node| node.targets.iter().copied())
            .filter(|target| target.value == RouteValue::Geometry)
            .collect();
        let Some(Item::Geometry(_, axis)) = targets.first().map(|target| &columns[target.column].item) else {
            return reader.skip_element().map_err(Into::into);
        };
        let context = ParseContext { srs_name: state.srs_name.clone(), srs_dimension: None, axis: None };
        match self.geometry.parse(reader, &context, &self.axis[*axis]) {
            Ok(parsed) => {
                for warning in parsed.warnings {
                    state.warnings.push(Warning::from_geometry(warning, Some(state.location.clone())));
                }
                let Some(geometry) = parsed.geometry else { return Ok(()) };
                for target in targets {
                    let column = &columns[target.column];
                    let Item::Geometry(spec, _) = &column.item else { continue };
                    if let Some(message) = self.other_crs(column, parsed.srs_name.as_deref()) {
                        state.geometry_error.get_or_insert(message);
                        continue;
                    }
                    match spec.prepare(geometry.clone()) {
                        Ok(geometry) => self.put(target.column, Value::Geometry(geometry), state),
                        Err(geometry) => {
                            state.geometry_error.get_or_insert(format!(
                                "column {}: a {:?}{} doesn't fit {}",
                                column.name,
                                geometry.kind(),
                                if geometry.dim() == Some(xeibe_geom::Dim::Xyz) { " with Z" } else { "" },
                                spec.describe()
                            ));
                        }
                    }
                }
                Ok(())
            }
            Err(xeibe_geom::Error::Core(error)) => Err(error.into()),
            Err(error) => {
                state.geometry_error.get_or_insert(error.to_string());
                Ok(())
            }
        }
    }

    /// A column has one CRS: a geometry whose srsName resolves to another one
    /// is a geometry error (`docs/geometry.md`, "CRS metadata"). Spellings of
    /// one CRS are the same; unknown srsNames are compared as written. With
    /// `crs_override`, the CRS doesn't come from the data.
    fn other_crs(&self, column: &crate::route::ColumnPlan, srs_name: Option<&str>) -> Option<String> {
        if self.plan.geometry.crs_override.is_some() {
            return None;
        }
        let (Some(column_srs), Some(srs_name)) = (column.srs_name.as_deref(), srs_name) else {
            return None;
        };
        if column_srs == srs_name {
            return None;
        }
        let crs = |srs: &str| match SrsName::parse(srs).crs {
            Some(crs) => crs.authority_code(),
            None => srs.to_string(),
        };
        (crs(column_srs) != crs(srs_name)).then(|| {
            format!(
                "column {}: srsName {srs_name:?} is another CRS than the column's ({column_srs:?})",
                column.name
            )
        })
    }

    /// A `boundedBy` routed to a box column.
    fn bounding_box(&self, reader: &mut GmlReader<'_>, target: Target, depth: usize, state: &mut RowState) -> crate::Result<()> {
        let context = ParseContext { srs_name: state.srs_name.clone(), ..ParseContext::default() };
        match self.geometry.parse_bounded_by(reader, &context) {
            Ok(Some(mut envelope)) => {
                if depth == 0 && state.srs_name.is_none() {
                    state.srs_name = envelope.srs_name.clone();
                }
                if let Item::Box(axis) = &self.plan.routes.columns[target.column].item
                    && self.axis[*axis].resolve(envelope.srs_name.as_deref(), Dialect::Gml3).swap {
                        for corner in [&mut envelope.lower, &mut envelope.upper] {
                            if corner.len() >= 2 {
                                corner.swap(0, 1);
                            }
                        }
                    }
                self.put(target.column, Value::Box(envelope), state);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(xeibe_geom::Error::Core(error)) => Err(error.into()),
            Err(error) => {
                state.geometry_error.get_or_insert(error.to_string());
                Ok(())
            }
        }
    }

    /// Read a feature's `boundedBy` for its srsName only.
    fn bounded_by_srs(&self, reader: &mut GmlReader<'_>, state: &mut RowState) -> crate::Result<()> {
        match self.geometry.parse_bounded_by(reader, &ParseContext::default()) {
            Ok(envelope) => {
                if state.srs_name.is_none() {
                    state.srs_name = envelope.and_then(|envelope| envelope.srs_name);
                }
                Ok(())
            }
            Err(xeibe_geom::Error::Core(error)) => Err(error.into()),
            // An envelope that can't be read gives no srsName; the geometries
            // may have their own.
            Err(_) => Ok(()),
        }
    }

    // ---- values -------------------------------------------------------------

    /// Parse `text` as the column's type and put it.
    fn put_text(&self, target: Target, text: &str, state: &mut RowState) {
        let column = &self.plan.routes.columns[target.column];
        let Item::Scalar(data_type) = &column.item else {
            return;
        };
        if text.is_empty() && (self.plan.empty_as_null || !is_string(data_type)) {
            return;
        }
        match parse_scalar(data_type, text) {
            Some(scalar) => self.put(target.column, Value::Scalar(scalar), state),
            None => state.fail(format!("column {}: {text:?} is not a valid {data_type}", column.name)),
        }
    }

    /// Put a value into its column: a scalar column takes one value per
    /// feature, a list column one per occurrence of its anchor.
    fn put(&self, column: usize, value: Value, state: &mut RowState) {
        let plan = &self.plan.routes.columns[column];
        let slot = &mut state.row[column];
        if !plan.list {
            if matches!(slot, Value::Null) {
                *slot = value;
            } else {
                state.fail(format!("column {}: a second value, but the column holds one per feature", plan.name));
            }
            return;
        }
        // The anchor is on the value's path, so it has occurred at least once.
        let count = (state.counts[plan.anchor] as usize).max(1);
        if !matches!(slot, Value::List(_)) {
            *slot = Value::List(Vec::new());
        }
        let Value::List(items) = slot else { unreachable!("just made a list") };
        if items.len() >= count {
            state.fail(format!(
                "column {}: a second value within one occurrence of the list's anchor",
                plan.name
            ));
            return;
        }
        items.resize(count - 1, Value::Null);
        items.push(value);
    }
}

fn is_string(data_type: &arrow_schema::DataType) -> bool {
    use arrow_schema::DataType;
    matches!(data_type, DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View)
}

/// An element's raw XML (from its content to its end tag) without the end tag.
fn inner_raw(raw: &str) -> String {
    match raw.rfind("</") {
        Some(end) => raw[..end].to_string(),
        None => raw.to_string(),
    }
}

/// All text of a subtree, markup removed.
fn inner_text(reader: &mut GmlReader<'_>) -> crate::Result<String> {
    let mut text = String::new();
    let mut depth = 0usize;
    loop {
        match reader.next_event()? {
            XmlEvent::Start { .. } => depth += 1,
            XmlEvent::Text(t) => text.push_str(&t),
            XmlEvent::End { .. } => {
                if depth == 0 {
                    return Ok(text);
                }
                depth -= 1;
            }
            XmlEvent::Eof => return Err(unexpected_eof(reader).into()),
        }
    }
}

/// A subtree as `path → text` (attributes as `path/@name`), paths relative
/// to the element.
fn subtree_map(reader: &mut GmlReader<'_>) -> crate::Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    let mut stack: Vec<(String, String)> = vec![(String::new(), String::new())];
    loop {
        match reader.next_event()? {
            XmlEvent::Start { name, attrs } => {
                let path = join_path(&stack.last().expect("inside the element").0, &name);
                for (_, local, value) in attrs.iter_raw() {
                    pairs.push((format!("{path}/@{local}"), value.into_owned()));
                }
                stack.push((path, String::new()));
            }
            XmlEvent::Text(t) => stack.last_mut().expect("inside the element").1.push_str(&t),
            XmlEvent::End { .. } => {
                let (path, text) = stack.pop().expect("inside the element");
                let text = text.trim();
                if !text.is_empty() {
                    let path = if path.is_empty() { "#text".to_string() } else { path };
                    pairs.push((path, text.to_string()));
                }
                if stack.is_empty() {
                    return Ok(pairs);
                }
            }
            XmlEvent::Eof => return Err(unexpected_eof(reader).into()),
        }
    }
}

/// `parent/child`, or `child` at the top.
fn join_path(parent: &str, child: &QName) -> String {
    if parent.is_empty() { child.local.to_string() } else { format!("{parent}/{}", child.local) }
}

fn unexpected_eof(reader: &GmlReader<'_>) -> xeibe_core::Error {
    xeibe_core::Error::Xml {
        location: reader.location(),
        message: "unexpected end of chunk inside a feature".into(),
    }
}
