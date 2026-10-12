//! Everything the editor features need from one document, computed once per
//! version: the parse, line positions, and per-IRI indexes.

use crate::syntax::{self, Node, Parsed, PrefixMap, Ref, Role, Span};
use line_index::{LineIndex, TextSize, WideEncoding};
use lsp_types::{Position, Range};
use rustc_hash::FxHashMap;
use std::sync::Arc;

const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const SKOS_PREF_LABEL: &str = "http://www.w3.org/2004/02/skos/core#prefLabel";
const RDFS_COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
const SKOS_DEFINITION: &str = "http://www.w3.org/2004/02/skos/core#definition";
const SH_NAME: &str = "http://www.w3.org/ns/shacl#name";
const DCT_DESCRIPTION: &str = "http://purl.org/dc/terms/description";
pub const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
pub const RDFS_SUBPROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

pub struct Analysis {
    pub uri: String,
    /// The text this analysis was computed from.
    pub text: Arc<str>,
    pub parsed: Parsed,
    pub lines: LineIndex,
    /// Prefixes supplied from outside the document (prefixes.ttl / settings).
    pub external: Arc<PrefixMap>,
    /// IRI -> spans where it is the subject of a top-level statement.
    pub defs: FxHashMap<String, Vec<Span>>,
    /// IRI -> indexes into `parsed.refs`.
    pub refs_by_iri: FxHashMap<String, Vec<u32>>,
    pub labels: FxHashMap<String, String>,
    pub comments: FxHashMap<String, String>,
    pub types: FxHashMap<String, Vec<String>>,
    /// IRI -> direct superclasses / superproperties (rdfs:subClassOf, rdfs:subPropertyOf).
    pub supers: FxHashMap<String, Vec<String>>,
}

impl Analysis {
    pub fn new(uri: String, text: &str, external: Arc<PrefixMap>, keep_tokens: bool) -> Analysis {
        let mut parsed = syntax::parse(text, &external);
        if !keep_tokens {
            parsed.tokens = Vec::new();
            parsed.comments = Vec::new();
        }
        let mut defs: FxHashMap<String, Vec<Span>> = FxHashMap::default();
        let mut refs_by_iri: FxHashMap<String, Vec<u32>> = FxHashMap::default();
        for (i, r) in parsed.refs.iter().enumerate() {
            if r.role == Role::Subject {
                defs.entry(r.iri.clone()).or_default().push(r.span);
            }
            refs_by_iri.entry(r.iri.clone()).or_default().push(i as u32);
        }
        let mut labels = FxHashMap::default();
        let mut comments = FxHashMap::default();
        let mut types: FxHashMap<String, Vec<String>> = FxHashMap::default();
        let mut supers: FxHashMap<String, Vec<String>> = FxHashMap::default();
        for t in &parsed.triples {
            let Node::Iri(s) = &t.s else { continue };
            match (&*t.p, &t.o) {
                (syntax::RDF_TYPE, Node::Iri(o)) => {
                    types.entry(s.clone()).or_default().push(o.clone())
                }
                (RDFS_SUBCLASS_OF | RDFS_SUBPROPERTY_OF, Node::Iri(o)) => {
                    supers.entry(s.clone()).or_default().push(o.clone())
                }
                (RDFS_LABEL | SKOS_PREF_LABEL | SH_NAME, Node::Literal { value, lang, .. }) => {
                    if lang.as_deref().is_none_or(|l| l.starts_with("en"))
                        || !labels.contains_key(s)
                    {
                        labels.insert(s.clone(), value.clone());
                    }
                }
                (
                    RDFS_COMMENT | SKOS_DEFINITION | DCT_DESCRIPTION,
                    Node::Literal { value, lang, .. },
                ) if (lang.as_deref().is_none_or(|l| l.starts_with("en"))
                    || !comments.contains_key(s)) =>
                {
                    comments.insert(s.clone(), value.clone());
                }
                _ => {}
            }
        }
        Analysis {
            uri,
            text: Arc::from(text),
            lines: LineIndex::new(text),
            parsed,
            external,
            defs,
            refs_by_iri,
            labels,
            comments,
            types,
            supers,
        }
    }

    /// All prefixes in scope at the end of the document: external, then declared.
    pub fn prefix_map(&self) -> PrefixMap {
        let mut m = (*self.external).clone();
        for p in &self.parsed.prefixes {
            m.insert(p.name.clone(), p.iri.clone());
        }
        m
    }

    pub fn ref_at(&self, offset: u32) -> Option<&Ref> {
        let refs = &self.parsed.refs;
        // Refs are recorded in text order.
        let i = refs.partition_point(|r| r.span.end < offset);
        refs[i..]
            .iter()
            .take_while(|r| r.span.start <= offset)
            .find(|r| r.span.contains(offset))
    }

    pub fn refs_of<'a>(&'a self, iri: &str) -> impl Iterator<Item = &'a Ref> + 'a {
        self.refs_by_iri
            .get(iri)
            .into_iter()
            .flatten()
            .map(|i| &self.parsed.refs[*i as usize])
    }

    pub fn position(&self, enc: Encoding, offset: u32) -> Position {
        let lc = self
            .lines
            .line_col(TextSize::from(offset.min(u32::from(self.lines.len()))));
        match enc {
            Encoding::Utf8 => Position::new(lc.line, lc.col),
            Encoding::Utf16 => {
                let w = self.lines.to_wide(WideEncoding::Utf16, lc).unwrap_or(
                    line_index::WideLineCol {
                        line: lc.line,
                        col: lc.col,
                    },
                );
                Position::new(w.line, w.col)
            }
        }
    }

    pub fn range(&self, enc: Encoding, span: Span) -> Range {
        Range::new(self.position(enc, span.start), self.position(enc, span.end))
    }

    pub fn offset(&self, enc: Encoding, pos: Position) -> Option<u32> {
        offset_in(&self.lines, &self.text, enc, pos)
    }
}

/// Byte offset of an LSP position. Clients send positions past the end of a
/// line or of the file, and (in UTF-8 mode) columns inside a multi-byte
/// character; all of these are clamped to a valid character boundary.
pub fn offset_in(lines: &LineIndex, text: &str, enc: Encoding, pos: Position) -> Option<u32> {
    let Some(line) = lines.line(pos.line) else {
        return Some(text.len() as u32);
    };
    let (start, end) = (
        u32::from(line.start()) as usize,
        u32::from(line.end()) as usize,
    );
    // The line's content, without its line break.
    let content_end = text[start..end].trim_end_matches(['\n', '\r']).len() + start;
    let lc = match enc {
        Encoding::Utf8 => line_index::LineCol {
            line: pos.line,
            col: pos.character,
        },
        Encoding::Utf16 => lines.to_utf8(
            WideEncoding::Utf16,
            line_index::WideLineCol {
                line: pos.line,
                col: pos.character,
            },
        )?,
    };
    let mut off = (start + lc.col as usize).min(content_end);
    while !text.is_char_boundary(off) {
        off -= 1;
    }
    Some(off as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_from_clients_are_clamped_to_valid_offsets() {
        let text = "ex:é a ex:B .\r\nab\n";
        let lines = LineIndex::new(text);
        let at = |enc, line, character| {
            offset_in(&lines, text, enc, Position::new(line, character)).unwrap() as usize
        };
        // Past the end of a line: the end of its content, before \r\n.
        assert_eq!(at(Encoding::Utf16, 0, 500), text.find('\r').unwrap());
        // Past the last line: the end of the text.
        assert_eq!(at(Encoding::Utf8, 9, 0), text.len());
        // UTF-8 column inside 'é' (2 bytes): snapped back to its start.
        assert_eq!(at(Encoding::Utf8, 0, 4), 3);
        for line in 0..4 {
            for col in 0..40 {
                for enc in [Encoding::Utf8, Encoding::Utf16] {
                    assert!(text.is_char_boundary(at(enc, line, col)));
                }
            }
        }
    }
}
