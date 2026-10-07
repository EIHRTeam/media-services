//! Reading an XMP packet.
//!
//! Built on a small element tree rather than a streaming state machine: packet
//! sizes are small, and having the whole subtree lets unmodelled shapes be kept
//! verbatim instead of dropped.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::model::{
    Element, LangAlt, MAX_DEPTH, RDF_NAMESPACE, Result, Value, X_DEFAULT, Xmp, XmpError,
    is_valid_ncname,
};

/// Prefixes are matched literally. Every real XMP packet binds `rdf` to the RDF
/// namespace by convention, and resolving prefixes through a stack of `xmlns`
/// declarations would buy nothing here — but a declaration that *contradicts* a
/// known prefix is rejected rather than trusted, since otherwise a packet could
/// relabel `dc` or `xmp` and have us write properties into an attacker's
/// namespace.
const RDF_PREFIX: &str = "rdf";

/// Parses a packet, with or without its `<?xpacket?>` wrapper.
pub fn parse(input: &str) -> Result<Xmp> {
    let root = build_tree(input)?;

    let mut xmp = Xmp::new();
    collect_namespaces(&root, &mut xmp)?;

    let rdf = find_rdf(&root).ok_or(XmpError::NoPacket)?;
    for description in rdf
        .children
        .iter()
        .filter(|c| c.prefix == RDF_PREFIX && c.name == "Description")
    {
        for (prefix, local, value) in &description.attributes {
            // `xmlns*` are declarations, `rdf:*` are RDF plumbing, and an
            // unprefixed attribute is not a property.
            if prefix == "xmlns" || local == "xmlns" || prefix.is_empty() || prefix == RDF_PREFIX {
                continue;
            }
            if prefix == "xml" {
                continue;
            }
            xmp.set_text(prefix, local, value.clone())?;
        }
        for child in &description.children {
            interpret_property(child, &mut xmp)?;
        }
    }
    Ok(xmp)
}

/// True when the input looks like an XMP packet at all, for callers that need to
/// tell "no XMP here" from "corrupt XMP".
pub fn looks_like_xmp(input: &str) -> bool {
    input.contains("xmpmeta") || input.contains("rdf:RDF")
}

fn build_tree(input: &str) -> Result<Element> {
    let mut reader = Reader::from_str(input);
    // Whitespace is significant to the tree builder; trimming happens where a
    // value is interpreted, so element text that is only indentation does not
    // leak into property values.
    reader.config_mut().trim_text(false);

    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    loop {
        let event = reader
            .read_event()
            .map_err(|e| XmpError::Xml(e.to_string()))?;
        match event {
            Event::Start(e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(XmpError::TooDeep(MAX_DEPTH));
                }
                stack.push(element_of(&e)?);
            }
            Event::Empty(e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(XmpError::TooDeep(MAX_DEPTH));
                }
                let el = element_of(&e)?;
                attach(&mut stack, &mut root, el);
            }
            Event::End(_) => {
                if let Some(el) = stack.pop() {
                    attach(&mut stack, &mut root, el);
                }
            }
            Event::Text(t) => {
                // quick-xml resolves entities before handing text back, so this
                // is already the decoded string.
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::GeneralRef(r) => {
                let name = String::from_utf8_lossy(&r.into_inner()).into_owned();
                let resolved = quick_xml::escape::resolve_xml_entity(&name)
                    .map(str::to_owned)
                    .or_else(|| resolve_char_ref(&name));
                match (resolved, stack.last_mut()) {
                    (Some(text), Some(top)) => top.text.push_str(&text),
                    // Silently dropping an entity is exactly how a value loses
                    // characters, so an unresolved reference is an error.
                    (None, _) => return Err(XmpError::Xml(format!("unresolved entity &{name};"))),
                    (Some(_), None) => {}
                }
            }
            Event::CData(c) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(c.as_ref()));
                }
            }
            Event::Eof => break,
            // Declarations, processing instructions (the `<?xpacket?>` wrapper),
            // comments and doctypes carry nothing we model.
            _ => {}
        }
    }

    root.ok_or(XmpError::NoPacket)
}

fn attach(stack: &mut [Element], root: &mut Option<Element>, element: Element) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(element),
        None => {
            if root.is_none() {
                *root = Some(element);
            }
        }
    }
}

fn element_of(e: &BytesStart<'_>) -> Result<Element> {
    let (prefix, name) = split_name(e.name().as_ref());
    let mut attributes = Vec::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|err| XmpError::Xml(err.to_string()))?;
        let (attr_prefix, attr_local) = split_name(attr.key.as_ref());
        let value = attr
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|err| XmpError::Xml(err.to_string()))?
            .into_owned();
        attributes.push((attr_prefix, attr_local, value));
    }
    Ok(Element {
        prefix,
        name,
        attributes,
        children: Vec::new(),
        text: String::new(),
    })
}

/// Numeric character references, which the predefined-entity table does not
/// cover.
///
/// Returns `None` for a code point XML cannot represent. Accepting one would put
/// a character into the packet that no parser can read back — and since `&#1;`
/// is a legal-looking way to write it, this is the boundary where that has to be
/// caught.
fn resolve_char_ref(name: &str) -> Option<String> {
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    let ch = char::from_u32(code)?;
    (ch == '\t' || ch == '\n' || ch == '\r' || ch >= ' ').then(|| ch.to_string())
}

fn split_name(raw: &[u8]) -> (String, String) {
    let full = String::from_utf8_lossy(raw);
    match full.split_once(':') {
        Some((prefix, local)) => (prefix.to_string(), local.to_string()),
        None => (String::new(), full.into_owned()),
    }
}

fn find_rdf(element: &Element) -> Option<&Element> {
    if element.prefix == RDF_PREFIX && element.name == "RDF" {
        return Some(element);
    }
    element.children.iter().find_map(find_rdf)
}

fn collect_namespaces(element: &Element, xmp: &mut Xmp) -> Result<()> {
    for (prefix, local, value) in &element.attributes {
        if prefix == "xmlns" {
            match xmp.namespace_uri(local) {
                // Already known: the declaration must agree with it.
                Some(known) if known != value => {
                    return Err(XmpError::NamespaceConflict {
                        prefix: local.clone(),
                        declared: value.clone(),
                        expected: known.to_string(),
                    });
                }
                Some(_) => {}
                None => {
                    if !is_valid_ncname(local) {
                        return Err(XmpError::IllegalName(local.clone()));
                    }
                    xmp.register_namespace(local, value)?;
                }
            }
            // `rdf` decides how the packet is read, so it is pinned explicitly
            // rather than left to the well-known table.
            if local == RDF_PREFIX && value != RDF_NAMESPACE {
                return Err(XmpError::NamespaceConflict {
                    prefix: RDF_PREFIX.to_string(),
                    declared: value.clone(),
                    expected: RDF_NAMESPACE.to_string(),
                });
            }
        }
    }
    for child in &element.children {
        collect_namespaces(child, xmp)?;
    }
    Ok(())
}

fn attr<'a>(element: &'a Element, prefix: &str, name: &str) -> Option<&'a str> {
    element
        .attributes
        .iter()
        .find(|(p, l, _)| p == prefix && l == name)
        .map(|(_, _, v)| v.as_str())
}

fn interpret_property(element: &Element, xmp: &mut Xmp) -> Result<()> {
    // rdf: plumbing, or a nested rdf:Description, is not a property.
    if element.prefix.is_empty() || element.prefix == RDF_PREFIX {
        return Ok(());
    }
    // `None` means the shape is unmodelled — keep the element verbatim rather
    // than dropping a property we did not understand.
    let value = value_of(element).unwrap_or_else(|| Value::Raw(Box::new(element.clone())));
    xmp.set(&element.prefix, &element.name, value)?;
    Ok(())
}

/// `None` means the shape is not modelled and the element should be kept
/// verbatim.
fn value_of(element: &Element) -> Option<Value> {
    // `<prop rdf:resource="..."/>` is how XMP spells a URI value.
    if element.children.is_empty()
        && let Some(uri) = attr(element, RDF_PREFIX, "resource")
    {
        return Some(Value::Text(uri.to_string()));
    }

    // A struct spelled inline (`rdf:parseType="Resource"`, or bare prefixed
    // attributes) has no children but is emphatically not a simple value.
    let has_prefixed_attr = element
        .attributes
        .iter()
        .any(|(p, _, _)| !p.is_empty() && p != "xml" && p != "xmlns");
    if has_prefixed_attr || attr(element, RDF_PREFIX, "parseType").is_some() {
        return None;
    }

    if let [container] = element.children.as_slice()
        && container.prefix == RDF_PREFIX
    {
        match container.name.as_str() {
            "Seq" => return simple_items(container).map(Value::Seq),
            "Bag" => return simple_items(container).map(Value::Bag),
            "Alt" => {
                let any_language = container
                    .children
                    .iter()
                    .any(|li| attr(li, "xml", "lang").is_some());
                if !any_language {
                    return simple_items(container).map(Value::Alt);
                }
                let mut alt = LangAlt::new();
                for li in &container.children {
                    if li.prefix != RDF_PREFIX || li.name != "li" {
                        return None;
                    }
                    if !li.children.is_empty() {
                        return None;
                    }
                    let lang = attr(li, "xml", "lang").unwrap_or(X_DEFAULT).to_string();
                    alt.set(lang, li.text.trim());
                }
                return Some(Value::LangAlt(alt));
            }
            _ => return None,
        }
    }

    if element.children.is_empty() {
        Some(Value::Text(element.text.trim().to_string()))
    } else {
        None
    }
}

/// The `rdf:li` texts of an array, or `None` if any item is not a simple string.
fn simple_items(container: &Element) -> Option<Vec<String>> {
    let mut items = Vec::new();
    for li in &container.children {
        if li.prefix != RDF_PREFIX || li.name != "li" || !li.children.is_empty() {
            return None;
        }
        items.push(li.text.trim().to_string());
    }
    Some(items)
}
