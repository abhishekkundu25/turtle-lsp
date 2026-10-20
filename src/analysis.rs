//! Everything the editor features need from one document, computed once per
//! version: the parse, line positions, and per-IRI indexes. An RDF document
//! gets an [`Analysis`]; a SPARQL document a [`SparqlAnalysis`], with the
//! same position helpers.

use crate::sparql;
use crate::syntax::{self, Iri, IriKey, IriMap, Node, Parsed, PrefixMap, Ref, Span, Syntax};
use line_index::{LineIndex, TextSize, WideEncoding};
use lsp_types::{Position, Range};
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
    /// The URL an import's text came from, when `uri` names its cached copy.
    pub origin: Option<String>,
    /// The text this analysis was computed from.
    pub text: Arc<str>,
    pub parsed: Parsed,
    pub lines: LineIndex,
    /// Prefixes supplied from outside the document (prefixes.ttl / settings).
    pub external: Arc<PrefixMap>,
    /// IRI -> spans where it is the subject of a top-level statement or the
    /// name of a TriG graph block.
    pub defs: IriMap<Vec<Span>>,
    /// IRI -> indexes into `parsed.refs`.
    pub refs_by_iri: IriMap<Vec<u32>>,
    /// IRI -> index into `parsed.triples` of its label; see [`Analysis::label`].
    labels: IriMap<u32>,
    /// IRI -> index into `parsed.triples` of its comment; see [`Analysis::comment`].
    comments: IriMap<u32>,
    pub types: IriMap<Vec<Iri>>,
    /// IRI -> direct superclasses / superproperties (rdfs:subClassOf, rdfs:subPropertyOf).
    pub supers: IriMap<Vec<Iri>>,
}

/// Which predicate feeds which index.
#[derive(Clone, Copy)]
enum Index {
    Type,
    Super,
    Label,
    Comment,
}

fn indexed_predicates() -> &'static [(Iri, Index)] {
    static P: std::sync::OnceLock<Vec<(Iri, Index)>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        [
            (syntax::RDF_TYPE, Index::Type),
            (RDFS_SUBCLASS_OF, Index::Super),
            (RDFS_SUBPROPERTY_OF, Index::Super),
            (RDFS_LABEL, Index::Label),
            (SKOS_PREF_LABEL, Index::Label),
            (SH_NAME, Index::Label),
            (RDFS_COMMENT, Index::Comment),
            (SKOS_DEFINITION, Index::Comment),
            (DCT_DESCRIPTION, Index::Comment),
        ]
        .into_iter()
        .map(|(p, i)| (Iri::intern(p), i))
        .collect()
    })
}

/// English (or untagged) text wins; otherwise the first one seen.
fn prefer(map: &mut IriMap<u32>, triples: &[syntax::Triple], s: Iri, i: u32) {
    let Node::Literal { lang, .. } = &triples[i as usize].o else {
        return;
    };
    if lang.is_none_or(|l| l.starts_with("en")) || !map.contains_key(&s) {
        map.insert(s, i);
    }
}

impl Analysis {
    /// The syntax comes from the URI's extension (Turtle if it has none of ours).
    pub fn new(uri: String, text: &str, external: Arc<PrefixMap>, keep_tokens: bool) -> Analysis {
        let syntax = Syntax::from_extension(&uri).unwrap_or_default();
        Analysis::with_syntax(uri, text, external, keep_tokens, syntax)
    }

    /// For a document whose language is known, e.g. from the LSP language id.
    /// JSON-LD contexts come from the bundled ones only (see
    /// [`Analysis::with_loader`]).
    pub fn with_syntax(
        uri: String,
        text: &str,
        external: Arc<PrefixMap>,
        keep_tokens: bool,
        language: Syntax,
    ) -> Analysis {
        let loader = crate::jsonld::loader::Bundled;
        Analysis::with_loader(uri, text, external, keep_tokens, language, &loader)
    }

    /// [`Analysis::with_syntax`], with JSON-LD's remote contexts from `loader`.
    pub fn with_loader(
        uri: String,
        text: &str,
        external: Arc<PrefixMap>,
        keep_tokens: bool,
        language: Syntax,
        loader: &dyn crate::jsonld::ContextLoader,
    ) -> Analysis {
        let mut parsed = match language {
            // Relative IRIs resolve against the document's URL.
            Syntax::JsonLd => crate::jsonld::parse(text, Some(&uri), loader),
            _ => syntax::parse(text, &external, language),
        };
        if !keep_tokens {
            parsed.tokens = Vec::new();
            parsed.comments = Vec::new();
        }
        // Analyses live until the next edit (or for the session, for files on
        // disk): drop the growth slack.
        parsed.triples.shrink_to_fit();
        parsed.refs.shrink_to_fit();
        parsed.prefix_uses.shrink_to_fit();
        parsed.statements.shrink_to_fit();
        let mut defs: IriMap<Vec<Span>> = IriMap::default();
        let mut refs_by_iri: IriMap<Vec<u32>> = IriMap::default();
        for (i, r) in parsed.refs.iter().enumerate() {
            if r.role.is_definition() {
                defs.entry(r.iri).or_default().push(r.span);
            }
            refs_by_iri.entry(r.iri).or_default().push(i as u32);
        }
        let mut labels = IriMap::default();
        let mut comments = IriMap::default();
        let mut types: IriMap<Vec<Iri>> = IriMap::default();
        let mut supers: IriMap<Vec<Iri>> = IriMap::default();
        let predicates = indexed_predicates();
        for (i, t) in parsed.triples.iter().enumerate() {
            let Node::Iri(s) = t.s else { continue };
            let Some(&(_, index)) = predicates.iter().find(|(p, _)| *p == t.p) else {
                continue;
            };
            match (index, &t.o) {
                (Index::Type, Node::Iri(o)) => types.entry(s).or_default().push(*o),
                (Index::Super, Node::Iri(o)) => supers.entry(s).or_default().push(*o),
                (Index::Label, Node::Literal { .. }) => {
                    prefer(&mut labels, &parsed.triples, s, i as u32)
                }
                (Index::Comment, Node::Literal { .. }) => {
                    prefer(&mut comments, &parsed.triples, s, i as u32)
                }
                _ => {}
            }
        }
        Analysis {
            uri,
            origin: None,
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

    fn literal(&self, i: u32) -> &str {
        match &self.parsed.triples[i as usize].o {
            Node::Literal { value, .. } => value,
            _ => "",
        }
    }

    /// The resource's label (rdfs:label, skos:prefLabel, sh:name), English preferred.
    pub fn label<K: IriKey + ?Sized>(&self, iri: &K) -> Option<&str> {
        self.labels.get(iri).map(|&i| self.literal(i))
    }

    /// The resource's comment (rdfs:comment, skos:definition, dcterms:description).
    pub fn comment<K: IriKey + ?Sized>(&self, iri: &K) -> Option<&str> {
        self.comments.get(iri).map(|&i| self.literal(i))
    }

    /// [`Analysis::label`] and whether it is English or untagged.
    pub fn label_en<K: IriKey + ?Sized>(&self, iri: &K) -> Option<(&str, bool)> {
        self.labels
            .get(iri)
            .map(|&i| (self.literal(i), self.is_en(i)))
    }

    /// [`Analysis::comment`] and whether it is English or untagged.
    pub fn comment_en<K: IriKey + ?Sized>(&self, iri: &K) -> Option<(&str, bool)> {
        self.comments
            .get(iri)
            .map(|&i| (self.literal(i), self.is_en(i)))
    }

    fn is_en(&self, i: u32) -> bool {
        match &self.parsed.triples[i as usize].o {
            Node::Literal { lang, .. } => lang.is_none_or(|l| l.starts_with("en")),
            _ => false,
        }
    }

    /// All prefixes in scope at the end of the document: external, then declared.
    pub fn prefix_map(&self) -> PrefixMap {
        let mut m = (*self.external).clone();
        for p in &self.parsed.prefixes {
            m.insert(p.name.clone(), p.iri.to_string());
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

    pub fn refs_of<'a, K: IriKey + ?Sized>(
        &'a self,
        iri: &K,
    ) -> impl Iterator<Item = &'a Ref> + 'a {
        self.refs_by_iri
            .get(iri)
            .into_iter()
            .flatten()
            .map(|i| &self.parsed.refs[*i as usize])
    }

    pub fn position(&self, enc: Encoding, offset: u32) -> Position {
        position_in(&self.lines, enc, offset)
    }

    pub fn range(&self, enc: Encoding, span: Span) -> Range {
        Range::new(self.position(enc, span.start), self.position(enc, span.end))
    }

    pub fn offset(&self, enc: Encoding, pos: Position) -> Option<u32> {
        offset_in(&self.lines, &self.text, enc, pos)
    }
}

/// The LSP position of byte `offset` (clamped to the end of the text).
pub fn position_in(lines: &LineIndex, enc: Encoding, offset: u32) -> Position {
    let lc = lines.line_col(TextSize::from(offset.min(u32::from(lines.len()))));
    match enc {
        Encoding::Utf8 => Position::new(lc.line, lc.col),
        Encoding::Utf16 => {
            let w = lines
                .to_wide(WideEncoding::Utf16, lc)
                .unwrap_or(line_index::WideLineCol {
                    line: lc.line,
                    col: lc.col,
                });
            Position::new(w.line, w.col)
        }
    }
}

/// A SPARQL document (query or update), parsed once per version, with the
/// line positions the features map spans with. It never feeds the knowledge
/// index.
pub struct SparqlAnalysis {
    pub uri: String,
    /// Parsed with [`SparqlAnalysis::external`] and the document's URI as
    /// base (a `BASE` in the query wins).
    pub doc: sparql::Document,
    pub lines: LineIndex,
    /// Prefixes supplied from outside the document (prefixes.ttl / settings).
    pub external: Arc<PrefixMap>,
}

impl SparqlAnalysis {
    pub fn new(uri: String, text: &str, external: Arc<PrefixMap>) -> SparqlAnalysis {
        let doc = sparql::Document::parse(text, &external, Some(&uri));
        SparqlAnalysis {
            uri,
            doc,
            lines: LineIndex::new(text),
            external,
        }
    }

    /// The text it was parsed from.
    pub fn text(&self) -> &str {
        self.doc.text()
    }

    /// The base IRI the document is resolved against: its URI (a `BASE` in
    /// the query overrides it for the query's own IRIs).
    pub fn base(&self) -> &str {
        &self.uri
    }

    /// All prefixes in scope at the end of the document: external, then the
    /// `PREFIX` lines (a later line for the same name wins).
    pub fn prefix_map(&self) -> PrefixMap {
        let mut m = (*self.external).clone();
        for p in &self.doc.structure().prefixes {
            m.insert(
                p.name.clone(),
                p.resolved.clone().unwrap_or_else(|| p.iri.clone()),
            );
        }
        m
    }

    pub fn position(&self, enc: Encoding, offset: u32) -> Position {
        position_in(&self.lines, enc, offset)
    }

    pub fn range(&self, enc: Encoding, span: Span) -> Range {
        Range::new(self.position(enc, span.start), self.position(enc, span.end))
    }

    pub fn offset(&self, enc: Encoding, pos: Position) -> Option<u32> {
        offset_in(&self.lines, self.text(), enc, pos)
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
