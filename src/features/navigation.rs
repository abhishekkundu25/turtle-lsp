//! Definition, references, rename and document highlight, all keyed by IRI.
//! Cursor on the prefix part of `ex:a` (or a prefix declaration) works on the
//! prefix instead.

use crate::analysis::{Analysis, Encoding};
use crate::features::refactor::text_edits;
use crate::knowledge::{Knowledge, Source};
use crate::syntax::names::{Scope, declaration_at, is_pn_prefix};
use crate::syntax::{Form, Iri, IriKey, PrefixMap, Span, Syntax};
use crate::workspace::{PREFIX_FILE, uri_to_path};
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

pub(crate) fn uri(s: &str) -> Uri {
    #[cfg(test)]
    URIS_PARSED.with(|c| c.set(c.get() + 1));
    Uri::from_str(s).expect("valid uri")
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

fn loc(a: &Analysis, enc: Encoding, span: Span) -> Location {
    DocLocs::new(a).at(enc, span)
}

/// Locations in one document, its URI parsed once: a result with tens of
/// thousands of them (references on `foaf:Person` in a big file) parsed the
/// same URI for each.
struct DocLocs<'a> {
    a: &'a Analysis,
    uri: Uri,
}

impl<'a> DocLocs<'a> {
    fn new(a: &'a Analysis) -> Self {
        DocLocs {
            a,
            uri: uri(&a.uri),
        }
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
                return vec![loc(a, enc, p.name_span)];
            }
            prefix_file
                .and_then(|pf| {
                    pf.parsed
                        .prefixes
                        .iter()
                        .find(|p| p.name == name)
                        .map(|p| loc(&pf, enc, p.name_span))
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
        let here = DocLocs::new(d.analysis);
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
            let here = DocLocs::new(d.analysis);
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
        let here = DocLocs::new(other);
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
            let here = DocLocs::new(a);
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
    let here = DocLocs::new(a);
    out.extend(jsonld::use_spans(a, t.definition).map(|s| here.at(enc, s)));
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
        let here = DocLocs::new(other);
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
/// bundled or remote context, which can't be renamed.
pub fn prepare_rename(
    a: &Analysis,
    enc: Encoding,
    pos: Position,
) -> Result<Option<PrepareRenameResponse>, String> {
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
    if r.is_some_and(|r| r.form == Form::A) {
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
/// ([`jsonld::term_rename`]).
pub fn rename(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    new_name: &str,
    scope: PrefixScope,
) -> Result<WorkspaceEdit, String> {
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
                reps.extend(
                    d.parsed
                        .prefix_uses
                        .iter()
                        .filter(|u| u.name == old)
                        .map(|u| {
                            let local = &d.text
                                [(u.span.start as usize + old.len() + 1)..u.span.end as usize];
                            (u.span, format!("{new}:{local}"))
                        }),
                );
                let edits = text_edits(d, enc, reps);
                if !edits.is_empty() || std::ptr::eq(d, a) {
                    changes.insert(uri(&d.uri), edits);
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
    if a.parsed.syntax == Syntax::JsonLd
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
/// brackets ([`jsonld::iri_rename`]). `Err` (the whole rename refused) when
/// a JSON-LD use cannot be rewritten.
pub(crate) fn rename_term(
    k: &Knowledge,
    enc: Encoding,
    old: &str,
    new_iri: &str,
    changes: &mut HashMap<Uri, Vec<TextEdit>>,
) -> Result<(), String> {
    let mut jsonld_changes = jsonld::Changes {
        by_uri: std::mem::take(changes),
    };
    let refused = k
        .docs_referencing(old)
        .filter(|(_, src, other)| *src != Source::External && other.parsed.syntax == Syntax::JsonLd)
        .try_for_each(|(_, _, other)| {
            jsonld::iri_rename(other, k, enc, old, new_iri, &mut jsonld_changes)
        });
    *changes = jsonld_changes.by_uri;
    refused?;
    for (_, _, other) in k
        .docs_referencing(old)
        .filter(|(_, src, other)| *src != Source::External && other.parsed.syntax != Syntax::JsonLd)
    {
        let mut uses: Vec<(Span, Form)> = other
            .refs_of(old)
            .filter(|r| r.form != Form::A)
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
            changes.insert(uri(&other.uri), text_edits(other, enc, reps));
        }
    }
    Ok(())
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
}
