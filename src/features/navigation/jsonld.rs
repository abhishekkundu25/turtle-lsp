//! Navigation on JSON-LD terms: a term or keyword alias, as used or as
//! defined in an `@context`, and the prefix part of a compact IRI. Uses and
//! definitions come from the document's [`SourceIndex`].
//!
//! [`SourceIndex`]: crate::jsonld::SourceIndex

use crate::analysis::{Analysis, Encoding};
use crate::jsonld::context::is_keyword;
use crate::jsonld::rename;
use crate::jsonld::source::{DefId, Definition, Origin, UseKind};
use crate::knowledge::{Knowledge, Source};
use crate::syntax::{PrefixMap, Span, Syntax};
use crate::workspace::{normalize_uri, uri_to_path};
use line_index::LineIndex;
use lsp_types::{Location, TextEdit, Uri};
use std::collections::HashMap;
use std::sync::Arc;

/// The JSON-LD term under the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TermAt {
    pub(crate) definition: DefId,
    /// The text under the cursor: a use, a compact IRI's prefix part or a
    /// definition's key.
    pub(crate) span: Span,
    /// The cursor is on the definition's key.
    pub(crate) on_definition: bool,
}

/// The term (or alias) used or defined at `offset`, or the prefix term of a
/// compact IRI whose prefix part `offset` is on. `None` elsewhere, and in
/// documents other than JSON-LD.
pub(crate) fn term_at(a: &Analysis, offset: u32) -> Option<TermAt> {
    let ix = &a.source_index;
    if let Some(id) = ix.definition_at(offset) {
        return Some(TermAt {
            definition: id,
            span: ix.definition(id).key,
            on_definition: true,
        });
    }
    let u = ix.use_at(offset)?;
    let (definition, span) = match u.kind {
        UseKind::Term { definition } | UseKind::Alias { definition } => (definition, u.span),
        UseKind::Compact { prefix_len, prefix } if offset <= u.span.start + prefix_len => {
            (prefix, Span::new(u.span.start, u.span.start + prefix_len))
        }
        _ => return None,
    };
    Some(TermAt {
        definition,
        span,
        on_definition: false,
    })
}

/// The spans in `a` of the uses of definition `id`: the whole string of a
/// term or alias, the prefix part of a compact IRI.
pub(crate) fn use_spans(a: &Analysis, id: DefId) -> impl Iterator<Item = Span> + '_ {
    a.source_index.uses_of(id).map(|u| match u.kind {
        UseKind::Compact { prefix_len, .. } => Span::new(u.span.start, u.span.start + prefix_len),
        _ => u.span,
    })
}

/// Where definition `d` of `a`'s index has its key: in `a`, or in a
/// workspace context file (its analysis, else its text on disk). `None` for
/// a bundled or remote context, or a file whose text no longer has the key
/// there.
pub(crate) fn definition_location(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    d: &Definition,
) -> Option<Location> {
    let spells = |text: &str| text.get(d.key.start as usize..d.key.end as usize) == Some(&d.term);
    match &d.origin {
        Origin::Document => Some(super::loc(a, enc, d.key)),
        Origin::Remote(_) => None,
        Origin::File(url) => {
            let uri = normalize_uri(url);
            if let Some(file) = k.get(&uri).filter(|f| spells(&f.text)) {
                return Some(super::loc(file, enc, d.key));
            }
            let text = std::fs::read_to_string(uri_to_path(&uri)?).ok()?;
            if !spells(&text) {
                return None;
            }
            let lines = LineIndex::new(&text);
            let pos = |o| crate::analysis::position_in(&lines, enc, o);
            Some(Location {
                uri: super::uri(&uri),
                range: lsp_types::Range::new(pos(d.key.start), pos(d.key.end)),
            })
        }
    }
}

/// A workspace context file's URI and text, by its URL: its analysis's
/// text, else its text on disk.
pub(crate) fn context_file(k: &Knowledge, url: &str) -> Option<(String, Arc<str>)> {
    let uri = normalize_uri(url);
    if let Some(f) = k.get(&uri) {
        let text = f.text.clone();
        return Some((uri, text));
    }
    let text = std::fs::read_to_string(uri_to_path(&uri)?).ok()?;
    Some((uri, Arc::from(text)))
}

/// The prefixes a new name typed at `offset` may use: the document's (and
/// the settings'), and in JSON-LD the prefix terms of the active context.
pub(crate) fn prefixes_at(a: &Analysis, offset: u32) -> PrefixMap {
    let mut m = a.prefix_map();
    if let Some(ctx) = a.source_index.context_at(offset) {
        for (t, d) in ctx.terms() {
            if let Some(iri) = d.iri.as_deref().filter(|i| d.prefix && !is_keyword(i)) {
                m.entry(t.to_string()).or_insert_with(|| iri.to_string());
            }
        }
    }
    m
}

/// Collects text edits by document URI, each range once.
#[derive(Default)]
pub(crate) struct Changes {
    pub(crate) by_uri: HashMap<Uri, Vec<TextEdit>>,
}

impl Changes {
    pub(crate) fn push(&mut self, uri: &str, range: lsp_types::Range, new_text: String) {
        let edits = self.by_uri.entry(super::uri(uri)).or_default();
        if !edits.iter().any(|e| e.range == range) {
            edits.push(TextEdit { range, new_text });
        }
    }

    /// `edits` (byte spans) of `text`, the text of `uri`.
    fn push_spans(
        &mut self,
        uri: &str,
        text: &str,
        enc: Encoding,
        edits: impl IntoIterator<Item = (Span, String)>,
    ) {
        let lines = LineIndex::new(text);
        let pos = |o| crate::analysis::position_in(&lines, enc, o);
        for (span, t) in edits {
            self.push(
                uri,
                lsp_types::Range::new(pos(span.start), pos(span.end)),
                t,
            );
        }
    }
}

/// Adds the edits renaming IRI `old` to `new` in JSON-LD document `a` (and
/// in the workspace context files whose definitions it uses). `Err`, naming
/// the document, when a use there cannot be rewritten.
pub(crate) fn iri_rename(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    old: &str,
    new: &str,
    out: &mut Changes,
) -> Result<(), String> {
    let mut files: HashMap<String, (String, Arc<str>)> = HashMap::new();
    let mut text_of = |url: &str| {
        if !files.contains_key(url) {
            let f = context_file(k, url)?;
            files.insert(url.to_string(), f);
        }
        files.get(url).map(|(_, t)| t.clone())
    };
    let edits = rename::iri_edits(&a.text, &a.source_index, old, new, &mut text_of)
        .map_err(|e| format!("can't rename in {}: {e}", super::shown(&a.uri)))?;
    for e in edits {
        match &e.origin {
            Origin::File(url) => {
                if let Some((uri, text)) = files.get(&**url) {
                    out.push_spans(uri, text, enc, [(e.span, e.text)]);
                }
            }
            _ => out.push(&a.uri, a.range(enc, e.span), e.text),
        }
    }
    Ok(())
}

/// `Err` when term `t` of `a` is defined by a bundled or remote context,
/// which can't be renamed.
pub(crate) fn check_term_renamable(a: &Analysis, t: TermAt) -> Result<(), String> {
    let d = a.source_index.definition(t.definition);
    match &d.origin {
        Origin::Remote(url) => Err(format!(
            "'{}' is defined by the context {url}, which is not in the workspace; it can't be renamed",
            d.term
        )),
        _ => Ok(()),
    }
}

/// Renames the term (or keyword alias) `t` of `a` to `new_name`: its
/// definition's key and every use resolving to it, here and, for a term of
/// a workspace context file, in that file and every workspace document
/// using it. A term of a bundled or remote context is refused.
pub(crate) fn term_rename(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    t: TermAt,
    new_name: &str,
) -> Result<HashMap<Uri, Vec<TextEdit>>, String> {
    check_term_renamable(a, t)?;
    rename::check_term_name(new_name)?;
    let ix = &a.source_index;
    let d = ix.definition(t.definition);
    let mut out = Changes::default();
    if *d.term == *new_name {
        return Ok(out.by_uri);
    }
    rename::check_renamable(d)?;
    let r = rename::TermRename {
        term: &d.term,
        new: new_name,
        prefix: rename::term_definition(ix, d).is_some_and(|td| td.prefix),
    };
    // The text holding the definition; the documents using it besides.
    let (file_uri, mut docs): (String, Vec<&Analysis>) = match &d.origin {
        Origin::Remote(_) => unreachable!("refused by check_term_renamable"),
        Origin::Document => {
            let document = |o: &Origin| *o == Origin::Document;
            let edits = rename::term_edits(
                &r,
                &a.text,
                Some(ix),
                Some(t.definition),
                &document,
                Some(d.key),
            )?;
            out.push_spans(&a.uri, &a.text, enc, edits);
            (normalize_uri(&a.uri), Vec::new())
        }
        Origin::File(url) => {
            let (file_uri, file_text) =
                context_file(k, url).ok_or_else(|| format!("can't read the context file {url}"))?;
            let edits = rename::term_edits(&r, &file_text, None, None, &|_| false, Some(d.key))
                .map_err(|e| format!("{e} (in {})", super::shown(&file_uri)))?;
            out.push_spans(&file_uri, &file_text, enc, edits);
            (file_uri, vec![a])
        }
    };
    // Every workspace document that has the definition in one of its
    // contexts, through the file.
    for (_, src, other) in k.docs() {
        if src != Source::External
            && other.parsed.syntax == Syntax::JsonLd
            && other.uri != a.uri
            && normalize_uri(&other.uri) != file_uri
        {
            docs.push(other);
        }
    }
    let same = |source: &str, key: Span, term: &str| {
        source.starts_with("file:")
            && normalize_uri(source) == file_uri
            && key == d.key
            && term == &*d.term
    };
    for doc in docs {
        let dix = &doc.source_index;
        let id = dix
            .definitions
            .iter()
            .position(|x| matches!(&x.origin, Origin::File(u) if same(u, x.key, &x.term)))
            .map(|i| DefId(i as u32));
        let in_scope = id.is_some()
            || dix.scopes().iter().any(|s| {
                dix.context(s).term(&d.term).is_some_and(|td| {
                    td.source
                        .as_deref()
                        .is_some_and(|u| same(u, td.span.unwrap_or_default(), &d.term))
                })
            });
        if !in_scope {
            continue;
        }
        let in_file = |o: &Origin| matches!(o, Origin::File(u) if normalize_uri(u) == file_uri);
        let edits = rename::term_edits(&r, &doc.text, Some(dix), id, &in_file, None)
            .map_err(|e| format!("{e} (in {})", super::shown(&doc.uri)))?;
        out.push_spans(&doc.uri, &doc.text, enc, edits);
    }
    Ok(out.by_uri)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Workspace, path_to_uri};
    use std::path::PathBuf;

    /// A fresh directory (canonical), removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let d =
                std::env::temp_dir().join(format!("turtle-lsp-nav-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            TempDir(std::fs::canonicalize(&d).unwrap())
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `text` (one line) with `edits` applied.
    fn apply(text: &str, edits: &[TextEdit]) -> String {
        let mut edits = edits.to_vec();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start.character));
        let mut out = text.to_string();
        for e in edits {
            assert_eq!(e.range.start.line, 0);
            let (s, t) = (
                e.range.start.character as usize,
                e.range.end.character as usize,
            );
            out.replace_range(s..t, &e.new_text);
        }
        out
    }

    /// A prefix of a workspace context file, renamed from a document: every
    /// document using the file gets its uses renamed and what its own
    /// context writes with the prefix, whether or not it uses the prefix
    /// outside its context, so each expands as before.
    #[test]
    fn a_context_file_prefix_is_renamed_in_each_document_s_own_context_too() {
        let dir = TempDir::new("prefix-rename");
        let files = [
            (
                "ctx.jsonld",
                r#"{"@context": {"old": "http://ex.org/", "name": "old:name"}}"#,
            ),
            (
                "a.jsonld",
                r#"{"@context": "ctx.jsonld", "@id": "http://ex.org/a", "old:p": 1}"#,
            ),
            (
                "b.jsonld",
                r#"{"@context": ["ctx.jsonld", {"x": "old:x"}], "@id": "http://ex.org/b", "x": 1, "old:q": 2}"#,
            ),
            (
                "c.jsonld",
                r#"{"@context": ["ctx.jsonld", {"y": {"@id": "old:y"}}], "@id": "http://ex.org/c", "y": 1}"#,
            ),
        ];
        let mut ws = Workspace::new(vec![dir.0.clone()], PrefixMap::default());
        let uri = |name: &str| path_to_uri(&dir.0.join(name)).unwrap();
        for (name, text) in files {
            std::fs::write(dir.0.join(name), text).unwrap();
            ws.open_doc(uri(name), text.into(), 1);
        }
        let triples = |ws: &mut Workspace, name: &str| {
            let a = ws.analysis(&uri(name)).unwrap();
            assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
            let mut t: Vec<String> = a.parsed.triples.iter().map(|t| format!("{t:?}")).collect();
            t.sort();
            t
        };
        let before: Vec<Vec<String>> = files.iter().map(|f| triples(&mut ws, f.0)).collect();
        let a = ws.analysis(&uri("a.jsonld")).unwrap();
        let at = files[1].1.find("old:p").unwrap() as u32 + 1;
        let t = term_at(&a, at).expect("the prefix of old:p");
        let changes = term_rename(&a, ws.knowledge(), Encoding::Utf16, t, "neu").unwrap();
        let renamed: Vec<(&str, String)> = files
            .iter()
            .map(|(name, text)| {
                let edits = changes
                    .iter()
                    .find(|(u, _)| u.as_str() == uri(name))
                    .map_or(&[][..], |(_, e)| &e[..]);
                (*name, apply(text, edits))
            })
            .collect();
        assert_eq!(
            renamed[2].1,
            r#"{"@context": ["ctx.jsonld", {"x": "neu:x"}], "@id": "http://ex.org/b", "x": 1, "neu:q": 2}"#
        );
        assert_eq!(
            renamed[3].1,
            r#"{"@context": ["ctx.jsonld", {"y": {"@id": "neu:y"}}], "@id": "http://ex.org/c", "y": 1}"#
        );
        // Each expands as before.
        for (name, text) in &renamed {
            ws.open_doc(uri(name), text.clone(), 2);
        }
        ws.context_changed(&uri("ctx.jsonld"), false);
        for (i, (name, _)) in files.iter().enumerate() {
            assert_eq!(triples(&mut ws, name), before[i], "{name}");
        }
    }
}
