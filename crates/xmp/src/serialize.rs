//! Writing an XMP packet.
//!
//! Output is always element form. The attribute shorthand XMP also allows is
//! accepted when reading, but emitting one canonical shape keeps the parser and
//! any downstream diff simple.

use crate::model::{Element, LangAlt, Value, Xmp};

/// The packet wrapper's fixed identifier. Every XMP implementation uses this
/// literal; it is not per-file.
const PACKET_ID: &str = "W5M0MpCehiHzreSzNTczkc9d";

/// A complete packet: the `<?xpacket?>` wrapper plus the XML. This is the form
/// that gets embedded in an image.
pub fn to_packet(xmp: &Xmp) -> String {
    // The `begin` attribute carries a byte-order mark so readers can detect the
    // packet's encoding.
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"{PACKET_ID}\"?>\n{}\n<?xpacket end=\"w\"?>",
        to_xml(xmp)
    )
}

/// Just the XML, without the wrapper.
pub fn to_xml(xmp: &Xmp) -> String {
    let mut out = String::from("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    out.push_str("  <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    out.push_str("    <rdf:Description rdf:about=\"\"");

    let namespaces = xmp.used_namespaces();
    for (prefix, uri) in &namespaces {
        out.push_str(&format!("\n      xmlns:{prefix}=\"{}\"", escape_attr(uri)));
    }
    out.push_str(">\n");

    for property in xmp.properties() {
        let qname = format!("{}:{}", property.prefix, property.name);
        // One level in from `rdf:Description`, which sits at level 2.
        write_value(&mut out, &qname, &property.value, 3);
    }

    out.push_str("    </rdf:Description>\n");
    out.push_str("  </rdf:RDF>\n");
    out.push_str("</x:xmpmeta>");
    out
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn write_value(out: &mut String, qname: &str, value: &Value, level: usize) {
    match value {
        Value::Text(text) => {
            indent(out, level);
            out.push_str(&format!("<{qname}>{}</{qname}>\n", escape_text(text)));
        }
        Value::Seq(items) => write_array(out, qname, "rdf:Seq", items, level),
        Value::Bag(items) => write_array(out, qname, "rdf:Bag", items, level),
        Value::Alt(items) => write_array(out, qname, "rdf:Alt", items, level),
        Value::LangAlt(alt) => write_lang_alt(out, qname, alt, level),
        Value::Raw(element) => write_element(out, element, level),
    }
}

fn write_array(out: &mut String, qname: &str, container: &str, items: &[String], level: usize) {
    indent(out, level);
    out.push_str(&format!("<{qname}>\n"));
    indent(out, level + 1);
    out.push_str(&format!("<{container}>\n"));
    for item in items {
        indent(out, level + 2);
        out.push_str(&format!("<rdf:li>{}</rdf:li>\n", escape_text(item)));
    }
    indent(out, level + 1);
    out.push_str(&format!("</{container}>\n"));
    indent(out, level);
    out.push_str(&format!("</{qname}>\n"));
}

fn write_lang_alt(out: &mut String, qname: &str, alt: &LangAlt, level: usize) {
    indent(out, level);
    out.push_str(&format!("<{qname}>\n"));
    indent(out, level + 1);
    out.push_str("<rdf:Alt>\n");
    for (lang, text) in alt.iter() {
        indent(out, level + 2);
        out.push_str(&format!(
            "<rdf:li xml:lang=\"{}\">{}</rdf:li>\n",
            escape_attr(lang),
            escape_text(text)
        ));
    }
    indent(out, level + 1);
    out.push_str("</rdf:Alt>\n");
    indent(out, level);
    out.push_str(&format!("</{qname}>\n"));
}

/// Re-emits a preserved element, which is how unmodelled shapes survive a round
/// trip unchanged.
fn write_element(out: &mut String, element: &Element, level: usize) {
    indent(out, level);
    out.push('<');
    out.push_str(&element.qname());
    for (prefix, local, value) in &element.attributes {
        let name = if prefix.is_empty() {
            local.clone()
        } else {
            format!("{prefix}:{local}")
        };
        out.push_str(&format!(" {name}=\"{}\"", escape_attr(value)));
    }

    if element.children.is_empty() && element.text.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push('>');
    if element.children.is_empty() {
        out.push_str(&escape_text(&element.text));
    } else {
        out.push('\n');
        if !element.text.trim().is_empty() {
            indent(out, level + 1);
            out.push_str(&escape_text(element.text.trim()));
            out.push('\n');
        }
        for child in &element.children {
            write_element(out, child, level + 1);
        }
        indent(out, level);
    }
    out.push_str(&format!("</{}>\n", element.qname()));
}

/// Escapes text for element content.
///
/// Names are checked where a property is set, and values are checked there too;
/// the control-character filter here is a last-resort net so that a packet can
/// never be emitted carrying a character XML cannot represent.
fn escape_text(input: &str) -> String {
    input
        .chars()
        .filter(|c| *c == '\t' || *c == '\n' || *c == '\r' || *c >= ' ')
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(input: &str) -> String {
    escape_text(input).replace('"', "&quot;")
}
