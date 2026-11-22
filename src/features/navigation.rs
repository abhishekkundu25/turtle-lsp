//! Definition, references, rename and document highlight, all keyed by IRI.
//! Cursor on the prefix part of `ex:a` (or a prefix declaration) works on the
//! prefix instead.

use super::prefixes::{Scope, declaration_at};
use crate::analysis::{Analysis, Encoding};
use crate::features::refactor::text_edits;
use crate::knowledge::{Knowledge, Source};
use crate::syntax::names::is_pn_prefix;
use crate::syntax::{Form, Iri, IriKey, Node, PrefixMap, Span, Syntax};
use crate::workspace::{PREFIX_FILE, as_uri, uri_to_path};
use lsp_types::*;
use rustc_hash::FxHashSet;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

pub(crate) mod jsonld;

pub(crate) enum Target {
    Iri(Iri),
    /// A prefix name, as used or declared in this document.
    Prefix(String),
}

pub(crate) fn target_at(a: &Analysis, offset: u32) -> Option<(Target, Span)> {
    // An XML namespace prefix is no Turtle prefix: in RDF/XML every
    // reference is its IRI, whatever part of it the cursor is on. So in an
    // OWL Functional document, which records no prefix uses.
    match a.parsed.syntax {
        Syntax::RdfXml | Syntax::OwlFunctional => {
            let r = a.ref_at(offset)?;
            return Some((Target::Iri(r.iri), r.span));
        }
        Syntax::Turtle | Syntax::TriG | Syntax::NTriples | Syntax::NQuads | Syntax::JsonLd => {}
    }
    if let Some(p) = a
        .parsed
        .prefixes
        .iter()
        .find(|p| p.name_span.contains(offset))
    {
        return Some((Target::Prefix(p.name.clone()), p.name_span));
    }
    let r = a.ref_at(offset)?;
    if let Form::PName { prefix_len } = r.form
        && offset <= r.span.start + prefix_len
    {
        let name = a
            .parsed
            .prefix_uses
            .iter()
            .find(|u| u.span == r.span)?
            .name
            .to_string();
        return Some((
            Target::Prefix(name),
            Span::new(r.span.start, r.span.start + prefix_len),
        ));
    }
    Some((Target::Iri(r.iri), r.span))
}

/// `s` as the protocol's URI. A document's URI is one: the client's, or
/// [`crate::workspace::path_to_uri`]'s. An import read without a cache is
/// named by the IRI its importer wrote, which may hold what a URI may not
/// (a letter outside ASCII): its URI is that IRI escaped ([`as_uri`]), the
/// URL it was fetched from. `None` when `s` is no URI even so.
pub(crate) fn uri(s: &str) -> Option<Uri> {
    #[cfg(test)]
    URIS_PARSED.with(|c| c.set(c.get() + 1));
    Uri::from_str(s).or_else(|_| Uri::from_str(&as_uri(s))).ok()
}

/// [`uri`] for an edit: `Err` refuses the whole edit, as one without this
/// document's part would change only some of the uses.
pub(crate) fn edited_uri(s: &str) -> Result<Uri, String> {
    uri(s).ok_or_else(|| format!("can't edit {s}: it is not a valid URI"))
}

#[cfg(test)]
thread_local! {
    /// URIs this thread has parsed with [`uri`].
    static URIS_PARSED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A document's path for a message, else its URI.
pub(crate) fn shown(uri: &str) -> String {
    uri_to_path(uri).map_or_else(|| uri.to_string(), |p| p.display().to_string())
}

/// Why a rename that would edit the document at `uri`, which
/// [`super::writes`] refuses, is refused.
fn not_edited(a: &Analysis) -> String {
    let what = match a.parsed.syntax {
        Syntax::OwlFunctional => "an OWL Functional syntax document",
        Syntax::Turtle
        | Syntax::TriG
        | Syntax::NTriples
        | Syntax::NQuads
        | Syntax::JsonLd
        | Syntax::RdfXml => "an RDF/XML document",
    };
    format!("can't rename in {}: {what} is not edited", shown(&a.uri))
}

fn loc(a: &Analysis, enc: Encoding, span: Span) -> Option<Location> {
    Some(DocLocs::new(a)?.at(enc, span))
}

/// Locations in one document, its URI parsed once: a result with tens of
/// thousands of them (references on `foaf:Person` in a big file) parsed the
/// same URI for each.
struct DocLocs<'a> {
    a: &'a Analysis,
    uri: Uri,
}

impl<'a> DocLocs<'a> {
    /// `None` for a document whose URI is not one ([`uri`]): it has no
    /// location a client could open, and a result leaves it out.
    fn new(a: &'a Analysis) -> Option<Self> {
        Some(DocLocs {
            a,
            uri: uri(&a.uri)?,
        })
    }

    fn at(&self, enc: Encoding, span: Span) -> Location {
        Location {
            uri: self.uri.clone(),
            range: self.a.range(enc, span),
        }
    }
}

pub fn definition(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    prefix_file: Option<Arc<Analysis>>,
) -> Vec<Location> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    // A JSON-LD term: its definition's key, unless a bundled or remote
    // context defines it.
    if let Some(l) = jsonld::term_at(a, offset).and_then(|t| {
        jsonld::definition_location(a, k, enc, a.source_index.definition(t.definition))
    }) {
        return vec![l];
    }
    match target_at(a, offset) {
        Some((Target::Prefix(name), _)) => {
            // The declaration under the cursor, else the one in scope there.
            if let Some(p) = a
                .parsed
                .prefixes
                .iter()
                .find(|p| p.name_span.contains(offset))
                .or_else(|| declaration_at(a, &name, offset))
            {
                return loc(a, enc, p.name_span).into_iter().collect();
            }
            prefix_file
                .and_then(|pf| {
                    pf.parsed
                        .prefixes
                        .iter()
                        .find(|p| p.name == name)
                        .and_then(|p| loc(&pf, enc, p.name_span))
                })
                .into_iter()
                .collect()
        }
        Some((Target::Iri(iri), _)) => descriptions(k, enc, &iri, &a.uri),
        None => vec![],
    }
}

/// Where the workspace describes `iri`: the document `here` first, then by
/// URI and line.
pub(crate) fn descriptions<K: IriKey + ?Sized>(
    k: &Knowledge,
    enc: Encoding,
    iri: &K,
    here: &str,
) -> Vec<Location> {
    let mut out = Vec::new();
    for d in k.descriptions(iri) {
        let Some(here) = DocLocs::new(d.analysis) else {
            continue;
        };
        for span in d.spans {
            out.push(here.at(enc, *span));
        }
    }
    out.sort_by(|x, y| {
        (x.uri.as_str() != here, x.uri.as_str(), x.range.start.line).cmp(&(
            y.uri.as_str() != here,
            y.uri.as_str(),
            y.range.start.line,
        ))
    });
    out
}

/// Where the resource's classes are described (`gy` in Neovim).
pub fn type_definition(a: &Analysis, k: &Knowledge, enc: Encoding, pos: Position) -> Vec<Location> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    let Some((Target::Iri(iri), _)) = target_at(a, offset) else {
        return vec![];
    };
    let mut out = Vec::new();
    for ty in k.types(&iri) {
        for d in k.descriptions(&ty) {
            let Some(here) = DocLocs::new(d.analysis) else {
                continue;
            };
            for span in d.spans {
                out.push(here.at(enc, *span));
            }
        }
    }
    out
}

/// Instances, subclasses and subproperties of the IRI under the cursor.
pub fn implementations(a: &Analysis, k: &Knowledge, enc: Encoding, pos: Position) -> Vec<Location> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    let Some((Target::Iri(iri), _)) = target_at(a, offset) else {
        return vec![];
    };
    // Each instance or sub term where it is linked to the IRI.
    let mut out = Vec::new();
    for (doc, related) in k.related_by_doc(&iri) {
        let other = k.doc(doc);
        let Some(here) = DocLocs::new(other) else {
            continue;
        };
        for s in related {
            for span in other.defs.get(&s).into_iter().flatten() {
                out.push(here.at(enc, *span));
            }
        }
    }
    out
}

pub fn references(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    include_decl: bool,
) -> Vec<Location> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    if let Some(t) = jsonld::term_at(a, offset) {
        return jsonld_references(a, k, enc, t, include_decl);
    }
    let mut out = Vec::new();
    match target_at(a, offset) {
        Some((Target::Prefix(name), _)) => {
            let Some(here) = DocLocs::new(a) else {
                return out;
            };
            if include_decl {
                out.extend(
                    a.parsed
                        .prefixes
                        .iter()
                        .filter(|p| p.name == name)
                        .map(|p| here.at(enc, p.name_span)),
                );
            }
            out.extend(
                a.parsed
                    .prefix_uses
                    .iter()
                    .filter(|u| u.name == name)
                    .map(|u| {
                        here.at(
                            enc,
                            Span::new(u.span.start, u.span.start + name.len() as u32),
                        )
                    }),
            );
        }
        Some((Target::Iri(iri), _)) => out = term_references(k, enc, &iri, include_decl),
        None => {}
    }
    out
}

/// A JSON-LD term's references: its definition's key (with `include_decl`),
/// its uses in this document and its IRI's uses in the workspace.
fn jsonld_references(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    t: jsonld::TermAt,
    include_decl: bool,
) -> Vec<Location> {
    let d = a.source_index.definition(t.definition);
    let mut out = Vec::new();
    if include_decl {
        out.extend(jsonld::definition_location(a, k, enc, d));
    }
    if let Some(here) = DocLocs::new(a) {
        out.extend(jsonld::use_spans(a, t.definition).map(|s| here.at(enc, s)));
    }
    if let Some(iri) = d.iri.as_deref() {
        out.extend(term_references(k, enc, iri, include_decl));
    }
    let mut seen = FxHashSet::default();
    out.retain(|l| {
        let r = l.range;
        seen.insert((
            l.uri.as_str().to_string(),
            r.start.line,
            r.start.character,
            r.end.line,
            r.end.character,
        ))
    });
    out
}

/// Where the workspace's RDF documents (imports left out) use `iri`; its
/// descriptions too with `include_decl`.
pub(crate) fn term_references<K: IriKey + ?Sized>(
    k: &Knowledge,
    enc: Encoding,
    iri: &K,
    include_decl: bool,
) -> Vec<Location> {
    let mut out = Vec::new();
    for (_, _, other) in k
        .docs_referencing(iri)
        .filter(|(_, src, _)| *src != Source::External)
    {
        let Some(here) = DocLocs::new(other) else {
            continue;
        };
        let mut seen = FxHashSet::default();
        for r in other.refs_of(iri) {
            if (include_decl || !r.role.is_definition()) && seen.insert(r.span) {
                out.push(here.at(enc, r.span));
            }
        }
    }
    out
}

pub fn highlights(a: &Analysis, enc: Encoding, pos: Position) -> Vec<DocumentHighlight> {
    let Some(offset) = a.offset(enc, pos) else {
        return vec![];
    };
    let mk = |span: Span, write: bool| DocumentHighlight {
        range: a.range(enc, span),
        kind: Some(if write {
            DocumentHighlightKind::WRITE
        } else {
            DocumentHighlightKind::READ
        }),
    };
    // A JSON-LD term: its definition, if in this document, and its uses.
    if let Some(t) = jsonld::term_at(a, offset) {
        let d = a.source_index.definition(t.definition);
        let def = (d.origin == crate::jsonld::source::Origin::Document).then_some(d.key);
        return def
            .into_iter()
            .map(|s| mk(s, true))
            .chain(jsonld::use_spans(a, t.definition).map(|s| mk(s, false)))
            .collect();
    }
    let Some((target, _)) = target_at(a, offset) else {
        return vec![];
    };
    match target {
        Target::Iri(iri) => a
            .refs_of(&iri)
            .map(|r| mk(r.span, r.role.is_definition()))
            .collect(),
        Target::Prefix(name) => a
            .parsed
            .prefixes
            .iter()
            .filter(|p| p.name == name)
            .map(|p| mk(p.name_span, true))
            .chain(
                a.parsed
                    .prefix_uses
                    .iter()
                    .filter(|u| u.name == name)
                    .map(|u| {
                        mk(
                            Span::new(u.span.start, u.span.start + name.len() as u32),
                            false,
                        )
                    }),
            )
            .collect(),
    }
}

/// What a rename at `pos` would change; `Err` for a JSON-LD term of a
/// bundled or remote context, which can't be renamed. Nothing in a document
/// that is not edited ([`super::writes`]: RDF/XML).
pub fn prepare_rename(
    a: &Analysis,
    enc: Encoding,
    pos: Position,
) -> Result<Option<PrepareRenameResponse>, String> {
    if !super::writes(a.parsed.syntax) {
        return Ok(None);
    }
    let Some(offset) = a.offset(enc, pos) else {
        return Ok(None);
    };
    // A JSON-LD term: its key (or a compact IRI's prefix part).
    if let Some(t) = jsonld::term_at(a, offset) {
        jsonld::check_term_renamable(a, t)?;
        return Ok(Some(PrepareRenameResponse::Range(a.range(enc, t.span))));
    }
    Ok(prepare_iri_rename(a, enc, offset))
}

fn prepare_iri_rename(a: &Analysis, enc: Encoding, offset: u32) -> Option<PrepareRenameResponse> {
    let r = a.ref_at(offset);
    // `a` has no name to change, and the text at an anchor is not the name.
    if r.is_some_and(|r| matches!(r.form, Form::A | Form::Anchor)) {
        return None;
    }
    let (_, span) = target_at(a, offset)?;
    Some(PrepareRenameResponse::Range(a.range(enc, span)))
}

/// Where the prefixes of a document come from, for renaming one.
pub struct PrefixScope<'a> {
    /// The prefixes.ttl that supplies the document's prefixes, if any.
    pub file: Option<Arc<Analysis>>,
    /// The prefixes.ttl that supplies the document at a URI, if any.
    pub file_of: &'a mut dyn FnMut(&str) -> Option<std::path::PathBuf>,
}

/// Rename an IRI everywhere it occurs, or a prefix. A prefix a document
/// declares itself is renamed in that document; one a prefixes.ttl declares is
/// renamed there and in every document that takes it from there.
/// `new_name` may be a local name (`robert`), a prefixed name (`ex:robert`)
/// or a full IRI (`<http://...>`); each occurrence keeps its file's style.
/// A JSON-LD term (or keyword alias) is renamed as a term
/// ([`jsonld::term_rename`]). `Err` when `a` is not edited
/// ([`super::writes`]: RDF/XML), and a prefix rename leaves such documents
/// out.
pub fn rename(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    new_name: &str,
    scope: PrefixScope,
) -> Result<WorkspaceEdit, String> {
    if !super::writes(a.parsed.syntax) {
        return Err(not_edited(a));
    }
    let offset = a.offset(enc, pos).ok_or("invalid position")?;
    if let Some(t) = jsonld::term_at(a, offset) {
        return Ok(WorkspaceEdit {
            changes: Some(jsonld::term_rename(a, k, enc, t, new_name)?),
            ..Default::default()
        });
    }
    let (target, _) = target_at(a, offset).ok_or("nothing to rename here")?;
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    match target {
        Target::Prefix(old) => {
            let new = new_name.trim_end_matches(':');
            if !is_pn_prefix(new) {
                return Err(format!("'{new_name}' is not a valid prefix name"));
            }
            let declares =
                |d: &Analysis, name: &str| d.parsed.prefixes.iter().any(|p| p.name == name);
            let is_prefix_file = |d: &Analysis| d.uri.ends_with(&format!("/{PREFIX_FILE}"));
            // The prefixes.ttl declaring `old` for this document, if it is shared.
            let shared: Option<&Analysis> = if declares(a, &old) {
                is_prefix_file(a).then_some(a)
            } else if let Some(pf) = scope.file.as_deref().filter(|pf| declares(pf, &old)) {
                Some(pf)
            } else if a.external.contains_key(&old) {
                return Err(format!(
                    "prefix '{old}:' comes from the turtle.prefixes setting; rename it there"
                ));
            } else {
                None
            };
            let mut docs: Vec<&Analysis> = vec![shared.unwrap_or(a)];
            if let Some(source) = shared {
                let source_path = uri_to_path(&source.uri);
                if !std::ptr::eq(source, a) {
                    docs.push(a);
                }
                for (_, src, d) in k.docs() {
                    if src == Source::External
                        || !super::writes(d.parsed.syntax)
                        || d.uri == a.uri
                        || d.uri == source.uri
                        || is_prefix_file(d)
                        || declares(d, &old)
                        // A file that does not use it is not renamed, so its
                        // own prefixes cannot clash with the new name.
                        || !d.parsed.prefix_uses.iter().any(|u| u.name == old)
                    {
                        continue;
                    }
                    if source_path.is_some() && (scope.file_of)(&d.uri) == source_path {
                        docs.push(d);
                    }
                }
            }
            for d in &docs {
                if declares(d, new) {
                    return Err(format!(
                        "prefix '{new}:' is already declared in {}",
                        shown(&d.uri)
                    ));
                }
                if d.parsed.prefix_uses.iter().any(|u| u.name == new) {
                    return Err(format!(
                        "prefix '{new}:' is already used in {}",
                        shown(&d.uri)
                    ));
                }
            }
            for d in docs {
                // Whole tokens: the `old:` of each declaration, each `old:local`.
                let mut reps: Vec<(Span, String)> = d
                    .parsed
                    .prefixes
                    .iter()
                    .filter(|p| p.name == old)
                    .map(|p| {
                        let colon = Span::new(p.name_span.start, p.name_span.end + 1);
                        (colon, format!("{new}:"))
                    })
                    .collect();
                // A use before the first declaration is not bound by it: it is
                // undeclared or comes from outside the document.
                let first = d.parsed.prefixes.iter().filter(|p| p.name == old);
                let first = first.map(|p| p.span.end).min();
                reps.extend(
                    d.parsed
                        .prefix_uses
                        .iter()
                        .filter(|u| u.name == old)
                        .filter(|u| first.map_or(u.resolved, |end| end <= u.span.start))
                        .map(|u| {
                            let local = &d.text
                                [(u.span.start as usize + old.len() + 1)..u.span.end as usize];
                            (u.span, format!("{new}:{local}"))
                        }),
                );
                let edits = text_edits(d, enc, reps);
                if !edits.is_empty() || std::ptr::eq(d, a) {
                    changes.insert(edited_uri(&d.uri)?, edits);
                }
            }
        }
        Target::Iri(old) => {
            let new_iri = renamed_iri_at(a, offset, &old, new_name)?;
            rename_term(k, enc, &old, &new_iri, &mut changes)?;
        }
    }
    Ok(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

/// [`renamed_iri`] for a rename started at `offset` of `a`: in JSON-LD,
/// with the prefix terms of the context there, and `new_name` may also be
/// an absolute IRI written as JSON-LD writes it (`http://...`).
pub(crate) fn renamed_iri_at(
    a: &Analysis,
    offset: u32,
    old: &str,
    new_name: &str,
) -> Result<String, String> {
    let prefixes = jsonld::prefixes_at(a, offset);
    // An RDF/XML or OWL Functional document is never renamed in
    // ([`rename`] refuses); the new name is read as in Turtle.
    let bare_iri = match a.parsed.syntax {
        Syntax::JsonLd => true,
        Syntax::Turtle
        | Syntax::TriG
        | Syntax::NTriples
        | Syntax::NQuads
        | Syntax::RdfXml
        | Syntax::OwlFunctional => false,
    };
    if bare_iri
        && let Some((p, rest)) = new_name.split_once(':')
        && !prefixes.contains_key(p)
        && rest.starts_with("//")
    {
        return checked_iri(new_name.to_string());
    }
    renamed_iri(old, new_name, &prefixes)
}

/// The IRI term `old` is renamed to, from `new_name` as the user typed it:
/// `<full IRI>`, `prefix:local` (with `prefixes`), or a local name that
/// replaces `old`'s after its last `#`, `/` or `:`. `Err` when the result is
/// no valid absolute IRI (`foo bar`), which no document could hold.
pub(crate) fn renamed_iri(
    old: &str,
    new_name: &str,
    prefixes: &PrefixMap,
) -> Result<String, String> {
    let iri = if let Some(i) = new_name.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        i.to_string()
    } else if let Some((p, l)) = new_name.split_once(':') {
        let ns = prefixes
            .get(p)
            .ok_or(format!("prefix '{p}:' is not declared"))?;
        format!("{ns}{l}")
    } else {
        let cut = old.rfind(['#', '/', ':']).map_or(0, |i| i + 1);
        format!("{}{new_name}", &old[..cut])
    };
    checked_iri(iri)
}

fn checked_iri(iri: String) -> Result<String, String> {
    match oxiri::Iri::parse(iri.as_str()) {
        Ok(_) => Ok(iri),
        Err(e) => Err(format!("'{iri}' is not a valid IRI: {e}")),
    }
}

/// Adds to `changes` the edits that rename IRI `old` to `new_iri` in the
/// workspace's RDF documents (imports left out): each use keeps its form, a
/// prefixed name when a prefix of its document shortens the new IRI. A
/// JSON-LD document writes each use in its own form, never in angle
/// brackets ([`jsonld::iri_rename`]). `Err` (the whole rename refused, with
/// nothing added to `changes`) when a workspace document that is not edited
/// ([`super::writes`]: RDF/XML) uses `old`, in any form: renaming the other
/// uses would leave a workspace that disagrees with itself. Such a document
/// uses every IRI its triples name, including the vocabulary a production
/// implies (`rdf:type`, `rdf:first`, `xsd:string`) and the names read from
/// an entity reference, which share its span and have no reference of
/// their own ([`rdfxml_names`]). Also `Err` when a
/// JSON-LD use cannot be rewritten. An anchored use ([`Form::Anchor`]) of a
/// Turtle-family document is left as it is: its text is not the IRI.
pub(crate) fn rename_term(
    k: &Knowledge,
    enc: Encoding,
    old: &str,
    new_iri: &str,
    changes: &mut HashMap<Uri, Vec<TextEdit>>,
) -> Result<(), String> {
    let (mut jsonld_docs, mut turtle_docs) = (Vec::new(), Vec::new());
    for (_, _, other) in k
        .docs_referencing(old)
        .filter(|(_, src, _)| *src != Source::External)
    {
        match other.parsed.syntax {
            Syntax::RdfXml | Syntax::OwlFunctional => return Err(not_edited(other)),
            Syntax::JsonLd => jsonld_docs.push(other),
            Syntax::Turtle | Syntax::TriG | Syntax::NTriples | Syntax::NQuads => {
                turtle_docs.push(other)
            }
        }
    }
    if let Some(old) = Iri::get(old) {
        let names_old = k
            .docs()
            .filter(|(_, src, d)| *src != Source::External && rdfxml_names(d, old))
            .map(|(_, _, d)| d)
            .min_by(|x, y| x.uri.cmp(&y.uri));
        if let Some(d) = names_old {
            return Err(not_edited(d));
        }
    }
    let mut jsonld_changes = jsonld::Changes {
        by_uri: std::mem::take(changes),
    };
    let refused = jsonld_docs
        .into_iter()
        .try_for_each(|other| jsonld::iri_rename(other, k, enc, old, new_iri, &mut jsonld_changes));
    *changes = jsonld_changes.by_uri;
    refused?;
    for other in turtle_docs {
        let mut uses: Vec<(Span, Form)> = other
            .refs_of(old)
            .filter(|r| !matches!(r.form, Form::A | Form::Anchor))
            .map(|r| (r.span, r.form))
            .collect();
        uses.sort_by_key(|(s, _)| s.start);
        uses.dedup_by_key(|(s, _)| *s);
        // Each prefixed name is written with the prefixes in scope at it.
        let mut scope = Scope::outside(other);
        let mut decls = other.parsed.prefixes.iter().peekable();
        let mut reps = Vec::new();
        for (span, form) in uses {
            while let Some(p) = decls.next_if(|p| p.span.end <= span.start) {
                scope.insert(p.name.clone(), p.iri.to_string());
            }
            let text = match form {
                Form::Iri => format!("<{new_iri}>"),
                _ => scope.write(new_iri),
            };
            reps.push((span, text));
        }
        if !reps.is_empty() {
            changes.insert(edited_uri(&other.uri)?, text_edits(other, enc, reps));
        }
    }
    Ok(())
}

/// Whether `a` is a document that is not edited ([`super::writes`]: RDF/XML,
/// OWL Functional syntax) whose triples name `iri`, as subject,
/// predicate, object or datatype, or inside a triple term. Its references
/// do not say it all: an entity reference stands for everything read from
/// it, and a production implies vocabulary no name is written for.
fn rdfxml_names(a: &Analysis, iri: Iri) -> bool {
    fn names(n: &Node, iri: Iri) -> bool {
        match n {
            Node::Iri(i) => *i == iri,
            Node::Literal { datatype, .. } => *datatype == iri,
            Node::Triple(t) => names(&t.0, iri) || t.1 == iri || names(&t.2, iri),
            Node::Blank(_) => false,
        }
    }
    !super::writes(a.parsed.syntax)
        && a.parsed
            .triples
            .iter()
            .any(|t| t.p == iri || names(&t.s, iri) || names(&t.o, iri))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renamed_iri_must_be_a_valid_iri() {
        let prefixes: PrefixMap =
            PrefixMap::from_iter([("ex".to_string(), "http://example.org/".to_string())]);
        let old = "http://example.org/a";
        assert_eq!(
            renamed_iri(old, "b", &prefixes).unwrap(),
            "http://example.org/b"
        );
        assert_eq!(
            renamed_iri(old, "ex:c", &prefixes).unwrap(),
            "http://example.org/c"
        );
        assert_eq!(
            renamed_iri(old, "<http://example.org/-a>", &prefixes).unwrap(),
            "http://example.org/-a"
        );
        for bad in [
            "foo bar",
            "ex:foo bar",
            "<http://example.org/a b>",
            "<not an iri>",
            "<rel/ative>",
            "a<b",
            "a\"b",
            "a{b}",
        ] {
            let err = renamed_iri(old, bad, &prefixes).unwrap_err();
            assert!(err.contains("not a valid IRI"), "{bad}: {err}");
        }
        assert!(
            renamed_iri(old, "nope:x", &prefixes)
                .unwrap_err()
                .contains("not declared")
        );
    }

    /// An import read without a cache is named by its IRI, and an IRI may
    /// hold letters a URI must escape: asking for a location in it panicked.
    /// Its URI is the IRI escaped; a name that is no URI even so has no
    /// location, and an edit of it is refused.
    #[test]
    fn a_document_named_by_an_iri_has_an_escaped_uri_and_one_that_is_no_uri_has_none() {
        let text = "@prefix ex: <http://example.org/> .\nex:a a ex:C .\nex:b ex:p ex:a .\n";
        let doc = |uri: &str| {
            Arc::new(Analysis::new(
                uri.into(),
                text,
                Arc::new(PrefixMap::default()),
                true,
            ))
        };
        let good = "file:///w/a.ttl";
        let (import, escaped) = (
            "http://example.org/données.ttl",
            "http://example.org/donn%C3%A9es.ttl",
        );
        let no_uri = "http://example.org/a#b#c";
        assert_eq!(uri(good).unwrap().as_str(), good);
        assert_eq!(uri(import).unwrap().as_str(), escaped);
        assert_eq!(
            uri("file:///w/a [1].ttl").unwrap().as_str(),
            "file:///w/a%20%5B1%5D.ttl"
        );
        for not_one in [no_uri, "http://[x/a", "1:a"] {
            assert_eq!(uri(not_one), None, "{not_one}");
        }
        let mut k = Knowledge::default();
        k.set(good, Source::Open, doc(good));
        k.add_external(import, doc(import));
        k.set(no_uri, Source::Disk, doc(no_uri));
        let iri = "http://example.org/a";
        let enc = Encoding::Utf16;
        let files = |locations: Vec<Location>| -> Vec<String> {
            locations.iter().map(|l| l.uri.to_string()).collect()
        };
        // References leave imports out; definitions list them.
        assert_eq!(files(term_references(&k, enc, iri, false)), [good]);
        assert_eq!(files(descriptions(&k, enc, iri, good)), [good, escaped]);
        let a = k.get(good).unwrap().clone();
        let at = a.position(enc, text.find("ex:a a").unwrap() as u32 + 3);
        assert_eq!(files(references(&a, &k, enc, at, false)), [good]);
        assert_eq!(files(definition(&a, &k, enc, at, None)), [good, escaped]);

        let mut changes = HashMap::new();
        let refused = rename_term(&k, enc, iri, "http://example.org/z", &mut changes);
        assert!(refused.unwrap_err().contains("not a valid URI"));
    }

    /// A result with a location per use parsed the document's URI for each
    /// (75,000 locations: 349 ms). It is parsed once per document.
    #[test]
    fn a_document_s_uri_is_parsed_once_for_all_its_locations() {
        let mut text = String::from("@prefix ex: <http://example.org/> .\n");
        for i in 0..300 {
            text.push_str(&format!("ex:s{i} a ex:C ; ex:p ex:C .\n"));
        }
        let other = text.replace("ex:s", "ex:t");
        let doc = |uri: &str, text: &str| {
            Arc::new(Analysis::new(
                uri.into(),
                text,
                Arc::new(PrefixMap::default()),
                true,
            ))
        };
        let (a, b) = (
            doc("file:///w/a.ttl", &text),
            doc("file:///w/b.ttl", &other),
        );
        let mut k = Knowledge::default();
        k.set("file:///w/a.ttl", Source::Open, a.clone());
        k.set("file:///w/b.ttl", Source::Open, b.clone());
        let at = a.position(Encoding::Utf16, text.find("ex:C ;").unwrap() as u32 + 3);
        let before = URIS_PARSED.with(|c| c.get());
        let refs = references(&a, &k, Encoding::Utf16, at, true);
        assert_eq!(refs.len(), 1200, "600 uses of ex:C in each file");
        assert_eq!(
            URIS_PARSED.with(|c| c.get()) - before,
            2,
            "one per document"
        );
        let found = definition(&a, &k, Encoding::Utf16, at, None);
        assert!(found.len() <= 1);
        assert!(refs.iter().all(|l| l.uri.as_str().ends_with(".ttl")));
    }

    /// The triples of `text`, sorted, for comparing graphs.
    fn graph(text: &str) -> Vec<String> {
        let p = crate::syntax::parse(text, &PrefixMap::default(), Default::default());
        assert!(p.errors.is_empty(), "{text}: {:?}", p.errors);
        let mut t: Vec<String> = p
            .triples
            .iter()
            .map(|t| format!("{:?} {:?} {:?}", t.s, t.p, t.o))
            .collect();
        t.sort();
        t
    }

    /// `text` (as file:///w/a.ttl) after a rename to `new_name` at the
    /// first `needle`, or of the term `old` to `new` when `needle` is empty.
    fn renamed(text: &str, needle: &str, new_name: &str, terms: (&str, &str)) -> String {
        let a = Arc::new(Analysis::new(
            "file:///w/a.ttl".into(),
            text,
            Arc::default(),
            true,
        ));
        let mut k = Knowledge::default();
        k.set("file:///w/a.ttl", Source::Open, a.clone());
        let edits = if needle.is_empty() {
            let mut changes = HashMap::new();
            rename_term(&k, Encoding::Utf8, terms.0, terms.1, &mut changes).unwrap();
            changes.into_values().flatten().collect::<Vec<_>>()
        } else {
            let at = a.position(Encoding::Utf8, text.find(needle).unwrap() as u32);
            let scope = PrefixScope {
                file: None,
                file_of: &mut |_| None,
            };
            let edit = rename(&a, &k, Encoding::Utf8, at, new_name, scope).unwrap();
            edit.changes.unwrap().into_values().flatten().collect()
        };
        crate::features::refactor::apply(&a, Encoding::Utf8, &edits)
    }

    /// Each rename keeps every triple but the renamed term: the names it
    /// writes stay apart from their neighbours.
    #[test]
    fn a_rename_keeps_every_other_token() {
        let cases = [
            (
                "@prefix:<http://e/> . :s :p :o .",
                (":s", "ex", ("", "")),
                "@prefix ex:<http://e/> . ex:s ex:p ex:o .",
            ),
            (
                "@prefix ex: <http://e/> .\nex:s ex:p (1ex:b) .",
                ("ex:s", "e5", ("", "")),
                "@prefix e5: <http://e/> .\ne5:s e5:p (1 e5:b) .",
            ),
            (
                "@prefix ex: <http://e/> .\nex:s ex:p ( ex:-1 ) .",
                ("", "", ("http://e/", "http://e/a")),
                "@prefix ex: <http://e/> .\nex:s ex:p ( ex:a -1 ) .",
            ),
            // The uses before the declaration are not its: they stay undeclared.
            (
                "ex:a ex:b ex:c .PREFIX ex: <http://e/>\nex:s ex:p ex:o .",
                ("ex: <", "e5", ("", "")),
                "ex:a ex:b ex:c .PREFIX e5: <http://e/>\ne5:s e5:p e5:o .",
            ),
            // `e:` would be shorter, but it is declared only after the use.
            (
                "@prefix ex: <http://e/> .\nex:x ex:p 1 .\n@prefix e: <http://e/> .\ne:x ex:p 2 .",
                ("", "", ("http://e/x", "http://e/z")),
                "@prefix ex: <http://e/> .\nex:z ex:p 1 .\n@prefix e: <http://e/> .\ne:z ex:p 2 .",
            ),
        ];
        for (text, (needle, new_name, terms), want) in cases {
            let out = renamed(text, needle, new_name, terms);
            assert_eq!(out, want);
            assert_eq!(graph(&out), graph(want));
        }
    }

    /// A rename lexes a document built without its tokens into a temporary,
    /// so they do not stay in memory after it.
    #[test]
    fn a_rename_keeps_no_tokens_of_a_document_built_without_them() {
        let text = "@prefix ex: <http://e/> .\nex:s ex:p ex:o .\n";
        let doc = |uri: &str, keep| Arc::new(Analysis::new(uri.into(), text, Arc::default(), keep));
        let (open, closed) = (doc("file:///w/a.ttl", true), doc("file:///w/b.ttl", false));
        assert!(!closed.has_tokens());
        let mut k = Knowledge::default();
        k.set("file:///w/a.ttl", Source::Open, open.clone());
        k.set("file:///w/b.ttl", Source::Disk, closed.clone());
        let at = open.position(Encoding::Utf8, text.find("ex:s").unwrap() as u32 + 3);
        let scope = PrefixScope {
            file: None,
            file_of: &mut |_| None,
        };
        let edit = rename(&open, &k, Encoding::Utf8, at, "t", scope).unwrap();
        assert_eq!(edit.changes.unwrap().len(), 2);
        assert!(!closed.has_tokens());
    }

    /// A use whose text is not at its span ([`Form::Anchor`]) is never
    /// written over: a rename from elsewhere leaves it, and none starts at it.
    #[test]
    fn an_anchored_use_is_never_renamed() {
        let text = "@prefix ex: <http://example.org/> .\nex:s ex:p <http://example.org/o> .\n";
        let at = |a: &Analysis, needle: &str| {
            a.position(
                Encoding::Utf8,
                (text.find(needle).unwrap() + needle.len() - 1) as u32,
            )
        };
        let open = Arc::new(Analysis::new(
            "file:///w/a.ttl".into(),
            text,
            Arc::default(),
            true,
        ));
        let renamed = |other: Arc<Analysis>| -> Vec<String> {
            let mut k = Knowledge::default();
            k.set("file:///w/a.ttl", Source::Open, open.clone());
            k.set("file:///w/b.ttl", Source::Disk, other);
            let scope = PrefixScope {
                file: None,
                file_of: &mut |_| None,
            };
            let edit = rename(&open, &k, Encoding::Utf8, at(&open, "ex:s"), "t", scope).unwrap();
            let mut uris: Vec<String> = edit
                .changes
                .unwrap()
                .into_keys()
                .map(|u| u.to_string())
                .collect();
            uris.sort();
            uris
        };
        let written = Arc::new(Analysis::new(
            "file:///w/b.ttl".into(),
            text,
            Arc::default(),
            true,
        ));
        assert_eq!(renamed(written), ["file:///w/a.ttl", "file:///w/b.ttl"]);
        let anchored = Arc::new(Analysis::anchored("file:///w/b.ttl", text, Syntax::Turtle));
        assert_eq!(renamed(anchored), ["file:///w/a.ttl"]);
        // An RDF/XML document refuses the rename instead
        // (`a_term_an_rdfxml_document_uses_is_not_renamed`).
        for syntax in [Syntax::Turtle, Syntax::RdfXml] {
            let anchored = Arc::new(Analysis::anchored("file:///w/b.ttl", text, syntax));
            for needle in ["ex:s", "ex:p", "example.org/o"] {
                let got = prepare_rename(&anchored, Encoding::Utf8, at(&anchored, needle)).unwrap();
                assert_eq!(got, None, "{syntax:?} {needle}");
            }
        }
        for needle in ["ex:s", "ex:p", "example.org/o"] {
            let got = prepare_rename(&open, Encoding::Utf8, at(&open, needle)).unwrap();
            assert!(got.is_some(), "{needle}");
        }
    }

    use crate::features::rdfxml_fixture;

    const OWL_URI: &str = "file:///w/onto.owl";

    /// `name` in the folder `/w` (`C:\w` on Windows): its path, and its URI
    /// as the workspace makes it. A URI with no drive is no path on Windows.
    fn in_w(name: &str) -> (std::path::PathBuf, String) {
        let path = std::path::Path::new(if cfg!(windows) { "C:\\w" } else { "/w" }).join(name);
        let uri = crate::workspace::path_to_uri(&path).unwrap();
        (path, uri)
    }

    fn turtle_doc(uri: &str, text: &str) -> Arc<Analysis> {
        Arc::new(Analysis::new(uri.into(), text, Arc::default(), true))
    }

    /// The offset of the first `needle` in `text`, plus `plus`.
    fn offset_of(text: &str, needle: &str, plus: usize) -> u32 {
        (text.find(needle).unwrap_or_else(|| panic!("{needle}")) + plus) as u32
    }

    /// Decision 9: a term that a workspace RDF/XML document uses, in any
    /// form, is renamed nowhere, and the message names that document. An
    /// import in RDF/XML neither refuses nor is edited.
    #[test]
    fn a_term_an_rdfxml_document_uses_is_not_renamed() {
        let text = "@prefix ex: <http://example.org/> .\nex:a ex:p ex:zz .\n";
        let ttl = turtle_doc("file:///w/a.ttl", text);
        let at = ttl.position(Encoding::Utf8, offset_of(text, "ex:a", 3));
        // The message names the document by its path.
        let (owl_path, owl_uri) = in_w("onto.owl");
        let owl_shown = owl_path.display().to_string();
        let owls = [
            ("read", Arc::new(rdfxml_fixture::read(&owl_uri))),
            (
                "written",
                Arc::new(rdfxml_fixture::written(&owl_uri, rdfxml_fixture::TURTLE)),
            ),
        ];
        for (how, owl) in owls {
            assert!(
                owl.refs_of("http://example.org/a").next().is_some(),
                "{how}"
            );
            let mut k = Knowledge::default();
            k.set("file:///w/a.ttl", Source::Open, ttl.clone());
            k.set(&owl_uri, Source::Disk, owl.clone());
            let err = rename(
                &ttl,
                &k,
                Encoding::Utf8,
                at,
                "z",
                PrefixScope {
                    file: None,
                    file_of: &mut |_| None,
                },
            )
            .unwrap_err();
            assert_eq!(
                err,
                format!("can't rename in {owl_shown}: an RDF/XML document is not edited"),
                "{how}"
            );
            // Nothing is added to the changes a caller already holds.
            let mut changes = HashMap::new();
            let old = "http://example.org/a";
            let refused = rename_term(
                &k,
                Encoding::Utf8,
                old,
                "http://example.org/z",
                &mut changes,
            );
            assert!(refused.unwrap_err().contains(&owl_shown), "{how}");
            assert!(changes.is_empty(), "{how}");
            // A term it does not use is renamed as before.
            let at_p = ttl.position(Encoding::Utf8, offset_of(text, "ex:zz", 3));
            let edit = rename(
                &ttl,
                &k,
                Encoding::Utf8,
                at_p,
                "c",
                PrefixScope {
                    file: None,
                    file_of: &mut |_| None,
                },
            )
            .unwrap();
            let uris: Vec<String> = edit
                .changes
                .unwrap()
                .keys()
                .map(|u| u.to_string())
                .collect();
            assert_eq!(uris, ["file:///w/a.ttl"], "{how}");

            // An import is never edited, so it does not refuse either.
            let mut k = Knowledge::default();
            k.set("file:///w/a.ttl", Source::Open, ttl.clone());
            k.add_external("http://example.org/onto", owl);
            let edit = rename(
                &ttl,
                &k,
                Encoding::Utf8,
                at,
                "z",
                PrefixScope {
                    file: None,
                    file_of: &mut |_| None,
                },
            )
            .unwrap();
            let uris: Vec<String> = edit
                .changes
                .unwrap()
                .keys()
                .map(|u| u.to_string())
                .collect();
            assert_eq!(uris, ["file:///w/a.ttl"], "{how}");
        }
    }

    /// The IRIs `n` names: itself, a literal's datatype, a triple term's.
    fn iris_of(n: &Node, out: &mut Vec<Iri>) {
        match n {
            Node::Iri(i) => out.push(*i),
            Node::Literal { datatype, .. } => out.push(*datatype),
            Node::Triple(t) => {
                iris_of(&t.0, out);
                out.push(t.1);
                iris_of(&t.2, out);
            }
            Node::Blank(_) => {}
        }
    }

    /// The names and values an entity reference expands to share its span
    /// and have no reference of their own, but are in the triples: a Turtle
    /// document's rename of such an IRI is refused all the same.
    #[test]
    fn a_term_an_rdfxml_entity_uses_is_not_renamed() {
        let text = "@prefix ex: <urn:ex:> .\nex:a ex:q <urn:ex:target> .\n";
        let ttl = turtle_doc("file:///w/a.ttl", text);
        let (owl_path, owl_uri) = in_w("onto.owl");
        let owl_text = "<!DOCTYPE r [<!ENTITY m \"<e:p rdf:resource='urn:ex:target'/>\">]>\n\
            <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:e=\"urn:ex:\">\n\
            <rdf:Description rdf:about=\"urn:ex:s\">&m;</rdf:Description>\n\
            </rdf:RDF>\n";
        let owl = Arc::new(Analysis::with_syntax(
            owl_uri.clone(),
            owl_text,
            Arc::default(),
            true,
            Syntax::RdfXml,
        ));
        assert!(owl.parsed.errors.is_empty(), "{:?}", owl.parsed.errors);
        let target = "urn:ex:target";
        let mut named = Vec::new();
        for t in &owl.parsed.triples {
            iris_of(&t.o, &mut named);
        }
        assert!(named.iter().any(|i| *i == target), "{named:?}");
        let mut k = Knowledge::default();
        k.set("file:///w/a.ttl", Source::Open, ttl.clone());
        k.set(&owl_uri, Source::Disk, owl);
        let at = ttl.position(Encoding::Utf8, offset_of(text, "target", 0));
        let err = rename(
            &ttl,
            &k,
            Encoding::Utf8,
            at,
            "z",
            PrefixScope {
                file: None,
                file_of: &mut |_| None,
            },
        )
        .unwrap_err();
        let owl_shown = owl_path.display().to_string();
        assert_eq!(
            err,
            format!("can't rename in {owl_shown}: an RDF/XML document is not edited")
        );
    }

    /// Every IRI in a workspace RDF/XML document's triples refuses a rename
    /// of it, whatever production gave it: `rdf:li`, `rdf:ID` against
    /// `xml:base`, a typed node element, the collection, resource and
    /// literal parse types, reification, a triple term, datatypes, and the vocabulary
    /// these imply (`rdf:type`, `rdf:first`, `xsd:string`).
    #[test]
    fn every_iri_an_rdfxml_document_says_refuses_a_rename() {
        let (_, owl_uri) = in_w("onto.owl");
        let owl_text = "<!DOCTYPE r [<!ENTITY m \"<e:in rdf:resource='urn:ex:inner'/>\">\
            <!ENTITY q \"<rdf:Description rdf:about='urn:ex:qs'><e:qp rdf:resource='urn:ex:qo'/></rdf:Description>\">]>\n\
            <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:e=\"urn:ex:\"\n\
            \x20   xml:base=\"http://example.org/base/\" rdf:version=\"1.2\">\n\
            <e:T rdf:about=\"s\" e:attr=\"v\">\n\
            \x20 <e:p rdf:ID=\"st\" rdf:resource=\"o\"/>\n\
            \x20 <e:list rdf:parseType=\"Collection\"><e:Item rdf:about=\"i1\"/></e:list>\n\
            \x20 <e:res rdf:parseType=\"Resource\"><e:q xml:lang=\"en\">x</e:q></e:res>\n\
            \x20 <e:xml rdf:parseType=\"Literal\"><b>y</b></e:xml>\n\
            \x20 <e:n rdf:datatype=\"http://www.w3.org/2001/XMLSchema#integer\">1</e:n>\n\
            \x20 &m;\n\
            \x20 <e:quotes rdf:parseType=\"Triple\">&q;</e:quotes>\n\
            </e:T>\n\
            <rdf:Seq rdf:about=\"seq\"><rdf:li>a</rdf:li></rdf:Seq>\n\
            </rdf:RDF>\n";
        let owl = Arc::new(Analysis::with_syntax(
            owl_uri.clone(),
            owl_text,
            Arc::default(),
            true,
            Syntax::RdfXml,
        ));
        assert!(owl.parsed.errors.is_empty(), "{:?}", owl.parsed.errors);
        let mut iris = Vec::new();
        for t in &owl.parsed.triples {
            iris_of(&t.s, &mut iris);
            iris.push(t.p);
            iris_of(&t.o, &mut iris);
        }
        iris.sort();
        iris.dedup();
        for want in [
            "http://example.org/base/s",
            "http://example.org/base/#st",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#_1",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#first",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#Statement",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral",
            "http://www.w3.org/2001/XMLSchema#string",
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString",
            "urn:ex:inner",
            // Only in a triple term.
            "urn:ex:qo",
        ] {
            assert!(iris.iter().any(|i| *i == want), "{want}: {iris:?}");
        }
        // Read from an entity, after the reference that stands for it.
        for gone in ["urn:ex:inner", "urn:ex:qo"] {
            assert!(owl.refs_of(gone).next().is_none(), "{gone}");
        }
        let mut k = Knowledge::default();
        k.set(&owl_uri, Source::Disk, owl);
        for iri in iris {
            let mut changes = HashMap::new();
            let renamed = format!("{iri}x");
            let got = rename_term(&k, Encoding::Utf8, &iri, &renamed, &mut changes);
            assert!(got.is_err_and(|e| e.contains("onto.owl")), "{iri}");
        }
    }

    /// No rename starts in an RDF/XML document, whatever the form of the
    /// reference or prefix under the cursor: prepare-rename offers nothing
    /// and a rename sent anyway is refused, editing no other document.
    #[test]
    fn nothing_is_renamed_from_inside_rdfxml() {
        let ttl = turtle_doc(
            "file:///w/a.ttl",
            "@prefix ex: <http://example.org/> .\nex:a ex:p ex:b .\n",
        );
        let cases = [
            (rdfxml_fixture::read(OWL_URI), rdfxml_fixture::TEXT),
            (
                rdfxml_fixture::written(OWL_URI, rdfxml_fixture::TURTLE),
                rdfxml_fixture::TURTLE,
            ),
        ];
        for (owl, text) in cases {
            let owl = Arc::new(owl);
            // The knowledge with the document, and one without it (the
            // refusal is the document's own, not its uses').
            let mut without = Knowledge::default();
            without.set("file:///w/a.ttl", Source::Open, ttl.clone());
            let mut k = Knowledge::default();
            k.set("file:///w/a.ttl", Source::Open, ttl.clone());
            k.set(OWL_URI, Source::Open, owl.clone());
            // Every reference, and every prefix declaration and use.
            let mut offsets: Vec<u32> = owl.parsed.refs.iter().map(|r| r.span.start + 1).collect();
            offsets.extend(owl.parsed.prefixes.iter().map(|p| p.name_span.start));
            offsets.extend(owl.parsed.prefix_uses.iter().map(|u| u.span.start));
            assert!(offsets.len() > 5, "{text}");
            for offset in offsets {
                let at = owl.position(Encoding::Utf8, offset);
                let got = prepare_rename(&owl, Encoding::Utf8, at).unwrap();
                assert_eq!(got, None, "{offset} in {text}");
                for k in [&k, &without] {
                    let err = rename(
                        &owl,
                        k,
                        Encoding::Utf8,
                        at,
                        "z",
                        PrefixScope {
                            file: None,
                            file_of: &mut |_| None,
                        },
                    )
                    .unwrap_err();
                    assert!(err.contains("/w/onto.owl"), "{offset}: {err}");
                }
            }
        }
    }

    /// A prefix renamed in the prefixes.ttl that serves the folder is not
    /// renamed in an RDF/XML document there, though its parse uses it.
    #[test]
    fn a_shared_prefix_is_not_renamed_in_rdfxml() {
        // The folder's prefixes.ttl is matched by path.
        let (prefixes_path, prefixes_uri) = in_w("prefixes.ttl");
        let (_, a_uri) = in_w("a.ttl");
        let decl = "@prefix ex: <http://example.org/> .\n";
        let prefixes = turtle_doc(&prefixes_uri, decl);
        let external: Arc<PrefixMap> = Arc::new(PrefixMap::from_iter([(
            "ex".to_string(),
            "http://example.org/".to_string(),
        )]));
        let body = "ex:a ex:p ex:b .\n";
        let ttl = Arc::new(Analysis::new(a_uri.clone(), body, external.clone(), true));
        let owl = Arc::new(rdfxml_fixture::written_with(OWL_URI, body, external));
        assert!(owl.parsed.prefix_uses.iter().any(|u| u.name == "ex"));
        let mut k = Knowledge::default();
        k.set(&prefixes_uri, Source::Open, prefixes.clone());
        k.set(&a_uri, Source::Open, ttl);
        k.set(OWL_URI, Source::Disk, owl);
        let mut file_of = |_: &str| Some(prefixes_path.clone());
        let scope = PrefixScope {
            file: Some(prefixes.clone()),
            file_of: &mut file_of,
        };
        let at = prefixes.position(Encoding::Utf8, offset_of(decl, "ex:", 0));
        let edit = rename(&prefixes, &k, Encoding::Utf8, at, "e2", scope).unwrap();
        let mut uris: Vec<String> = edit
            .changes
            .unwrap()
            .keys()
            .map(|u| u.to_string())
            .collect();
        uris.sort();
        assert_eq!(uris, [a_uri, prefixes_uri]);
    }

    /// Definition, references, highlights, the type hierarchy and the
    /// reference lenses work in an RDF/XML document, and the prefix of a
    /// QName there is part of its IRI: no Turtle prefix to go to.
    #[test]
    fn navigation_works_in_rdfxml() {
        let enc = Encoding::Utf8;
        for owl in [
            rdfxml_fixture::read(OWL_URI),
            rdfxml_fixture::written(OWL_URI, rdfxml_fixture::TURTLE),
        ] {
            let owl = Arc::new(owl);
            let mut k = Knowledge::default();
            k.set(OWL_URI, Source::Open, owl.clone());
            let class = "http://example.org/A";
            // A place where the cursor is on the class (the reference
            // found there is the cursor's).
            let offset = owl
                .refs_of(class)
                .map(|r| r.span.start)
                .find(|&o| owl.ref_at(o).is_some_and(|r| r.iri == class))
                .unwrap();
            let at = owl.position(enc, offset);
            let found = definition(&owl, &k, enc, at, None);
            assert!(!found.is_empty());
            assert!(found.iter().all(|l| l.uri.as_str() == OWL_URI));
            let refs = references(&owl, &k, enc, at, true);
            let spans: FxHashSet<Span> = owl.refs_of(class).map(|r| r.span).collect();
            assert_eq!(refs.len(), spans.len());
            assert!(highlights(&owl, enc, at).len() >= refs.len());
            let items = crate::features::hierarchy::prepare(&owl, &k, enc, at).unwrap();
            assert_eq!(items[0].data.as_ref().unwrap()["iri"], class);
            let lenses = crate::features::lens::code_lenses(&owl, enc, true);
            assert!(
                lenses
                    .iter()
                    .any(|l| l.data.as_ref().unwrap()["iri"] == class)
            );
        }
        // On the prefix part of a QName.
        let owl = Arc::new(rdfxml_fixture::written(OWL_URI, rdfxml_fixture::TURTLE));
        let mut k = Knowledge::default();
        k.set(OWL_URI, Source::Open, owl.clone());
        let text = rdfxml_fixture::TURTLE;
        let on = |plus| {
            let at = owl.position(enc, offset_of(text, "ex:a a ex:A", plus));
            definition(&owl, &k, enc, at, None)
        };
        let (prefix, local) = (on(8), on(10));
        assert!(!local.is_empty());
        assert_eq!(prefix, local);
    }
}
