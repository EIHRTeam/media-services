//! XMP packet reading and writing.
//!
//! Scope is deliberately narrow: the property shapes XMP metadata for images
//! actually uses — simple values, `rdf:Seq` / `rdf:Bag` / `rdf:Alt` arrays, and
//! `rdf:Alt` language alternatives. Anything else is preserved verbatim rather
//! than modelled, so a packet this crate does not fully understand still
//! survives a read/write cycle intact.
//!
//! ```
//! use xmp::{LangAlt, Xmp};
//!
//! let mut xmp = Xmp::new();
//! xmp.set_text("xmp", "CreatorTool", "EIHRTeam Media Provenance Service").unwrap();
//! let mut rights = LangAlt::new();
//! rights.set("x-default", "All rights reserved.");
//! xmp.set_lang_alt("dc", "rights", rights).unwrap();
//!
//! let packet = xmp::to_packet(&xmp);
//! let parsed = Xmp::parse(&packet).unwrap();
//! assert_eq!(parsed.get_text("dc", "rights"), None); // it is a LangAlt, not text
//! ```

pub mod model;
pub mod parse;
pub mod serialize;

pub use model::{Element, LangAlt, Property, Result, Value, X_DEFAULT, Xmp, XmpError};
pub use parse::{looks_like_xmp, parse};
pub use serialize::{to_packet, to_xml};

impl Xmp {
    /// Parses a packet, with or without its `<?xpacket?>` wrapper.
    pub fn parse(input: &str) -> Result<Self> {
        parse::parse(input)
    }

    /// Renders a complete packet, ready to embed in an image.
    pub fn to_packet(&self) -> String {
        serialize::to_packet(self)
    }

    /// Renders the XML alone, without the `<?xpacket?>` wrapper.
    pub fn to_xml(&self) -> String {
        serialize::to_xml(self)
    }
}
