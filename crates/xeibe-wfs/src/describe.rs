//! A `DescribeFeatureType` XSD read for one feature type's direct properties
//! (`docs/wfs.md`, "Schema hint from `DescribeFeatureType`").
//!
//! Only what a schema hint needs: the feature type's element, its complex
//! type (named or inline, with the bases of `complexContent` extensions
//! defined in the same document), and the elements of its content model with
//! their type and whether they repeat. A simple type defined in the document
//! is replaced by the built-in type it restricts. References (`ref`),
//! wildcards, groups and imported schemas are left out: nothing is fetched
//! beyond the response itself.

use crate::xml::Element;

/// The XML Schema namespace.
const XSD: &str = "http://www.w3.org/2001/XMLSchema";

/// A feature type as its `DescribeFeatureType` schema declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureTypeSchema {
    /// The schema's `targetNamespace`.
    pub namespace: Option<String>,
    pub name: String,
    /// Direct properties, in the order of the content model.
    pub properties: Vec<DeclaredProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredProperty {
    /// The target namespace for a qualified element, else `None`.
    pub namespace: Option<String>,
    pub name: String,
    /// `(namespace, local name)` of the declared type, a simple type defined
    /// in the document replaced by its built-in base. `None` for an inline
    /// complex type, or no type at all.
    pub type_name: Option<(Option<String>, String)>,
    /// `maxOccurs` above 1, on the element or a group around it.
    pub repeated: bool,
}

/// Nesting deeper than this (extension bases, simple type bases, groups) is
/// not followed: a loop in a broken schema ends here.
const MAX_DEPTH: usize = 16;

impl FeatureTypeSchema {
    /// The declaration of the feature type `name` (a local name; `namespace`
    /// is its namespace, if known) in `xsd`. An exception report is
    /// [`crate::Error::Exception`]; a document that is no schema, or one
    /// that doesn't declare the type, is [`crate::Error::Describe`].
    pub fn parse(xsd: &[u8], namespace: Option<&str>, name: &str) -> crate::Result<Self> {
        let xsd = crate::xml::utf8(xsd)?;
        if let Some(report) = crate::exception::ExceptionReport::parse(&xsd) {
            return Err(crate::Error::Exception(report));
        }
        let root = crate::xml::parse(&xsd)?;
        if !(root.is("schema") && root.ns.as_deref() == Some(XSD)) {
            return Err(crate::Error::Describe(format!(
                "not an XML schema (root element <{}>)",
                root.local
            )));
        }
        let schema = Schema::new(&root);
        if let (Some(wanted), Some(target)) = (namespace, schema.target)
            && wanted != target
        {
            return Err(crate::Error::Describe(format!(
                "the schema is for the namespace {target}, not {wanted}"
            )));
        }
        let element = schema
            .globals("element")
            .find(|element| element.attr("name") == Some(name))
            .ok_or_else(|| {
                crate::Error::Describe(format!("the schema declares no element {name}"))
            })?;
        let mut properties = Vec::new();
        match schema.element_type(element) {
            Some(complex) => schema.complex_type(complex, false, 0, &mut properties),
            None => {
                return Err(crate::Error::Describe(format!(
                    "the type of the element {name} is not in the schema"
                )));
            }
        }
        Ok(FeatureTypeSchema {
            namespace: schema.target.map(str::to_string),
            name: name.to_string(),
            properties,
        })
    }
}

struct Schema<'a> {
    root: &'a Element,
    target: Option<&'a str>,
    qualified: bool,
}

impl<'a> Schema<'a> {
    fn new(root: &'a Element) -> Self {
        Schema {
            root,
            target: root.attr("targetNamespace").filter(|ns| !ns.is_empty()),
            qualified: root.attr("elementFormDefault") == Some("qualified"),
        }
    }

    /// Top-level XSD declarations named `local` (`element`, `complexType`, …).
    fn globals(&self, local: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        xsd_children(self.root, local)
    }

    /// A top-level declaration `kind` named by the QName `(ns, name)`, if it
    /// is in this document.
    fn global(&self, kind: &'a str, ns: Option<&str>, name: &str) -> Option<&'a Element> {
        if ns.is_some() && ns != self.target {
            return None;
        }
        self.globals(kind)
            .find(|declaration| declaration.attr("name") == Some(name))
    }

    /// The complex type of a global element: inline, or named.
    fn element_type(&self, element: &'a Element) -> Option<&'a Element> {
        if let Some(inline) = xsd_child(element, "complexType") {
            return Some(inline);
        }
        let (ns, name) = element.attr_qname("type")?;
        self.global("complexType", ns, name)
    }

    /// The properties of a complex type: its base's first (a `complexContent`
    /// extension of a type in this document), then its own.
    fn complex_type(
        &self,
        complex: &'a Element,
        repeated: bool,
        depth: usize,
        out: &mut Vec<DeclaredProperty>,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let derivation = xsd_child(complex, "complexContent").and_then(|content| {
            xsd_child(content, "extension").or_else(|| xsd_child(content, "restriction"))
        });
        let particles = match derivation {
            Some(derivation) => {
                if derivation.is("extension")
                    && let Some((ns, name)) = derivation.attr_qname("base")
                    && let Some(base) = self.global("complexType", ns, name)
                {
                    self.complex_type(base, repeated, depth + 1, out);
                }
                derivation
            }
            None => complex,
        };
        for particle in &particles.children {
            self.particle(particle, repeated, depth + 1, out);
        }
    }

    /// An element, or a `sequence`, `choice` or `all` of them.
    fn particle(
        &self,
        particle: &'a Element,
        repeated: bool,
        depth: usize,
        out: &mut Vec<DeclaredProperty>,
    ) {
        if depth > MAX_DEPTH || particle.ns.as_deref() != Some(XSD) {
            return;
        }
        let repeated = repeated || repeats(particle);
        match particle.local.as_str() {
            "sequence" | "choice" | "all" => {
                for child in &particle.children {
                    self.particle(child, repeated, depth + 1, out);
                }
            }
            "element" => {
                // A reference to a global element (`gml:boundedBy`, a
                // substitution group head) is left out.
                let Some(name) = particle.attr("name") else {
                    return;
                };
                let qualified = match particle.attr("form") {
                    Some(form) => form == "qualified",
                    None => self.qualified,
                };
                out.push(DeclaredProperty {
                    namespace: self.target.filter(|_| qualified).map(str::to_string),
                    name: name.to_string(),
                    type_name: self.property_type(particle, depth),
                    repeated,
                });
            }
            _ => {}
        }
    }

    /// A property's type: named, or an inline simple type's base; a simple
    /// type of this document is followed to the built-in type it restricts.
    fn property_type(&self, element: &Element, depth: usize) -> Option<(Option<String>, String)> {
        if let Some((ns, name)) = element.attr_qname("type") {
            return Some(self.simple_base(ns, name, depth));
        }
        let simple = xsd_child(element, "simpleType")?;
        let (ns, name) = xsd_child(simple, "restriction")?.attr_qname("base")?;
        Some(self.simple_base(ns, name, depth))
    }

    fn simple_base(&self, ns: Option<&str>, name: &str, depth: usize) -> (Option<String>, String) {
        let own = (ns.map(str::to_string), name.to_string());
        if depth > MAX_DEPTH {
            return own;
        }
        let base = self
            .global("simpleType", ns, name)
            .and_then(|simple| xsd_child(simple, "restriction"))
            .and_then(|restriction| restriction.attr_qname("base"));
        match base {
            Some((ns, name)) => self.simple_base(ns, name, depth + 1),
            None => own,
        }
    }
}

fn xsd_children<'a>(
    element: &'a Element,
    local: &'a str,
) -> impl Iterator<Item = &'a Element> + 'a {
    element
        .children
        .iter()
        .filter(move |child| child.is(local) && child.ns.as_deref() == Some(XSD))
}

fn xsd_child<'a>(element: &'a Element, local: &'a str) -> Option<&'a Element> {
    xsd_children(element, local).next()
}

/// `maxOccurs="unbounded"`, or a number above 1.
fn repeats(particle: &Element) -> bool {
    match particle.attr("maxOccurs") {
        Some("unbounded") => true,
        Some(n) => n.trim().parse::<u64>().is_ok_and(|n| n > 1),
        None => false,
    }
}
