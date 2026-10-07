//! Describing a change to an image's XMP.
//!
//! The shape is named fields rather than raw property setters so that callers —
//! including the JS layer — never have to know namespace prefixes. Dates are
//! supplied rather than read from the clock: that keeps the library
//! deterministic, which is what makes the output testable at all.

use std::collections::BTreeMap;

use serde::Deserialize;
use xmp::{LangAlt, X_DEFAULT, Xmp};

use crate::error::{Error, Result};

/// The three XMP timestamps, as ISO 8601 with a UTC offset.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dates {
    pub create: Option<String>,
    pub modify: Option<String>,
    pub metadata: Option<String>,
}

impl Dates {
    /// The instant these describe.
    ///
    /// The usual case is that all three come from one clock reading, so any of
    /// them can stand for the whole. The order matters only when a caller
    /// deliberately sets them apart.
    pub fn stamp(&self) -> Option<&str> {
        self.create
            .as_deref()
            .or(self.modify.as_deref())
            .or(self.metadata.as_deref())
    }
}

/// A value for an arbitrary property.
///
/// A bare string is a simple value; the tagged forms say which array shape is
/// meant, because `dc:creator` and `dc:subject` are both lists but only one of
/// them is ordered.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum XmpValue {
    Text(String),
    Tagged(TaggedValue),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaggedValue {
    pub text: Option<String>,
    pub seq: Option<Vec<String>>,
    pub bag: Option<Vec<String>>,
    pub lang_alt: Option<BTreeMap<String, String>>,
}

impl XmpValue {
    fn into_value(self) -> Result<(xmp::Value, &'static str)> {
        Ok(match self {
            Self::Text(text) => (xmp::Value::Text(text), "text"),
            Self::Tagged(tagged) => {
                // Exactly one shape, so a mistyped key is an error rather than a
                // silently empty property.
                let chosen: Vec<(&str, xmp::Value)> = [
                    tagged.text.map(|t| ("text", xmp::Value::Text(t))),
                    tagged.seq.map(|s| ("seq", xmp::Value::Seq(s))),
                    tagged.bag.map(|b| ("bag", xmp::Value::Bag(b))),
                    tagged
                        .lang_alt
                        .map(|a| ("langAlt", xmp::Value::LangAlt(lang_alt(&a)))),
                ]
                .into_iter()
                .flatten()
                .collect();

                match <[_; 1]>::try_from(chosen) {
                    Ok([(shape, value)]) => (value, shape),
                    Err(mut found) => {
                        return Err(Error::MalformedXmp(if found.is_empty() {
                            "a property value must set one of text, seq, bag or langAlt".into()
                        } else {
                            found.sort_by_key(|(shape, _)| *shape);
                            format!(
                                "a property value must set exactly one shape, found {}",
                                found
                                    .iter()
                                    .map(|(shape, _)| *shape)
                                    .collect::<Vec<_>>()
                                    .join(" and ")
                            )
                        }));
                    }
                }
            }
        })
    }
}

/// What to change. Every field is optional; absent fields are left alone.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct XmpEdit {
    /// A packet to start from when the image has no XMP of its own. Parsing
    /// fails loudly rather than silently starting empty, because discarding a
    /// rights statement is worse than refusing.
    pub base_packet: Option<String>,
    /// Namespaces for prefixes XMP does not define, as `prefix: uri`.
    pub namespaces: Option<BTreeMap<String, String>>,
    /// Any property at all, keyed `prefix:name`.
    ///
    /// The named fields below cover the vocabulary this was built for;
    /// this is how everything else — `photoshop:Credit`, an IPTC field, a
    /// private namespace — is reached. Applied last, so it wins over the
    /// named fields when both set the same property.
    pub properties: Option<BTreeMap<String, XmpValue>>,
    pub creator_tool: Option<String>,
    pub dates: Option<Dates>,
    /// `dc:creator`, an ordered list.
    pub creator: Option<Vec<String>>,
    pub source: Option<String>,
    /// `dc:rights`, keyed by language.
    pub rights: Option<BTreeMap<String, String>>,
    pub web_statement: Option<String>,
    /// `xmpRights:UsageTerms`, keyed by language.
    pub usage_terms: Option<BTreeMap<String, String>>,
    /// `xmpRights:Marked`.
    pub marked: Option<bool>,
}

fn lang_alt(values: &BTreeMap<String, String>) -> LangAlt {
    let mut alt = LangAlt::new();
    for (lang, text) in values {
        // `LangAlt::set` keeps x-default at the front regardless of insertion
        // order, so the sorted iteration here is fine.
        alt.set(lang.clone(), text.clone());
    }
    alt
}

impl XmpEdit {
    /// True when there is nothing to do, so the caller can skip a rewrite.
    pub fn is_empty(&self) -> bool {
        self.namespaces.is_none()
            && self.properties.is_none()
            && self.creator_tool.is_none()
            && self.dates.is_none()
            && self.creator.is_none()
            && self.source.is_none()
            && self.rights.is_none()
            && self.web_statement.is_none()
            && self.usage_terms.is_none()
            && self.marked.is_none()
    }

    /// Applies the edit to `existing` if the image carries a packet, otherwise
    /// to `base_packet`, otherwise to an empty one.
    pub fn apply(&self, existing: Option<&str>) -> Result<Xmp> {
        let mut xmp = match existing {
            Some(text) => Xmp::parse(text).map_err(|e| Error::MalformedXmp(e.to_string()))?,
            None => match &self.base_packet {
                Some(base) => Xmp::parse(base).map_err(|e| Error::MalformedXmp(e.to_string()))?,
                None => Xmp::new(),
            },
        };

        // Before anything is set, so a property in a private namespace has
        // somewhere to live.
        for (prefix, uri) in self.namespaces.iter().flatten() {
            xmp.register_namespace(prefix, uri)
                .map_err(|e| Error::MalformedXmp(e.to_string()))?;
        }

        let set = |result: std::result::Result<&mut Xmp, xmp::XmpError>| {
            result
                .map(|_| ())
                .map_err(|e| Error::MalformedXmp(e.to_string()))
        };

        if let Some(tool) = &self.creator_tool {
            set(xmp.set_text("xmp", "CreatorTool", tool.clone()))?;
        }
        if let Some(dates) = &self.dates {
            for (name, value) in [
                ("CreateDate", &dates.create),
                ("ModifyDate", &dates.modify),
                ("MetadataDate", &dates.metadata),
            ] {
                if let Some(value) = value {
                    set(xmp.set_text("xmp", name, value.clone()))?;
                }
            }
        }
        if let Some(creator) = &self.creator {
            set(xmp.set_seq("dc", "creator", creator.clone()))?;
        }
        if let Some(source) = &self.source {
            set(xmp.set_text("dc", "source", source.clone()))?;
        }
        if let Some(rights) = &self.rights {
            set(xmp.set_lang_alt("dc", "rights", lang_alt(rights)))?;
        }
        if let Some(statement) = &self.web_statement {
            set(xmp.set_text("xmpRights", "WebStatement", statement.clone()))?;
        }
        if let Some(terms) = &self.usage_terms {
            set(xmp.set_lang_alt("xmpRights", "UsageTerms", lang_alt(terms)))?;
        }
        if let Some(marked) = self.marked {
            // XMP spells booleans as the text `True` / `False`.
            let text = if marked { "True" } else { "False" };
            set(xmp.set_text("xmpRights", "Marked", text))?;
        }

        // Last, so the general form wins where both set the same property.
        for (key, value) in self.properties.iter().flatten() {
            let (prefix, name) = key.split_once(':').ok_or_else(|| {
                Error::MalformedXmp(format!(
                    "property key {key:?} is not in prefix:name form, \
                     for example \"photoshop:Credit\""
                ))
            })?;
            if prefix.is_empty() || name.is_empty() {
                return Err(Error::MalformedXmp(format!(
                    "property key {key:?} is not in prefix:name form"
                )));
            }
            let (value, _shape) = value.clone().into_value()?;
            set(xmp.set(prefix, name, value))?;
        }

        Ok(xmp)
    }
}

/// Reads `dc:rights`' x-default as a convenience for callers that only want the
/// copyright line.
pub fn default_rights(xmp: &Xmp) -> Option<&str> {
    match xmp.get("dc", "rights")? {
        xmp::Value::LangAlt(alt) => alt.get(X_DEFAULT),
        xmp::Value::Text(text) => Some(text),
        _ => None,
    }
}
