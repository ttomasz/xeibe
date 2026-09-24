//! The read's view of a [`LayerSchema`]: a tree of path steps with the columns
//! each element and attribute fills, and how every output column is built.
//!
//! The tree only holds what the schema's paths lead to. An element without a
//! node is skipped unparsed (`docs/schema-inference.md` §6.3).

use std::sync::Arc;

use arrow_schema::{DataType, Field};
use xeibe_core::QName;
use xeibe_schema::{LayerSchema, RouteValue};

use crate::geometry_column::{GeometrySpec, geoarrow_type};

/// How one output column is filled.
#[derive(Debug, Clone)]
pub struct ColumnPlan {
    pub name: String,
    pub nullable: bool,
    /// A list column, aligned with the occurrences of its anchor.
    pub list: bool,
    /// The anchor's counter (list columns).
    pub anchor: usize,
    /// Whether list items may be null (always true for inferred schemas).
    pub item_nullable: bool,
    pub item: Item,
    /// Geometry columns: the srsName the column's CRS comes from
    /// (`gml:srs_name`), if known.
    pub srs_name: Option<String>,
}

/// One value of a column (the column itself, or a list column's item).
#[derive(Debug, Clone)]
pub enum Item {
    Scalar(DataType),
    /// With the index of the column's axis resolver.
    Geometry(GeometrySpec, usize),
    Map,
    /// `geoarrow.box`, with the index of the column's axis resolver.
    Box(usize),
}

impl ColumnPlan {
    fn of(field: &Field, geometry_index: &mut Vec<String>) -> crate::Result<Self> {
        let (list, item) = match field.data_type() {
            DataType::List(item) | DataType::LargeList(item) if !is_geoarrow(field) => (true, item.as_ref()),
            _ => (false, field),
        };
        let mut axis = || {
            geometry_index.push(field.name().clone());
            geometry_index.len() - 1
        };
        let value = match item.metadata().get("ARROW:extension:name").map(String::as_str) {
            Some("geoarrow.box") => Item::Box(axis()),
            Some(name) if name.starts_with("geoarrow.") => {
                Item::Geometry(GeometrySpec::for_type(&geoarrow_type(item)?)?, axis())
            }
            _ => match item.data_type() {
                DataType::Map(..) => Item::Map,
                data_type => Item::Scalar(data_type.clone()),
            },
        };
        Ok(ColumnPlan {
            name: field.name().clone(),
            nullable: field.is_nullable(),
            list,
            anchor: 0,
            item_nullable: item.is_nullable(),
            item: value,
            srs_name: field.metadata().get(xeibe_schema::rules::meta::SRS_NAME).cloned(),
        })
    }
}

/// What a node fills.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    /// The output column.
    pub column: usize,
    pub value: RouteValue,
}

/// One step of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `*`: any element.
    Any,
    /// A local name; `ns: None` matches it in any namespace.
    Name { ns: Option<Arc<str>>, local: Arc<str> },
}

impl Step {
    fn of(name: &QName) -> Self {
        if name.ns.is_none() && &*name.local == "*" {
            Step::Any
        } else {
            Step::Name { ns: name.ns.clone(), local: name.local.clone() }
        }
    }

    /// Exactly this namespace and local name.
    fn is_exact(&self, ns: Option<&str>, local: &str) -> bool {
        matches!(self, Step::Name { ns: Some(uri), local: l } if Some(&**uri) == ns && &**l == local)
    }

    /// The local name in any namespace.
    fn is_any_namespace(&self, local: &str) -> bool {
        matches!(self, Step::Name { ns: None, local: l } if &**l == local)
    }
}

/// One node of the tree: an element some path leads to or through.
#[derive(Debug)]
pub struct RouteNode {
    pub step: Step,
    pub children: Vec<RouteNode>,
    pub attributes: Vec<(Step, Target)>,
    /// Columns that take the element itself (text, geometry, map, box).
    pub targets: Vec<Target>,
    /// The counter of the lists anchored on this element.
    pub anchor: Option<usize>,
}

impl RouteNode {
    fn new(step: Step) -> Self {
        RouteNode { step, children: Vec::new(), attributes: Vec::new(), targets: Vec::new(), anchor: None }
    }

    fn child_mut(&mut self, step: Step) -> &mut RouteNode {
        let index = match self.children.iter().position(|child| child.step == step) {
            Some(index) => index,
            None => {
                self.children.push(RouteNode::new(step));
                self.children.len() - 1
            }
        };
        &mut self.children[index]
    }

    /// Add the children that match element `name` to `out`: `*`, the exact
    /// namespace, and an unprefixed step unless a sibling names this exact
    /// namespace (the more specific step wins).
    pub fn matching_children<'n>(&'n self, name: &QName, out: &mut Vec<&'n RouteNode>) {
        let ns = name.ns.as_deref();
        let exact = self.children.iter().any(|child| child.step.is_exact(ns, &name.local));
        for child in &self.children {
            let matches = match &child.step {
                Step::Any => true,
                step => step.is_exact(ns, &name.local) || (!exact && step.is_any_namespace(&name.local)),
            };
            if matches {
                out.push(child);
            }
        }
    }

    /// Add the targets of attribute `(ns, local)` to `out`, matched like
    /// elements.
    pub fn attribute_targets(&self, ns: Option<&str>, local: &str, out: &mut Vec<Target>) {
        let exact = self.attributes.iter().any(|(step, _)| step.is_exact(ns, local));
        for (step, target) in &self.attributes {
            if step.is_exact(ns, local) || (!exact && step.is_any_namespace(local)) {
                out.push(*target);
            }
        }
    }

    /// Nothing below this node needs the element's content.
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// The feature's route tree and the output columns.
#[derive(Debug)]
pub struct RouteTree {
    pub root: RouteNode,
    /// One per output column.
    pub columns: Vec<ColumnPlan>,
    /// Anchor counters (one per anchoring node).
    pub anchors: usize,
    /// Names of the geometry and box columns, by axis-resolver index.
    pub geometry_columns: Vec<String>,
}

impl RouteTree {
    /// `columns[i]`: the output index of the schema's column `i`, or `None`
    /// if the projection leaves it out. `output`: the output fields.
    pub fn new(layer: &LayerSchema, columns: &[Option<usize>], output: &[Arc<Field>]) -> crate::Result<Self> {
        let mut geometry_columns = Vec::new();
        let mut plans = output
            .iter()
            .map(|field| ColumnPlan::of(field, &mut geometry_columns))
            .collect::<crate::Result<Vec<_>>>()?;
        let mut root = RouteNode::new(Step::Any);
        let mut anchors = 0;
        for route in &layer.routes {
            let Some(column) = route.field_path.first().and_then(|index| columns.get(*index).copied().flatten())
            else {
                continue;
            };
            let mut node = &mut root;
            for (index, name) in route.source_path.iter().enumerate() {
                node = node.child_mut(Step::of(name));
                if route.anchor == Some(index) {
                    let counter = *node.anchor.get_or_insert_with(|| {
                        anchors += 1;
                        anchors - 1
                    });
                    plans[column].anchor = counter;
                }
            }
            let target = Target { column, value: route.value };
            match &route.attribute {
                Some(attribute) => node.attributes.push((Step::of(attribute), target)),
                None => node.targets.push(target),
            }
        }
        Ok(RouteTree { root, columns: plans, anchors, geometry_columns })
    }
}

fn is_geoarrow(field: &Field) -> bool {
    field
        .metadata()
        .get("ARROW:extension:name")
        .is_some_and(|name| name.starts_with("geoarrow."))
}
