//! Curation lints: mistakes in how a workspace's own terms are labelled,
//! defined and published, which the syntax does not catch.
//!
//! On by default: `duplicate-label`, `label-whitespace` and
//! `multiple-labels`. Off until `lint.enable` lists them ([`OPT_IN`]): the
//! house-style checks. `lint.ignore` switches any of them off; the
//! composer ([`crate::diagnose::document`]) applies both lists.

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::SOURCE;
use crate::features::quickfix::FixData;
use crate::jsonld::json;
use crate::knowledge::{Description, Knowledge, Source};
use crate::syntax::lexer::{self, Kind};
use crate::syntax::names::shorten;
use crate::syntax::{Iri, Node, PrefixMap, Span, Syntax, Triple};
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::OnceLock;

pub(crate) const DUPLICATE_LABEL: &str = "duplicate-label";
pub(crate) const LABEL_WHITESPACE: &str = "label-whitespace";
pub(crate) const MULTIPLE_LABELS: &str = "multiple-labels";
pub(crate) const MISSING_LABEL: &str = "missing-label";
pub(crate) const MISSING_DEFINITION: &str = "missing-definition";
pub(crate) const MULTIPLE_DEFINITIONS: &str = "multiple-definitions";
pub(crate) const DUPLICATE_DEFINITION: &str = "duplicate-definition";
pub(crate) const LOWERCASE_DEFINITION: &str = "lowercase-definition";
pub(crate) const MISSING_ONTOLOGY_TITLE: &str = "missing-ontology-title";
pub(crate) const MISSING_ONTOLOGY_DESCRIPTION: &str = "missing-ontology-description";
pub(crate) const MISSING_ONTOLOGY_LICENSE: &str = "missing-ontology-license";

/// The checks that are off until `lint.enable` lists them: house style,
/// which projects that do not follow it should not see.
pub(crate) const OPT_IN: &[&str] = &[
    MISSING_LABEL,
    MISSING_DEFINITION,
    MULTIPLE_DEFINITIONS,
    DUPLICATE_DEFINITION,
    LOWERCASE_DEFINITION,
    MISSING_ONTOLOGY_TITLE,
    MISSING_ONTOLOGY_DESCRIPTION,
    MISSING_ONTOLOGY_LICENSE,
];

/// The curation diagnostics of `a`; the [`OPT_IN`] checks only with
/// `house_style`.
pub(crate) fn diagnostics(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    house_style: bool,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    // One scan of the document for both label checks.
    let labels = label_triples(a);
    label_whitespace(a, &labels, enc, &mut out);
    let mut c = Checks::new(a, k, enc);
    c.labels(&labels, &mut out);
    if house_style {
        c.definitions(&mut out);
        c.missing(&mut out);
        c.headers(&mut out);
    }
    out
}

/// The IRIs the label, definition and header checks read.
struct Vocab {
    /// What [`Knowledge::labelled`] compares, and what `missing-label` wants.
    labels: [Iri; 2],
    definition: Iri,
    comment: Iri,
    ontology: Iri,
    title: [Iri; 3],
    description: [Iri; 3],
    license: [Iri; 3],
}

fn vocab() -> &'static Vocab {
    static V: OnceLock<Vocab> = OnceLock::new();
    V.get_or_init(|| {
        let i = Iri::intern;
        let rdfs = |l: &str| i(&format!("http://www.w3.org/2000/01/rdf-schema#{l}"));
        let skos = |l: &str| i(&format!("http://www.w3.org/2004/02/skos/core#{l}"));
        let [title, description, license] = *crate::knowledge::header_properties();
        Vocab {
            labels: [rdfs("label"), skos("prefLabel")],
            definition: skos("definition"),
            comment: rdfs("comment"),
            ontology: i(crate::knowledge::OWL_ONTOLOGY),
            title,
            description,
            license,
        }
    })
}

/// How many other terms a message names before "and N more".
const NAMED: usize = 3;

/// A literal object of a declared term in the document being checked.
struct Lit<'a> {
    subject: Iri,
    p: Iri,
    value: &'a str,
    lang: Option<Iri>,
    /// The quoted string ([`string_token`]), else the whole object.
    span: Span,
}

/// The label, definition and header checks of one document. Each reads the
/// document's own triples once, and the knowledge index by key, so a
/// keystroke costs about the size of the document.
struct Checks<'a> {
    a: &'a Analysis,
    k: &'a Knowledge,
    enc: Encoding,
    /// [`Knowledge::is_term`] by subject.
    terms: FxHashMap<Iri, bool>,
    /// [`TermKind::of`] by term, for the terms a label collides with.
    kinds: FxHashMap<Iri, TermKind>,
    /// The prefixes messages write IRIs with, read on first use.
    prefixes: Option<PrefixMap>,
}

impl<'a> Checks<'a> {
    fn new(a: &'a Analysis, k: &'a Knowledge, enc: Encoding) -> Checks<'a> {
        Checks {
            a,
            k,
            enc,
            terms: FxHashMap::default(),
            kinds: FxHashMap::default(),
            prefixes: None,
        }
    }

    fn is_term(&mut self, s: Iri) -> bool {
        *self.terms.entry(s).or_insert_with(|| self.k.is_term(&s))
    }

    fn kind(&mut self, t: Iri) -> TermKind {
        let k = self.k;
        *self.kinds.entry(t).or_insert_with(|| TermKind::of(k, t))
    }

    /// `iri` as the document can write it: a prefixed name, else `<iri>`.
    fn name(&mut self, iri: &str) -> String {
        let a = self.a;
        let prefixes = self.prefixes.get_or_insert_with(|| match a.parsed.syntax {
            Syntax::JsonLd => crate::features::lint::jsonld_prefixes(a),
            _ => a.prefix_map(),
        });
        shorten(iri, prefixes).unwrap_or_else(|| format!("<{iri}>"))
    }

    /// `terms` other than `except` (sorted, unique), named: at most
    /// [`NAMED`] of them and then how many more.
    fn names(&mut self, terms: &[Iri], except: Iri) -> String {
        let count = others(terms, except);
        let mut named: Vec<String> = terms
            .iter()
            .filter(|t| **t != except)
            .take(NAMED)
            .map(|t| self.name(t))
            .collect();
        let more = count.saturating_sub(NAMED);
        if more > 0 {
            return format!("{} and {more} more", named.join(", "));
        }
        let last = named.pop().unwrap_or_default();
        if named.is_empty() {
            last
        } else {
            format!("{} and {last}", named.join(", "))
        }
    }

    fn warning(&self, span: Span, code: &str, message: String) -> Diagnostic {
        Diagnostic {
            range: self.a.range(self.enc, span),
            severity: Some(DiagnosticSeverity::WARNING),
            code: Some(NumberOrString::String(code.to_string())),
            source: Some(SOURCE.to_string()),
            message,
            ..Default::default()
        }
    }

    /// The literals of `predicates` among `triples` (of the document, in
    /// text order) whose subject is a declared term, in text order, one per
    /// literal written (a JSON-LD value can give several triples one
    /// literal).
    fn literals(
        &mut self,
        triples: impl IntoIterator<Item = &'a Triple>,
        predicates: &[Iri],
    ) -> Vec<Lit<'a>> {
        let a = self.a;
        let mut out: Vec<Lit<'a>> = Vec::new();
        let mut seen: FxHashSet<(Span, Iri, Iri)> = FxHashSet::default();
        for t in triples.into_iter().filter(|t| predicates.contains(&t.p)) {
            let (Node::Iri(s), Node::Literal { value, lang, .. }) = (&t.s, &t.o) else {
                continue;
            };
            if !self.is_term(*s) {
                continue;
            }
            let span = string_token(a, t.o_span, value).unwrap_or(t.o_span);
            if seen.insert((span, *s, t.p)) {
                out.push(Lit {
                    subject: *s,
                    p: t.p,
                    value,
                    lang: *lang,
                    span,
                });
            }
        }
        out
    }

    /// `duplicate-label` and `multiple-labels`. Empty labels (after
    /// trimming) say nothing to tell terms apart by, so they never collide;
    /// nor do terms of different [`TermKind`]s (a class `Judge` and a property
    /// `judge` are two terms by convention, not a copy-paste slip).
    ///
    /// The terms a label can collide with are filtered once per (label,
    /// kind), so N terms that share one label cost O(N), not O(N²).
    /// `labels` are the document's triples of [`label_triples`].
    fn labels(&mut self, labels: &[&'a Triple], out: &mut Vec<Diagnostic>) {
        let lits = self.literals(labels.iter().copied(), &vocab().labels);
        let k = self.k;
        let mut sharing = Sharing::default();
        let mut by_kind: FxHashMap<(TextKey, TermKind), Vec<Iri>> = FxHashMap::default();
        for l in lits.iter().filter(|l| !l.value.trim().is_empty()) {
            let key = Sharing::key(l);
            let all = sharing.all(key.clone(), l, |text, lang| k.labelled(text, lang));
            if others(all, l.subject) == 0 {
                continue;
            }
            let kind = self.kind(l.subject);
            let group = (key, kind);
            if !by_kind.contains_key(&group) {
                let colliding = all
                    .iter()
                    .copied()
                    .filter(|t| kind.collides(self.kind(*t)))
                    .collect();
                by_kind.insert(group.clone(), colliding);
            }
            let colliding = &by_kind[&group];
            if others(colliding, l.subject) == 0 {
                continue;
            }
            let message = format!(
                "Label \"{}\" is also the label of {}",
                l.value.trim(),
                self.names(colliding, l.subject)
            );
            out.push(self.warning(l.span, DUPLICATE_LABEL, message));
        }
        self.multiple(&lits, MULTIPLE_LABELS, out);
    }

    /// `duplicate-definition`, `multiple-definitions` and
    /// `lowercase-definition` (empty definitions, like empty labels, never
    /// collide). A term's definition is its `skos:definition`,
    /// or its `rdfs:comment` when no workspace source gives it a
    /// `skos:definition`: [`Knowledge::defined_as`] lists a comment's
    /// subject only then.
    fn definitions(&mut self, out: &mut Vec<Diagnostic>) {
        let v = vocab();
        let lits = self.literals(&self.a.parsed.triples, &[v.definition, v.comment]);
        let k = self.k;
        let mut sharing = Sharing::default();
        let mut defs: Vec<Lit> = Vec::new();
        for l in lits {
            let all = sharing.all(Sharing::key(&l), &l, |text, lang| k.defined_as(text, lang));
            if all.binary_search(&l.subject).is_err() {
                continue;
            }
            if others(all, l.subject) > 0 && !l.value.trim().is_empty() {
                let names = self.names(all, l.subject);
                let message = format!("Another term has this definition: {names}");
                out.push(self.warning(l.span, DUPLICATE_DEFINITION, message));
            }
            if l.value
                .trim_start()
                .chars()
                .next()
                .is_some_and(char::is_lowercase)
            {
                let message = "Definition starts with a lowercase letter".to_string();
                out.push(self.warning(l.span, LOWERCASE_DEFINITION, message));
            }
            defs.push(l);
        }
        self.multiple(&defs, MULTIPLE_DEFINITIONS, out);
    }

    /// `code` on each of `lits` whose subject has another value of the same
    /// property in the same language here.
    fn multiple(&mut self, lits: &[Lit], code: &str, out: &mut Vec<Diagnostic>) {
        let mut values: FxHashMap<(Iri, Iri, Option<Iri>), Vec<&str>> = FxHashMap::default();
        for l in lits {
            let v = values.entry((l.subject, l.p, l.lang)).or_default();
            if !v.contains(&l.value) {
                v.push(l.value);
            }
        }
        for l in lits {
            let n = values[&(l.subject, l.p, l.lang)].len();
            if n < 2 {
                continue;
            }
            let (s, p) = (self.name(&l.subject), self.name(&l.p));
            let message = match l.lang {
                Some(lang) => format!("{s} has {n} {p} values tagged @{}", &*lang),
                None => format!("{s} has {n} {p} values without a language tag"),
            };
            out.push(self.warning(l.span, code, message));
        }
    }

    /// `missing-label` and `missing-definition`, on the subject of the first
    /// statement about each declared term, in one document only: the first
    /// by URI of the workspace documents that type the term or give it a
    /// super class or property, else of those that describe it. A label or
    /// definition in any workspace document counts: this document's are
    /// read from its triples, another's from its label and comment indexes
    /// ([`Analysis::label`], [`Analysis::comment_en`]), which look the term
    /// up without a scan and also count `sh:name` and `dcterms:description`.
    fn missing(&mut self, out: &mut Vec<Diagnostic>) {
        let (a, k, v) = (self.a, self.k, vocab());
        let mut labelled: FxHashSet<Iri> = FxHashSet::default();
        let mut defined: FxHashSet<Iri> = FxHashSet::default();
        for t in &a.parsed.triples {
            if let (Node::Iri(s), Node::Literal { .. }) = (&t.s, &t.o) {
                if v.labels.contains(&t.p) {
                    labelled.insert(*s);
                } else if t.p == v.definition || t.p == v.comment {
                    defined.insert(*s);
                }
            }
        }
        let mut seen: FxHashSet<Iri> = FxHashSet::default();
        for st in &a.parsed.statements {
            let Some((s, span)) = st.subject else {
                continue;
            };
            if !seen.insert(s) || !self.is_term(s) {
                continue;
            }
            let descriptions: Vec<_> = k
                .descriptions(&s)
                .into_iter()
                .filter(|d| d.source != Source::External)
                .collect();
            let declares = |d: &Description| {
                d.analysis.types.contains_key(&s) || d.analysis.supers.contains_key(&s)
            };
            let reporter = descriptions
                .iter()
                .filter(|d| declares(d))
                .map(|d| d.analysis.uri.as_str())
                .min()
                .or_else(|| descriptions.iter().map(|d| d.analysis.uri.as_str()).min());
            if reporter.is_some_and(|uri| uri != a.uri) {
                continue;
            }
            let elsewhere = || descriptions.iter().filter(|d| d.analysis.uri != a.uri);
            if !labelled.contains(&s) && !elsewhere().any(|d| d.analysis.label(&s).is_some()) {
                let message = format!("{} has no rdfs:label or skos:prefLabel", self.name(&s));
                out.push(self.warning(span, MISSING_LABEL, message));
            }
            if !defined.contains(&s) && !elsewhere().any(|d| d.analysis.comment_en(&s).is_some()) {
                let message = format!("{} has no skos:definition or rdfs:comment", self.name(&s));
                out.push(self.warning(span, MISSING_DEFINITION, message));
            }
        }
    }

    /// `missing-ontology-title`, `-description` and `-license`, on the
    /// subject of each IRI typed `owl:Ontology`, in the first by URI of the
    /// workspace documents that type it so; a header property any workspace
    /// document gives it counts.
    fn headers(&mut self, out: &mut Vec<Diagnostic>) {
        let (a, k, v) = (self.a, self.k, vocab());
        let rdf_type = crate::shacl::vocab::get().rdf_type;
        let mut seen: FxHashSet<Iri> = FxHashSet::default();
        for t in &a.parsed.triples {
            let Node::Iri(o) = &t.s else {
                continue;
            };
            if t.p != rdf_type || t.o != Node::Iri(v.ontology) || !seen.insert(*o) {
                continue;
            }
            let o = *o;
            let descriptions: Vec<_> = k
                .descriptions(&o)
                .into_iter()
                .filter(|d| d.source != Source::External)
                .collect();
            let reporter = descriptions
                .iter()
                .filter(|d| {
                    d.analysis
                        .types
                        .get(&o)
                        .is_some_and(|ts| ts.contains(&v.ontology))
                })
                .map(|d| d.analysis.uri.as_str())
                .min();
            if reporter.is_some_and(|uri| uri != a.uri) {
                continue;
            }
            let span = a
                .parsed
                .statements
                .get(t.statement as usize)
                .and_then(|st| st.subject)
                .filter(|(s, _)| *s == o)
                .map_or(t.p_span, |(_, span)| span);
            let has = |props: &[Iri]| {
                std::iter::once(a)
                    .chain(descriptions.iter().map(|d| d.analysis))
                    .filter(|d| std::ptr::eq(*d, a) || d.uri != a.uri)
                    .any(|d| {
                        d.parsed
                            .triples
                            .iter()
                            .any(|t| t.s == Node::Iri(o) && props.contains(&t.p))
                    })
            };
            let checks = [
                (
                    &v.title,
                    MISSING_ONTOLOGY_TITLE,
                    "title (dcterms:title, dc:title or rdfs:label)",
                ),
                (
                    &v.description,
                    MISSING_ONTOLOGY_DESCRIPTION,
                    "description (dcterms:description, dc:description or rdfs:comment)",
                ),
                (
                    &v.license,
                    MISSING_ONTOLOGY_LICENSE,
                    "licence (dcterms:license or cc:license)",
                ),
            ];
            for (props, code, what) in checks {
                if !has(props) {
                    let message = format!("Ontology {} has no {what}", self.name(&o));
                    out.push(self.warning(span, code, message));
                }
            }
        }
    }
}

/// What sort of term an IRI is, as the bits of [`Knowledge::term_kind`].
/// Terms of different kinds may share a label.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TermKind(u8);

impl TermKind {
    /// The kinds the workspace gives `t`: its types (`owl:Class`,
    /// `rdfs:Class`; the four property types; `skos:Concept`), and being the
    /// subject of `rdfs:subClassOf` or `rdfs:subPropertyOf`, in workspace
    /// documents only.
    fn of(k: &Knowledge, t: Iri) -> TermKind {
        TermKind(k.term_kind(&t))
    }

    /// Whether terms of these kinds can collide: the same one kind, or
    /// either unknown (none, or several).
    fn collides(self, other: TermKind) -> bool {
        let unknown = |k: TermKind| k.0.count_ones() != 1;
        self == other || unknown(self) || unknown(other)
    }
}

/// A literal's folded text and language: what [`Sharing`] looks terms up by.
type TextKey = (String, Option<Iri>);

/// How many of `terms` (sorted, unique) are not `except`.
fn others(terms: &[Iri], except: Iri) -> usize {
    terms.len() - usize::from(terms.binary_search(&except).is_ok())
}

/// The terms the index gives for each (folded text, language) the
/// document's literals have, looked up once per key.
#[derive(Default)]
struct Sharing {
    found: FxHashMap<TextKey, Vec<Iri>>,
}

impl Sharing {
    fn key(l: &Lit) -> TextKey {
        let folded = l
            .value
            .trim()
            .chars()
            .flat_map(char::to_lowercase)
            .collect();
        (folded, l.lang)
    }

    /// The terms (sorted, unique) that share `l`'s text, whose key is `key`,
    /// `l`'s subject among them if the index counts it.
    fn all(
        &mut self,
        key: TextKey,
        l: &Lit,
        find: impl Fn(&str, Option<&str>) -> Vec<Iri>,
    ) -> &[Iri] {
        self.found
            .entry(key)
            .or_insert_with(|| find(l.value, l.lang.as_ref().map(|x| x.as_str())))
    }
}

/// The predicates whose literals [`label_whitespace`] checks, on any subject:
/// those [`Checks::labels`] reads ([`Vocab::labels`]) and `skos:altLabel`.
const WHITESPACE_CHECKED: [&str; 3] = [
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2004/02/skos/core#prefLabel",
    "http://www.w3.org/2004/02/skos/core#altLabel",
];

/// The triples of `a` with a [`WHITESPACE_CHECKED`] predicate, in text order.
fn label_triples(a: &Analysis) -> Vec<&Triple> {
    let checked: Vec<Iri> = WHITESPACE_CHECKED
        .iter()
        .filter_map(|p| Iri::get(p))
        .collect();
    if checked.is_empty() {
        return Vec::new();
    }
    a.parsed
        .triples
        .iter()
        .filter(|t| checked.contains(&t.p))
        .collect()
}

/// `label-whitespace`: a label literal whose value starts or ends with
/// whitespace ([`char::is_whitespace`]). The warning covers the literal's
/// quoted string, and its [`FixData::Replace`] is that string trimmed.
/// `labels` are the document's triples of [`label_triples`].
fn label_whitespace(a: &Analysis, labels: &[&Triple], enc: Encoding, out: &mut Vec<Diagnostic>) {
    // A JSON-LD value can give one literal to several triples.
    let mut seen: FxHashSet<Span> = FxHashSet::default();
    for t in labels {
        let Node::Literal { value, .. } = &t.o else {
            continue;
        };
        let (start, end) = (
            value.trim_start().len() < value.len(),
            value.trim_end().len() < value.len(),
        );
        let message = match (start, end) {
            (false, false) => continue,
            (true, false) => "Label starts with whitespace",
            (false, true) => "Label ends with whitespace",
            (true, true) => "Label starts and ends with whitespace",
        };
        let string = string_token(a, t.o_span, value);
        let span = string.unwrap_or(t.o_span);
        if !seen.insert(span) {
            continue;
        }
        let mut d = Diagnostic {
            range: a.range(enc, span),
            severity: Some(DiagnosticSeverity::WARNING),
            code: Some(NumberOrString::String(LABEL_WHITESPACE.to_string())),
            source: Some(SOURCE.to_string()),
            message: message.to_string(),
            ..Default::default()
        };
        let json = a.parsed.syntax == Syntax::JsonLd;
        if let Some(s) = string
            && let Some(replace) = trimmed(&a.text[s.start as usize..s.end as usize], json)
        {
            FixData::Replace { replace }.attach(&mut d);
        }
        out.push(d);
    }
}

/// The span of the quoted string that gives the literal at `o_span` its
/// `value`: the literal's first token in the Turtle family, the JSON string
/// itself or a value object's string whose value is `value` in JSON-LD.
/// `None` for a malformed string or one not found.
fn string_token(a: &Analysis, o_span: Span, value: &str) -> Option<Span> {
    let text = a.text.get(o_span.start as usize..o_span.end as usize)?;
    let at = |s: Span| Span::new(o_span.start + s.start, o_span.start + s.end);
    if a.parsed.syntax != Syntax::JsonLd {
        let t = *lexer::tokenize(text).first()?;
        return (t.kind == Kind::String && t.error().is_none())
            .then(|| at(Span::new(t.start, t.end)));
    }
    if text.starts_with('"') {
        return Some(o_span);
    }
    // A value object: the string after a key at its top level that reads as
    // `value` (an `@value` key, or an alias of it).
    let tokens = json::tokens(text);
    let mut depth = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        match t.kind {
            json::TokenKind::LBrace | json::TokenKind::LBracket => depth += 1,
            json::TokenKind::RBrace | json::TokenKind::RBracket => depth = depth.saturating_sub(1),
            json::TokenKind::String
                if depth == 1
                    && i > 0
                    && tokens[i - 1].kind == json::TokenKind::Colon
                    && serde_json::from_str::<String>(
                        &text[t.span.start as usize..t.span.end as usize],
                    )
                    .is_ok_and(|s| s == value) =>
            {
                return Some(at(t.span));
            }
            _ => {}
        }
    }
    None
}

/// The string token `token` (quotes included) without the whitespace at
/// either end of its value, written raw or as an escape (`\t`, `\u0020`);
/// its quotes and other escapes kept. `json` for a JSON string, else a
/// Turtle one in any of its four quote styles. `None` when nothing changes.
fn trimmed(token: &str, json: bool) -> Option<String> {
    let q = token.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    // `get`: the first three bytes can end inside a character (`" é "`).
    let long =
        !json && token.len() >= 6 && token.get(..3).is_some_and(|d| d.chars().all(|c| c == q));
    let delim = if long { 3 } else { 1 };
    let content = token.get(delim..token.len().checked_sub(delim)?)?;
    // Each character or escape of the content: (start, end, whitespace).
    let mut units: Vec<(usize, usize, bool)> = Vec::new();
    let mut chars = content.char_indices();
    while let Some((i, c)) = chars.next() {
        if c != '\\' {
            units.push((i, i + c.len_utf8(), c.is_whitespace()));
            continue;
        }
        let Some((_, e)) = chars.next() else {
            units.push((i, i + 1, false));
            break;
        };
        let hex = match e {
            'u' => 4,
            'U' => 8,
            _ => 0,
        };
        let decoded = match e {
            't' => Some('\t'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            'f' => Some('\u{c}'),
            _ if hex > 0 => content
                .get(i + 2..i + 2 + hex)
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .and_then(char::from_u32),
            _ => None,
        };
        let mut end = i + 1 + e.len_utf8();
        if decoded.is_some() && hex > 0 {
            end += hex;
            chars.nth(hex - 1);
        }
        units.push((i, end, decoded.is_some_and(char::is_whitespace)));
    }
    let first = units.iter().position(|u| !u.2);
    let last = units.iter().rposition(|u| !u.2);
    let (kept, quote_last) = match (first, last) {
        (Some(f), Some(l)) => {
            let (s, e) = (units[f].0, units[l].1);
            (
                &content[s..e],
                &content[units[l].0..e] == q.encode_utf8(&mut [0; 4]),
            )
        }
        _ => ("", false),
    };
    if kept.len() == content.len() {
        return None;
    }
    let delim = &token[..delim];
    // A long string's content cannot end with a raw quote of its own kind,
    // which would close it early: escape that quote.
    if long && quote_last {
        let body = &kept[..kept.len() - 1];
        return Some(format!("{delim}{body}\\{q}{delim}"));
    }
    Some(format!("{delim}{kept}{delim}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::quickfix::Fixer;
    use std::sync::Arc;

    const PREFIXES: &str = "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n@prefix ex: <http://example.org/> .\n";

    /// Each `label-whitespace` diagnostic of `text` in file `name`: the text
    /// it covers, its message, and the text after its quick fix (None
    /// without one).
    fn checked(name: &str, text: &str) -> Vec<(String, String, Option<String>)> {
        let a = Analysis::new(format!("file:///x/{name}"), text, Arc::default(), true);
        let k = Knowledge::default();
        let uri: lsp_types::Uri = a.uri.parse().unwrap();
        let fixer = Fixer::new(&a, &k, Encoding::Utf16, &uri);
        diagnostics(&a, &k, Encoding::Utf16, false)
            .into_iter()
            .filter(|d| d.code == Some(NumberOrString::String(LABEL_WHITESPACE.into())))
            .map(|d| {
                assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
                let (s, e) = (
                    a.offset(Encoding::Utf16, d.range.start).unwrap() as usize,
                    a.offset(Encoding::Utf16, d.range.end).unwrap() as usize,
                );
                let fixed = fixer.for_diagnostic(&d).first().map(|c| {
                    assert_eq!(c.is_preferred, Some(true));
                    let edits = c.edit.clone().unwrap().changes.unwrap();
                    let edits: Vec<_> = edits.into_values().flatten().collect();
                    crate::features::refactor::apply(&a, Encoding::Utf16, &edits)
                });
                (text[s..e].to_string(), d.message, fixed)
            })
            .collect()
    }

    /// The one diagnostic of Turtle `objects` (the label after `ex:a`): the
    /// text it covers, and the line after its fix.
    fn turtle(objects: &str) -> (String, String) {
        let text = format!("{PREFIXES}ex:a {objects} .\n");
        let found = checked("a.ttl", &text);
        assert_eq!(found.len(), 1, "{found:?}");
        let (at, _, fixed) = found.into_iter().next().unwrap();
        let fixed = fixed.expect("a fix");
        let line = fixed.strip_prefix(PREFIXES).unwrap().to_string();
        (at, line)
    }

    #[test]
    fn leading_trailing_or_both() {
        let text = format!(
            "{PREFIXES}ex:a rdfs:label \" Pump\" .\nex:b rdfs:label \"Pump \" .\nex:c rdfs:label \"  Pump  \" .\n"
        );
        let found: Vec<(String, String)> = checked("a.ttl", &text)
            .into_iter()
            .map(|(at, m, _)| (at, m))
            .collect();
        let f = |a: &str, m: &str| (a.to_string(), m.to_string());
        assert_eq!(
            found,
            vec![
                f("\" Pump\"", "Label starts with whitespace"),
                f("\"Pump \"", "Label ends with whitespace"),
                f("\"  Pump  \"", "Label starts and ends with whitespace"),
            ]
        );
        assert_eq!(
            turtle("rdfs:label \"  Pump  \""),
            ("\"  Pump  \"".into(), "ex:a rdfs:label \"Pump\" .\n".into())
        );
    }

    #[test]
    fn tabs_and_newlines_raw_or_escaped() {
        let t = |o: &str| turtle(o).1;
        assert_eq!(
            t("rdfs:label \"\\tPump\\n\""),
            "ex:a rdfs:label \"Pump\" .\n"
        );
        assert_eq!(t("rdfs:label \"\tPump\""), "ex:a rdfs:label \"Pump\" .\n");
        assert_eq!(
            t("rdfs:label \"\\u0020Pump\\r\\f\\U00000009\""),
            "ex:a rdfs:label \"Pump\" .\n"
        );
        // Escapes inside the value stay; a backslash escape is not `\t`.
        assert_eq!(
            t("rdfs:label \" Pu\\tmp\\\\t\""),
            "ex:a rdfs:label \"Pu\\tmp\\\\t\" .\n"
        );
        assert_eq!(
            t("rdfs:label \"\"\"\n  Pump\n\"\"\""),
            "ex:a rdfs:label \"\"\"Pump\"\"\" .\n"
        );
    }

    #[test]
    fn a_multibyte_character_near_the_quote_is_not_cut() {
        // The first three bytes of `" é "` and `" 日本"` end inside a character.
        assert_eq!(
            turtle("rdfs:label \" é \""),
            ("\" é \"".into(), "ex:a rdfs:label \"é\" .\n".into())
        );
        assert_eq!(
            turtle("rdfs:label \" 日本\"").1,
            "ex:a rdfs:label \"日本\" .\n"
        );
        assert_eq!(trimmed("\"é", false), None);
        assert_eq!(trimmed("'日本 '", false), Some("'日本'".into()));
    }

    /// 40,000 labels with a trailing space: the literals already reported
    /// are found by hash, not by a scan of the list (which took 200 ms).
    #[cfg(not(debug_assertions))]
    #[test]
    fn timing_many_spaced_labels_are_not_quadratic() {
        let text: String = std::iter::once(PREFIXES.to_string())
            .chain((0..40_000).map(|i| format!("ex:s{i} rdfs:label \"n{i} \" .\n")))
            .collect();
        let a = Analysis::new("file:///x/a.ttl".into(), &text, Arc::default(), true);
        let mut out = Vec::new();
        let start = std::time::Instant::now();
        label_whitespace(&a, &label_triples(&a), Encoding::Utf16, &mut out);
        let took = start.elapsed();
        assert_eq!(out.len(), 40_000);
        assert!(took.as_millis() < 100, "took {took:?}");
    }

    #[test]
    fn every_quote_style_is_kept() {
        let t = |o: &str| turtle(o).1;
        assert_eq!(t("rdfs:label ' Pump '"), "ex:a rdfs:label 'Pump' .\n");
        assert_eq!(
            t("rdfs:label ''' Pump '''"),
            "ex:a rdfs:label '''Pump''' .\n"
        );
        assert_eq!(
            t("rdfs:label \"\"\" Pump \"\"\""),
            "ex:a rdfs:label \"\"\"Pump\"\"\" .\n"
        );
        // A long string cannot end with its own quote: the last one is escaped.
        assert_eq!(
            t("rdfs:label \"\"\"say \"hi\" \"\"\""),
            "ex:a rdfs:label \"\"\"say \"hi\\\"\"\"\" .\n"
        );
        assert_eq!(
            t("rdfs:label ''' it''s '''"),
            "ex:a rdfs:label '''it''s''' .\n"
        );
    }

    #[test]
    fn language_tag_and_datatype_are_kept() {
        assert_eq!(
            turtle("skos:prefLabel \" Pumpe\"@de"),
            (
                "\" Pumpe\"".into(),
                "ex:a skos:prefLabel \"Pumpe\"@de .\n".into()
            )
        );
        assert_eq!(
            turtle("skos:altLabel \"Pump \"^^xsd:string").1,
            "ex:a skos:altLabel \"Pump\"^^xsd:string .\n"
        );
    }

    #[test]
    fn a_label_of_only_whitespace_becomes_empty() {
        assert_eq!(turtle("rdfs:label \" \\t \"").1, "ex:a rdfs:label \"\" .\n");
        assert_eq!(
            turtle("rdfs:label \"\"\"  \"\"\"").1,
            "ex:a rdfs:label \"\"\"\"\"\" .\n"
        );
    }

    #[test]
    fn clean_labels_and_other_predicates_are_not_flagged() {
        let text = format!(
            "{PREFIXES}ex:a rdfs:label \"Pump\", \"Two words\", \"\" ;\n  rdfs:comment \" spaced \" ;\n  ex:label \" spaced \" ;\n  skos:prefLabel \"\"\"Pump\"\"\"@en .\n"
        );
        assert_eq!(checked("a.ttl", &text), vec![]);
    }

    #[test]
    fn found_in_trig_and_n_triples() {
        let trig = format!("{PREFIXES}ex:g {{ ex:a rdfs:label \" Pump\" }}\n");
        let found = checked("a.trig", &trig);
        assert_eq!(found.len(), 1);
        assert!(
            found[0]
                .2
                .as_ref()
                .unwrap()
                .contains("rdfs:label \"Pump\" }")
        );
        let nt = "<http://example.org/a> <http://www.w3.org/2000/01/rdf-schema#label> \"Pump\\t\"@en .\n";
        let found = checked("a.nt", nt);
        assert_eq!(
            found[0].2.as_deref(),
            Some(
                "<http://example.org/a> <http://www.w3.org/2000/01/rdf-schema#label> \"Pump\"@en .\n"
            )
        );
    }

    #[test]
    fn json_ld_string_values_are_trimmed() {
        let text = r#"{
  "@context": {
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "skos": "http://www.w3.org/2004/02/skos/core#",
    "value": "@value"
  },
  "@id": "http://example.org/a",
  "rdfs:label": " Pump\t",
  "skos:prefLabel": { "@language": "de", "@value": "Pumpe\n" },
  "skos:altLabel": [{ "value": " Kreisel", "@language": "de" }, "ok"]
}"#;
        let found = checked("a.jsonld", text);
        let at: Vec<&str> = found.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(at, vec![r#"" Pump\t""#, r#""Pumpe\n""#, r#"" Kreisel""#]);
        assert!(
            found[0]
                .2
                .as_ref()
                .unwrap()
                .contains(r#""rdfs:label": "Pump","#)
        );
        assert!(
            found[1]
                .2
                .as_ref()
                .unwrap()
                .contains(r#""@value": "Pumpe" }"#)
        );
        assert!(
            found[2]
                .2
                .as_ref()
                .unwrap()
                .contains(r#""value": "Kreisel","#)
        );
    }

    // ---- duplicate-label, multiple-labels and the house-style checks ----

    const HEADER: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n@prefix dcterms: <http://purl.org/dc/terms/> .\n@prefix dc: <http://purl.org/dc/elements/1.1/> .\n@prefix ex: <http://example.org/> .\n";

    /// A workspace of the Turtle documents `files` (name, body after
    /// [`HEADER`]), all open.
    fn workspace(files: &[(&str, &str)]) -> crate::workspace::Workspace {
        let mut ws = crate::workspace::Workspace::new(Vec::new(), Default::default());
        for (name, body) in files {
            ws.open_doc(format!("file:///w/{name}"), format!("{HEADER}{body}"), 1);
        }
        ws
    }

    /// Settings with `lint.enable` and `lint.ignore` from the settings file.
    fn settings(enable: &[&str], ignore: &[&str]) -> crate::settings::Settings {
        let v = serde_json::json!({ "lint": { "enable": enable, "ignore": ignore } });
        crate::settings::Settings::from_layers([(crate::settings::Layer::File, &v)])
    }

    /// The curation diagnostics of `name` as the composer reports them with
    /// `settings`: (code, the text it covers, message), in order.
    fn reported(
        ws: &mut crate::workspace::Workspace,
        name: &str,
        settings: &crate::settings::Settings,
    ) -> Vec<(String, String, String)> {
        use crate::diagnose::{DiagnosticInputs, Doc, document};
        let a = ws.analysis(&format!("file:///w/{name}")).unwrap();
        let k = ws.knowledge();
        let inputs = DiagnosticInputs::new(settings);
        document(Doc::Rdf(&a), k, Encoding::Utf16, &inputs)
            .into_iter()
            .filter_map(|d| {
                let Some(NumberOrString::String(code)) = &d.code else {
                    return None;
                };
                if !CODES.contains(&code.as_str()) {
                    return None;
                }
                assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
                let (s, e) = (
                    a.offset(Encoding::Utf16, d.range.start).unwrap() as usize,
                    a.offset(Encoding::Utf16, d.range.end).unwrap() as usize,
                );
                Some((code.clone(), a.text[s..e].to_string(), d.message))
            })
            .collect()
    }

    const CODES: &[&str] = &[
        DUPLICATE_LABEL,
        MULTIPLE_LABELS,
        MISSING_LABEL,
        MISSING_DEFINITION,
        MULTIPLE_DEFINITIONS,
        DUPLICATE_DEFINITION,
        LOWERCASE_DEFINITION,
        MISSING_ONTOLOGY_TITLE,
        MISSING_ONTOLOGY_DESCRIPTION,
        MISSING_ONTOLOGY_LICENSE,
    ];

    /// The default-on checks of `name`.
    fn by_default(files: &[(&str, &str)], name: &str) -> Vec<(String, String, String)> {
        reported(&mut workspace(files), name, &settings(&[], &[]))
    }

    fn row(code: &str, at: &str, message: &str) -> (String, String, String) {
        (code.into(), at.into(), message.into())
    }

    #[test]
    fn a_duplicate_label_across_two_files_is_reported_on_both() {
        let files = [
            ("a.ttl", "ex:Pump a owl:Class ; rdfs:label \"Pump\"@en .\n"),
            (
                "b.ttl",
                "ex:Pump2 a owl:Class ; skos:prefLabel \" pump\"@en .\n",
            ),
        ];
        assert_eq!(
            by_default(&files, "a.ttl"),
            vec![row(
                DUPLICATE_LABEL,
                "\"Pump\"",
                "Label \"Pump\" is also the label of ex:Pump2"
            )]
        );
        assert_eq!(
            by_default(&files, "b.ttl"),
            vec![row(
                DUPLICATE_LABEL,
                "\" pump\"",
                "Label \"pump\" is also the label of ex:Pump"
            )]
        );
    }

    #[test]
    fn labels_collide_ignoring_case_but_not_across_languages() {
        let files = [(
            "a.ttl",
            "ex:A a owl:Class ; rdfs:label \"Pump\"@en .\n\
             ex:B a owl:Class ; rdfs:label \"Pump\"@de .\n\
             ex:C a owl:Class ; rdfs:label \"Pump\" .\n\
             ex:D a owl:Class ; rdfs:label \"Pump\"@en-GB .\n\
             ex:E a owl:Class ; rdfs:label \"PUMP\"@EN .\n",
        )];
        let found = by_default(&files, "a.ttl");
        assert_eq!(
            found,
            vec![
                row(
                    DUPLICATE_LABEL,
                    "\"Pump\"",
                    "Label \"Pump\" is also the label of ex:E"
                ),
                row(
                    DUPLICATE_LABEL,
                    "\"PUMP\"",
                    "Label \"PUMP\" is also the label of ex:A"
                ),
            ]
        );
    }

    #[test]
    fn instance_data_and_external_sources_are_not_compared() {
        let files = [(
            "a.ttl",
            "ex:Person a owl:Class ; rdfs:label \"Alice\" .\n\
             ex:alice a ex:Person ; rdfs:label \"Alice\" .\n\
             ex:bob rdfs:label \"Alice\" .\n\
             ex:Valve a owl:Class ; rdfs:label \"Valve\" .\n",
        )];
        let mut ws = workspace(&files);
        let ext = format!("{HEADER}ex:Other a owl:Class ; rdfs:label \"Valve\" .\n");
        let ext = Analysis::new("import:x".into(), &ext, Arc::default(), false);
        ws.knowledge_mut().add_external("import:x", Arc::new(ext));
        assert_eq!(reported(&mut ws, "a.ttl", &settings(&[], &[])), vec![]);
    }

    #[test]
    fn a_term_never_collides_with_itself_nor_an_empty_label() {
        let files = [(
            "a.ttl",
            "ex:A a owl:Class ; rdfs:label \"Pump\" ; skos:prefLabel \"pump\" .\n\
             ex:A rdfs:label \"Pump\" .\n\
             ex:B a owl:Class ; rdfs:label \" \" .\nex:C a owl:Class ; rdfs:label \"\" .\n",
        )];
        // Nor do two empty labels make a duplicate.
        assert_eq!(by_default(&files, "a.ttl"), vec![]);
    }

    #[test]
    fn only_terms_of_the_same_kind_collide() {
        let files = [(
            "a.ttl",
            "ex:Judge a owl:Class ; rdfs:label \"Judge\" .\n\
             ex:judge a owl:ObjectProperty ; rdfs:label \"judge\" .\n\
             ex:Court a owl:Class ; rdfs:label \"Court\" .\n\
             ex:Court2 a rdfs:Class ; rdfs:label \"court\" .\n\
             ex:court rdfs:subPropertyOf ex:judge ; rdfs:label \"Court\" .\n\
             ex:Pump a owl:Class ; rdfs:label \"Pump\" .\n\
             ex:Pump2 rdfs:subClassOf ex:Thing ; rdfs:label \"Pump\" .\n\
             ex:pump a skos:Concept ; rdfs:label \"Pump\" .\n\
             ex:Valve a owl:Class, skos:Concept ; rdfs:label \"Valve\" .\n\
             ex:valve a owl:DatatypeProperty ; rdfs:label \"Valve\" .\n",
        )];
        let at: Vec<String> = by_default(&files, "a.ttl")
            .into_iter()
            .map(|r| r.2)
            .collect();
        assert_eq!(
            at,
            vec![
                "Label \"Court\" is also the label of ex:Court2",
                "Label \"court\" is also the label of ex:Court",
                // An untyped subClassOf subject is a class.
                "Label \"Pump\" is also the label of ex:Pump2",
                "Label \"Pump\" is also the label of ex:Pump",
                // A term of several kinds collides with any.
                "Label \"Valve\" is also the label of ex:valve",
                "Label \"Valve\" is also the label of ex:Valve",
            ]
        );
    }

    #[test]
    fn an_external_type_does_not_change_a_term_kind() {
        // The workspace makes B a class; an import calling it a property too
        // would make its kind unknown, and so collide with the property A.
        let files = [(
            "a.ttl",
            "ex:A a owl:ObjectProperty ; rdfs:label \"Pump\" .\n\
             ex:B a owl:Class ; rdfs:label \"Pump\" .\n",
        )];
        let mut ws = workspace(&files);
        let ext = format!("{HEADER}ex:B a owl:ObjectProperty .\n");
        let ext = Analysis::new("import:x".into(), &ext, Arc::default(), false);
        ws.knowledge_mut().add_external("import:x", Arc::new(ext));
        assert_eq!(reported(&mut ws, "a.ttl", &settings(&[], &[])), vec![]);
    }

    /// 20,000 classes with one label in one document: each literal's other
    /// terms come from one list per label and kind, not a copy of it (which
    /// took about a second).
    #[cfg(not(debug_assertions))]
    #[test]
    fn timing_one_label_shared_by_20000_terms_is_not_quadratic() {
        let body: String = (0..20_000)
            .map(|i| format!("ex:T{i} a owl:Class ; rdfs:label \"Pump\" .\n"))
            .collect();
        let mut ws = workspace(&[("a.ttl", &body)]);
        let a = ws.analysis("file:///w/a.ttl").unwrap();
        let k = ws.knowledge();
        let start = std::time::Instant::now();
        let found = diagnostics(&a, k, Encoding::Utf16, false);
        let took = start.elapsed();
        assert_eq!(found.len(), 20_000);
        let first = &found[0].message;
        assert!(first.ends_with(" and 19996 more"), "{first}");
        assert!(took.as_millis() < 200, "took {took:?}");
    }

    #[test]
    fn the_message_names_a_few_terms_then_counts_the_rest() {
        let mut body = String::from("ex:A a owl:Class ; rdfs:label \"Pump\" .\n");
        for t in ["B", "C", "D", "E", "F"] {
            body += &format!("ex:{t} a owl:Class ; rdfs:label \"Pump\" .\n");
        }
        let found = by_default(&[("a.ttl", &body)], "a.ttl");
        assert_eq!(found.len(), 6);
        assert_eq!(
            found[0].2,
            "Label \"Pump\" is also the label of ex:B, ex:C, ex:D and 2 more"
        );
        let found = by_default(
            &[(
                "a.ttl",
                "ex:A a owl:Class ; rdfs:label \"Pump\" .\n\
                 <http://other.org/x> a owl:Class ; rdfs:label \"Pump\" .\n\
                 ex:B a owl:Class ; rdfs:label \"Pump\" .\n",
            )],
            "a.ttl",
        );
        assert_eq!(
            found[0].2,
            "Label \"Pump\" is also the label of ex:B and <http://other.org/x>"
        );
    }

    #[test]
    fn multiple_labels_count_per_language_and_per_property() {
        let files = [(
            "a.ttl",
            "ex:A a owl:Class ; rdfs:label \"Pump\"@en, \"Pumpe\"@de ; skos:prefLabel \"Pumping\"@en .\n\
             ex:B a owl:Class ; rdfs:label \"Valve\"@en, \"Tap\"@en, \"Ventil\"@de .\n\
             ex:C a owl:Class ; skos:prefLabel \"One\", \"Two\" .\n\
             ex:i rdfs:label \"x\", \"y\" .\n",
        )];
        assert_eq!(
            by_default(&files, "a.ttl"),
            vec![
                row(
                    MULTIPLE_LABELS,
                    "\"Valve\"",
                    "ex:B has 2 rdfs:label values tagged @en"
                ),
                row(
                    MULTIPLE_LABELS,
                    "\"Tap\"",
                    "ex:B has 2 rdfs:label values tagged @en"
                ),
                row(
                    MULTIPLE_LABELS,
                    "\"One\"",
                    "ex:C has 2 skos:prefLabel values without a language tag"
                ),
                row(
                    MULTIPLE_LABELS,
                    "\"Two\"",
                    "ex:C has 2 skos:prefLabel values without a language tag"
                ),
            ]
        );
    }

    #[test]
    fn house_style_checks_are_off_until_enabled_and_ignore_wins() {
        let body = "<http://example.org/onto> a owl:Ontology .\n\
                    ex:A a owl:Class .\n\
                    ex:B a owl:Class ; rdfs:label \"B\" ; skos:definition \"a thing.\", \"Other.\" .\n\
                    ex:C a owl:Class ; rdfs:label \"C\" ; skos:definition \"A thing.\" .\n";
        let files = [("a.ttl", body)];
        assert_eq!(by_default(&files, "a.ttl"), vec![]);
        let all = reported(&mut workspace(&files), "a.ttl", &settings(OPT_IN, &[]));
        assert_eq!(
            all,
            vec![
                row(
                    DUPLICATE_DEFINITION,
                    "\"a thing.\"",
                    "Another term has this definition: ex:C"
                ),
                row(
                    LOWERCASE_DEFINITION,
                    "\"a thing.\"",
                    "Definition starts with a lowercase letter"
                ),
                row(
                    DUPLICATE_DEFINITION,
                    "\"A thing.\"",
                    "Another term has this definition: ex:B"
                ),
                row(
                    MULTIPLE_DEFINITIONS,
                    "\"a thing.\"",
                    "ex:B has 2 skos:definition values without a language tag"
                ),
                row(
                    MULTIPLE_DEFINITIONS,
                    "\"Other.\"",
                    "ex:B has 2 skos:definition values without a language tag"
                ),
                row(
                    MISSING_LABEL,
                    "ex:A",
                    "ex:A has no rdfs:label or skos:prefLabel"
                ),
                row(
                    MISSING_DEFINITION,
                    "ex:A",
                    "ex:A has no skos:definition or rdfs:comment"
                ),
                row(
                    MISSING_ONTOLOGY_TITLE,
                    "<http://example.org/onto>",
                    "Ontology ex:onto has no title (dcterms:title, dc:title or rdfs:label)"
                ),
                row(
                    MISSING_ONTOLOGY_DESCRIPTION,
                    "<http://example.org/onto>",
                    "Ontology ex:onto has no description (dcterms:description, dc:description or rdfs:comment)"
                ),
                row(
                    MISSING_ONTOLOGY_LICENSE,
                    "<http://example.org/onto>",
                    "Ontology ex:onto has no licence (dcterms:license or cc:license)"
                ),
            ]
        );
        // Each code alone, and `lint.ignore` over `lint.enable`.
        for code in OPT_IN {
            let one = reported(&mut workspace(&files), "a.ttl", &settings(&[code], &[]));
            assert!(!one.is_empty(), "{code}");
            assert!(one.iter().all(|r| r.0 == *code), "{code}: {one:?}");
            let ignored = reported(&mut workspace(&files), "a.ttl", &settings(&[code], &[code]));
            assert_eq!(ignored, vec![], "{code}");
        }
    }

    #[test]
    fn a_comment_is_a_definition_only_without_a_skos_definition() {
        let enable = settings(&[MISSING_DEFINITION, LOWERCASE_DEFINITION], &[]);
        let files = [
            (
                "a.ttl",
                "ex:A a owl:Class ; rdfs:label \"A\" ; rdfs:comment \"moves water.\" .\n\
                 ex:B a owl:Class ; rdfs:label \"B\" ; rdfs:comment \"lowercase note.\" .\n",
            ),
            ("b.ttl", "ex:B skos:definition \"Defined.\" .\n"),
        ];
        assert_eq!(
            reported(&mut workspace(&files), "a.ttl", &enable),
            vec![row(
                LOWERCASE_DEFINITION,
                "\"moves water.\"",
                "Definition starts with a lowercase letter"
            )]
        );
    }

    #[test]
    fn a_missing_check_reads_every_workspace_document_and_reports_once() {
        let enable = settings(&[MISSING_LABEL, MISSING_DEFINITION], &[]);
        // B is labelled and defined in another file; C is described in both
        // and labelled in neither: reported where it is declared.
        let files = [
            ("z-core.ttl", "ex:B a owl:Class .\nex:C a owl:Class .\n"),
            (
                "a-labels.ttl",
                "ex:B rdfs:label \"B\" ; rdfs:comment \"A b.\" .\nex:C rdfs:comment \"A c.\" .\n",
            ),
        ];
        let mut ws = workspace(&files);
        assert_eq!(
            reported(&mut ws, "z-core.ttl", &enable),
            vec![row(
                MISSING_LABEL,
                "ex:C",
                "ex:C has no rdfs:label or skos:prefLabel"
            )]
        );
        assert_eq!(reported(&mut ws, "a-labels.ttl", &enable), vec![]);
        // Declared in two documents: the first by URI reports.
        let files = [
            ("b.ttl", "ex:D a owl:Class .\n"),
            ("a.ttl", "ex:D rdfs:subClassOf ex:E .\n"),
        ];
        let mut ws = workspace(&files);
        let enable = settings(&[MISSING_LABEL], &[]);
        assert_eq!(reported(&mut ws, "b.ttl", &enable), vec![]);
        assert_eq!(
            reported(&mut ws, "a.ttl", &enable),
            vec![row(
                MISSING_LABEL,
                "ex:D",
                "ex:D has no rdfs:label or skos:prefLabel"
            )]
        );
    }

    #[test]
    fn the_header_accepts_each_alternative_property() {
        let enable = settings(
            &[
                MISSING_ONTOLOGY_TITLE,
                MISSING_ONTOLOGY_DESCRIPTION,
                MISSING_ONTOLOGY_LICENSE,
            ],
            &[],
        );
        let headers = [
            "dcterms:title \"T\" ; dcterms:description \"D\" ; dcterms:license <http://l> .",
            "dc:title \"T\" ; dc:description \"D\" ; <http://creativecommons.org/ns#license> <http://l> .",
            "rdfs:label \"T\" ; rdfs:comment \"D\" ; <http://web.resource.org/cc/license> <http://l> .",
        ];
        for h in headers {
            let body = format!("ex:onto a owl:Ontology ; {h}\n");
            let found = reported(&mut workspace(&[("a.ttl", &body)]), "a.ttl", &enable);
            assert_eq!(found, vec![], "{h}");
        }
        // A header split over two documents: the one that types it reports.
        let files = [
            ("a.ttl", "ex:onto dcterms:title \"T\" .\n"),
            (
                "b.ttl",
                "ex:onto a owl:Ontology ; dcterms:description \"D\" .\n",
            ),
        ];
        let mut ws = workspace(&files);
        assert_eq!(reported(&mut ws, "a.ttl", &enable), vec![]);
        let found = reported(&mut ws, "b.ttl", &enable);
        let codes: Vec<&str> = found.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(codes, vec![MISSING_ONTOLOGY_LICENSE]);
    }

    #[test]
    fn json_ld_labels_are_compared_too() {
        let mut ws = workspace(&[("a.ttl", "ex:A a owl:Class ; rdfs:label \"Pump\" .\n")]);
        let json = r#"{
  "@context": { "rdfs": "http://www.w3.org/2000/01/rdf-schema#", "owl": "http://www.w3.org/2002/07/owl#", "ex": "http://example.org/" },
  "@id": "ex:B", "@type": "owl:Class",
  "rdfs:label": "pump"
}"#;
        ws.open_doc("file:///w/b.jsonld".into(), json.into(), 1);
        assert_eq!(
            reported(&mut ws, "b.jsonld", &settings(&[], &[])),
            vec![row(
                DUPLICATE_LABEL,
                "\"pump\"",
                "Label \"pump\" is also the label of ex:A"
            )]
        );
    }
}
