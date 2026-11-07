//! A schema hint: a feature type's declared properties, from a WFS
//! `DescribeFeatureType` XSD (`docs/wfs.md`, "Schema hint from
//! `DescribeFeatureType`").
//!
//! Inference uses it only where the sample shows nothing: a direct property
//! that never had a value gets its declared type, and a declared property the
//! sample never saw gets a column, after the sampled ones. A column the sample
//! typed keeps its type, whatever the declaration says.

use xeibe_core::{QName, ns};

/// The XML Schema namespace.
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaHint {
    /// The feature type the hint is for. Without a namespace it matches the
    /// layer of that local name in any namespace.
    pub layer: QName,
    /// Where the declarations come from, for the columns' reasons, e.g.
    /// `"the DescribeFeatureType schema"`.
    pub source: String,
    /// The feature type's direct properties, in schema order.
    pub properties: Vec<DeclaredProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredProperty {
    /// Without a namespace it matches the element of that local name in any
    /// namespace.
    pub name: QName,
    /// The declared type (a simple type's built-in base), e.g. `xs:int` or
    /// `gml:PointPropertyType`. `None` for a type the schema gives no name
    /// to (an inline complex type): ignored.
    pub type_name: Option<QName>,
    /// `maxOccurs > 1`, on the element or a group around it.
    pub repeated: bool,
}

/// What a declared type becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Declared {
    Text,
    Int,
    UInt,
    Float,
    Date,
    Time,
    Bool,
    Geometry,
    /// `xs:dateTime`: text, since a timestamp column takes either values
    /// with a time zone or values without one, and a declaration doesn't
    /// say which.
    DateTime,
}

impl SchemaHint {
    /// `true` if the hint is for `layer`: the same local name, and the same
    /// namespace when both have one.
    pub fn is_for(&self, layer: &QName) -> bool {
        same_name(&self.layer, layer)
    }

    /// The declaration of the direct property `name`.
    pub(crate) fn property(&self, name: &QName) -> Option<&DeclaredProperty> {
        self.properties
            .iter()
            .find(|property| same_name(&property.name, name))
    }
}

impl DeclaredProperty {
    /// What the declared type becomes; `None` for anything but a built-in
    /// simple type or a GML geometry property type.
    pub(crate) fn declared(&self) -> Option<Declared> {
        declared(self.type_name.as_ref()?)
    }

    /// The type as the reasons name it: `xs:int`, `gml:PointPropertyType`.
    pub(crate) fn type_label(&self) -> String {
        match &self.type_name {
            Some(name) => match name.ns.as_deref() {
                Some(XSD) => format!("xs:{}", name.local),
                Some(ns::GML | ns::GML_32) => format!("gml:{}", name.local),
                _ => name.local.to_string(),
            },
            None => "an anonymous type".to_string(),
        }
    }
}

/// The same local name, and the same namespace when both have one.
pub(crate) fn same_name(a: &QName, b: &QName) -> bool {
    a.local == b.local
        && match (a.ns.as_deref(), b.ns.as_deref()) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}

/// GML's geometry property types (2, 3.1 and 3.2 share the names).
const GEOMETRY_PROPERTY_TYPES: &[&str] = &[
    "GeometryPropertyType",
    "GeometricPrimitivePropertyType",
    "PointPropertyType",
    "MultiPointPropertyType",
    "LineStringPropertyType",
    "MultiLineStringPropertyType",
    "CurvePropertyType",
    "MultiCurvePropertyType",
    "SurfacePropertyType",
    "MultiSurfacePropertyType",
    "PolygonPropertyType",
    "MultiPolygonPropertyType",
    "MultiGeometryPropertyType",
    "GeometricComplexPropertyType",
    "CompositeCurvePropertyType",
    "CompositeSurfacePropertyType",
];

fn declared(type_name: &QName) -> Option<Declared> {
    match type_name.ns.as_deref() {
        Some(XSD) => Some(match &*type_name.local {
            "boolean" => Declared::Bool,
            "integer" | "long" | "int" | "short" | "byte" | "nonNegativeInteger"
            | "positiveInteger" | "nonPositiveInteger" | "negativeInteger" | "unsignedInt"
            | "unsignedShort" | "unsignedByte" => Declared::Int,
            "unsignedLong" => Declared::UInt,
            "double" | "float" | "decimal" => Declared::Float,
            "date" => Declared::Date,
            "dateTime" => Declared::DateTime,
            "time" => Declared::Time,
            // Not a simple type.
            "anyType" => return None,
            // Strings, names, URIs, durations, `gYear`, binary, …
            _ => Declared::Text,
        }),
        Some(ns::GML | ns::GML_32) => GEOMETRY_PROPERTY_TYPES
            .contains(&&*type_name.local)
            .then_some(Declared::Geometry),
        _ => None,
    }
}
