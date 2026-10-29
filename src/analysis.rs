//! Everything the editor features need from one document, computed once per
//! version: the parse, line positions, and per-IRI indexes. An RDF document
//! gets an [`Analysis`]; a SPARQL document a [`SparqlAnalysis`], with the
//! same position helpers.

use crate::sparql;
use crate::syntax::lexer::{Kind, Token};
pub use crate::syntax::splice::Fallback;
use crate::syntax::splice::{self, Change, Edit, Window};
use crate::syntax::{self, Iri, IriKey, IriMap, Node, Parsed, PrefixMap, Ref, Span, Syntax};
use lsp_types::{Position, Range};
use rustc_hash::FxHashMap;
use std::sync::{Arc, Mutex, OnceLock};

const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const SKOS_PREF_LABEL: &str = "http://www.w3.org/2004/02/skos/core#prefLabel";
const RDFS_COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
const SKOS_DEFINITION: &str = "http://www.w3.org/2004/02/skos/core#definition";
const SH_NAME: &str = "http://www.w3.org/ns/shacl#name";
const DCT_DESCRIPTION: &str = "http://purl.org/dc/terms/description";
pub(crate) const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
pub(crate) const RDFS_SUBPROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";

/// How an analysis's text was parsed: by the statement splice, from the
/// analysis before an edit, or in full, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParsePath {
    Spliced,
    /// In full, with no edit to build on: the first analysis, a change of
    /// the whole text, an analysis before it that failed, or the same text
    /// with other prefixes or contexts.
    Whole,
    /// In full: the setting `parse.incremental` is off.
    Off,
    /// In full: the splice fell back.
    Fallback(Fallback),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

pub struct Analysis {
    pub(crate) uri: String,
    /// The URL an import's text came from, when `uri` names its cached copy.
    pub(crate) origin: Option<String>,
    /// The text this analysis was computed from.
    pub(crate) text: Arc<str>,
    /// The parse, without its tokens and comment spans: those are
    /// [`Analysis::tokens`] and [`Analysis::comments`].
    pub parsed: Parsed,
    /// The parse's tokens, when it kept them; else filled on first use.
    tokens: OnceLock<Vec<Token>>,
    /// JSON-LD only: what processing dropped silently, and the references
    /// that are context terms, for the JSON-LD lints (empty otherwise).
    pub dropped: Vec<crate::jsonld::Dropped>,
    pub(crate) used_terms: Vec<crate::jsonld::UsedTerm>,
    /// JSON-LD only: the keys, types and `@id`s as written, the term
    /// definitions they use, and the context inside each object (empty
    /// otherwise).
    pub(crate) source_index: crate::jsonld::SourceIndex,
    pub(crate) lines: LineIndex,
    /// Prefixes supplied from outside the document (prefixes.ttl / settings).
    pub external: Arc<PrefixMap>,
    /// IRI -> spans where it is the subject of a top-level statement or the
    /// name of a TriG graph block.
    pub(crate) defs: IriMap<Vec<Span>>,
    /// IRI -> indexes into `parsed.refs`.
    pub(crate) refs_by_iri: IriMap<Vec<u32>>,
    /// IRI -> index into `parsed.triples` of its label; see [`Analysis::label`].
    labels: IriMap<u32>,
    /// IRI -> index into `parsed.triples` of its comment; see [`Analysis::comment_en`].
    descriptions: IriMap<u32>,
    pub(crate) types: IriMap<Vec<Iri>>,
    /// IRI -> direct superclasses / superproperties (rdfs:subClassOf, rdfs:subPropertyOf).
    pub(crate) supers: IriMap<Vec<Iri>>,
    /// The SHACL query literals parsed for the editor features, by the index
    /// of the triple whose object is the literal; filled by
    /// `features::embedded` as features ask, so a keystroke in a file with
    /// hundreds of queries does not parse them all again.
    pub(crate) embedded_queries: Mutex<FxHashMap<u32, EmbeddedQuery>>,
}

/// A query literal parsed as SPARQL, with the prefixes it was parsed with:
/// what the workspace declares for the literal's node can change without the
/// document changing, and then the query is parsed again.
pub(crate) struct EmbeddedQuery {
    pub(crate) prefixes: Vec<(String, String)>,
    pub(crate) query: Arc<SparqlAnalysis>,
}

/// Which predicate feeds which index: `types`, `supers`, the label index
/// ([`Analysis::label`]) or the comment index ([`Analysis::comment_en`]).
#[derive(Clone, Copy)]
pub(crate) enum Index {
    Type,
    Super,
    Label,
    Comment,
}

/// The index triples with predicate `p` feed: those about an IRI subject,
/// with an IRI object for [`Index::Type`] and [`Index::Super`], a literal
/// for [`Index::Label`] and [`Index::Comment`].
pub(crate) fn feeds(p: Iri) -> Option<Index> {
    indexed_predicates()
        .iter()
        .find(|(q, _)| *q == p)
        .map(|&(_, index)| index)
}

/// Every predicate of [`feeds`].
pub(crate) fn indexed_predicates() -> &'static [(Iri, Index)] {
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

/// Whether text in language `lang` is English for display: untagged, `en` or
/// `en-*` (not `enm`, Middle English).
pub(crate) fn is_english(lang: Option<Iri>) -> bool {
    lang.is_none_or(|l| &*l == "en" || l.starts_with("en-"))
}

/// The first English (or untagged) text wins; otherwise the first one seen.
fn prefer(map: &mut IriMap<u32>, triples: &[syntax::Triple], s: Iri, i: u32) {
    let Node::Literal { lang, .. } = &triples[i as usize].o else {
        return;
    };
    let better = match map.get(&s) {
        None => true,
        Some(&old) => {
            is_english(*lang)
                && !matches!(&triples[old as usize].o,
                    Node::Literal { lang, .. } if is_english(*lang))
        }
    };
    if better {
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
        let lines = LineIndex::new(text);
        Analysis::with_lines(uri, text, external, keep_tokens, language, loader, lines)
    }

    /// [`Analysis::with_loader`], with the line index of `text` made by the
    /// caller (from an earlier analysis, see [`LineIndex::edited`]).
    pub(crate) fn with_lines(
        uri: String,
        text: &str,
        external: Arc<PrefixMap>,
        keep_tokens: bool,
        language: Syntax,
        loader: &dyn crate::jsonld::ContextLoader,
        lines: LineIndex,
    ) -> Analysis {
        let document = match language {
            // Relative IRIs resolve against the document's URL.
            Syntax::JsonLd => crate::jsonld::parse_document_with(
                text,
                Some(&uri),
                loader,
                crate::jsonld::context::ContextCache::global(),
            ),
            _ => crate::jsonld::Document {
                // Relative IRIs resolve against the document's URL, as in JSON-LD:
                // `<#me>` in two files is two IRIs.
                parsed: syntax::parse_with_base(text, &external, language, Some(&uri)),
                ..Default::default()
            },
        };
        Analysis::with_document(uri, text, external, keep_tokens, document, lines)
    }

    /// The parse of `text`, which `edit` made from this analysis's text, by
    /// the statement splice, and the window it parsed again; or why the
    /// splice does not apply. `uri` and `external` are the document's URL
    /// and prefixes now.
    pub(crate) fn splice(
        &self,
        uri: &str,
        text: &str,
        edit: Edit,
        external: &PrefixMap,
    ) -> Result<(Parsed, Window), Fallback> {
        let change = Change {
            old_text: &self.text,
            new_text: text,
            edit,
        };
        let base = Some(uri);
        splice::applies(&self.parsed, change, &self.external, external, base)?;
        let tokens = self.tokens.get().ok_or(Fallback::NoTokens)?;
        // The splice works on the parse as the parser made it: with its
        // tokens and comment spans, which this analysis keeps apart or
        // drops. It is shared (the knowledge index holds it too), so the
        // splice works on a copy.
        let mut old = self.parsed.clone();
        old.tokens = tokens.clone();
        old.comments = comments_of(tokens).collect();
        splice::splice(old, change, &self.external, external, base)
    }

    /// The analysis of `text` from its parse `parsed` ([`Analysis::splice`]).
    pub(crate) fn with_parse(
        uri: String,
        text: &str,
        external: Arc<PrefixMap>,
        keep_tokens: bool,
        parsed: Parsed,
        lines: LineIndex,
    ) -> Analysis {
        let document = crate::jsonld::Document {
            parsed,
            ..Default::default()
        };
        Analysis::with_document(uri, text, external, keep_tokens, document, lines)
    }

    /// The analysis of `text` from its parse, and JSON-LD's extras.
    fn with_document(
        uri: String,
        text: &str,
        external: Arc<PrefixMap>,
        keep_tokens: bool,
        document: crate::jsonld::Document,
        lines: LineIndex,
    ) -> Analysis {
        let crate::jsonld::Document {
            mut parsed,
            dropped,
            used_terms,
            index: source_index,
        } = document;
        // The comment spans are the comment tokens.
        parsed.comments = Vec::new();
        let tokens = OnceLock::new();
        let mut kept = std::mem::take(&mut parsed.tokens);
        if keep_tokens {
            kept.shrink_to_fit();
            let _ = tokens.set(kept);
        } else {
            drop(kept);
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
        let mut descriptions = IriMap::default();
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
                    prefer(&mut descriptions, &parsed.triples, s, i as u32)
                }
                _ => {}
            }
        }
        Analysis {
            uri,
            origin: None,
            text: Arc::from(text),
            lines,
            parsed,
            tokens,
            dropped,
            used_terms,
            source_index,
            external,
            defs,
            refs_by_iri,
            labels,
            descriptions,
            types,
            supers,
            embedded_queries: Mutex::default(),
        }
    }

    /// Every token of the text, comments included, as the parser read them
    /// (none for JSON-LD). An analysis built without its tokens lexes the
    /// text again on the first call, once.
    pub fn tokens(&self) -> &[Token] {
        self.tokens
            .get_or_init(|| {
                let mut all = self.parsed.syntax.tokens(&self.text);
                all.shrink_to_fit();
                all
            })
            .as_slice()
    }

    /// [`Analysis::tokens`] when they are in memory, else lexed into a
    /// temporary: an edit to a document built without them leaves it so.
    pub(crate) fn tokens_or_lexed(&self) -> std::borrow::Cow<'_, [Token]> {
        match self.tokens.get() {
            Some(all) => all.as_slice().into(),
            None => self.parsed.syntax.tokens(&self.text).into(),
        }
    }

    /// Whether the tokens are in memory: kept by the parse, or made since.
    #[cfg(test)]
    pub(crate) fn has_tokens(&self) -> bool {
        self.tokens.get().is_some()
    }

    /// The comments, in text order (see [`Analysis::tokens`]).
    pub fn comments(&self) -> impl Iterator<Item = Span> + '_ {
        comments_of(self.tokens())
    }

    fn literal(&self, i: u32) -> &str {
        match &self.parsed.triples[i as usize].o {
            Node::Literal { value, .. } => value,
            _ => "",
        }
    }

    /// The resource's label (rdfs:label, skos:prefLabel, sh:name), English preferred.
    pub(crate) fn label<K: IriKey + ?Sized>(&self, iri: &K) -> Option<&str> {
        self.labels.get(iri).map(|&i| self.literal(i))
    }

    /// [`Analysis::label`] and whether it is English or untagged.
    pub(crate) fn label_en<K: IriKey + ?Sized>(&self, iri: &K) -> Option<(&str, bool)> {
        self.labels
            .get(iri)
            .map(|&i| (self.literal(i), self.is_en(i)))
    }

    /// The resource's comment (rdfs:comment, skos:definition,
    /// dcterms:description) and whether it is English or untagged.
    pub(crate) fn comment_en<K: IriKey + ?Sized>(&self, iri: &K) -> Option<(&str, bool)> {
        self.descriptions
            .get(iri)
            .map(|&i| (self.literal(i), self.is_en(i)))
    }

    fn is_en(&self, i: u32) -> bool {
        match &self.parsed.triples[i as usize].o {
            Node::Literal { lang, .. } => is_english(*lang),
            _ => false,
        }
    }

    /// All prefixes in scope at the end of the document: external, then declared.
    pub(crate) fn prefix_map(&self) -> PrefixMap {
        let mut m = (*self.external).clone();
        for p in &self.parsed.prefixes {
            m.insert(p.name.clone(), p.iri.to_string());
        }
        m
    }

    pub(crate) fn ref_at(&self, offset: u32) -> Option<&Ref> {
        let refs = &self.parsed.refs;
        // Refs are recorded in text order.
        let i = refs.partition_point(|r| r.span.end < offset);
        refs[i..]
            .iter()
            .take_while(|r| r.span.start <= offset)
            .find(|r| r.span.contains(offset))
    }

    pub(crate) fn refs_of<'a, K: IriKey + ?Sized>(
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
        position_in(&self.lines, &self.text, enc, offset)
    }

    pub(crate) fn range(&self, enc: Encoding, span: Span) -> Range {
        Range::new(self.position(enc, span.start), self.position(enc, span.end))
    }

    pub fn offset(&self, enc: Encoding, pos: Position) -> Option<u32> {
        offset_in(&self.lines, &self.text, enc, pos)
    }
}

/// The spans of the comment tokens among `tokens`.
fn comments_of(tokens: &[Token]) -> impl Iterator<Item = Span> + '_ {
    tokens
        .iter()
        .filter(|t| t.kind == Kind::Comment)
        .map(|t| Span::new(t.start, t.end))
}

/// Where the lines of a text start, which of them hold a multi-byte
/// character, and marks inside the long ones: a column converts with a
/// lookup and a scan of at most [`LONG`] bytes, or [`STRIDE`] bytes on a long
/// line, however many multi-byte characters its line has. Every count is
/// from the start of its line, so an edit changes the entries of the lines
/// it touched and moves the later ones by its change in length
/// ([`LineIndex::edited`]). Lines end as in [`LineStarts`]. The conversions
/// take the text it indexes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LineIndex {
    starts: LineStarts,
    /// Per line: whether it holds a multi-byte character. Columns on the
    /// other lines are byte counts.
    wide: Vec<bool>,
    /// In order: on each wide line longer than [`LONG`] bytes, mark `k >= 1`
    /// at the first character boundary at or after `k * STRIDE` bytes into
    /// the line, while that is before the line's end.
    marks: Vec<Mark>,
}

/// Lines up to this many bytes, line break included, have no marks.
const LONG: usize = 4096;

/// Bytes between two [`LineIndex`] marks.
const STRIDE: usize = 64;

/// UTF-16 code units and code points from the start of its line to byte `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mark {
    at: u32,
    utf16: u32,
    chars: u32,
}

/// UTF-16 code units and code points of the UTF-8 bytes `b`: one of each per
/// leading byte, and a second code unit for a 4-byte character.
fn tally(b: &[u8]) -> (u32, u32) {
    let chars = b.iter().filter(|&&x| (x as i8) >= -0x40).count() as u32;
    let wide = b.iter().filter(|&&x| x >= 0xF0).count() as u32;
    (chars + wide, chars)
}

#[cfg(test)]
thread_local! {
    /// Bytes the conversions on this thread scanned past a line start or
    /// a mark.
    static SCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn scanned(_n: usize) {
    #[cfg(test)]
    SCANNED.with(|s| s.set(s.get() + _n));
}

impl LineIndex {
    pub(crate) fn new(text: &str) -> LineIndex {
        let starts = LineStarts::new(text);
        let mut lines = LineIndex {
            wide: Vec::with_capacity(starts.0.len()),
            marks: Vec::new(),
            starts,
        };
        if text.is_ascii() {
            lines.wide.resize(lines.starts.0.len(), false);
        } else {
            lines.index(text, 0..lines.starts.0.len());
        }
        lines
    }

    /// The index of `text`, which `edit` made from the text this index is
    /// of; `starts` are the line starts of `text` ([`LineStarts::replace`]
    /// moved them through the edit). Only the lines the edit touched are
    /// read: the entries of the others are copied, those after it moved by
    /// its change in length.
    pub(crate) fn edited(&self, text: &str, edit: &Edit, starts: &LineStarts) -> LineIndex {
        let new = &starts.0;
        let line_at = |x: usize| new.partition_point(|&s| s as usize <= x) - 1;
        // The line before the edit can lose or gain its line break (a `\r`
        // then a `\n`), so it is read again too.
        let first = line_at((edit.start as usize).saturating_sub(1));
        let last = line_at(edit.new_end() as usize);
        // The lines after `last` start past the edit: they are the old
        // text's last lines, moved.
        let after = new.len() - (last + 1);
        let old_after = self.starts.0.len() - after;
        let old_tail = self.starts.0.get(old_after).map_or(u32::MAX, |&s| s);
        let grown = i64::from(edit.new_len) - i64::from(edit.end - edit.start);
        let head = self.marks.partition_point(|m| m.at < new[first]);
        let tail = self.marks.partition_point(|m| m.at < old_tail);
        let mut lines = LineIndex {
            starts: starts.clone(),
            wide: Vec::with_capacity(new.len()),
            marks: Vec::with_capacity(self.marks.len()),
        };
        lines.wide.extend_from_slice(&self.wide[..first]);
        lines.marks.extend_from_slice(&self.marks[..head]);
        lines.index(text, first..last + 1);
        lines.wide.extend_from_slice(&self.wide[old_after..]);
        lines.marks.extend(self.marks[tail..].iter().map(|m| Mark {
            at: (i64::from(m.at) + grown) as u32,
            ..*m
        }));
        lines
    }

    /// Add the entries of lines `lines` of `text`, after those of the lines
    /// before them.
    fn index(&mut self, text: &str, lines: std::ops::Range<usize>) {
        let b = text.as_bytes();
        for line in lines {
            let (start, end) = self.span(text, line);
            let wide = !b[start..end].is_ascii();
            self.wide.push(wide);
            if !wide || end - start <= LONG {
                continue;
            }
            let (mut at, mut utf16, mut chars) = (start, 0, 0);
            for k in 1..=(end - start - 1) / STRIDE {
                let next = text.ceil_char_boundary(start + k * STRIDE);
                if next >= end {
                    break;
                }
                let (u, c) = tally(&b[at..next]);
                (at, utf16, chars) = (next, utf16 + u, chars + c);
                let at = at as u32;
                self.marks.push(Mark { at, utf16, chars });
            }
        }
    }

    /// The bytes line `line` of `text` spans, its line break included.
    fn span(&self, text: &str, line: usize) -> (usize, usize) {
        let start = self.starts.0[line] as usize;
        let end = self.starts.0.get(line + 1);
        (start, end.map_or(text.len(), |&e| e as usize))
    }

    /// The line starts of the text.
    pub(crate) fn starts(&self) -> &LineStarts {
        &self.starts
    }

    /// The UTF-16 code units and code points of line `line` of `text`
    /// before byte `x` of it, a character boundary.
    fn counts(&self, text: &str, line: usize, x: usize) -> (u32, u32) {
        let start = self.starts.0[line] as usize;
        if !self.wide[line] {
            return ((x - start) as u32, (x - start) as u32);
        }
        let mark = self.marks.partition_point(|m| m.at as usize <= x);
        let (from, utf16, chars) = match mark.checked_sub(1).map(|k| self.marks[k]) {
            Some(m) if m.at as usize >= start => (m.at as usize, m.utf16, m.chars),
            _ => (start, 0, 0),
        };
        scanned(x - from);
        let (u, c) = tally(&text.as_bytes()[from..x]);
        (utf16 + u, chars + c)
    }

    /// The line of byte `offset` (clamped to the end of `text` and moved back
    /// to a character's start), the byte its line starts at, and the offset.
    fn line_of(&self, text: &str, offset: u32) -> (u32, usize, usize) {
        let at = text.floor_char_boundary(offset as usize);
        let line = self.starts.0.partition_point(|&s| s as usize <= at) - 1;
        (line as u32, self.starts.0[line] as usize, at)
    }

    /// The bytes line `line` of `text` spans, its line break included.
    fn line(&self, text: &str, line: u32) -> Option<(usize, usize)> {
        let line = line as usize;
        (line < self.starts.0.len()).then(|| self.span(text, line))
    }

    /// The position of byte `offset` of `text` with its column in code points.
    pub(crate) fn code_point_position(&self, text: &str, offset: u32) -> Position {
        let (line, _, at) = self.line_of(text, offset);
        Position::new(line, self.counts(text, line as usize, at).1)
    }

    /// The UTF-8 column of UTF-16 column `w` on line `line`, the bytes
    /// `start..end` of `text`, as `line_index::LineIndex::to_utf8` gives it:
    /// `w` plus the bytes beyond their UTF-16 length of the characters that
    /// start before `w`. Past `u32::MAX` it is `None`.
    fn utf8_col(&self, text: &str, line: u32, (start, end): (usize, usize), w: u32) -> Option<u32> {
        if !self.wide[line as usize] {
            return Some(w);
        }
        let first = self.marks.partition_point(|m| (m.at as usize) < start);
        let marks = &self.marks[first..self.marks.partition_point(|m| (m.at as usize) < end)];
        // Start from the line's last mark at or before `w` code units: the
        // next one is past it, so the scan stops within a stride.
        let (mut x, mut units) = match marks.partition_point(|m| m.utf16 <= w).checked_sub(1) {
            Some(k) => (marks[k].at as usize, u64::from(marks[k].utf16)),
            None => (start, 0),
        };
        let (b, from, want) = (text.as_bytes(), x, u64::from(w));
        while x < end && units < want {
            let len = match b[x] {
                0..0x80 => 1,
                0x80..0xE0 => 2,
                0xE0..0xF0 => 3,
                _ => 4,
            };
            units += 1 + u64::from(len == 4);
            x += len;
        }
        scanned(x - from);
        u32::try_from(want + (x - start) as u64 - units).ok()
    }
}

/// Whether a line starts at byte `x` of `text`: after a `\n`, or after a
/// `\r` that no `\n` follows.
fn starts_line(b: &[u8], x: usize) -> bool {
    match b.get(x.wrapping_sub(1)) {
        Some(b'\n') => true,
        Some(b'\r') => b.get(x) != Some(&b'\n'),
        _ => false,
    }
}

/// The LSP position of byte `offset` (clamped to the end of the text).
///
/// An offset inside a multi-byte character (a stale or foreign span) moves back
/// to the character's start.
pub(crate) fn position_in(lines: &LineIndex, text: &str, enc: Encoding, offset: u32) -> Position {
    let (line, start, at) = lines.line_of(text, offset);
    let col = match enc {
        Encoding::Utf8 => (at - start) as u32,
        Encoding::Utf16 => lines.counts(text, line as usize, at).0,
    };
    Position::new(line, col)
}

/// A SPARQL document (query or update), parsed once per version, with the
/// line positions the features map spans with. It never feeds the knowledge
/// index.
pub struct SparqlAnalysis {
    pub uri: String,
    /// Parsed with [`SparqlAnalysis::external`] and the document's URI as
    /// base (a `BASE` in the query wins).
    pub(crate) doc: sparql::Document,
    pub(crate) lines: LineIndex,
    /// Prefixes supplied from outside the document (prefixes.ttl / settings).
    pub(crate) external: Arc<PrefixMap>,
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

    /// A query embedded in an RDF document (a SHACL query literal's value),
    /// for the display and completion features: no checks are run.
    pub fn embedded(uri: String, text: &str, external: Arc<PrefixMap>) -> SparqlAnalysis {
        let doc = sparql::Document::parse_unchecked(text, &external, Some(&uri));
        SparqlAnalysis {
            uri,
            doc,
            lines: LineIndex::new(text),
            external,
        }
    }

    /// The text it was parsed from.
    pub(crate) fn text(&self) -> &str {
        self.doc.text()
    }

    /// The base IRI the document is resolved against: its URI (a `BASE` in
    /// the query overrides it for the query's own IRIs).
    pub(crate) fn base(&self) -> &str {
        &self.uri
    }

    /// All prefixes in scope at the end of the document: external, then the
    /// `PREFIX` lines (a later line for the same name wins).
    pub(crate) fn prefix_map(&self) -> PrefixMap {
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
        position_in(&self.lines, self.text(), enc, offset)
    }

    pub(crate) fn range(&self, enc: Encoding, span: Span) -> Range {
        Range::new(self.position(enc, span.start), self.position(enc, span.end))
    }

    pub fn offset(&self, enc: Encoding, pos: Position) -> Option<u32> {
        offset_in(&self.lines, self.text(), enc, pos)
    }
}

/// Byte offset of an LSP position. Clients send positions past the end of a
/// line or of the file, and (in UTF-8 mode) columns inside a multi-byte
/// character; all of these are clamped to a valid character boundary.
pub(crate) fn offset_in(
    lines: &LineIndex,
    text: &str,
    enc: Encoding,
    pos: Position,
) -> Option<u32> {
    let Some((start, end)) = lines.line(text, pos.line) else {
        return Some(text.len() as u32);
    };
    let col = match enc {
        Encoding::Utf8 => pos.character,
        Encoding::Utf16 => lines.utf8_col(text, pos.line, (start, end), pos.character)?,
    };
    Some(clamped(text, start, end, col))
}

/// The offset `col` bytes into the line `start..end` of `text`, at most the
/// end of the line's content (before its line break), moved back to a
/// character boundary.
fn clamped(text: &str, start: usize, end: usize, col: u32) -> u32 {
    let content_end = text[start..end].trim_end_matches(['\n', '\r']).len() + start;
    let mut off = (start + col as usize).min(content_end);
    while !text.is_char_boundary(off) {
        off -= 1;
    }
    off as u32
}

/// Where the lines of a text being edited start, kept current through each
/// edit: a batch of edits converts its positions without indexing the text
/// again per edit. Lines end at `\n`, `\r\n` and a lone `\r`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LineStarts(Vec<u32>);

impl LineStarts {
    pub(crate) fn new(text: &str) -> LineStarts {
        let b = text.as_bytes();
        let breaks = text.match_indices(['\n', '\r']).map(|(i, _)| i + 1);
        let breaks = breaks.filter(|&x| starts_line(b, x)).map(|x| x as u32);
        LineStarts(std::iter::once(0).chain(breaks).collect())
    }

    /// [`offset_in`], for the text these are the line starts of.
    pub(crate) fn offset(&self, text: &str, enc: Encoding, pos: Position) -> Option<u32> {
        let line = pos.line as usize;
        let Some(&start) = self.0.get(line) else {
            return Some(text.len() as u32);
        };
        let start = start as usize;
        let end = self.0.get(line + 1).map_or(text.len(), |&e| e as usize);
        let col = match enc {
            Encoding::Utf8 => pos.character,
            // As [`LineIndex`] converts it: each multi-byte character the column
            // passes adds the bytes it has beyond its UTF-16 length.
            Encoding::Utf16 => {
                let mut col = pos.character;
                for (at, c) in text[start..end].char_indices() {
                    let bytes = c.len_utf8() as u32;
                    if bytes == 1 {
                        continue;
                    }
                    if col <= at as u32 {
                        break;
                    }
                    col = col.checked_add(bytes - c.len_utf16() as u32)?;
                }
                col
            }
        };
        Some(clamped(text, start, end, col))
    }

    /// The bytes `range` of the text were replaced by `new`, leaving `text`.
    pub(crate) fn replace(&mut self, text: &str, range: std::ops::Range<usize>, new: &str) {
        let (s, e) = (range.start as u32, range.end as u32);
        // A line start depends on the bytes on either side of it, so those
        // from `s` to the range's end are found again; later ones move by
        // the change in length.
        let gone = self.0.partition_point(|&x| x < s.max(1))..self.0.partition_point(|&x| x <= e);
        let grown = new.len() as i64 - i64::from(e - s);
        for x in &mut self.0[gone.end..] {
            *x = (i64::from(*x) + grown) as u32;
        }
        let b = text.as_bytes();
        let added = (range.start.max(1)..=range.start + new.len())
            .filter(|&x| starts_line(b, x))
            .map(|x| x as u32);
        self.0.splice(gone, added);
    }
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

    #[test]
    fn a_position_inside_a_character_snaps_to_its_start() {
        let text = "é😀x\n";
        let lines = LineIndex::new(text);
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            for offset in 0..=text.len() as u32 + 3 {
                let at = offset.min(text.len() as u32);
                let mut start = at as usize;
                while !text.is_char_boundary(start) {
                    start -= 1;
                }
                // No panic, and the same position as the character's start.
                assert_eq!(
                    position_in(&lines, text, enc, offset),
                    position_in(&lines, text, enc, start as u32),
                    "{enc:?} {offset}"
                );
            }
        }
    }

    fn doc(uri: &str, text: &str) -> Analysis {
        Analysis::new(uri.into(), text, Arc::new(PrefixMap::default()), false)
    }

    fn iris(a: &Analysis) -> Vec<String> {
        a.parsed.refs.iter().map(|r| r.iri.to_string()).collect()
    }

    #[test]
    fn relative_iris_resolve_against_the_document_uri() {
        let text = "<#me> <p> <rel/x> .\n";
        let a = doc("file:///w/a.ttl", text);
        let b = doc("file:///w/b.ttl", text);
        assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
        assert_eq!(
            iris(&a),
            ["file:///w/a.ttl#me", "file:///w/p", "file:///w/rel/x"]
        );
        assert_eq!(iris(&b)[0], "file:///w/b.ttl#me");
        // Definitions, references and triples all carry the absolute IRI, so
        // the two files' `<#me>` are two resources.
        assert!(a.defs.contains_key("file:///w/a.ttl#me"));
        assert!(!a.defs.contains_key("file:///w/b.ttl#me"));
        assert_eq!(a.parsed.document_base.as_deref(), Some("file:///w/a.ttl"));
    }

    #[test]
    fn a_relative_base_directive_resolves_against_the_document_uri() {
        let a = doc(
            "file:///w/a.ttl",
            "@base <rel/> .\n<x> <p> <o> .\nBASE <../up/>\n<y> <p> <o> .\n",
        );
        assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
        assert_eq!(a.parsed.base.as_deref(), Some("file:///w/up/"));
        assert_eq!(iris(&a)[0], "file:///w/rel/x");
        assert_eq!(iris(&a)[3], "file:///w/up/y");
        // An absolute base needs no document base.
        let b = doc(
            "untitled:Untitled-1",
            "@base <http://e/d/> .\n<x> <p> <o> .\n",
        );
        assert!(b.parsed.errors.is_empty(), "{:?}", b.parsed.errors);
        assert_eq!(iris(&b)[0], "http://e/d/x");
        // An unsaved buffer's URI is a base too.
        let c = doc("untitled:Untitled-1", "<#me> <p> <o> .\n");
        assert!(c.parsed.errors.is_empty(), "{:?}", c.parsed.errors);
        assert_eq!(iris(&c)[0], "untitled:Untitled-1#me");
    }

    #[test]
    fn every_line_based_and_trig_syntax_reads_the_uri_as_base() {
        let trig = doc("file:///w/a.trig", "<g> { <#me> <p> <o> }\n");
        assert_eq!(
            iris(&trig),
            [
                "file:///w/g",
                "file:///w/a.trig#me",
                "file:///w/p",
                "file:///w/o"
            ]
        );
        let nq = doc(
            "file:///w/a.nq",
            "<http://e/s> <http://e/p> <http://e/o> <http://e/g> .\n",
        );
        assert!(nq.parsed.errors.is_empty(), "{:?}", nq.parsed.errors);
        assert_eq!(nq.parsed.document_base.as_deref(), Some("file:///w/a.nq"));
        // N-Quads still wants absolute IRIs: a relative one is reported.
        let rel = doc("file:///w/a.nq", "<s> <http://e/p> <http://e/o> .\n");
        assert!(
            rel.parsed
                .errors
                .iter()
                .any(|e| e.message.contains("absolute"))
        );
    }

    #[test]
    fn the_first_english_label_wins() {
        let a = doc(
            "file:///w/a.ttl",
            "@prefix ex: <http://e/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             ex:a rdfs:label \"alt\"@enm , \"first\"@en-GB , \"second\"@en , \"plain\" .\n\
             ex:b rdfs:label \"nur\"@de , \"mittel\"@enm .\n\
             ex:c rdfs:label \"un\"@fr , \"one\"@en .\n",
        );
        assert_eq!(a.label("http://e/a"), Some("first"));
        // No English text: the first one seen, and `enm` is not English.
        assert_eq!(a.label("http://e/b"), Some("nur"));
        assert_eq!(a.label_en("http://e/b"), Some(("nur", false)));
        assert_eq!(a.label("http://e/c"), Some("one"));
        assert!(!is_english(Some(Iri::intern("enm"))));
        assert!(is_english(Some(Iri::intern("en-us"))));
        assert!(is_english(None));
    }

    #[test]
    fn tokens_made_on_demand_are_the_tokens_the_parse_kept() {
        let texts = [
            (
                "file:///w/a.ttl",
                "@prefix ex: <http://e/> . # one\nex:a ex:p \"x\" , ( 1 2 ) .\nGRAPH oops\n",
            ),
            ("file:///w/a.trig", "GRAPH <g> { <a> <p> <o> } # two\n"),
            (
                "file:///w/a.nt",
                "<http://e/a> <http://e/p> <http://e/o> . # three\n",
            ),
            (
                "file:///w/a.nq",
                "<http://e/a> <http://e/p> <http://e/o> <http://e/g> .\n",
            ),
            ("file:///w/a.jsonld", "{\"@id\": \"http://e/a\"}"),
            ("file:///w/a.ttl", ""),
        ];
        let key = |a: &Analysis| -> Vec<_> {
            a.tokens()
                .iter()
                .map(|t| (t.kind, t.start, t.end, t.error()))
                .collect()
        };
        for (uri, text) in texts {
            let kept = Analysis::new(uri.into(), text, Arc::default(), true);
            let dropped = Analysis::new(uri.into(), text, Arc::default(), false);
            assert!(dropped.tokens.get().is_none(), "{uri}");
            assert_eq!(key(&dropped), key(&kept), "{uri}");
            assert_eq!(
                dropped.comments().collect::<Vec<_>>(),
                kept.comments().collect::<Vec<_>>(),
                "{uri}"
            );
            let parsed = syntax::parse(text, &PrefixMap::default(), kept.parsed.syntax);
            if kept.parsed.syntax != Syntax::JsonLd {
                assert_eq!(
                    kept.comments().collect::<Vec<_>>(),
                    parsed.comments,
                    "{uri}"
                );
            }
        }
    }

    #[test]
    fn line_starts_convert_positions_as_the_line_index_does() {
        let text = "ab\r\n\u{e9}\u{1F600}x\n\n\u{4e2d}\r\r\nend";
        let starts = LineStarts::new(text);
        let lines = LineIndex::new(text);
        for line in 0..8 {
            for col in (0..12).chain([u32::MAX - 1, u32::MAX]) {
                for enc in [Encoding::Utf8, Encoding::Utf16] {
                    let pos = Position::new(line, col);
                    assert_eq!(
                        starts.offset(text, enc, pos),
                        offset_in(&lines, text, enc, pos),
                        "{enc:?} {line}:{col}"
                    );
                }
            }
        }
    }

    #[test]
    fn lines_end_at_lf_crlf_and_a_lone_cr() {
        let texts = [
            ("a\nb", 2),
            ("a\r\nb", 2),
            ("a\rb", 2),
            ("a\rb\r\nc\nd\r", 5),
            ("a\r", 2),
            ("a\r\n", 2),
            ("x\r\u{e9}\u{1F600}y", 2),
        ];
        for (text, line_count) in texts {
            let lines = LineIndex::new(text);
            let starts = LineStarts::new(text);
            assert_eq!(starts.0.len(), line_count, "{text:?}");
            for enc in [Encoding::Utf8, Encoding::Utf16] {
                for (o, _) in text.char_indices().chain([(text.len(), ' ')]) {
                    let pos = position_in(&lines, text, enc, o as u32);
                    let line_start = starts.0[pos.line as usize] as usize;
                    // A `\n` after `\r` is the same line end: back to its content end.
                    let want = if text[..o].ends_with('\r') && text[o..].starts_with('\n') {
                        o - 1
                    } else {
                        o
                    };
                    assert!(line_start <= o, "{text:?} {o}");
                    assert_eq!(offset_in(&lines, text, enc, pos), Some(want as u32));
                    assert_eq!(starts.offset(text, enc, pos), Some(want as u32));
                }
            }
        }
        // After a lone `\r`, the column counts the multi-byte characters.
        let text = "x\r\u{e9}\u{1F600}y";
        let lines = LineIndex::new(text);
        let y = text.len() as u32 - 1;
        assert_eq!(
            position_in(&lines, text, Encoding::Utf16, y),
            Position::new(1, 3)
        );
        assert_eq!(
            position_in(&lines, text, Encoding::Utf8, y),
            Position::new(1, 6)
        );
    }

    /// The conversions as the `line_index` crate made them, before
    /// [`LineIndex`]: the oracle for the equivalence test.
    mod crate_index {
        use super::super::{Encoding, clamped};
        use line_index::{LineIndex, TextSize, WideEncoding, WideLineCol};
        use lsp_types::Position;

        /// A lone `\r` ends a line too: the crate indexes a copy with each
        /// one as `\n` (same length, so every column stays the same).
        pub(super) fn index(text: &str) -> LineIndex {
            let b = text.as_bytes();
            let mut bytes = b.to_vec();
            for (i, _) in text.match_indices('\r') {
                if b.get(i + 1) != Some(&b'\n') {
                    bytes[i] = b'\n';
                }
            }
            LineIndex::new(&String::from_utf8(bytes).unwrap())
        }

        pub(super) fn position(lines: &LineIndex, enc: Encoding, offset: u32) -> Position {
            let mut at = offset.min(u32::from(lines.len()));
            let lc = loop {
                match lines.try_line_col(TextSize::from(at)) {
                    Some(lc) => break lc,
                    None => at -= 1,
                }
            };
            match enc {
                Encoding::Utf8 => Position::new(lc.line, lc.col),
                Encoding::Utf16 => {
                    let w = lines.to_wide(WideEncoding::Utf16, lc).unwrap();
                    Position::new(w.line, w.col)
                }
            }
        }

        pub(super) fn offset(
            lines: &LineIndex,
            text: &str,
            enc: Encoding,
            pos: Position,
        ) -> Option<u32> {
            let Some(line) = lines.line(pos.line) else {
                return Some(text.len() as u32);
            };
            let col = match enc {
                Encoding::Utf8 => pos.character,
                Encoding::Utf16 => {
                    let wide = WideLineCol {
                        line: pos.line,
                        col: pos.character,
                    };
                    lines.to_utf8(WideEncoding::Utf16, wide)?.col
                }
            };
            let (start, end) = (u32::from(line.start()), u32::from(line.end()));
            Some(clamped(text, start as usize, end as usize, col))
        }

        /// As `batch` made code point columns: from a character boundary.
        pub(super) fn code_point_position(lines: &LineIndex, text: &str, off: u32) -> Position {
            let lc = lines.line_col(TextSize::from(off));
            let line_start = off as usize - lc.col as usize;
            Position::new(
                lc.line,
                text[line_start..off as usize].chars().count() as u32,
            )
        }
    }

    /// Texts of ASCII, 2-, 3- and 4-byte characters (a UTF-16 surrogate
    /// pair), DEL, `\n`, `\r\n`, lone `\r` and empty lines, with lines
    /// both shorter and longer than a stride.
    fn mixed_texts() -> Vec<String> {
        const PIECES: [&str; 13] = [
            "a",
            "b",
            " ",
            "\u{7f}",
            "\u{e9}",
            "\u{e9}",
            "\u{4e2d}",
            "\u{20ac}",
            "\u{1F600}",
            "\u{10FFFF}",
            "\n",
            "\r\n",
            "\r",
        ];
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let mut texts: Vec<String> = ["", "\n", "\r", "\r\n\r\n", "\u{1F600}"]
            .map(String::from)
            .into();
        for i in 0..300 {
            // A few lines long enough for marks.
            let len = next(match i {
                _ if i % 60 == 0 => 3000,
                _ if i % 4 == 0 => 600,
                _ => 80,
            });
            // Every few texts, mostly one long line.
            let pieces = if i % 3 == 0 { 10 } else { PIECES.len() };
            texts.push((0..len).map(|_| PIECES[next(pieces)]).collect());
        }
        texts
    }

    #[test]
    fn conversions_match_the_line_index_crate() {
        for text in mixed_texts() {
            let lines = LineIndex::new(&text);
            let old = crate_index::index(&text);
            let len = text.len() as u32;
            for offset in 0..=len + 3 {
                for enc in [Encoding::Utf8, Encoding::Utf16] {
                    assert_eq!(
                        position_in(&lines, &text, enc, offset),
                        crate_index::position(&old, enc, offset),
                        "{enc:?} {offset} in {text:?}"
                    );
                }
                if text.is_char_boundary(offset.min(len) as usize) && offset <= len {
                    assert_eq!(
                        lines.code_point_position(&text, offset),
                        crate_index::code_point_position(&old, &text, offset),
                        "{offset} in {text:?}"
                    );
                }
            }
            let line_count = lines.starts.0.len() as u32;
            let longest = text.split(['\n', '\r']).map(str::len).max().unwrap_or(0) as u32;
            for line in 0..=line_count + 1 {
                let cols = (0..=longest + 3).chain(u32::MAX - 8..=u32::MAX);
                for col in cols {
                    for enc in [Encoding::Utf8, Encoding::Utf16] {
                        let pos = Position::new(line, col);
                        assert_eq!(
                            offset_in(&lines, &text, enc, pos),
                            crate_index::offset(&old, &text, enc, pos),
                            "{enc:?} {line}:{col} in {text:?}"
                        );
                    }
                }
            }
        }
    }

    /// The bytes the conversions scan past a mark while `f` runs.
    fn scanned_by(f: impl FnOnce()) -> usize {
        let before = SCANNED.with(|s| s.get());
        f();
        SCANNED.with(|s| s.get()) - before
    }

    #[test]
    fn a_conversion_on_a_long_line_of_multi_byte_characters_scans_a_stride() {
        // 100,000 `é` after a short line, then a short line.
        let text = format!("ab\n{}x\ny", "\u{e9}".repeat(100_000));
        let lines = LineIndex::new(&text);
        let bound = 3 * STRIDE;
        let mut most = 0;
        for k in (0..=100_000u32)
            .step_by(997)
            .chain([99_999, 100_000, 100_001])
        {
            for (enc, col) in [(Encoding::Utf8, 2 * k), (Encoding::Utf16, k)] {
                let mut offset = None;
                let n =
                    scanned_by(|| offset = offset_in(&lines, &text, enc, Position::new(1, col)));
                most = most.max(n);
                let want = 3 + 2 * k.min(100_000) as usize + usize::from(k > 100_000);
                assert_eq!(offset, Some(want as u32), "{enc:?} {k}");
                let mut pos = None;
                let n = scanned_by(|| pos = Some(position_in(&lines, &text, enc, want as u32)));
                most = most.max(n);
                // Past the line's end, the column is the end of its content.
                let col = col.min(want as u32 - 3);
                assert_eq!(pos, Some(Position::new(1, col)), "{enc:?} {k}");
                // Inside an `é`: back to its start.
                let n = scanned_by(|| pos = Some(position_in(&lines, &text, enc, want as u32 + 1)));
                most = most.max(n);
            }
            let n = scanned_by(|| {
                assert_eq!(
                    lines.code_point_position(&text, 3 + 2 * k.min(100_000)),
                    Position::new(1, k.min(100_000))
                );
            });
            most = most.max(n);
        }
        // A column past the line's end, and in UTF-16 one that overflows.
        for col in [200_000, u32::MAX] {
            let n = scanned_by(|| {
                offset_in(&lines, &text, Encoding::Utf16, Position::new(1, col));
            });
            most = most.max(n);
        }
        assert!(most > 0 && most <= bound, "{most} bytes scanned");
    }

    #[test]
    fn line_starts_follow_edits() {
        let mut text = String::from("ab\ncd\nef\n");
        let mut starts = LineStarts::new(&text);
        // The last four join and split `\r\n` across an edit's ends.
        let edits: [(usize, usize, &str); 10] = [
            (0, 0, "1\n"),
            (3, 6, ""),
            (2, 2, "x\ny\n\u{e9}"),
            (0, 4, "\n\n"),
            (4, 9, "z"),
            (0, 0, ""),
            (1, 1, "\r"),
            (2, 2, "\n"),
            (2, 2, "q"),
            (0, 3, "\r\r"),
        ];
        for (s, e, new) in edits {
            let e = e.min(text.len());
            text.replace_range(s..e, new);
            starts.replace(&text, s..e, new);
            assert_eq!(
                starts.0,
                LineStarts::new(&text).0,
                "after {s}..{e} -> {new:?}: {text:?}"
            );
        }
    }

    /// A random number generator for the edit tests.
    fn xorshift(mut seed: u64) -> impl FnMut(usize) -> usize {
        move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        }
    }

    /// A character boundary of `text` at random.
    fn boundary(text: &str, next: &mut impl FnMut(usize) -> usize) -> usize {
        text.floor_char_boundary(next(text.len() + 1))
    }

    #[test]
    fn collapsed_edits_cover_every_change() {
        let mut next = xorshift(0x9E37_79B9_7F4A_7C15);
        let pieces = ["", "a", "\u{e9}", "\n", "xyz\r\n", "\u{1F600}"];
        for _ in 0..2000 {
            let last = (0..next(12))
                .map(|_| pieces[next(pieces.len())])
                .collect::<String>();
            let mut text = last.clone();
            let mut edit: Option<Edit> = None;
            for _ in 0..1 + next(4) {
                let (a, b) = (boundary(&text, &mut next), boundary(&text, &mut next));
                let range = a.min(b)..a.max(b);
                let new = pieces[next(pieces.len())];
                text.replace_range(range.clone(), new);
                edit = Some(match edit {
                    None => Edit::new(range, new.len()),
                    Some(e) => e.then(range, new.len()),
                });
            }
            let e = edit.unwrap();
            let (old, new) = (e.start as usize..e.end as usize, e.new_end() as usize);
            // The last text with the edit's bytes replaced is the text now.
            let replaced = format!(
                "{}{}{}",
                &last[..old.start],
                &text[old.start..new],
                &last[old.end..]
            );
            assert_eq!(replaced, text, "{e:?} from {last:?}");
        }
    }

    #[test]
    fn an_edited_line_index_is_the_index_of_the_edited_text() {
        // The `\n` of a long line's `\r\n` becomes a `x`: the line before
        // the edit ends a byte sooner, at exactly `LONG` bytes, so it has no
        // marks any more.
        let mut text = format!("a{}\r\nz", "\u{e9}".repeat((LONG - 2) / 2));
        let lines = LineIndex::new(&text);
        let at = LONG;
        assert!(!lines.marks.is_empty());
        text.replace_range(at..at + 1, "x");
        let mut starts = lines.starts().clone();
        starts.replace(&text, at..at + 1, "x");
        let edited = lines.edited(&text, &Edit::new(at..at + 1, 1), &starts);
        assert_eq!(edited, LineIndex::new(&text));

        let mut next = xorshift(0x2545_F491_4F6C_DD1D);
        let pieces = [
            "a",
            "b c",
            "\u{e9}",
            "\u{4e2d}",
            "\u{1F600}",
            "\n",
            "\r\n",
            "\r",
            "\r\r\n",
        ];
        // A long line (over `LONG`) of multi-byte characters, and one of ASCII.
        let long_wide = "\u{e9}x\u{1F600}".repeat(1000);
        let long_ascii = "ab".repeat(3000);
        let inserts = [
            "",
            "x",
            "\n",
            "\r",
            "\r\n",
            "\u{e9}",
            "\u{1F600}\n\u{4e2d}",
            "a\rb\nc",
            &long_wide,
            &long_ascii,
        ];
        for round in 0..300 {
            let mut text: String = (0..next(200)).map(|_| pieces[next(pieces.len())]).collect();
            match round % 3 {
                0 => text.insert_str(boundary(&text, &mut next), &long_wide),
                1 => text.insert_str(boundary(&text, &mut next), &long_ascii),
                _ => {}
            }
            if round % 10 == 0 {
                text = text.replace(['\r', '\n'], "");
            }
            let mut lines = LineIndex::new(&text);
            for _ in 0..8 {
                // A batch of edits before the next analysis, as the
                // workspace keeps them: one edit, and moved line starts.
                let mut starts = lines.starts().clone();
                let mut edit: Option<Edit> = None;
                for _ in 0..1 + next(3) {
                    let a = boundary(&text, &mut next);
                    let b = match next(4) {
                        0 => boundary(&text, &mut next),
                        _ => text.ceil_char_boundary(a + next(6)),
                    };
                    let range = a.min(b)..a.max(b);
                    let new = inserts[next(inserts.len())];
                    text.replace_range(range.clone(), new);
                    starts.replace(&text, range.clone(), new);
                    edit = Some(match edit {
                        None => Edit::new(range, new.len()),
                        Some(e) => e.then(range, new.len()),
                    });
                }
                lines = lines.edited(&text, &edit.unwrap(), &starts);
                assert_eq!(lines, LineIndex::new(&text), "round {round}: {text:?}");
            }
        }
    }
}
