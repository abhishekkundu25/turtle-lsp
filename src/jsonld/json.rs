//! JSON with spans: the tree the JSON-LD stages read. Every value, object
//! key and bracket keeps its byte span, so each triple and each diagnostic
//! can point at the text that produced it.
//!
//! The parser (a stub for now) is fault-tolerant: it reports `json-syntax`
//! errors, recovers at commas and brackets, and still returns the values it
//! could read. A value it could not read is [`Value::Invalid`], so a key
//! with a broken value keeps its span.

use crate::syntax::{Span, SyntaxError};

/// A parsed JSON document.
#[derive(Clone, Debug, Default)]
pub struct JsonDoc {
    /// The top-level value; `None` for an empty (or whitespace-only) text.
    pub root: Option<Json>,
    /// `json-syntax` errors, in text order.
    pub errors: Vec<SyntaxError>,
}

/// A JSON value and where it is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Json {
    pub value: Value,
    /// The whole value: a string with its quotes, an object or array from
    /// its opening bracket to its closing one (or to its last member when
    /// unclosed).
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number as written (`1`, `-0.5`, `1e3`): JSON-LD tells integers
    /// from doubles by the value, and the lexical form keeps every digit.
    Number(Box<str>),
    /// The string with its escapes decoded.
    String(Box<str>),
    Array(Array),
    Object(Object),
    /// What could not be read as a value (a missing value after `:`, a stray
    /// word); its span covers the skipped text and may be empty.
    Invalid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Array {
    pub items: Vec<Json>,
    /// The `[`.
    pub open: Span,
    /// The `]`; `None` when the array is unclosed.
    pub close: Option<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// Members in text order, duplicates included.
    pub members: Vec<Member>,
    /// The `{`.
    pub open: Span,
    /// The `}`; `None` when the object is unclosed.
    pub close: Option<Span>,
}

/// A `"key": value` pair.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    /// The key with its escapes decoded.
    pub key: Box<str>,
    /// The key with its quotes.
    pub key_span: Span,
    pub value: Json,
}

impl Object {
    /// The member for `key`; with duplicate keys, the last one (JSON-LD
    /// processors read objects as maps).
    pub fn get(&self, key: &str) -> Option<&Member> {
        self.members.iter().rev().find(|m| &*m.key == key)
    }
}

impl Json {
    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match &self.value {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Array> {
        match &self.value {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
}

/// Parse JSON text. A stub: returns an empty document with no errors.
pub fn parse(text: &str) -> JsonDoc {
    let _ = text;
    JsonDoc::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_duplicate_key_wins() {
        let member = |key: &str, at: u32, v: bool| Member {
            key: key.into(),
            key_span: Span::new(at, at + 3),
            value: Json {
                value: Value::Bool(v),
                span: Span::new(at + 5, at + 9),
            },
        };
        let o = Object {
            members: vec![
                member("a", 1, false),
                member("b", 12, true),
                member("a", 23, true),
            ],
            open: Span::new(0, 1),
            close: Some(Span::new(33, 34)),
        };
        assert_eq!(o.get("a").map(|m| m.key_span.start), Some(23));
        assert!(o.get("c").is_none());
    }

    #[test]
    fn the_stub_reads_nothing() {
        let doc = parse("{\"a\": 1}");
        assert!(doc.root.is_none() && doc.errors.is_empty());
    }
}
