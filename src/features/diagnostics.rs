//! The diagnostics of one RDF document, check by check ([`Check`], each
//! with what it reads), in one order. An open document keeps the findings
//! of some between edits ([`Kept`]). The quick fixes for them are in
//! [`super::quickfix`], which also owns the data a diagnostic carries for its
//! fix; [`crate::diagnose`] composes these with the other sources.

use super::kept::Kept;
use super::quickfix::{FixData, PrefixCandidate, add_prefix_note, prefix_candidates};
use crate::analysis::{Analysis, Encoding};
use crate::jsonld;
use crate::knowledge::Knowledge;
use crate::syntax::lexer::Kind;
use crate::syntax::{Family, Form, Iri, Ref, Role, Severity, Span, Syntax, SyntaxError};
use crate::vocab::VOCABS;
use crate::workspace::{PREFIX_FILE, file_uri_name};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::BorrowMut;

pub(crate) const SOURCE: &str = "turtle";
/// The code of a use of a deprecated term.
const DEPRECATED: &str = "deprecated-term";

/// Codes of diagnostics that claim something is known nowhere in the
/// workspace, so are only reported once the initial index is in.
pub(crate) const NEEDS_INDEX: &[&str] = crate::features::shacl_lint::NEEDS_INDEX;

/// The curation checks that say a term or ontology lacks something any
/// workspace document could give it: they wait for the index too.
const CURATION_NEEDS_INDEX: &[&str] = &[
    crate::features::curation::MISSING_LABEL,
    crate::features::curation::MISSING_DEFINITION,
    crate::features::curation::MISSING_ONTOLOGY_TITLE,
    crate::features::curation::MISSING_ONTOLOGY_DESCRIPTION,
    crate::features::curation::MISSING_ONTOLOGY_LICENSE,
];

pub(crate) fn needs_index(d: &Diagnostic) -> bool {
    matches!(&d.code, Some(NumberOrString::String(c))
        if NEEDS_INDEX.contains(&c.as_str())
            || CURATION_NEEDS_INDEX.contains(&c.as_str())
            || c == crate::features::lint::CODE)
}

/// One written occurrence of an IRI (never `a`), for the knowledge lints
/// RDF and SPARQL documents share. `K` is how the document holds IRIs: an
/// interned [`crate::syntax::Iri`], or a `&str`.
pub(crate) struct TermUse<'a, K> {
    pub(crate) span: crate::syntax::Span,
    pub(crate) iri: K,
    /// The prefix as written, for a prefixed name (`foaf` in `foaf:name`).
    pub(crate) prefix: Option<&'a str>,
    /// JSON-LD: the IRI is written in full in a string, as this text (an
    /// absolute IRI, or a word `@vocab` or the base completes), so a fix
    /// keeps that spelling, without angle brackets.
    pub(crate) bare: Option<&'a str>,
}

/// `unknown-term` (warning, with a "did you mean" and `data.replace` in the
/// document's own spelling) and `deprecated-term` (hint, deprecated tag) for
/// terms of the well-known vocabularies, matched by namespace IRI. `mk`
/// builds a diagnostic at a span (or a [`Found`]). No hint where `warned`,
/// the spans [`own_deprecations`] reported, has the use's span: that
/// warning says it.
pub(crate) fn vocabulary_terms<'a, K, D>(
    uses: impl Iterator<Item = TermUse<'a, K>>,
    k: &Knowledge,
    warned: &FxHashSet<crate::syntax::Span>,
    mk: &impl Fn(crate::syntax::Span, DiagnosticSeverity, &str, String) -> D,
    out: &mut Vec<D>,
) where
    K: std::ops::Deref<Target = str> + crate::syntax::IriKey,
    D: BorrowMut<Diagnostic>,
{
    // Suggestions are memoised: a file can use one unknown term thousands of times.
    let mut suggestions: FxHashMap<(&str, String), Option<&str>> = FxHashMap::default();
    for u in uses {
        let Some((v, local)) = VOCABS.lookup(&u.iri) else {
            continue;
        };
        if local.is_empty() {
            continue;
        }
        match v.term(local) {
            // A workspace deprecation too: [`own_deprecations`] warns instead.
            Some(t) if t.deprecated && warned.contains(&u.span) => {}
            Some(t) if t.deprecated => {
                let mut d = mk(
                    u.span,
                    DiagnosticSeverity::HINT,
                    DEPRECATED,
                    format!("'{}:{local}' is deprecated", v.prefix),
                );
                d.borrow_mut().tags = Some(vec![DiagnosticTag::DEPRECATED]);
                out.push(d);
            }
            Some(_) => {}
            // Described by the workspace or a resolved import: newer than the snapshot.
            None if k.is_described(&u.iri) => {}
            None => {
                let mut msg = format!(
                    "'{local}' is not defined in the {} vocabulary <{}>",
                    v.prefix, v.namespace
                );
                let mut d = mk(
                    u.span,
                    DiagnosticSeverity::WARNING,
                    "unknown-term",
                    String::new(),
                );
                let suggestion = *suggestions
                    .entry((v.namespace, local.to_string()))
                    .or_insert_with(|| v.suggest(local).map(|t| t.local.as_str()));
                if let Some(s) = suggestion {
                    msg.push_str(&format!(". Did you mean '{s}'?"));
                    let replacement = match (u.prefix, u.bare) {
                        (Some(p), _) => format!("{p}:{s}"),
                        (None, Some(written)) => bare_replacement(&u.iri, written, v.namespace, s),
                        (None, None) => format!("<{}{s}>", v.namespace),
                    };
                    FixData::Replace {
                        replace: replacement,
                    }
                    .attach(d.borrow_mut());
                }
                d.borrow_mut().message = msg;
                out.push(d);
            }
        }
    }
}

/// `deprecated-term` (warning, deprecated tag) for each use of a term that
/// a workspace file marks `owl:deprecated true`
/// ([`Knowledge::deprecated_here`]), named as written in `text`. The caller
/// leaves out subjects: they describe the term. Needs no index wait: an
/// unindexed file can only hide a deprecation, never invent one. Returns
/// the spans it reported, for [`vocabulary_terms`].
pub(crate) fn own_deprecations<'a, K, D>(
    uses: impl Iterator<Item = TermUse<'a, K>>,
    text: &str,
    k: &Knowledge,
    mk: &impl Fn(crate::syntax::Span, DiagnosticSeverity, &str, String) -> D,
    out: &mut Vec<D>,
) -> FxHashSet<crate::syntax::Span>
where
    K: crate::syntax::IriKey,
    D: BorrowMut<Diagnostic>,
{
    let mut warned = FxHashSet::default();
    if !k.has_deprecations() {
        return warned;
    }
    for u in uses.filter(|u| k.deprecated_here(&u.iri)) {
        warned.insert(u.span);
        let written = text
            .get(u.span.start as usize..u.span.end as usize)
            .unwrap_or_default();
        let mut d = mk(
            u.span,
            DiagnosticSeverity::WARNING,
            DEPRECATED,
            format!("'{written}' is deprecated: the workspace marks it owl:deprecated"),
        );
        d.borrow_mut().tags = Some(vec![DiagnosticTag::DEPRECATED]);
        out.push(d);
    }
    warned
}

/// What replaces `written`, a JSON-LD string that spells `iri` in full or
/// as its end (a `@vocab` word), so it spells `namespace` + `local`: the
/// same beginning stays implied, else the full IRI.
fn bare_replacement(iri: &str, written: &str, namespace: &str, local: &str) -> String {
    let target = format!("{namespace}{local}");
    iri.strip_suffix(written)
        .and_then(|implied| target.strip_prefix(implied))
        .filter(|r| !r.is_empty())
        .map_or_else(|| target.clone(), str::to_string)
}

/// How many syntax errors and warnings of a Turtle, TriG, N-Triples or
/// N-Quads document the editor shows. JSON-LD has its own cap
/// ([`crate::jsonld::json::MAX_ERRORS`]).
pub(crate) const MAX_SYNTAX_ERRORS: usize = 1000;

/// Which optional diagnostics to compute (from the client's settings).
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub lint_undescribed_terms: bool,
    /// Some off-by-default curation check is enabled
    /// ([`crate::features::curation::OPT_IN`]); without one they are not
    /// computed. Which codes are reported is decided by the composer.
    pub house_style: bool,
    /// At most this many syntax errors and warnings of a Turtle-family
    /// document: past it, the ones that start first in the text (the first
    /// found, on a tie), in the order found, and one error that says there
    /// are more, empty and at the end of the last one shown. None shows them
    /// all, as `turtle-lsp check` does. The editor's default is 1,000
    /// (`MAX_SYNTAX_ERRORS`).
    pub max_syntax_errors: Option<usize>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            lint_undescribed_terms: true,
            house_style: false,
            max_syntax_errors: Some(MAX_SYNTAX_ERRORS),
        }
    }
}

impl Options {
    pub(crate) fn from_settings(s: &crate::settings::Settings) -> Options {
        Options {
            lint_undescribed_terms: s.lint_on(crate::features::lint::CODE),
            house_style: crate::features::curation::OPT_IN
                .iter()
                .any(|c| s.lint_on(c)),
            ..Options::default()
        }
    }
}

/// Which of `errors` to show when there are more than `max`: the `max` that
/// start first in the text, the first found on a tie.
fn first_in_text(errors: &[SyntaxError], max: usize) -> Vec<bool> {
    let mut order: Vec<usize> = (0..errors.len()).collect();
    let mut shown = vec![false; errors.len()];
    if max < errors.len() {
        order.select_nth_unstable_by_key(max, |&i| (errors[i].span.start, i));
        order.truncate(max);
    }
    for i in order {
        shown[i] = true;
    }
    shown
}

/// [`diagnostics_with`] with default options.
pub fn diagnostics(a: &Analysis, k: &Knowledge, enc: Encoding) -> Vec<Diagnostic> {
    diagnostics_with(a, k, enc, &Options::default())
}

/// What a check reads besides the statement it reports in: what can change
/// its findings, so when a finding kept from before an edit still holds
/// ([`Kept`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reads {
    /// Its statement alone, with the prefixes in effect, which a splice
    /// never changes (a directive in its window makes it fall back).
    Statement,
    /// The statements about the same subject, and what the index says of
    /// that subject.
    Subject,
    /// Its statement, and what the knowledge index says of the IRIs and
    /// texts written there: a change of that is listed in the index's
    /// change set ([`crate::knowledge::Changes`]).
    Index,
    /// The whole document.
    Document,
    /// The workspace beyond the change set: facts read from other
    /// documents' analyses (their types, super classes, labels by IRI), or
    /// a budget shared by the whole document. Any change can change them.
    Workspace,
}

/// The checks of an RDF document, in the order the composer gives their
/// findings ([`Check::ALL`]), each with what it reads ([`Check::reads`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Check {
    /// The parse's errors and warnings: `syntax` and the parser's own codes.
    Syntax,
    /// `undeclared-prefix`, whose fix offers the namespaces the index binds
    /// the prefix to.
    UndeclaredPrefix,
    /// `duplicate-prefix` and `redefined-prefix`.
    PrefixDeclarations,
    /// `unused-prefix`.
    UnusedPrefix,
    /// JSON-LD's own: what processing dropped, and undefined prefixes.
    JsonLd,
    /// `unknown-term` and `deprecated-term`: the terms of well-known
    /// vocabularies, then the uses of terms the workspace deprecates.
    TermUses,
    /// `invalid-literal`.
    Literals,
    /// The SHACL shape lint ([`crate::features::shacl_lint`]).
    ShaclLint,
    /// `undescribed-term`, which shares one budget of suggestions.
    UndescribedTerms,
    /// `label-whitespace`.
    LabelWhitespace,
    /// `duplicate-label`.
    DuplicateLabel,
    /// `multiple-labels`.
    MultipleLabels,
    /// The opt-in curation checks ([`crate::features::curation::OPT_IN`]).
    HouseStyle,
}

impl Check {
    pub(crate) const ALL: [Check; 13] = [
        Check::Syntax,
        Check::UndeclaredPrefix,
        Check::PrefixDeclarations,
        Check::UnusedPrefix,
        Check::JsonLd,
        Check::TermUses,
        Check::Literals,
        Check::ShaclLint,
        Check::UndescribedTerms,
        Check::LabelWhitespace,
        Check::DuplicateLabel,
        Check::MultipleLabels,
        Check::HouseStyle,
    ];

    pub(crate) fn reads(self) -> Reads {
        match self {
            Check::Syntax | Check::Literals | Check::LabelWhitespace => Reads::Statement,
            Check::MultipleLabels => Reads::Subject,
            Check::UndeclaredPrefix | Check::TermUses | Check::DuplicateLabel => Reads::Index,
            Check::PrefixDeclarations | Check::UnusedPrefix | Check::JsonLd => Reads::Document,
            Check::ShaclLint | Check::UndescribedTerms | Check::HouseStyle => Reads::Workspace,
        }
    }

    /// Whether an open document keeps the findings between edits
    /// ([`Kept`]). Kept: the checks that cost the most on a keystroke in a
    /// big vocabulary (the label checks and the term uses were 80% of the
    /// composition, profiled on 17,200 classes), and `invalid-literal`,
    /// which reads its statement alone. The rest run in full each time:
    /// the parse keeps the syntax errors per statement already, and the
    /// others cost little or read more than the change set says. Only what
    /// an edit's window or the index's change set can tell has changed is
    /// kept: never a check that reads the document or the workspace.
    pub(crate) fn kept(self) -> bool {
        let followed = matches!(
            self.reads(),
            Reads::Statement | Reads::Subject | Reads::Index
        );
        followed && self.costly()
    }

    /// Whether the check runs on a document of `syntax`.
    pub(crate) fn runs_on(self, syntax: Syntax) -> bool {
        match syntax.family() {
            Family::Turtle | Family::JsonLd => true,
            // Read-only, as RDF/XML: the same checks.
            Family::ReadOnly => self.runs_read_only(),
        }
    }

    /// Whether the check runs on an RDF/XML, OWL Functional or OWL/XML
    /// document, which no fix edits. Its syntax errors do (`xml-syntax`,
    /// `rdfxml-syntax`, `unsupported-syntax`, `ofn-syntax`, `owx-syntax`); SHACL results
    /// and reasoning findings come from outside these checks. Of the lints,
    /// those of its terms and literals and the curation checks, at the
    /// exact spans of its references and literals; none offers a fix.
    fn runs_read_only(self) -> bool {
        match self {
            Check::Syntax => true,
            // `invalid-literal`, `undescribed-term` and the term uses
            // (`unknown-term`, `deprecated-term`; [`term_uses`]), each
            // suggestion in the message only.
            Check::Literals | Check::UndescribedTerms | Check::TermUses => true,
            // The label checks and house style, which read the parsed
            // values of the literals: `label-whitespace` with no string to
            // trim ([`crate::features::curation::whitespace`]).
            Check::LabelWhitespace
            | Check::DuplicateLabel
            | Check::MultipleLabels
            | Check::HouseStyle => true,
            // Lints of Turtle prefixes or a JSON-LD context, which these
            // syntaxes have not, and the shape lint.
            Check::UndeclaredPrefix
            | Check::PrefixDeclarations
            | Check::UnusedPrefix
            | Check::JsonLd
            | Check::ShaclLint => false,
        }
    }

    /// The checks [`Check::kept`] picks by their cost.
    fn costly(self) -> bool {
        match self {
            Check::TermUses
            | Check::Literals
            | Check::LabelWhitespace
            | Check::DuplicateLabel
            | Check::MultipleLabels => true,
            Check::Syntax
            | Check::UndeclaredPrefix
            | Check::PrefixDeclarations
            | Check::UnusedPrefix
            | Check::JsonLd
            | Check::ShaclLint
            | Check::UndescribedTerms
            | Check::HouseStyle => false,
        }
    }
}

/// A finding at a span of the document: the diagnostic without its range,
/// which [`Found::diagnostic`] fills in when the editor asks. Kept findings
/// move with their statement ([`Kept`]); ranges would not.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    pub(crate) span: Span,
    pub(crate) diagnostic: Diagnostic,
}

impl Found {
    pub(crate) fn new(
        span: Span,
        severity: DiagnosticSeverity,
        code: &str,
        message: String,
    ) -> Found {
        Found {
            span,
            diagnostic: Diagnostic {
                severity: Some(severity),
                code: Some(NumberOrString::String(code.to_string())),
                source: Some(SOURCE.to_string()),
                message,
                ..Default::default()
            },
        }
    }

    /// The diagnostic, at its span of `a` in `enc` columns.
    pub(crate) fn diagnostic(&self, a: &Analysis, enc: Encoding) -> Diagnostic {
        self.clone().into_diagnostic(a, enc)
    }

    /// [`Found::diagnostic`], without a copy.
    pub(crate) fn into_diagnostic(self, a: &Analysis, enc: Encoding) -> Diagnostic {
        Diagnostic {
            range: a.range(enc, self.span),
            ..self.diagnostic
        }
    }
}

impl std::borrow::Borrow<Diagnostic> for Found {
    fn borrow(&self) -> &Diagnostic {
        &self.diagnostic
    }
}

impl std::borrow::BorrowMut<Diagnostic> for Found {
    fn borrow_mut(&mut self) -> &mut Diagnostic {
        &mut self.diagnostic
    }
}

pub fn diagnostics_with(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    opts: &Options,
) -> Vec<Diagnostic> {
    diagnostics_kept(a, k, enc, opts, &Kept::of(a, k))
}

/// [`diagnostics_with`], with the findings of the [`Check::kept`] checks
/// from `kept`, which must be up to date with `a` and `k`. The checks that
/// do not run on `a`'s syntax ([`Check::runs_on`]) give nothing.
pub(crate) fn diagnostics_kept(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    opts: &Options,
    kept: &Kept,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mk = |span, severity, code: &str, message: String| {
        Found::new(span, severity, code, message).into_diagnostic(a, enc)
    };
    for check in Check::ALL {
        if !check.runs_on(a.parsed.syntax) {
            continue;
        }
        if check.kept() {
            out.extend(kept.findings(check).map(|f| f.diagnostic(a, enc)));
            continue;
        }
        match check {
            Check::Syntax => syntax_errors(a, opts, &mk, &mut out),
            Check::UndeclaredPrefix => undeclared_prefixes(a, k, &mk, &mut out),
            Check::PrefixDeclarations => prefix_declarations(a, &mk, &mut out),
            Check::UnusedPrefix => unused_prefixes(a, &mk, &mut out),
            Check::JsonLd => match a.parsed.syntax {
                Syntax::JsonLd => jsonld_lints(a, k, &mk, &mut out),
                Syntax::Turtle
                | Syntax::TriG
                | Syntax::NTriples
                | Syntax::NQuads
                | Syntax::RdfXml
                | Syntax::OwlFunctional
                | Syntax::OwlXml => {}
            },
            Check::ShaclLint => {
                for f in crate::features::shacl_lint::lint(a, k) {
                    let sev = if f.error {
                        DiagnosticSeverity::ERROR
                    } else {
                        DiagnosticSeverity::WARNING
                    };
                    out.push(mk(f.span, sev, f.code, f.message));
                }
            }
            Check::UndescribedTerms if opts.lint_undescribed_terms => {
                out.extend(crate::features::lint::undescribed_terms(a, k, enc));
            }
            Check::HouseStyle if opts.house_style => {
                out.extend(crate::features::curation::house_style(a, k, enc));
            }
            Check::UndescribedTerms | Check::HouseStyle => {}
            Check::TermUses
            | Check::Literals
            | Check::LabelWhitespace
            | Check::DuplicateLabel
            | Check::MultipleLabels => unreachable!("kept: {check:?}"),
        }
    }
    out
}

fn severity_of(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    }
}

/// The parse's errors and warnings, at most [`Options::max_syntax_errors`].
fn syntax_errors(a: &Analysis, opts: &Options, mk: &Mk, out: &mut Vec<Diagnostic>) {
    let errors = &a.parsed.errors;
    // JSON-LD's errors are not capped.
    let capped = match a.parsed.syntax {
        Syntax::JsonLd => false,
        Syntax::Turtle
        | Syntax::TriG
        | Syntax::NTriples
        | Syntax::NQuads
        | Syntax::RdfXml
        | Syntax::OwlFunctional
        | Syntax::OwlXml => true,
    };
    let cap = opts
        .max_syntax_errors
        .filter(|&max| errors.len() > max && capped);
    let shown = cap.map(|max| first_in_text(errors, max));
    let mut last_end = 0;
    for (i, e) in errors.iter().enumerate() {
        if shown.as_ref().is_some_and(|s| !s[i]) {
            continue;
        }
        last_end = last_end.max(e.span.end);
        out.push(mk(
            e.span,
            severity_of(e.severity),
            e.code,
            e.message().to_string(),
        ));
    }
    if let Some(max) = cap {
        // A Functional document's errors have their own code, which hides
        // this one too.
        let code = match a.parsed.syntax {
            Syntax::OwlFunctional => crate::ofn::CODE,
            Syntax::OwlXml => crate::owx::CODE,
            Syntax::Turtle
            | Syntax::TriG
            | Syntax::NTriples
            | Syntax::NQuads
            | Syntax::JsonLd
            | Syntax::RdfXml => "syntax",
        };
        out.push(mk(
            Span::new(last_end, last_end),
            DiagnosticSeverity::ERROR,
            code,
            format!("More than {max} syntax errors; the rest are not shown"),
        ));
    }
}

/// How the composer makes a diagnostic at a span.
type Mk<'m> = dyn Fn(Span, DiagnosticSeverity, &str, String) -> Diagnostic + 'm;

/// Undeclared prefixes, with "add @prefix" fixes for every known namespace.
fn undeclared_prefixes(a: &Analysis, k: &Knowledge, mk: &Mk, out: &mut Vec<Diagnostic>) {
    // N-Triples and N-Quads have no prefixes at all; the parser already
    // reports the pname.
    if !a.parsed.syntax.has_prefixes() {
        return;
    }
    let mut candidates: FxHashMap<&str, Vec<PrefixCandidate>> = FxHashMap::default();
    for u in a.parsed.prefix_uses.iter().filter(|u| !u.resolved) {
        let shown = if u.name.is_empty() {
            ":".to_string()
        } else {
            format!("{}:", u.name)
        };
        let mut d = mk(
            u.span,
            DiagnosticSeverity::ERROR,
            "undeclared-prefix",
            format!("Undeclared prefix '{shown}'"),
        );
        let cands = candidates
            .entry(u.name.as_str())
            .or_insert_with(|| prefix_candidates(&u.name, k));
        add_prefix_note(&mut d, &u.name, cands, "@prefix");
        out.push(d);
    }
}

/// Duplicate and redefined declarations.
fn prefix_declarations(a: &Analysis, mk: &Mk, out: &mut Vec<Diagnostic>) {
    let mut seen: FxHashMap<&str, &str> = FxHashMap::default();
    for p in &a.parsed.prefixes {
        if let Some(prev) = seen.insert(&p.name, &p.iri) {
            if prev == p.iri {
                let mut d = mk(
                    p.span,
                    DiagnosticSeverity::HINT,
                    "duplicate-prefix",
                    format!("Prefix '{}:' is declared twice", p.name),
                );
                d.tags = Some(vec![DiagnosticTag::UNNECESSARY]);
                out.push(d);
            } else {
                out.push(mk(
                    p.name_span,
                    DiagnosticSeverity::WARNING,
                    "redefined-prefix",
                    format!("Prefix '{}:' is redefined (was <{prev}>)", p.name),
                ));
            }
        }
    }
}

/// Declarations no prefixed name uses, except in a prefixes.ttl.
fn unused_prefixes(a: &Analysis, mk: &Mk, out: &mut Vec<Diagnostic>) {
    if file_uri_name(&a.uri).as_deref() == Some(PREFIX_FILE) {
        return;
    }
    let declared: FxHashSet<&str> = a.parsed.prefixes.iter().map(|p| p.name.as_str()).collect();
    if declared.is_empty() {
        return;
    }
    // Most documents use every prefix they declare, and early: stop at
    // the use that leaves none unused.
    let mut used: FxHashSet<&str> = FxHashSet::default();
    for u in &a.parsed.prefix_uses {
        if declared.contains(u.name.as_str())
            && used.insert(u.name.as_str())
            && used.len() == declared.len()
        {
            return;
        }
    }
    // A statement skipped by error recovery records no uses; its prefixed
    // names still keep their prefix. Without a syntax error nothing is
    // skipped, so the recorded uses are all of them.
    if a.parsed
        .errors
        .iter()
        .any(|e| e.severity == Severity::Error)
    {
        let tokens = a.tokens_or_lexed();
        let names: FxHashSet<u32> = a
            .parsed
            .prefixes
            .iter()
            .map(|p| p.name_span.start)
            .collect();
        used.extend(
            tokens
                .iter()
                .filter(|t| t.kind == Kind::PName && !names.contains(&t.start))
                .filter_map(|t| a.text[t.start as usize..t.end as usize].split(':').next()),
        );
    }
    for p in a
        .parsed
        .prefixes
        .iter()
        .filter(|p| !used.contains(p.name.as_str()))
    {
        let mut d = mk(
            p.span,
            DiagnosticSeverity::HINT,
            "unused-prefix",
            format!("Prefix '{}:' is never used", p.name),
        );
        d.tags = Some(vec![DiagnosticTag::UNNECESSARY]);
        out.push(d);
    }
}

/// What JSON-LD processing dropped, and `rdfs:lable` with no `rdfs` in the
/// context, for a prefix the workspace knows: an IRI with scheme `rdfs:`.
fn jsonld_lints(a: &Analysis, k: &Knowledge, mk: &Mk, out: &mut Vec<Diagnostic>) {
    for f in jsonld::lint::dropped(&a.text, &a.dropped) {
        out.push(mk(f.span, severity_of(f.severity), f.code, f.message));
    }
    let mut namespaces: FxHashMap<&str, Option<String>> = FxHashMap::default();
    for (span, word) in jsonld::lint::undefined_prefixes(&a.text, &a.source_index) {
        let ns = namespaces.entry(word).or_insert_with(|| {
            prefix_candidates(word, k)
                .into_iter()
                .next()
                .map(|c| c.namespace)
        });
        let Some(ns) = ns else { continue };
        let written = text_at(a, span, span.end - span.start).unwrap_or_default();
        let mut d = mk(
            span,
            DiagnosticSeverity::WARNING,
            jsonld::lint::UNDEFINED_PREFIX,
            format!(
                "'{word}' is not defined in the @context, so '{written}' is an IRI with scheme '{word}:'"
            ),
        );
        FixData::UndefinedPrefix {
            prefix: word.to_string(),
            namespace: ns.clone(),
        }
        .attach(&mut d);
        out.push(d);
    }
}

/// The `len` bytes of `a`'s text from the start of `span`.
fn text_at(a: &Analysis, span: Span, len: u32) -> Option<&str> {
    a.text.get(span.start as usize..(span.start + len) as usize)
}

/// The use of the term `r` refers to, for the term checks: a prefixed name
/// with its prefix as written, or an IRI; none for `a`, nor for an anchor,
/// whose text is not the term's (a fix written there would break the file).
pub(crate) fn term_use<'a>(a: &'a Analysis, r: &Ref) -> Option<TermUse<'a, Iri>> {
    match r.form {
        Form::A | Form::Anchor => None,
        Form::PName { prefix_len } => Some(TermUse {
            span: r.span,
            iri: r.iri,
            prefix: text_at(a, r.span, prefix_len),
            bare: None,
        }),
        Form::Iri => Some(TermUse {
            span: r.span,
            iri: r.iri,
            prefix: None,
            bare: None,
        }),
    }
}

/// [`Check::TermUses`] for the references `refs` of a Turtle-family,
/// RDF/XML, OWL Functional or OWL/XML document: the findings of
/// [`vocabulary_terms`] (in a document no fix edits, without the fix data
/// of `unknown-term`), and those of [`own_deprecations`] (subjects left
/// out), each in the order of `refs`.
pub(crate) fn term_uses<'a>(
    a: &Analysis,
    k: &Knowledge,
    refs: impl Iterator<Item = &'a Ref> + Clone,
) -> (Vec<Found>, Vec<Found>) {
    let mk = Found::new;
    let mut own = Vec::new();
    let subjects_left_out = refs.clone().filter(|r| !r.role.is_subject());
    let warned = own_deprecations(
        subjects_left_out.filter_map(|r| term_use(a, r)),
        &a.text,
        k,
        &mk,
        &mut own,
    );
    let mut vocab = Vec::new();
    vocabulary_terms(
        refs.filter_map(|r| term_use(a, r)),
        k,
        &warned,
        &mk,
        &mut vocab,
    );
    // Nothing edits the document: the "did you mean" stays in the message,
    // with no fix to apply.
    if !super::writes(a.parsed.syntax) {
        for f in &mut vocab {
            f.diagnostic.data = None;
        }
    }
    (vocab, own)
}

/// [`Check::TermUses`] for a JSON-LD document: a term of the context at its
/// definition, anything else as written.
pub(crate) fn jsonld_term_uses(a: &Analysis, k: &Knowledge) -> (Vec<Found>, Vec<Found>) {
    let mk = Found::new;
    let mut own = Vec::new();
    let uses = a.parsed.refs.iter().filter_map(|r| {
        (r.role != Role::Subject && !matches!(r.form, Form::A)).then_some(TermUse {
            span: r.span,
            iri: r.iri,
            prefix: None,
            bare: None,
        })
    });
    let warned = own_deprecations(uses, &a.text, k, &mk, &mut own);
    let iris = jsonld::lint::vocabulary_iris(&a.text, &a.parsed.refs, &a.used_terms);
    let uses = iris.iter().map(|w| match w.form {
        Form::PName { prefix_len } => TermUse {
            span: w.span,
            iri: w.iri,
            prefix: text_at(a, w.span, prefix_len),
            bare: None,
        },
        _ => TermUse {
            span: w.span,
            iri: w.iri,
            prefix: None,
            bare: text_at(a, w.span, w.span.end - w.span.start),
        },
    });
    let mut vocab = Vec::new();
    vocabulary_terms(uses, k, &warned, &mk, &mut vocab);
    (vocab, own)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Each diagnostic as (code, severity, the text it covers, data.replace).
    fn jsonld(text: &str) -> Vec<(String, DiagnosticSeverity, String, Option<String>)> {
        let a = Analysis::with_syntax(
            "file:///x/a.jsonld".to_string(),
            text,
            Arc::default(),
            false,
            Syntax::JsonLd,
        );
        diagnostics(&a, &Knowledge::default(), Encoding::Utf8)
            .into_iter()
            .map(|d| {
                let start = a.offset(Encoding::Utf8, d.range.start).unwrap();
                let end = a.offset(Encoding::Utf8, d.range.end).unwrap();
                let code = match d.code {
                    Some(NumberOrString::String(c)) => c,
                    _ => String::new(),
                };
                let replace = d
                    .data
                    .and_then(|x| x["replace"].as_str().map(str::to_string));
                (
                    code,
                    d.severity.unwrap(),
                    text[start as usize..end as usize].to_string(),
                    replace,
                )
            })
            .collect()
    }

    fn d(
        code: &str,
        severity: DiagnosticSeverity,
        at: &str,
        replace: Option<&str>,
    ) -> (String, DiagnosticSeverity, String, Option<String>) {
        (
            code.to_string(),
            severity,
            at.to_string(),
            replace.map(str::to_string),
        )
    }

    #[test]
    fn jsonld_lints_at_their_positions() {
        // LINTS of test/jsonld-fixtures.mjs.
        let text = r#"{
  "@context": {
    "ex": "https://jsonld.example.org/def/",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "vcard": "http://www.w3.org/2006/vcard/ns#",
    "weight": "ex:weight",
    "title": "rdfs:lable",
    "seeAlso": { "@id": "rdfs:seeAlso", "@type": "@id" }
  },
  "@id": "ex:w11",
  "@type": "vcard:Email",
  "weight": 3,
  "wieght": 4,
  "rdfs:labl": "Widget eleven",
  "title": "Eleven",
  "seeAlso": "http://exa mple.org/x",
  "rdfs:comment": { "@value": "Hallo", "@language": "de_DE" }
}
"#;
        use DiagnosticSeverity as S;
        let mut got = jsonld(text);
        got.sort_by_key(|x| text.find(&format!("\"{}\"", x.2)));
        assert_eq!(
            got,
            [
                // On the definition, not on "title" where it is used.
                d("unknown-term", S::WARNING, "rdfs:lable", Some("rdfs:label")),
                d(DEPRECATED, S::HINT, "vcard:Email", None),
                d("jsonld-undefined-term", S::WARNING, "wieght", None),
                d("unknown-term", S::WARNING, "rdfs:labl", Some("rdfs:label")),
                d("invalid-iri", S::WARNING, "http://exa mple.org/x", None),
                d("invalid-language-tag", S::WARNING, "de_DE", None),
            ]
        );
    }

    #[test]
    fn jsonld_fixes_keep_the_spelling_written() {
        use DiagnosticSeverity as S;
        // A definition written as a compact IRI, an absolute IRI and a
        // @vocab word, each fixed in its own spelling.
        let text = r#"{
  "@context": {
    "@vocab": "http://www.w3.org/2000/01/rdf-schema#",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "a": "rdfs:labl",
    "b": {"@id": "http://www.w3.org/2000/01/rdf-schema#seeAlsoo"}
  },
  "@id": "http://ex.org/x",
  "a": "x",
  "b": "y",
  "a2": "z",
  "commnt": "w"
}"#;
        let mut got = jsonld(text);
        got.sort_by_key(|x| text.find(&x.2));
        assert_eq!(
            got,
            [
                d("unknown-term", S::WARNING, "rdfs:labl", Some("rdfs:label")),
                d(
                    "unknown-term",
                    S::WARNING,
                    "http://www.w3.org/2000/01/rdf-schema#seeAlsoo",
                    Some("http://www.w3.org/2000/01/rdf-schema#seeAlso")
                ),
                d("unknown-term", S::WARNING, "a2", None),
                d("unknown-term", S::WARNING, "commnt", Some("comment")),
            ]
        );
    }

    #[test]
    fn only_a_file_named_prefixes_ttl_may_leave_prefixes_unused() {
        let text = "@prefix a: <http://a/> .\n";
        let codes = |uri: &str| -> Vec<String> {
            let a = Analysis::new(uri.into(), text, Arc::default(), true);
            diagnostics(&a, &Knowledge::default(), Encoding::Utf8)
                .into_iter()
                .filter_map(|d| match d.code {
                    Some(NumberOrString::String(c)) => Some(c),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(codes("file:///t/myprefixes.ttl"), ["unused-prefix"]);
        assert_eq!(codes("file:///t/dir/prefixes.ttl"), Vec::<String>::new());
    }

    #[test]
    fn a_prefix_used_in_a_skipped_statement_is_used() {
        // Reduced from a chaos input: `check --fix` deleted `dct:`.
        let text = "@prefix dct: <http://purl.org/dc/terms/> .\n.ex:a) dct:title \"x\" .\n";
        for keep in [true, false] {
            let a = Analysis::new("file:///t/a.ttl".into(), text, Arc::default(), keep);
            let d = diagnostics(&a, &Knowledge::default(), Encoding::Utf8);
            assert!(
                !d.iter()
                    .any(|d| d.code == Some(NumberOrString::String("unused-prefix".into()))),
                "{d:?}"
            );
        }
    }

    /// `n` lines, each a statement that parses and one that does not.
    fn broken_lines(n: usize) -> String {
        (0..n)
            .map(|i| format!("<http://e/s{i}> <http://e/p> <http://e/o{i}> . <http://e/t{i}> <http://e/p> .\n"))
            .collect()
    }

    /// The syntax diagnostics of `text` as a Turtle file, showing at most `max`.
    fn syntax_diagnostics(text: &str, max: Option<usize>) -> (Arc<Analysis>, Vec<Diagnostic>) {
        let a = Arc::new(Analysis::new(
            "file:///x/a.ttl".into(),
            text,
            Arc::default(),
            false,
        ));
        let opts = Options {
            max_syntax_errors: max,
            ..Options::default()
        };
        let d = diagnostics_with(&a, &Knowledge::default(), Encoding::Utf8, &opts)
            .into_iter()
            .filter(|d| d.code == Some(NumberOrString::String("syntax".into())))
            .collect();
        (a, d)
    }

    #[test]
    fn the_editor_shows_the_first_syntax_errors_and_one_note() {
        let text = broken_lines(1500);
        let (a, d) = syntax_diagnostics(&text, Options::default().max_syntax_errors);
        assert_eq!(a.parsed.errors.len(), 1500);
        assert_eq!(d.len(), MAX_SYNTAX_ERRORS + 1);
        for (i, e) in d[..MAX_SYNTAX_ERRORS].iter().enumerate() {
            assert_eq!(e.range.start.line, i as u32, "{e:?}");
            assert_eq!(e.message, "Expected an object, found '.'");
        }
        let note = &d[MAX_SYNTAX_ERRORS];
        assert_eq!(
            note.message,
            "More than 1000 syntax errors; the rest are not shown"
        );
        assert_eq!(note.severity, Some(DiagnosticSeverity::ERROR));
        let end = d[MAX_SYNTAX_ERRORS - 1].range.end;
        assert_eq!(note.range, Range::new(end, end));
    }

    #[test]
    fn up_to_the_cap_every_syntax_error_is_shown() {
        let max = Some(MAX_SYNTAX_ERRORS);
        let (_, d) = syntax_diagnostics(&broken_lines(MAX_SYNTAX_ERRORS), max);
        assert_eq!(d.len(), MAX_SYNTAX_ERRORS);
        assert!(d.iter().all(|e| e.message.starts_with("Expected")));
        let (_, d) = syntax_diagnostics(&broken_lines(MAX_SYNTAX_ERRORS + 1), max);
        assert_eq!(d.len(), MAX_SYNTAX_ERRORS + 1);
        assert!(d[MAX_SYNTAX_ERRORS].message.starts_with("More than 1000"));
    }

    #[test]
    fn without_a_cap_every_syntax_error_is_shown() {
        let (_, d) = syntax_diagnostics(&broken_lines(1500), None);
        assert_eq!(d.len(), 1500);
        assert!(d.iter().all(|e| !e.message.starts_with("More than")));
    }

    #[test]
    fn the_cap_keeps_the_errors_that_come_first_in_the_text() {
        // The lexer's errors are found before the parser's; here they come
        // last in the text, so the parser's are the ones shown.
        let mut text = broken_lines(30);
        for i in 0..30 {
            text.push_str(&format!("<http://e/u{i}> <http://e/p> \"open\n"));
        }
        let (_, d) = syntax_diagnostics(&text, Some(10));
        assert_eq!(d.len(), 11);
        let lines: Vec<u32> = d[..10].iter().map(|e| e.range.start.line).collect();
        assert_eq!(lines, (0..10).collect::<Vec<u32>>());
        assert!(
            d[..10]
                .iter()
                .all(|e| e.message == "Expected an object, found '.'")
        );
        assert_eq!(
            d[10].message,
            "More than 10 syntax errors; the rest are not shown"
        );
        // On a tie the first found: line 30's lexer error, not the parser's.
        let mut tied = broken_lines(30);
        tied.push_str("<http://e/u> <http://e/p> \"open\n");
        let (_, d) = syntax_diagnostics(&tied, Some(31));
        assert_eq!(d.len(), 32);
        let on_30: Vec<&str> = d[..31]
            .iter()
            .filter(|e| e.range.start.line == 30)
            .map(|e| e.message.as_str())
            .collect();
        assert_eq!(on_30, ["Unterminated string"]);
    }

    #[test]
    fn json_ld_keeps_its_own_cap() {
        let text = format!("[{}]", "1 ".repeat(50));
        let shown = |max| {
            let a = Analysis::with_syntax(
                "file:///x/a.jsonld".to_string(),
                &text,
                Arc::default(),
                false,
                Syntax::JsonLd,
            );
            let opts = Options {
                max_syntax_errors: max,
                ..Options::default()
            };
            diagnostics_with(&a, &Knowledge::default(), Encoding::Utf8, &opts)
        };
        let all = shown(None);
        assert!(all.len() > 5, "{all:?}");
        assert_eq!(shown(Some(5)), all);
    }

    /// The `deprecated-term` diagnostics of `uses` (a document of `syntax`)
    /// as (severity, text covered, message), with `vocab.ttl` beside it.
    fn own_deprecated(
        vocab: &str,
        uses: &str,
        syntax: Syntax,
    ) -> Vec<(DiagnosticSeverity, String, String)> {
        use crate::knowledge::Source;
        let header = "@prefix ex: <http://ex.org/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                      @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                      @prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n";
        let v = Analysis::new(
            "file:///w/vocab.ttl".into(),
            &format!("{header}{vocab}"),
            Arc::default(),
            false,
        );
        let a = Arc::new(Analysis::with_syntax(
            "file:///w/uses".into(),
            uses,
            Arc::default(),
            false,
            syntax,
        ));
        let mut k = Knowledge::default();
        k.set("file:///w/vocab.ttl", Source::Disk, Arc::new(v));
        k.set("file:///w/uses", Source::Open, a.clone());
        diagnostics(&a, &k, Encoding::Utf8)
            .into_iter()
            .filter(|d| d.code == Some(NumberOrString::String("deprecated-term".into())))
            .map(|d| {
                assert_eq!(d.tags, Some(vec![DiagnosticTag::DEPRECATED]));
                let start = a.offset(Encoding::Utf8, d.range.start).unwrap() as usize;
                let end = a.offset(Encoding::Utf8, d.range.end).unwrap() as usize;
                (d.severity.unwrap(), uses[start..end].to_string(), d.message)
            })
            .collect()
    }

    #[test]
    fn a_use_of_an_own_deprecated_term_is_a_warning() {
        use DiagnosticSeverity as S;
        let uses = "@prefix ex: <http://ex.org/> .\n\
                    ex:old ex:p ex:x .\nex:a ex:old <http://ex.org/old> .\n";
        let got = own_deprecated("ex:old owl:deprecated true .\n", uses, Syntax::Turtle);
        // Not at the subject: that describes the term.
        assert_eq!(
            got,
            [
                (
                    S::WARNING,
                    "ex:old".into(),
                    "'ex:old' is deprecated: the workspace marks it owl:deprecated".into()
                ),
                (
                    S::WARNING,
                    "<http://ex.org/old>".into(),
                    "'<http://ex.org/old>' is deprecated: the workspace marks it owl:deprecated"
                        .into()
                ),
            ]
        );
        // The typed form counts; `false` anywhere clears it.
        let typed = "ex:old owl:deprecated \"true\"^^xsd:boolean .\n";
        assert_eq!(own_deprecated(typed, uses, Syntax::Turtle).len(), 2);
        let undone = "ex:old owl:deprecated true .\nex:old owl:deprecated false .\n";
        assert_eq!(own_deprecated(undone, uses, Syntax::Turtle), []);
        // The deprecating file's own description is not flagged.
        let v = "ex:old owl:deprecated true .\n";
        assert_eq!(own_deprecated(v, "", Syntax::Turtle), []);
    }

    #[test]
    fn a_vocabulary_deprecation_stays_a_hint_unless_the_workspace_says_so() {
        use DiagnosticSeverity as S;
        let uses = "@prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n\
                    <http://ex.org/a> a vcard:Email .\n";
        assert_eq!(
            own_deprecated("", uses, Syntax::Turtle),
            [(
                S::HINT,
                "vcard:Email".into(),
                "'vcard:Email' is deprecated".into()
            )]
        );
        // Deprecated by both: reported once, as the workspace's warning.
        let got = own_deprecated("vcard:Email owl:deprecated true .\n", uses, Syntax::Turtle);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, S::WARNING);
        // A subject gets no warning (it describes the term): it keeps the hint.
        let uses = "@prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n\
                    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                    vcard:Email rdfs:comment \"x\" .\n";
        let got = own_deprecated("vcard:Email owl:deprecated true .\n", uses, Syntax::Turtle);
        assert_eq!(
            got,
            [(
                S::HINT,
                "vcard:Email".into(),
                "'vcard:Email' is deprecated".into()
            )]
        );
    }

    /// RDF/XML (decision 8): a use of a deprecated term, at the text that
    /// names it: an attribute value, a qualified name, a collection item;
    /// not where it is described, nor through an entity (no name is
    /// written there).
    #[test]
    fn an_rdfxml_use_of_an_own_deprecated_term_is_a_warning() {
        let uses = "<?xml version=\"1.0\"?>\n\
            <!DOCTYPE rdf:RDF [<!ENTITY ex \"http://ex.org/\">]>\n\
            <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n\
            \x20        xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns:ex=\"http://ex.org/\">\n\
            \x20 <owl:Class rdf:about=\"http://ex.org/old\"/>\n\
            \x20 <ex:old rdf:about=\"http://ex.org/a\">\n\
            \x20   <ex:p rdf:resource=\"http://ex.org/old\"/>\n\
            \x20   <ex:p rdf:resource=\"&ex;old\"/>\n\
            \x20 </ex:old>\n\
            \x20 <owl:Class rdf:about=\"http://ex.org/b\">\n\
            \x20   <owl:unionOf rdf:parseType=\"Collection\">\n\
            \x20     <rdf:Description rdf:about=\"http://ex.org/old\"/>\n\
            \x20   </owl:unionOf>\n\
            \x20 </owl:Class>\n\
            </rdf:RDF>\n";
        let got = own_deprecated("ex:old owl:deprecated true .\n", uses, Syntax::RdfXml);
        let at: Vec<(DiagnosticSeverity, &str)> = got.iter().map(|g| (g.0, g.1.as_str())).collect();
        let w = DiagnosticSeverity::WARNING;
        assert_eq!(
            at,
            [
                (w, "ex:old"),
                (w, "http://ex.org/old"),
                (w, "http://ex.org/old")
            ],
            "{got:?}"
        );
    }

    #[test]
    fn a_jsonld_use_of_an_own_deprecated_term_is_a_warning() {
        let uses = r#"{
  "@context": {"ex": "http://ex.org/", "old": {"@id": "ex:old", "@type": "@id"}},
  "@id": "ex:a",
  "old": "ex:b",
  "ex:p": {"@id": "ex:old"}
}"#;
        let got = own_deprecated("ex:old owl:deprecated true .\n", uses, Syntax::JsonLd);
        let at: Vec<&str> = got.iter().map(|g| g.1.as_str()).collect();
        assert!(
            got.iter().all(|g| g.0 == DiagnosticSeverity::WARNING),
            "{got:?}"
        );
        // The term at its use, and the compact IRI; not the subject `ex:a`.
        assert_eq!(at, ["old", "ex:old"], "{got:?}");
    }

    #[test]
    fn a_bare_replacement_keeps_what_is_implied() {
        let schema = "http://schema.org/";
        let r = |iri, written| bare_replacement(iri, written, schema, "Person");
        assert_eq!(r("http://schema.org/Persn", "Persn"), "Person");
        assert_eq!(
            r("http://schema.org/Persn", "http://schema.org/Persn"),
            "http://schema.org/Person"
        );
        // Written escaped, or under another namespace: the full IRI.
        assert_eq!(
            r("http://schema.org/Persn", "Pers\\u006E"),
            "http://schema.org/Person"
        );
        assert_eq!(
            bare_replacement("https://schema.org/Persn", "Persn", schema, "Person"),
            "http://schema.org/Person"
        );
    }

    /// The `unknown-term` warnings of `text`, a document of `syntax` open in
    /// the workspace, as (the text covered, the message). None carries fix
    /// data where no fix edits the document.
    fn unknown_terms(uri: &str, text: &str, syntax: Syntax) -> Vec<(String, String)> {
        use crate::knowledge::Source;
        let a = Arc::new(Analysis::with_syntax(
            uri.into(),
            text,
            Arc::default(),
            true,
            syntax,
        ));
        assert_eq!(a.parsed.syntax, syntax);
        assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
        let mut k = Knowledge::default();
        k.set(uri, Source::Open, a.clone());
        diagnostics(&a, &k, Encoding::Utf8)
            .into_iter()
            .filter(|d| d.code == Some(NumberOrString::String("unknown-term".into())))
            .map(|d| {
                assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
                assert_eq!(d.data.is_none(), !super::super::writes(syntax), "{d:?}");
                let start = a.offset(Encoding::Utf8, d.range.start).unwrap() as usize;
                let end = a.offset(Encoding::Utf8, d.range.end).unwrap() as usize;
                (text[start..end].to_string(), d.message)
            })
            .collect()
    }

    const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";

    /// The message for a misspelt `skos:` term: `local`, and what it means.
    fn misspelt(local: &str, meant: &str) -> String {
        format!("'{local}' is not defined in the skos vocabulary <{SKOS}>. Did you mean '{meant}'?")
    }

    /// RDF/XML reports a misspelt term at a typed node element's name, a
    /// property element's, a property attribute's and an `rdf:resource`
    /// value, with the "did you mean" in the message and no fix. A
    /// reference whose text is not the term's ([`Form::Anchor`]) gets
    /// none: an IRI written through an entity, and an unprefixed name in a
    /// default namespace.
    #[test]
    fn rdfxml_reports_a_misspelt_term_where_it_is_written() {
        let text = "<?xml version=\"1.0\"?>\n\
            <!DOCTYPE rdf:RDF [<!ENTITY skos \"http://www.w3.org/2004/02/skos/core#\">]>\n\
            <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n\
            \x20        xmlns:skos=\"http://www.w3.org/2004/02/skos/core#\">\n\
            \x20 <skos:Conceptt rdf:about=\"http://example.org/a\">\n\
            \x20   <skos:prefLabl>a</skos:prefLabl>\n\
            \x20   <rdf:type rdf:resource=\"http://www.w3.org/2004/02/skos/core#Conceptt\"/>\n\
            \x20   <rdf:type rdf:resource=\"&skos;Conceptt\"/>\n\
            \x20 </skos:Conceptt>\n\
            \x20 <rdf:Description rdf:about=\"http://example.org/b\" skos:prefLabl=\"b\"/>\n\
            \x20 <Conceptt xmlns=\"http://www.w3.org/2004/02/skos/core#\" rdf:about=\"http://example.org/c\"/>\n\
            </rdf:RDF>\n";
        let concept = misspelt("Conceptt", "Concept");
        let label = misspelt("prefLabl", "prefLabel");
        assert_eq!(
            unknown_terms("file:///w/a.rdf", text, Syntax::RdfXml),
            [
                ("skos:Conceptt".to_string(), concept.clone()),
                ("skos:prefLabl".to_string(), label.clone()),
                (format!("{SKOS}Conceptt"), concept),
                ("skos:prefLabl".to_string(), label),
            ]
        );
    }

    /// OWL Functional syntax reports a misspelt term written as a prefixed
    /// name and as a full IRI, at its text (the brackets with the IRI).
    #[test]
    fn functional_syntax_reports_a_misspelt_term_where_it_is_written() {
        let text = format!(
            "Prefix(:=<http://example.org/o#>)\n\
             Prefix(skos:=<{SKOS}>)\n\
             Ontology(<http://example.org/o>\n\
             Declaration(Class(:A))\n\
             SubClassOf(:A skos:Conceptt)\n\
             SubClassOf(:A <{SKOS}Conceptt>)\n\
             AnnotationAssertion(skos:prefLabl :A \"a\")\n\
             )\n"
        );
        let concept = misspelt("Conceptt", "Concept");
        assert_eq!(
            unknown_terms("file:///w/o.ofn", &text, Syntax::OwlFunctional),
            [
                ("skos:Conceptt".to_string(), concept.clone()),
                (format!("<{SKOS}Conceptt>"), concept),
                (
                    "skos:prefLabl".to_string(),
                    misspelt("prefLabl", "prefLabel")
                ),
            ]
        );
    }

    /// OWL/XML reports a misspelt term in an `abbreviatedIRI` and an `IRI`
    /// attribute, and in an `AbbreviatedIRI` and an `IRI` element, at the
    /// value (without the quotes) or the content; not where it is written
    /// through an entity, whose text is not the term's.
    #[test]
    fn owl_xml_reports_a_misspelt_term_where_it_is_written() {
        let text = format!(
            "<?xml version=\"1.0\"?>\n\
             <!DOCTYPE Ontology [<!ENTITY skos \"{SKOS}\">]>\n\
             <Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n\
             \x20 <Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n\
             \x20 <Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>\n\
             \x20 <Prefix name=\"skos\" IRI=\"{SKOS}\"/>\n\
             \x20 <Declaration><Class abbreviatedIRI=\":A\"/></Declaration>\n\
             \x20 <SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\"skos:Conceptt\"/></SubClassOf>\n\
             \x20 <SubClassOf><Class abbreviatedIRI=\":A\"/><Class IRI=\"{SKOS}Conceptt\"/></SubClassOf>\n\
             \x20 <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><AbbreviatedIRI>:A</AbbreviatedIRI><AbbreviatedIRI>skos:Conceptt</AbbreviatedIRI></AnnotationAssertion>\n\
             \x20 <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><AbbreviatedIRI>:A</AbbreviatedIRI><IRI>{SKOS}Conceptt</IRI></AnnotationAssertion>\n\
             \x20 <SubClassOf><Class abbreviatedIRI=\":A\"/><Class IRI=\"&skos;Conceptt\"/></SubClassOf>\n\
             </Ontology>\n"
        );
        let concept = misspelt("Conceptt", "Concept");
        let (pname, iri) = ("skos:Conceptt".to_string(), format!("{SKOS}Conceptt"));
        assert_eq!(
            unknown_terms("file:///w/o.owx", &text, Syntax::OwlXml),
            [
                (pname.clone(), concept.clone()),
                (iri.clone(), concept.clone()),
                (pname, concept.clone()),
                (iri, concept),
            ]
        );
    }

    /// A correct document of each read-only syntax gets no `unknown-term`:
    /// not for the syntax's own names (RDF/XML's `rdf:li`, `rdf:_n`,
    /// `rdf:parseType`, `rdf:datatype`; the element names of OWL/XML), nor
    /// for the built-in datatypes and facets of OWL 2.
    #[test]
    fn a_correct_read_only_document_has_no_unknown_term() {
        let rdfxml = "<?xml version=\"1.0\"?>\n\
            <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n\
            \x20        xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\"\n\
            \x20        xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xml:base=\"http://example.org/\">\n\
            \x20 <owl:Ontology rdf:about=\"http://example.org/o\"/>\n\
            \x20 <owl:Class rdf:about=\"http://example.org/A\">\n\
            \x20   <rdfs:subClassOf rdf:resource=\"http://www.w3.org/2002/07/owl#Thing\"/>\n\
            \x20   <rdfs:label xml:lang=\"en\">a</rdfs:label>\n\
            \x20   <rdfs:comment rdf:parseType=\"Literal\"><b>x</b></rdfs:comment>\n\
            \x20 </owl:Class>\n\
            \x20 <rdf:Description rdf:ID=\"l\">\n\
            \x20   <rdfs:member><rdf:Bag><rdf:li>1</rdf:li><rdf:_2>2</rdf:_2></rdf:Bag></rdfs:member>\n\
            \x20   <rdfs:seeAlso rdf:parseType=\"Collection\"><rdf:Description rdf:about=\"http://example.org/x\"/></rdfs:seeAlso>\n\
            \x20   <rdfs:seeAlso rdf:parseType=\"Resource\"><rdfs:label>r</rdfs:label></rdfs:seeAlso>\n\
            \x20   <rdfs:seeAlso rdf:nodeID=\"n1\"/>\n\
            \x20   <rdf:value rdf:datatype=\"http://www.w3.org/2001/XMLSchema#integer\">1</rdf:value>\n\
            \x20   <rdf:value rdf:datatype=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral\">p@en</rdf:value>\n\
            \x20 </rdf:Description>\n\
            </rdf:RDF>\n";
        assert_eq!(unknown_terms("file:///w/a.rdf", rdfxml, Syntax::RdfXml), []);
        let functional = "Prefix(:=<http://example.org/o#>)\n\
            Ontology(<http://example.org/o>\n\
            Declaration(Class(:A))\n\
            Declaration(ObjectProperty(:p))\n\
            Declaration(DataProperty(:d))\n\
            SubClassOf(:A owl:Thing)\n\
            SubObjectPropertyOf(:p owl:topObjectProperty)\n\
            DataPropertyRange(:d DatatypeRestriction(xsd:integer xsd:minInclusive \"1\"^^xsd:integer xsd:maxExclusive \"9\"^^xsd:integer))\n\
            DataPropertyRange(:d DatatypeRestriction(rdf:PlainLiteral rdf:langRange \"en\"))\n\
            DataPropertyRange(:d DataUnionOf(owl:real owl:rational rdfs:Literal xsd:dateTimeStamp))\n\
            AnnotationAssertion(rdfs:label :A \"a\"@en)\n\
            AnnotationAssertion(rdfs:comment :A \"c@en\"^^rdf:PlainLiteral)\n\
            )\n";
        assert_eq!(
            unknown_terms("file:///w/o.ofn", functional, Syntax::OwlFunctional),
            []
        );
        let owl_xml = "<?xml version=\"1.0\"?>\n\
            <Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n\
            \x20 <Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n\
            \x20 <Prefix name=\"owl\" IRI=\"http://www.w3.org/2002/07/owl#\"/>\n\
            \x20 <Prefix name=\"rdf\" IRI=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/>\n\
            \x20 <Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>\n\
            \x20 <Prefix name=\"xsd\" IRI=\"http://www.w3.org/2001/XMLSchema#\"/>\n\
            \x20 <Declaration><Class abbreviatedIRI=\":A\"/></Declaration>\n\
            \x20 <Declaration><DataProperty abbreviatedIRI=\":d\"/></Declaration>\n\
            \x20 <SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\"owl:Thing\"/></SubClassOf>\n\
            \x20 <DataPropertyRange><DataProperty abbreviatedIRI=\":d\"/><DatatypeRestriction><Datatype abbreviatedIRI=\"xsd:integer\"/><FacetRestriction facet=\"http://www.w3.org/2001/XMLSchema#minInclusive\"><Literal datatypeIRI=\"http://www.w3.org/2001/XMLSchema#integer\">1</Literal></FacetRestriction></DatatypeRestriction></DataPropertyRange>\n\
            \x20 <DataPropertyRange><DataProperty abbreviatedIRI=\":d\"/><DataUnionOf><Datatype abbreviatedIRI=\"owl:real\"/><Datatype abbreviatedIRI=\"owl:rational\"/><Datatype abbreviatedIRI=\"rdf:PlainLiteral\"/></DataUnionOf></DataPropertyRange>\n\
            \x20 <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>:A</AbbreviatedIRI><Literal xml:lang=\"en\">a</Literal></AnnotationAssertion>\n\
            </Ontology>\n";
        assert_eq!(
            unknown_terms("file:///w/o.owx", owl_xml, Syntax::OwlXml),
            []
        );
    }

    fn codes_of(ds: &[Diagnostic]) -> Vec<String> {
        ds.iter()
            .filter_map(|d| match &d.code {
                Some(NumberOrString::String(c)) => Some(c.clone()),
                _ => None,
            })
            .collect()
    }

    /// On RDF/XML run the syntax errors, the lints of its terms and
    /// literals (`invalid-literal`, `undescribed-term`,
    /// `unknown-term`, `deprecated-term`) and the curation checks
    /// (`label-whitespace` here), none with a fix; not the prefix lints or
    /// the shape lint. SHACL results and reasoning findings come through
    /// the composer as for any document.
    #[test]
    fn rdfxml_gets_its_syntax_errors_and_the_lints_of_its_terms() {
        use crate::features::rdfxml_fixture as fx;
        let uri = "file:///w/onto.owl";
        let k = Knowledge::default();
        let opts = Options {
            lint_undescribed_terms: true,
            house_style: true,
            max_syntax_errors: Some(1),
        };
        let codes = |a: &Analysis| codes_of(&diagnostics_with(a, &k, Encoding::Utf8, &opts));
        let linted = codes(&fx::turtle(uri));
        for code in [
            "unused-prefix",
            "unknown-term",
            "invalid-literal",
            "label-whitespace",
        ] {
            assert!(linted.iter().any(|c| c == code), "{code}: {linted:?}");
        }
        let lints = ["unknown-term", "invalid-literal", "label-whitespace"];
        assert_eq!(codes(&fx::written(uri, fx::TURTLE)), lints);
        let read = fx::read(uri);
        let found = diagnostics_with(&read, &k, Encoding::Utf8, &opts);
        assert_eq!(codes_of(&found), lints);
        // The misspelt `rdfs:labl` at its element name, the "did you mean"
        // in the message only; the literals at their text, between the tags.
        assert_eq!(
            found[0].range,
            lsp_types::Range::new(
                lsp_types::Position::new(10, 5),
                lsp_types::Position::new(10, 14)
            )
        );
        assert!(
            found[0].message.ends_with("Did you mean 'label'?"),
            "{found:?}"
        );
        assert_eq!(found[1].range.start, lsp_types::Position::new(11, 66));
        assert_eq!(
            found[2].range,
            lsp_types::Range::new(
                lsp_types::Position::new(6, 16),
                lsp_types::Position::new(6, 24)
            )
        );
        assert!(found.iter().all(|d| d.data.is_none()));
        let broken = fx::TEXT.replace("</ex:A>", "</ex:B>");
        for (text, want) in [
            // Read on past the error: the term and the literal after it are
            // still checked; the second error (`ex:A` not closed) is past
            // the cap.
            (
                broken.as_str(),
                &[
                    "xml-syntax",
                    "syntax",
                    "unknown-term",
                    "invalid-literal",
                    "label-whitespace",
                ][..],
            ),
            ("Prefix: : <http://example.org/o#>", &["unsupported-syntax"]),
        ] {
            let a = Analysis::with_syntax(uri.into(), text, Arc::default(), true, Syntax::RdfXml);
            assert_eq!(codes(&a), want, "{text}");
        }
        // A Functional `.owl` reaches its own frontend: its errors, no
        // `unsupported-syntax`, and the lints RDF/XML gets, at the exact
        // spans of its terms: here an `invalid-literal` after the error.
        let functional = "Prefix(:=<http://example.org/o#>)\nOntology(SubClassOf(:A nope:B)\nDataPropertyAssertion(:d :a \"x\"^^xsd:integer))";
        let a = Analysis::new(uri.into(), functional, Arc::default(), true);
        assert_eq!(codes(&a), ["ofn-syntax", "invalid-literal"]);
        let found = diagnostics_with(&a, &k, Encoding::Utf8, &opts);
        assert_eq!(found[1].range.start, lsp_types::Position::new(2, 28));
        for check in Check::ALL {
            let on = !matches!(
                check,
                Check::UndeclaredPrefix
                    | Check::PrefixDeclarations
                    | Check::UnusedPrefix
                    | Check::JsonLd
                    | Check::ShaclLint
            );
            assert_eq!(check.runs_on(Syntax::RdfXml), on, "{check:?}");
            assert_eq!(check.runs_on(Syntax::OwlFunctional), on, "{check:?}");
            assert_eq!(check.runs_on(Syntax::OwlXml), on, "{check:?}");
            assert!(check.runs_on(Syntax::Turtle), "{check:?}");
        }

        // Through the composer, with every lint enabled.
        let settings = crate::settings::Settings {
            lint_enable: (crate::features::curation::OPT_IN.iter())
                .map(|c| c.to_string())
                .collect(),
            ..Default::default()
        };
        let finding = |c: &str| Diagnostic {
            code: Some(NumberOrString::String(c.into())),
            ..Default::default()
        };
        let (shacl, reasoning) = ([finding("shacl-violation")], [finding("turbowl/cax-dw")]);
        let inputs = crate::diagnose::DiagnosticInputs {
            shacl: &shacl,
            reasoning: &reasoning,
            ..crate::diagnose::DiagnosticInputs::new(&settings)
        };
        let a = Analysis::with_syntax(uri.into(), &broken, Arc::default(), true, Syntax::RdfXml);
        assert!(!a.parsed.triples.is_empty());
        let got =
            crate::diagnose::document(crate::diagnose::Doc::Rdf(&a), &k, Encoding::Utf8, &inputs);
        assert_eq!(
            codes_of(&got),
            [
                "xml-syntax",
                "xml-syntax",
                "unknown-term",
                "invalid-literal",
                "label-whitespace",
                "shacl-violation",
                "turbowl/cax-dw"
            ]
        );
    }
}
