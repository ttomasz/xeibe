//! Parses one feature element and routes its values into the layer builder.
//!
//! The feature is read into a row of [`Value`]s first and appended only when
//! it is complete: a feature error can then skip the feature, or null its
//! geometry, without leaving half a row in the builders.

use xeibe_core::reader::{Attributes, GmlReader, XmlEvent};
use xeibe_core::{Dialect, Location, QName, ns};
use xeibe_geom::{AxisResolver, GeometryParser, ParseContext};
use xeibe_schema::{OnSchemaMismatch, RouteValue};

use crate::OnFeatureError;
use crate::axis::AxisDecisions;
use crate::builders::{LayerBatchBuilder, Value};
use crate::overflow::OverflowCollector;
use crate::pipeline::ReadPlan;
use crate::report::{ReadReport, Warning};
use crate::route::{FieldShape, RouteNode, Shape, Target, join_key};
use crate::value::{Scalar, parse_offset_minutes, parse_scalar};

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
    overflow: OverflowCollector,
    /// The first geometry error; the rest of the feature is still read.
    geometry_error: Option<xeibe_geom::Error>,
    /// srsName of the feature's `boundedBy`, inherited by its geometries.
    srs_name: Option<String>,
    location: Location,
    warnings: Vec<Warning>,
}

/// What an element's start tag said.
#[derive(Debug, Default)]
struct StartInfo {
    nil: bool,
    /// `xlink:href`, with a local reference's `#` stripped if so configured.
    href: Option<String>,
    /// A struct target was filled already (an unexpected repetition).
    occupied: bool,
}

/// Where a value went.
enum Put {
    Done,
    /// The slot has a value already (an unexpected repetition).
    Occupied,
    /// The value doesn't fit the field's type.
    Mismatch,
    /// The column is not built (projection).
    NotBuilt,
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
        let plan = self.plan;
        let root = &plan.routes.root;
        let mut state = RowState {
            row: vec![Value::Null; plan.shapes.len()],
            overflow: OverflowCollector::default(),
            geometry_error: None,
            srs_name: None,
            location,
            warnings: Vec::new(),
        };
        {
            let (_, attrs) = reader.current_start().ok_or_else(|| {
                xeibe_core::Error::Xml { location: reader.location(), message: "no feature start".into() }
            })?;
            self.start_element(root, &attrs, false, &mut state)?;
        }
        let mut text = String::new();
        self.children(reader, root, 0, &mut state, &mut text)?;
        self.commit(state, out, report)
    }

    fn commit(
        &self,
        mut state: RowState,
        out: &mut LayerBatchBuilder,
        report: &mut ReadReport,
    ) -> crate::Result<FeatureOutcome> {
        let plan = self.plan;
        for warning in state.warnings.drain(..) {
            report.warn(warning);
        }
        if let Some(error) = state.geometry_error.take() {
            match plan.on_feature_error {
                OnFeatureError::Error => return Err(error.into()),
                OnFeatureError::Skip => {
                    report.skipped.push((state.location, error.to_string()));
                    return Ok(FeatureOutcome::Skipped);
                }
                // The geometry was never put, so it is null.
                OnFeatureError::NullGeometry => {}
            }
        }
        if let Err(column) = check_non_null(&state.row, &plan.shapes, "") {
            let error = crate::Error::MissingValue { location: state.location.clone(), column };
            return match plan.on_feature_error {
                OnFeatureError::Skip => {
                    report.skipped.push((state.location, error.to_string()));
                    Ok(FeatureOutcome::Skipped)
                }
                OnFeatureError::Error | OnFeatureError::NullGeometry => Err(error),
            };
        }
        if let Some(index) = plan.routes.overflow {
            let entries = state.overflow.take_row();
            for (path, _) in &entries {
                *report.overflow_per_path.entry(path.clone()).or_default() += 1;
            }
            if !entries.is_empty() {
                state.row[index] = Value::Map(entries);
            }
        }
        out.append_row(state.row)?;
        Ok(FeatureOutcome::Row)
    }

    // ---- elements -----------------------------------------------------------

    /// The start tag of an element with a node in the route tree: begin its
    /// structs, route its attributes. `consumed`: the element's content goes
    /// to one value whole (raw XML, map, geometry), attributes included.
    fn start_element(
        &self,
        node: &RouteNode,
        attrs: &Attributes<'_>,
        consumed: bool,
        state: &mut RowState,
    ) -> crate::Result<StartInfo> {
        let mut info = StartInfo::default();
        for (namespace, local, value) in attrs.iter_raw() {
            match namespace {
                Some(ns::XSI) if local == "nil" => info.nil = matches!(value.trim(), "true" | "1"),
                Some(ns::XLINK) if local == "href" => {
                    let href = value.trim();
                    let href = match href.strip_prefix('#') {
                        Some(local) if self.plan.strip_local_href_hash => local,
                        _ => href,
                    };
                    info.href = Some(href.to_string());
                }
                _ => {}
            }
        }
        if !info.nil {
            for target in node.targets.iter().filter(|t| t.value == RouteValue::Struct) {
                if !self.begin_struct(target, state) {
                    info.occupied = true;
                }
            }
        }
        if info.occupied || consumed {
            return Ok(info);
        }
        for (namespace, local, value) in attrs.iter_raw() {
            if let Some(target) = node.attribute(namespace, local, self.plan.routes.by_name) {
                self.put_text(target, value.trim(), state)?;
            } else if !ignorable_attribute(namespace, local) && !node.is_known_attribute(namespace, local) {
                let key = join_key(&node.key, &format!("@{local}"));
                self.mismatch(state, &key, &value)?;
            }
        }
        Ok(info)
    }

    /// The child elements of `node` (at `depth`, the feature being 0), up to
    /// and including its end tag. Direct text goes to `text`.
    fn children(
        &self,
        reader: &mut GmlReader<'_>,
        node: &RouteNode,
        depth: usize,
        state: &mut RowState,
        text: &mut String,
    ) -> crate::Result<()> {
        loop {
            match reader.next_event()? {
                XmlEvent::Start { name, attrs } => match node.child(&name, self.plan.routes.by_name) {
                    Some(child) => {
                        let consumed = consuming_target(child).is_some();
                        let info = self.start_element(child, &attrs, consumed, state)?;
                        self.content(reader, child, &name, depth + 1, info, state)?;
                    }
                    None => self.unknown(reader, node, &name, depth + 1, state, text)?,
                },
                XmlEvent::Text(t) => text.push_str(&t),
                XmlEvent::End { .. } => return Ok(()),
                XmlEvent::Eof => return Err(unexpected_eof(reader).into()),
            }
        }
    }

    /// The content of an element whose start tag was handled.
    fn content(
        &self,
        reader: &mut GmlReader<'_>,
        node: &RouteNode,
        name: &QName,
        depth: usize,
        info: StartInfo,
        state: &mut RowState,
    ) -> crate::Result<()> {
        if info.occupied {
            let raw = reader.capture_element()?;
            return self.mismatch(state, &node.key, &raw);
        }
        if !node.routed {
            // Known, but nothing of it is built: skip it whole. A feature's
            // `boundedBy` still gives its geometries their srsName.
            if depth == 1 && name.is_gml_named("boundedBy") && self.plan.has_geometry {
                return self.bounded_by_srs(reader, state);
            }
            return reader.skip_element().map_err(Into::into);
        }
        if let Some(target) = consuming_target(node) {
            return match target.value {
                RouteValue::Geometry => self.geometry(reader, target, state),
                RouteValue::BoundingBox => self.bounding_box(reader, target, depth, state),
                RouteValue::RawXml => {
                    let raw = reader.capture_element()?;
                    self.put_value(target, Value::Scalar(Scalar::Str(raw.clone())), &raw, state)
                }
                RouteValue::Map => {
                    let pairs = subtree_map(reader)?;
                    let value = if pairs.is_empty() { Value::Null } else { Value::Map(pairs) };
                    self.put_value(target, value, "", state)
                }
                _ => {
                    let text = inner_text(reader)?;
                    self.put_text(target, text.trim(), state)
                }
            };
        }

        let mut text = String::new();
        self.children(reader, node, depth, state, &mut text)?;
        for target in &node.targets {
            match target.value {
                RouteValue::Text if !info.nil => {
                    let value = text.trim();
                    match (&info.href, value.is_empty()) {
                        (Some(href), true) if self.plan.routes.by_name => self.put_text(target, href, state)?,
                        (_, true) if self.plan.empty_as_null => {}
                        _ => self.put_text(target, value, state)?,
                    }
                }
                RouteValue::Href => {
                    if let Some(href) = &info.href {
                        self.put_text(target, href, state)?;
                    }
                }
                RouteValue::OffsetMinutes if !info.nil => {
                    if let Some(offset) = parse_offset_minutes(text.trim()) {
                        self.put_text(target, &offset.to_string(), state)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// An element without a node: a type wrapper to look through (given
    /// schemas; its text counts as the parent's), or data outside the schema.
    fn unknown(
        &self,
        reader: &mut GmlReader<'_>,
        parent: &RouteNode,
        name: &QName,
        depth: usize,
        state: &mut RowState,
        parent_text: &mut String,
    ) -> crate::Result<()> {
        if depth == 1 && name.is_gml_named("boundedBy") {
            if self.plan.has_geometry {
                return self.bounded_by_srs(reader, state);
            }
            return reader.skip_element().map_err(Into::into);
        }
        let wrapper = self.plan.routes.by_name && depth > 1 && name.local.starts_with(char::is_uppercase);
        if wrapper {
            return self.children(reader, parent, depth - 1, state, parent_text);
        }
        let key = join_key(&parent.key, &name.local);
        match self.plan.on_mismatch {
            OnSchemaMismatch::Drop => reader.skip_element().map_err(Into::into),
            OnSchemaMismatch::Error => Err(crate::Error::SchemaMismatch {
                location: state.location.clone(),
                message: format!("element {key} is not in the schema"),
            }),
            OnSchemaMismatch::Overflow => {
                if self.plan.routes.overflow.is_none() {
                    return reader.skip_element().map_err(Into::into);
                }
                let value = element_value(reader)?;
                state.overflow.push(&key, &value);
                Ok(())
            }
        }
    }

    // ---- geometry -----------------------------------------------------------

    /// A geometry property: the first GML element inside it is the geometry.
    fn geometry(&self, reader: &mut GmlReader<'_>, target: &Target, state: &mut RowState) -> crate::Result<()> {
        let spec = match self.shape(target).map(|shape| &shape.item().shape) {
            Some(Shape::Geometry(spec)) => *spec,
            _ => return reader.skip_element().map_err(Into::into),
        };
        let resolver = target.axis.and_then(|index| self.axis.get(index));
        let mut seen = false;
        loop {
            match reader.next_event()? {
                XmlEvent::Start { name, .. } => {
                    if seen || !name.is_gml() {
                        reader.skip_element()?;
                        continue;
                    }
                    seen = true;
                    let start = reader.last_start_position();
                    let context = ParseContext { srs_name: state.srs_name.clone(), srs_dimension: None, axis: None };
                    let parsed = match resolver {
                        Some(resolver) => self.geometry.parse(reader, &context, resolver),
                        None => self.geometry.parse(reader, &context, &AsWritten),
                    };
                    match parsed {
                        Ok(parsed) => {
                            for warning in parsed.warnings {
                                state.warnings.push(Warning::from_geometry(warning, Some(state.location.clone())));
                            }
                            let Some(geometry) = parsed.geometry else { continue };
                            match spec.prepare(geometry) {
                                Ok(geometry) => {
                                    if let Put::Occupied = self.put(target, Value::Geometry(geometry), state) {
                                        let raw = reader.raw_since(start);
                                        self.mismatch(state, &target.key, &raw)?;
                                    }
                                }
                                Err(_) => {
                                    let raw = reader.raw_since(start);
                                    self.mismatch(state, &target.key, &raw)?;
                                }
                            }
                        }
                        Err(xeibe_geom::Error::Core(error)) => return Err(error.into()),
                        Err(error) => {
                            state.geometry_error.get_or_insert(error);
                        }
                    }
                }
                XmlEvent::Text(_) => {}
                XmlEvent::End { .. } => return Ok(()),
                XmlEvent::Eof => return Err(unexpected_eof(reader).into()),
            }
        }
    }

    /// A `boundedBy` routed to a box column.
    fn bounding_box(
        &self,
        reader: &mut GmlReader<'_>,
        target: &Target,
        depth: usize,
        state: &mut RowState,
    ) -> crate::Result<()> {
        let context = ParseContext { srs_name: state.srs_name.clone(), ..ParseContext::default() };
        match self.geometry.parse_bounded_by(reader, &context) {
            Ok(Some(mut envelope)) => {
                if depth == 1 && state.srs_name.is_none() {
                    state.srs_name = envelope.srs_name.clone();
                }
                let resolver = target.axis.and_then(|index| self.axis.get(index));
                let swap = resolver.is_some_and(|r| r.resolve(envelope.srs_name.as_deref(), Dialect::Gml3).swap);
                if swap {
                    for corner in [&mut envelope.lower, &mut envelope.upper] {
                        if corner.len() >= 2 {
                            corner.swap(0, 1);
                        }
                    }
                }
                if let Put::Occupied = self.put(target, Value::Box(envelope), state) {
                    self.mismatch(state, &target.key, "a second envelope")?;
                }
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(xeibe_geom::Error::Core(error)) => Err(error.into()),
            Err(error) => {
                state.geometry_error.get_or_insert(error);
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

    fn shape(&self, target: &Target) -> Option<&'a FieldShape> {
        let path = target.field_path.as_ref()?;
        let (first, rest) = path.split_first()?;
        let mut shape = self.plan.shapes.get(*first)?;
        for index in rest {
            shape = match &shape.item().shape {
                Shape::Struct(children) => children.get(*index)?,
                _ => return None,
            };
        }
        Some(shape)
    }

    /// Parse `text` as the target field's type and put it.
    fn put_text(&self, target: &Target, text: &str, state: &mut RowState) -> crate::Result<()> {
        let Some(shape) = self.shape(target) else {
            return Ok(());
        };
        let Shape::Scalar(data_type) = &shape.item().shape else {
            return self.mismatch(state, &target.key, text);
        };
        if text.is_empty() && self.plan.empty_as_null {
            return Ok(());
        }
        match parse_scalar(data_type, text) {
            Some(scalar) => self.put_value(target, Value::Scalar(scalar), text, state),
            None => self.mismatch(state, &target.key, text),
        }
    }

    /// Put a value; a repetition or a misfit goes by `on_mismatch` with `raw`.
    fn put_value(&self, target: &Target, value: Value, raw: &str, state: &mut RowState) -> crate::Result<()> {
        match self.put(target, value, state) {
            Put::Done | Put::NotBuilt => Ok(()),
            Put::Occupied | Put::Mismatch => self.mismatch(state, &target.key, raw),
        }
    }

    fn put(&self, target: &Target, value: Value, state: &mut RowState) -> Put {
        let Some(path) = &target.field_path else {
            return Put::NotBuilt;
        };
        let Some((slot, shape)) = slot(&mut state.row, &self.plan.shapes, path) else {
            return Put::Mismatch;
        };
        match &shape.shape {
            Shape::List(_) => {
                match slot {
                    Value::List(items) => items.push(value),
                    _ => *slot = Value::List(vec![value]),
                }
                Put::Done
            }
            _ if matches!(slot, Value::Null) => {
                *slot = value;
                Put::Done
            }
            _ => Put::Occupied,
        }
    }

    /// Start a struct value (a new item of a list of structs). `false` if a
    /// (non-list) struct has a value already.
    fn begin_struct(&self, target: &Target, state: &mut RowState) -> bool {
        let Some(path) = &target.field_path else {
            return true;
        };
        let Some((slot, shape)) = slot(&mut state.row, &self.plan.shapes, path) else {
            return true;
        };
        match &shape.shape {
            Shape::List(item) => {
                let value = empty_value(item);
                match slot {
                    Value::List(items) => items.push(value),
                    _ => *slot = Value::List(vec![value]),
                }
                true
            }
            Shape::Struct(children) if matches!(slot, Value::Null) => {
                *slot = Value::Struct(vec![Value::Null; children.len()]);
                true
            }
            Shape::Struct(_) => false,
            _ => true,
        }
    }

    /// Data outside the schema, per `on_mismatch`.
    fn mismatch(&self, state: &mut RowState, key: &str, value: &str) -> crate::Result<()> {
        match self.plan.on_mismatch {
            OnSchemaMismatch::Overflow => {
                if self.plan.routes.overflow.is_some() {
                    state.overflow.push(key, value);
                }
                Ok(())
            }
            OnSchemaMismatch::Drop => Ok(()),
            OnSchemaMismatch::Error => Err(crate::Error::SchemaMismatch {
                location: state.location.clone(),
                message: format!("{key}: {value:?} does not fit the schema"),
            }),
        }
    }
}

/// Resolver for geometry outside any geometry column (never asked in practice).
struct AsWritten;

impl AxisResolver for AsWritten {
    fn resolve(&self, _: Option<&str>, _: Dialect) -> xeibe_geom::AxisDecision {
        xeibe_geom::AxisDecision { swap: false, reason: "as written".into(), conflicts: Vec::new() }
    }
}

/// The target that takes the element's content whole, if any.
fn consuming_target(node: &RouteNode) -> Option<&Target> {
    node.targets.iter().find(|target| {
        target.field_path.is_some()
            && matches!(
                target.value,
                RouteValue::Geometry
                    | RouteValue::BoundingBox
                    | RouteValue::RawXml
                    | RouteValue::Map
                    | RouteValue::InnerText
            )
    })
}

/// Attributes that are GML/XLink machinery, not data: never `_overflow`.
fn ignorable_attribute(namespace: Option<&str>, local: &str) -> bool {
    match namespace {
        Some(ns::XSI | ns::XLINK) => true,
        Some(ns::GML | ns::GML_32) => matches!(local, "id" | "remoteSchema" | "owns"),
        None => matches!(local, "nilReason" | "owns" | "remoteSchema" | "aggregationType"),
        _ => false,
    }
}

/// The value slot at `path` and its field's shape. Structs and list items on
/// the way are created as needed; a list level goes to its last item.
fn slot<'r>(row: &'r mut [Value], shapes: &'r [FieldShape], path: &[usize]) -> Option<(&'r mut Value, &'r FieldShape)> {
    let (first, rest) = path.split_first()?;
    let mut value = row.get_mut(*first)?;
    let mut shape = shapes.get(*first)?;
    for index in rest {
        if let Shape::List(item) = &shape.shape {
            match value {
                Value::List(items) if !items.is_empty() => {}
                _ => *value = Value::List(vec![empty_value(item)]),
            }
            let Value::List(items) = value else { return None };
            value = items.last_mut()?;
            shape = item;
        }
        let Shape::Struct(children) = &shape.shape else {
            return None;
        };
        if !matches!(value, Value::Struct(_)) {
            *value = Value::Struct(vec![Value::Null; children.len()]);
        }
        let Value::Struct(values) = value else { return None };
        value = values.get_mut(*index)?;
        shape = children.get(*index)?;
    }
    Some((value, shape))
}

fn empty_value(shape: &FieldShape) -> Value {
    match &shape.shape {
        Shape::Struct(children) => Value::Struct(vec![Value::Null; children.len()]),
        _ => Value::Null,
    }
}

/// `Err(column)` for the first non-null field without a value.
fn check_non_null(values: &[Value], shapes: &[FieldShape], prefix: &str) -> Result<(), String> {
    for (value, shape) in values.iter().zip(shapes) {
        let name = format!("{prefix}{}", shape.name);
        if matches!(value, Value::Null) {
            if !shape.nullable {
                return Err(name);
            }
            continue;
        }
        check_value(value, shape, &name)?;
    }
    Ok(())
}

fn check_value(value: &Value, shape: &FieldShape, name: &str) -> Result<(), String> {
    match (value, &shape.shape) {
        (Value::Struct(values), Shape::Struct(children)) => check_non_null(values, children, &format!("{name}.")),
        (Value::List(items), Shape::List(item)) => {
            for value in items {
                if matches!(value, Value::Null) {
                    if !item.nullable {
                        return Err(name.to_string());
                    }
                } else {
                    check_value(value, item, name)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// An unknown element's value: its text, or its raw XML if it has children.
fn element_value(reader: &mut GmlReader<'_>) -> crate::Result<String> {
    let start = reader.last_start_position();
    let mut text = String::new();
    let mut nested = false;
    loop {
        match reader.next_event()? {
            XmlEvent::Start { .. } => {
                nested = true;
                reader.skip_element()?;
            }
            XmlEvent::Text(t) => text.push_str(&t),
            XmlEvent::End { .. } => break,
            XmlEvent::Eof => return Err(unexpected_eof(reader).into()),
        }
    }
    Ok(if nested { reader.raw_since(start) } else { text.trim().to_string() })
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
                let path = join_key(&stack.last().expect("inside the element").0, &name.local);
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

fn unexpected_eof(reader: &GmlReader<'_>) -> xeibe_core::Error {
    xeibe_core::Error::Xml {
        location: reader.location(),
        message: "unexpected end of chunk inside a feature".into(),
    }
}
