//! The XMP data model.
//!
//! XMP is RDF/XML in a specific shape, and the shape matters: a property is
//! either a simple value, an array (`rdf:Seq` / `rdf:Bag` / `rdf:Alt`), or a
//! language alternative. Modelling only those three keeps the parser honest and
//! the serializer predictable.

use std::fmt;

/// The standard namespaces, so callers can write `set_text("dc", "creator", _)`
/// without registering anything first.
const WELL_KNOWN: &[(&str, &str)] = &[
    ("x", "adobe:ns:meta/"),
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("xml", "http://www.w3.org/XML/1998/namespace"),
    ("xmp", "http://ns.adobe.com/xap/1.0/"),
    ("xmpRights", "http://ns.adobe.com/xap/1.0/rights/"),
    ("xmpMM", "http://ns.adobe.com/xap/1.0/mm/"),
    ("dc", "http://purl.org/dc/elements/1.1/"),
    ("photoshop", "http://ns.adobe.com/photoshop/1.0/"),
    ("plus", "http://ns.useplus.org/ldf/xmp/1.0/"),
    (
        "Iptc4xmpCore",
        "http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/",
    ),
];

/// The language used by an `rdf:Alt` when no language applies.
pub const X_DEFAULT: &str = "x-default";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum XmpError {
    #[error("malformed XMP XML: {0}")]
    Xml(String),
    #[error("no x:xmpmeta packet found")]
    NoPacket,
    #[error("unrecognised namespace prefix: {0}")]
    UnknownPrefix(String),
    /// A name that cannot go into XML as written.
    #[error("illegal XML name: {0:?}")]
    IllegalName(String),
    /// A value containing characters XML cannot represent.
    #[error("value contains characters that are not legal in XML")]
    IllegalCharacters,
    /// The document binds a prefix to a URI that contradicts the standard one.
    #[error(
        "namespace conflict for prefix {prefix:?}: document declares {declared:?}, expected {expected:?}"
    )]
    NamespaceConflict {
        prefix: String,
        declared: String,
        expected: String,
    },
    #[error("XMP is nested more than {0} levels deep")]
    TooDeep(usize),
}

/// The RDF namespace. Prefix matching alone is not enough to identify RDF, so
/// the URI is checked too — otherwise a document could bind `rdf` to something
/// else and have its elements read as RDF plumbing.
pub const RDF_NAMESPACE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// Ceiling on element nesting. XMP packets are shallow in practice; this exists
/// so that a hostile document cannot drive the recursive walks into a stack
/// overflow.
pub const MAX_DEPTH: usize = 64;

/// True for a string that can appear as an XML name (NCName).
///
/// Names reach us from callers — including the JS layer, where they may
/// originate in user input — and are written into the packet verbatim, so an
/// unvalidated name is an injection point.
pub fn is_valid_ncname(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// True for text XML can carry. Control characters other than tab, newline and
/// carriage return have no representation at all, so a value containing one
/// would serialise into a packet no parser accepts.
pub fn is_legal_text(text: &str) -> bool {
    text.chars()
        .all(|c| c == '\t' || c == '\n' || c == '\r' || c >= ' ')
}

pub type Result<T> = std::result::Result<T, XmpError>;

/// An ordered set of language-tagged alternatives.
///
/// `x-default` is kept first, which is what the XMP spec expects and what Adobe
/// tools emit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LangAlt {
    items: Vec<(String, String)>,
}

impl LangAlt {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces `lang`. `x-default` is kept at the front.
    pub fn set(&mut self, lang: impl Into<String>, text: impl Into<String>) -> &mut Self {
        let lang = lang.into();
        let text = text.into();
        match self.items.iter().position(|(l, _)| *l == lang) {
            Some(i) => self.items[i].1 = text,
            None if lang == X_DEFAULT => self.items.insert(0, (lang, text)),
            None => self.items.push((lang, text)),
        }
        self
    }

    pub fn get(&self, lang: &str) -> Option<&str> {
        self.items
            .iter()
            .find(|(l, _)| l == lang)
            .map(|(_, t)| t.as_str())
    }

    /// The `x-default` entry, falling back to the first item.
    pub fn default_text(&self) -> Option<&str> {
        self.get(X_DEFAULT)
            .or_else(|| self.items.first().map(|(_, t)| t.as_str()))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.items.iter().map(|(l, t)| (l.as_str(), t.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// A raw XML element, kept verbatim.
///
/// Real-world XMP carries shapes this model does not cover — a Photoshop export
/// brings `xmpMM:DerivedFrom` and history structs. Round-tripping those as an
/// opaque element is the difference between preserving a packet and silently
/// corrupting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub prefix: String,
    pub name: String,
    /// `(prefix, local name, value)`, preserving order.
    pub attributes: Vec<(String, String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn qname(&self) -> String {
        if self.prefix.is_empty() {
            self.name.clone()
        } else {
            format!("{}:{}", self.prefix, self.name)
        }
    }
}

/// A property value, in the shapes XMP actually uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A simple property: the element's text content.
    Text(String),
    /// `rdf:Seq` — an ordered array.
    Seq(Vec<String>),
    /// `rdf:Bag` — an unordered array.
    Bag(Vec<String>),
    /// `rdf:Alt` — alternatives with no language.
    Alt(Vec<String>),
    /// `rdf:Alt` where each item carries `xml:lang`.
    LangAlt(LangAlt),
    /// A shape the model does not cover, preserved verbatim.
    Raw(Box<Element>),
}

impl Value {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(t) => Some(t),
            _ => None,
        }
    }

    /// True for the boolean XMP spells as the text `True` or `False`.
    pub fn as_bool(&self) -> Option<bool> {
        match self.as_text()?.trim() {
            "True" | "true" => Some(true),
            "False" | "false" => Some(false),
            _ => None,
        }
    }
}

/// One `prefix:name` property, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    pub prefix: String,
    pub name: String,
    pub value: Value,
}

/// An XMP packet: a namespace registry plus ordered properties.
///
/// Order is preserved rather than sorted so that re-serialising a packet leaves
/// a reviewable diff instead of a reshuffle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xmp {
    namespaces: Vec<(String, String)>,
    properties: Vec<Property>,
}

impl Default for Xmp {
    fn default() -> Self {
        Self::new()
    }
}

impl Xmp {
    pub fn new() -> Self {
        Self {
            namespaces: WELL_KNOWN
                .iter()
                .map(|(p, u)| ((*p).to_string(), (*u).to_string()))
                .collect(),
            properties: Vec::new(),
        }
    }

    pub fn namespace_uri(&self, prefix: &str) -> Option<&str> {
        self.namespaces
            .iter()
            .find(|(p, _)| p == prefix)
            .map(|(_, u)| u.as_str())
    }

    /// Registers a prefix. Callers wanting a private namespace must do this
    /// before setting a property in it.
    pub fn register_namespace(&mut self, prefix: &str, uri: &str) -> Result<()> {
        if !is_valid_ncname(prefix) {
            return Err(XmpError::IllegalName(prefix.to_string()));
        }
        if !is_legal_text(uri) {
            return Err(XmpError::IllegalCharacters);
        }
        match self.namespaces.iter_mut().find(|(p, _)| p == prefix) {
            Some(entry) => entry.1 = uri.to_string(),
            None => self.namespaces.push((prefix.to_string(), uri.to_string())),
        }
        Ok(())
    }

    pub fn properties(&self) -> &[Property] {
        &self.properties
    }

    /// Namespaces actually used by at least one property. Serialisation declares
    /// only these, so a packet never carries a prefix it does not use.
    pub fn used_namespaces(&self) -> Vec<(&str, &str)> {
        let mut used: Vec<(&str, &str)> = Vec::new();
        for property in &self.properties {
            push_prefix(&mut used, self, &property.prefix);
            if let Value::Raw(element) = &property.value {
                collect_raw_prefixes(element, &mut used, self);
            }
        }
        used
    }

    fn position(&self, prefix: &str, name: &str) -> Option<usize> {
        self.properties
            .iter()
            .position(|p| p.prefix == prefix && p.name == name)
    }

    pub fn get(&self, prefix: &str, name: &str) -> Option<&Value> {
        self.position(prefix, name)
            .map(|i| &self.properties[i].value)
    }

    pub fn get_text(&self, prefix: &str, name: &str) -> Option<&str> {
        self.get(prefix, name)?.as_text()
    }

    /// Sets a property, keeping its original position if it already existed.
    pub fn set(&mut self, prefix: &str, name: &str, value: Value) -> Result<&mut Self> {
        if self.namespace_uri(prefix).is_none() {
            return Err(XmpError::UnknownPrefix(prefix.to_string()));
        }
        // Both names are written into the packet as-is, so they are checked
        // rather than trusted.
        for candidate in [prefix, name] {
            if !is_valid_ncname(candidate) {
                return Err(XmpError::IllegalName(candidate.to_string()));
            }
        }
        check_value_text(&value)?;
        match self.position(prefix, name) {
            Some(i) => self.properties[i].value = value,
            None => self.properties.push(Property {
                prefix: prefix.to_string(),
                name: name.to_string(),
                value,
            }),
        }
        Ok(self)
    }

    pub fn set_text(
        &mut self,
        prefix: &str,
        name: &str,
        text: impl Into<String>,
    ) -> Result<&mut Self> {
        self.set(prefix, name, Value::Text(text.into()))
    }

    pub fn set_seq(&mut self, prefix: &str, name: &str, items: Vec<String>) -> Result<&mut Self> {
        self.set(prefix, name, Value::Seq(items))
    }

    pub fn set_lang_alt(&mut self, prefix: &str, name: &str, alt: LangAlt) -> Result<&mut Self> {
        self.set(prefix, name, Value::LangAlt(alt))
    }

    pub fn remove(&mut self, prefix: &str, name: &str) -> Option<Value> {
        self.position(prefix, name)
            .map(|i| self.properties.remove(i).value)
    }
}

impl fmt::Display for Xmp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::serialize::to_xml(self))
    }
}

/// Records `prefix` as used, if it resolves and is not already recorded.
fn push_prefix<'a>(used: &mut Vec<(&'a str, &'a str)>, xmp: &'a Xmp, prefix: &'a str) {
    if prefix.is_empty() || used.iter().any(|(p, _)| *p == prefix) {
        return;
    }
    if let Some(uri) = xmp.namespace_uri(prefix) {
        used.push((prefix, uri));
    }
}

/// Rejects values carrying characters XML cannot represent.
fn check_value_text(value: &Value) -> Result<()> {
    let ok = match value {
        Value::Text(t) => is_legal_text(t),
        Value::Seq(items) | Value::Bag(items) | Value::Alt(items) => {
            items.iter().all(|i| is_legal_text(i))
        }
        Value::LangAlt(alt) => alt
            .iter()
            .all(|(l, t)| is_legal_text(l) && is_legal_text(t)),
        Value::Raw(_) => true,
    };
    if ok {
        Ok(())
    } else {
        Err(XmpError::IllegalCharacters)
    }
}

/// Prefixes an opaque element and its descendants refer to, so the packet still
/// declares them.
fn collect_raw_prefixes<'a>(
    element: &'a Element,
    used: &mut Vec<(&'a str, &'a str)>,
    xmp: &'a Xmp,
) {
    push_prefix(used, xmp, &element.prefix);
    for (prefix, _, _) in &element.attributes {
        push_prefix(used, xmp, prefix);
    }
    for child in &element.children {
        collect_raw_prefixes(child, used, xmp);
    }
}
