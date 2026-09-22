//! The read's view of a [`LayerSchema`]: a tree of XML element names with the
//! targets (fields) each element and attribute fills, and the shape of every
//! output field.

use arrow_schema::{DataType, Field, Schema};
use xeibe_core::QName;
use xeibe_geom::options::DimMode;
use xeibe_schema::{ElementNode, LayerSchema, RouteValue};

use crate::geometry_column::{GeometrySpec, geoarrow_type};

/// How one output field is built; mirrors the Arrow field.
#[derive(Debug, Clone)]
pub struct FieldShape {
    pub name: String,
    pub nullable: bool,
    pub shape: Shape,
}

#[derive(Debug, Clone)]
pub enum Shape {
    Scalar(DataType),
    Struct(Vec<FieldShape>),
    List(Box<FieldShape>),
    Map,
    Geometry(GeometrySpec),
    Box,
}

impl FieldShape {
    pub fn of(field: &Field, dimension: DimMode) -> crate::Result<Self> {
        let extension = field.metadata().get("ARROW:extension:name").map(String::as_str);
        let shape = match extension {
            Some("geoarrow.box") => Shape::Box,
            Some(name) if name.starts_with("geoarrow.") => {
                Shape::Geometry(GeometrySpec::for_type(&geoarrow_type(field)?, dimension)?)
            }
            _ => match field.data_type() {
                DataType::Struct(children) => Shape::Struct(
                    children
                        .iter()
                        .map(|child| FieldShape::of(child, dimension))
                        .collect::<crate::Result<_>>()?,
                ),
                DataType::List(item) | DataType::LargeList(item) => {
                    Shape::List(Box::new(FieldShape::of(item, dimension)?))
                }
                DataType::Map(..) => Shape::Map,
                data_type => Shape::Scalar(data_type.clone()),
            },
        };
        Ok(FieldShape { name: field.name().clone(), nullable: field.is_nullable(), shape })
    }

    /// The shape of one value: a list's item, else the field itself.
    pub fn item(&self) -> &FieldShape {
        match &self.shape {
            Shape::List(item) => item,
            _ => self,
        }
    }
}

/// What a route fills.
#[derive(Debug, Clone)]
pub struct Target {
    pub value: RouteValue,
    /// Into the row; `None` when the column is not built (projection).
    pub field_path: Option<Vec<usize>>,
    /// Path for `_overflow` and messages: local names from the feature.
    pub key: String,
    /// Index of the geometry column's axis resolver (geometry and box targets).
    pub axis: Option<usize>,
}

/// One element name in the tree.
#[derive(Debug, Default)]
pub struct RouteNode {
    pub children: Vec<(QName, RouteNode)>,
    pub attributes: Vec<(QName, Target)>,
    /// Routes that take the element itself (text, struct, geometry, …).
    pub targets: Vec<Target>,
    /// Attributes seen in the sample that have no column on purpose (constant
    /// attributes moved to field metadata, dropped by the options).
    pub known_attributes: Vec<QName>,
    /// Some route starts here or below; a node without one is skipped whole.
    pub routed: bool,
    pub key: String,
}

impl RouteNode {
    /// The child for element `name`: exact, or by local name for routes
    /// without a namespace (`by_name`, a given schema).
    pub fn child(&self, name: &QName, by_name: bool) -> Option<&RouteNode> {
        self.children
            .iter()
            .find(|(qname, _)| qname == name)
            .or_else(|| {
                by_name
                    .then(|| {
                        self.children
                            .iter()
                            .find(|(qname, _)| qname.ns.is_none() && qname.local == name.local)
                    })
                    .flatten()
            })
            .map(|(_, node)| node)
    }

    /// The target of an attribute, matched like [`Self::child`].
    pub fn attribute(&self, ns: Option<&str>, local: &str, by_name: bool) -> Option<&Target> {
        self.attributes
            .iter()
            .find(|(qname, _)| &*qname.local == local && qname.ns.as_deref() == ns)
            .or_else(|| {
                by_name
                    .then(|| {
                        self.attributes
                            .iter()
                            .find(|(qname, _)| qname.ns.is_none() && &*qname.local == local)
                    })
                    .flatten()
            })
            .map(|(_, target)| target)
    }

    pub fn is_known_attribute(&self, ns: Option<&str>, local: &str) -> bool {
        self.known_attributes
            .iter()
            .any(|qname| &*qname.local == local && qname.ns.as_deref() == ns)
    }

    fn child_mut(&mut self, name: &QName) -> &mut RouteNode {
        let index = match self.children.iter().position(|(qname, _)| qname == name) {
            Some(index) => index,
            None => {
                let key = join_key(&self.key, &name.local);
                self.children.push((name.clone(), RouteNode { key, ..RouteNode::default() }));
                self.children.len() - 1
            }
        };
        &mut self.children[index].1
    }

    /// Mark the nodes that have a route at or below them.
    fn mark_routed(&mut self) -> bool {
        let built = |target: &Target| target.field_path.is_some();
        let mut routed =
            self.targets.iter().any(built) || self.attributes.iter().any(|(_, target)| built(target));
        for (_, child) in &mut self.children {
            routed |= child.mark_routed();
        }
        self.routed = routed;
        routed
    }

    /// Add the elements and attributes of an observed tree as known.
    fn add_known(&mut self, node: &ElementNode) {
        for name in node.attributes.keys() {
            if !self.known_attributes.contains(name) {
                self.known_attributes.push(name.clone());
            }
        }
        for (name, child) in &node.children {
            self.child_mut(name).add_known(child);
        }
    }
}

/// `parent/child`, or `child` at the feature.
pub fn join_key(parent: &str, child: &str) -> String {
    if parent.is_empty() { child.to_string() } else { format!("{parent}/{child}") }
}

/// The feature's route tree and where `_overflow` goes.
#[derive(Debug)]
pub struct RouteTree {
    pub root: RouteNode,
    /// Routes of a given schema are matched by local name and look through
    /// type wrappers (`LayerSchema::match_by_name`).
    pub by_name: bool,
    /// Top-level index of `_overflow` in the output, if it is built.
    pub overflow: Option<usize>,
    /// The field path of each geometry column (the index of its axis resolver).
    pub geometry_columns: Vec<(Vec<usize>, String)>,
}

impl RouteTree {
    /// `columns[i]`: the output index of the schema's top-level field `i`, or
    /// `None` if the projection leaves it out. `observed`: the sample's tree
    /// of the layer, whose elements are known even without a column.
    pub fn new(
        layer: &LayerSchema,
        columns: &[Option<usize>],
        observed: Option<&ElementNode>,
    ) -> Self {
        let mut root = RouteNode::default();
        let mut overflow = None;
        let mut geometry_columns = Vec::new();
        for route in &layer.routes {
            let field_path = route.field_path.split_first().and_then(|(first, rest)| {
                let mut path = vec![columns.get(*first).copied().flatten()?];
                path.extend_from_slice(rest);
                Some(path)
            });
            if route.value == RouteValue::Overflow {
                overflow = field_path.map(|path| path[0]);
                continue;
            }
            let mut node = &mut root;
            for name in &route.source_path {
                node = node.child_mut(name);
            }
            let axis = match (&field_path, route.value) {
                (Some(path), RouteValue::Geometry | RouteValue::BoundingBox) => {
                    geometry_columns.push((path.clone(), field_name(&layer.schema, &route.field_path)));
                    Some(geometry_columns.len() - 1)
                }
                _ => None,
            };
            match &route.attribute {
                Some(attribute) => {
                    let key = join_key(&node.key, &format!("@{}", attribute.local));
                    node.attributes.push((attribute.clone(), Target { value: route.value, field_path, key, axis }));
                }
                None => {
                    let key = node.key.clone();
                    node.targets.push(Target { value: route.value, field_path, key, axis });
                }
            }
        }
        if let Some(observed) = observed {
            root.add_known(observed);
        }
        root.mark_routed();
        RouteTree { root, by_name: layer.match_by_name, overflow, geometry_columns }
    }
}

/// The dotted name of a (nested) field, for messages and axis overrides.
fn field_name(schema: &Schema, field_path: &[usize]) -> String {
    let mut names = Vec::new();
    let mut fields = schema.fields();
    for index in field_path {
        let Some(field) = fields.get(*index) else { break };
        names.push(field.name().clone());
        let mut data_type = field.data_type();
        if let DataType::List(item) | DataType::LargeList(item) = data_type {
            data_type = item.data_type();
        }
        match data_type {
            DataType::Struct(children) => fields = children,
            _ => break,
        }
    }
    names.join(".")
}
