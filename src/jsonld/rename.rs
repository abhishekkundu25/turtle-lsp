//! Rename in JSON-LD: the edits that rename an IRI or a term in one text,
//! from the document's [`SourceIndex`]. The editor feature
//! (`features::navigation`) gathers them across the workspace.
//!
//! - **An IRI** ([`iri_edits`]): every use keeps its form. A compact IRI is
//!   shortened with the context's prefixes, a `@vocab`-relative word stays
//!   relative while the new IRI is under `@vocab`, an absolute IRI stays
//!   absolute (JSON has no `<...>`), a relative `@id` stays relative to the
//!   base while it can. A use through a term is left alone: its
//!   definition's IRI is rewritten once, in its own form.
//! - **A term or keyword alias** ([`term_edits`]): its definition's key,
//!   every use that resolves to that definition (only the prefix part of a
//!   compact IRI), and what the contexts write with it: a definition's
//!   `@id`, `@reverse`, `@type`, `@index` or `@nest`, `@vocab`, and
//!   compact-IRI keys of the term as a prefix.
//!
//! Each rewritten form is checked by expanding it again in the active
//! context of the use: a form that would not give the new IRI (or that is a
//! term, which would bring the term's coercions) is not written. When no
//! form passes, or the context cannot be trusted (the old form does not
//! expand to the old IRI in it), the new IRI is written absolute.

use super::context::{ActiveContext, TermDefinition, expand_iri};
use super::json::{self, Json, Object, Value};
use super::source::{DefId, Definition, Origin, SourceIndex, Use, UseKind, UseRole};
use super::unquoted;
use crate::syntax::Span;
use std::fmt::Write as _;
use std::sync::Arc;

/// A text edit: `span` of the text named by `origin` (this document's, or a
/// workspace context file's) becomes `text`, already escaped for a JSON
/// string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub origin: Origin,
    pub span: Span,
    pub text: String,
}

/// `s` escaped for the inside of a JSON string.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// The text at `span`, when it is written without escapes (so it is the
/// string's value).
fn raw(text: &str, span: Span) -> Option<&str> {
    text.get(span.start as usize..span.end as usize)
        .filter(|s| !s.contains('\\'))
}

// ---- IRIs ----

/// The edits in a JSON-LD document (`text`, indexed as `ix`) that rename IRI
/// `old` to `new`. Definitions in a workspace context file are edited in
/// its text, which `context_text` gives by URL. `Err` (and no edits) when a
/// use of `old` cannot be rewritten: through a term of a bundled or remote
/// context, a datatype coerced by a term's `@type`, a context file's term
/// whose key is its IRI, a term also used as a prefix; or when the rename
/// would give an object two equal keys.
pub fn iri_edits(
    text: &str,
    ix: &SourceIndex,
    old: &str,
    new: &str,
    context_text: &mut dyn FnMut(&str) -> Option<Arc<str>>,
) -> Result<Vec<Edit>, String> {
    let mut edits = Vec::new();
    // Each definition once, with the first use that goes through it.
    let mut through: Vec<(DefId, Span)> = Vec::new();
    let mut seen = rustc_hash::FxHashSet::default();
    for u in &ix.uses {
        // A datatype a term coerces its values to is written in the
        // term's definition, not at a use.
        if let Some(id) = u.definition()
            && seen.insert(id)
            && term_definition(ix, ix.definition(id))
                .is_some_and(|td| td.type_mapping.as_deref() == Some(old))
        {
            return Err(format!(
                "<{old}> is the datatype of the term '{}' (its @type), which rename does not edit",
                ix.definition(id).term
            ));
        }
        if u.iri.as_deref() != Some(old) {
            continue;
        }
        match u.kind {
            UseKind::Term { definition } => {
                if !through.iter().any(|(d, _)| *d == definition) {
                    through.push((definition, u.span));
                }
            }
            UseKind::Compact { .. }
            | UseKind::VocabRelative
            | UseKind::Absolute
            | UseKind::Relative => {
                let written = rewrite_use(ix.context_at(u.span.start), raw(text, u.span), u, new);
                edits.push(Edit {
                    origin: Origin::Document,
                    span: u.span,
                    text: json_escape(&written),
                });
            }
            UseKind::Keyword | UseKind::Alias { .. } | UseKind::BlankNode => {}
        }
    }
    for (id, at) in through {
        definition_edits(text, ix, id, at, new, context_text, &mut edits)?;
    }
    let here: Vec<(Span, String)> = edits
        .iter()
        .filter(|e| e.origin == Origin::Document)
        .map(|e| (e.span, e.text.clone()))
        .collect();
    check_duplicate_keys(text, &here)?;
    Ok(edits)
}

/// `Err` when `edits` would make a key of a JSON object of `text` equal to
/// another key of that object, as written.
pub fn check_duplicate_keys(text: &str, edits: &[(Span, String)]) -> Result<(), String> {
    fn walk(text: &str, v: &Json, edits: &[(Span, String)]) -> Result<(), String> {
        match &v.value {
            Value::Object(o) => {
                let mut keys: Vec<(&str, bool)> = Vec::new();
                for m in &o.members {
                    let span = unquoted(m.key_span);
                    let written = text
                        .get(span.start as usize..span.end as usize)
                        .unwrap_or("");
                    keys.push(match edits.iter().find(|(s, _)| *s == span) {
                        Some((_, t)) => (t.as_str(), true),
                        None => (written, false),
                    });
                }
                for (i, (k, changed)) in keys.iter().enumerate() {
                    if *changed && keys.iter().enumerate().any(|(j, (o, _))| j != i && o == k) {
                        return Err(format!(
                            "the rename would write the key \"{k}\" twice in one object"
                        ));
                    }
                }
                o.members
                    .iter()
                    .try_for_each(|m| walk(text, &m.value, edits))
            }
            Value::Array(a) => a.items.iter().try_for_each(|i| walk(text, i, edits)),
            _ => Ok(()),
        }
    }
    if edits.is_empty() {
        return Ok(());
    }
    match &json::parse(text).root {
        Some(root) => walk(text, root, edits),
        None => Ok(()),
    }
}

/// The IRI-expansion flags (document-relative, `@vocab`) a use of `role`
/// may have been expanded with.
fn flags(role: UseRole) -> &'static [(bool, bool)] {
    match role {
        UseRole::Key => &[(false, true)],
        UseRole::Type => &[(true, true)],
        // A plain `@id`, or a value a term coerces to `@vocab`.
        UseRole::Id => &[(true, false), (true, true)],
    }
}

/// How use `u` (written `written`, in `ctx`) writes `new`.
fn rewrite_use(ctx: Option<&ActiveContext>, written: Option<&str>, u: &Use, new: &str) -> String {
    let (Some(ctx), Some(written)) = (ctx, written) else {
        return new.to_string();
    };
    // The flags under which the use, as written, gives its IRI in `ctx`;
    // none: `ctx` is not the context it was expanded in.
    let flags: Vec<(bool, bool)> = flags(u.role)
        .iter()
        .copied()
        .filter(|&(d, v)| expand_iri(ctx, written, d, v).as_deref() == u.iri.as_deref())
        .collect();
    if flags.is_empty() {
        return new.to_string();
    }
    let gives = |c: &str| {
        flags
            .iter()
            .all(|&(d, v)| expand_iri(ctx, c, d, v).as_deref() == Some(new))
    };
    let forms = match u.kind {
        UseKind::Compact { prefix_len, .. } => {
            compact_forms(ctx, new, written.get(..prefix_len as usize))
        }
        UseKind::VocabRelative => {
            let mut f: Vec<String> = vocab_form(ctx, new).into_iter().collect();
            f.extend(compact_forms(ctx, new, None));
            f
        }
        UseKind::Relative => relative_forms(ctx.base(), new, written),
        _ => Vec::new(),
    };
    pick(ctx, new, forms, gives)
}

/// The first of `forms`, then the absolute IRI, then a compact IRI, that
/// `gives` the new IRI and is not a term of `ctx`; else the absolute IRI.
fn pick(
    ctx: &ActiveContext,
    new: &str,
    forms: Vec<String>,
    gives: impl Fn(&str) -> bool,
) -> String {
    let ok = |c: &str| ctx.term(c).is_none() && gives(c);
    forms
        .into_iter()
        .chain([new.to_string()])
        .chain(compact_forms(ctx, new, None))
        .find(|c| ok(c))
        .unwrap_or_else(|| new.to_string())
}

/// `new` as compact IRIs with the prefix terms of `ctx`: `preferred` first,
/// then the longest prefix IRI.
fn compact_forms(ctx: &ActiveContext, new: &str, preferred: Option<&str>) -> Vec<String> {
    let mut prefixes: Vec<(&str, &str)> = ctx
        .terms()
        .filter(|(t, d)| d.prefix && !t.is_empty() && *t != "_" && !t.contains(':'))
        .filter_map(|(t, d)| {
            let iri = d.iri.as_deref()?;
            let suffix = new.strip_prefix(iri)?;
            (!iri.starts_with('@') && !suffix.starts_with("//")).then_some((t, iri))
        })
        .collect();
    prefixes.sort_by(|a, b| {
        (Some(b.0) == preferred, b.1.len(), a.0).cmp(&(Some(a.0) == preferred, a.1.len(), b.0))
    });
    prefixes
        .into_iter()
        .map(|(t, iri)| format!("{t}:{}", &new[iri.len()..]))
        .collect()
}

/// `new` relative to `@vocab`, if it is under it.
fn vocab_form(ctx: &ActiveContext, new: &str) -> Option<String> {
    let local = new.strip_prefix(ctx.vocab()?)?;
    (!local.is_empty()).then(|| local.to_string())
}

/// `new` relative to `base`: a fragment, or a path from the base's
/// directory (the fragment first when `written` is one).
fn relative_forms(base: Option<&str>, new: &str, written: &str) -> Vec<String> {
    let Some(base) = base else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let document = base.split('#').next().unwrap_or(base);
    if let Some(f) = new.strip_prefix(document).and_then(|r| r.strip_prefix('#')) {
        out.push(format!("#{f}"));
    }
    let path = base.split(['?', '#']).next().unwrap_or(base);
    if let Some(dir) = path.rfind('/').map(|i| &path[..=i])
        && let Some(r) = new.strip_prefix(dir).filter(|r| !r.is_empty())
    {
        out.push(r.to_string());
    }
    if !written.starts_with('#') {
        out.reverse();
    }
    out
}

/// The edits for the uses of `old` through term definition `id` (first
/// used at `at`): its IRI, rewritten where it is written. A definition
/// whose key is its IRI is renamed too, at its key and its uses. `Err` when
/// that cannot be done.
fn definition_edits(
    text: &str,
    ix: &SourceIndex,
    id: DefId,
    at: Span,
    new: &str,
    context_text: &mut dyn FnMut(&str) -> Option<Arc<str>>,
    edits: &mut Vec<Edit>,
) -> Result<(), String> {
    let d = ix.definition(id);
    let term = &d.term;
    let fail = |why: &str| {
        Err(format!(
            "<{new}> can't replace the IRI of the term '{term}': {why}"
        ))
    };
    let Some(old) = d.iri.as_deref() else {
        return Ok(());
    };
    let Some(ctx) = ix.context_at(at.start) else {
        return fail("its context is not known");
    };
    match &d.origin {
        Origin::Remote(url) => {
            return fail(&format!(
                "it is defined by {url}, which is not in the workspace"
            ));
        }
        Origin::File(url) if d.iri_span.is_none() || term_is_iri(term) => {
            return fail(&format!(
                "its key in {url} is its IRI; rename the term there first"
            ));
        }
        _ => {}
    }
    // A prefix keeps its IRI: changing it would change its compact IRIs'.
    if ix
        .uses_of(id)
        .any(|u| matches!(u.kind, UseKind::Compact { .. }))
    {
        return fail("it is also the prefix of compact IRIs");
    }
    // A definition's IRI is expanded relative to `@vocab`, not the base.
    let gives = |c: &str, iri: &str| expand_iri(ctx, c, false, true).as_deref() == Some(iri);
    // A key that is an IRI (or a compact IRI, or has no `@id`) must expand
    // to the term's IRI: the term is renamed too, at its key and uses.
    let key_is_iri = d.iri_span.is_none() || term_is_iri(term);
    let mut out = Vec::new();
    if key_is_iri {
        let Some(written) = raw(text, d.key).filter(|w| *w == &**term) else {
            return fail("its key is written with escapes");
        };
        let Some(key) = definition_forms(ctx, written, new)
            .into_iter()
            .chain([new.to_string()])
            .find(|c| ctx.term(c).is_none() && gives(c, new))
        else {
            return fail("its key is its IRI, and no key for the new IRI is free");
        };
        let mut uses: Vec<Span> = ix.uses_of(id).map(|u| u.span).collect();
        // An `@id` that spells the key (ignored by JSON-LD) follows it.
        uses.extend(own_id(text, d.key));
        if uses.iter().any(|&s| raw(text, s) != Some(written)) {
            return fail("a use is written with escapes");
        }
        for span in std::iter::once(d.key).chain(uses) {
            out.push(Edit {
                origin: Origin::Document,
                span,
                text: json_escape(&key),
            });
        }
    }
    if let Some(span) = d.iri_span {
        let source: Arc<str> = match &d.origin {
            Origin::File(url) => match context_text(url) {
                Some(t) => t,
                None => return fail(&format!("{url} can't be read")),
            },
            _ => Arc::from(text),
        };
        // As written, it still gives the old IRI (a context file may have
        // changed since the document was read).
        let Some(written) = raw(&source, span).filter(|w| gives(w, old)) else {
            return fail(
                "its definition no longer reads as the old IRI (has the context changed?)",
            );
        };
        let forms = definition_forms(ctx, written, new);
        out.push(Edit {
            origin: d.origin.clone(),
            span,
            text: json_escape(&pick(ctx, new, forms, |c| gives(c, new))),
        });
    }
    edits.extend(out);
    Ok(())
}

/// The `@id` of the term definition whose key is at `key`, when it spells
/// the key.
fn own_id(text: &str, key: Span) -> Option<Span> {
    fn find(v: &Json, key: Span) -> Option<&Json> {
        match &v.value {
            Value::Object(o) => o.members.iter().find_map(|m| {
                if unquoted(m.key_span) == key {
                    Some(&m.value)
                } else {
                    find(&m.value, key)
                }
            }),
            Value::Array(a) => a.items.iter().find_map(|i| find(i, key)),
            _ => None,
        }
    }
    let doc = json::parse(text);
    let id = find(doc.root.as_ref()?, key)?.as_object()?.get("@id")?;
    let term = text.get(key.start as usize..key.end as usize)?;
    (id.value.as_str()? == term).then(|| unquoted(id.value.span))
}

/// A term that must expand to its own IRI: one with a `/`, or a `:`
/// neither first nor last (JSON-LD 1.1 Create Term Definition, 14.2).
fn term_is_iri(term: &str) -> bool {
    term.contains('/')
        || term
            .char_indices()
            .any(|(i, c)| c == ':' && i > 0 && i + 1 < term.len())
}

/// The forms of `new` for a definition's IRI written `written`: compact
/// with the same prefix, `@vocab`-relative for a word, else none (absolute).
fn definition_forms(ctx: &ActiveContext, written: &str, new: &str) -> Vec<String> {
    match written.split_once(':') {
        Some((p, rest)) if !rest.starts_with("//") && ctx.term(p).is_some_and(|d| d.prefix) => {
            compact_forms(ctx, new, Some(p))
        }
        Some(_) => Vec::new(),
        None => {
            let mut f: Vec<String> = vocab_form(ctx, new).into_iter().collect();
            f.extend(compact_forms(ctx, new, None));
            f
        }
    }
}

// ---- terms ----

/// Why `name` cannot be a term's new name, if it cannot.
pub fn check_term_name(name: &str) -> Result<(), String> {
    let bad = name.is_empty()
        || name.starts_with('@')
        || name.contains(|c: char| {
            c.is_whitespace() || c.is_control() || matches!(c, '"' | '\\' | ':' | '/')
        });
    if bad {
        Err(format!(
            "'{name}' is not a valid term: it must not be empty, start with '@', or contain whitespace, quotes, backslashes, a colon or a slash"
        ))
    } else {
        Ok(())
    }
}

/// Why definition `d` cannot be renamed as a term, if it cannot: its key is
/// its IRI (it has no `@id`), so another key would be another IRI.
pub fn check_renamable(d: &Definition) -> Result<(), String> {
    match d.iri_span {
        Some(_) => Ok(()),
        None => Err(format!(
            "'{}' has no @id: its key is its IRI, so renaming it would change the IRI; rename the IRI instead",
            d.term
        )),
    }
}

/// A term (or keyword alias) being renamed.
#[derive(Clone, Copy, Debug)]
pub struct TermRename<'a> {
    pub term: &'a str,
    pub new: &'a str,
    /// The term is a prefix: compact IRIs written with it are renamed too.
    pub prefix: bool,
}

/// The term definition `d` of `ix` is, as the active contexts hold it.
pub fn term_definition<'i>(ix: &'i SourceIndex, d: &Definition) -> Option<&'i TermDefinition> {
    ix.scopes().iter().find_map(|s| {
        let t = ix.context(s).term(&d.term)?;
        let same_origin = match &d.origin {
            Origin::Document => t.source.is_none(),
            Origin::File(u) | Origin::Remote(u) => t.source.as_deref() == Some(&**u),
        };
        (same_origin && t.span == Some(d.key)).then_some(t)
    })
}

/// The edits renaming `r` in one text: the definition's key at `key` (when
/// the text holds it), the uses of definition `id` of the text's index
/// `ix` (and, for a prefix, of the compact-IRI terms written with it where
/// it is written: `same_origin`), and what the text's contexts write with
/// the term. `Err` when the
/// rename would change the meaning of the text: the new name is already a
/// term or used, or the contexts write the term where another definition of
/// it may be meant.
pub fn term_edits(
    r: &TermRename,
    text: &str,
    ix: Option<&SourceIndex>,
    id: Option<DefId>,
    same_origin: &dyn Fn(&Origin) -> bool,
    key: Option<Span>,
) -> Result<Vec<(Span, String)>, String> {
    let mut edits: Vec<(Span, String)> = Vec::new();
    let changed = |what: &str| format!("'{}' is written differently {what}", r.term);
    let new = r.new.to_string();
    let prefixed = |s: &str, name: &str| {
        s.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|rest| !rest.starts_with("//"))
    };
    let prefix_part = |span: Span| Span::new(span.start, span.start + r.term.len() as u32);
    if let Some(key) = key {
        if raw(text, key) != Some(r.term) {
            return Err(changed("at its definition (has the file changed?)"));
        }
        edits.push((key, new.clone()));
    }
    if let Some(ix) = ix {
        // The new name is not a term or a use already.
        let taken = |s: &str| s == r.new || prefixed(s, r.new);
        if ix.definitions.iter().any(|d| &*d.term == r.new)
            || ix
                .scopes()
                .iter()
                .any(|s| ix.context(s).term(r.new).is_some())
        {
            return Err(format!("'{}' is already a term here", r.new));
        }
        if ix.uses.iter().any(|u| {
            text.get(u.span.start as usize..u.span.end as usize)
                .is_some_and(taken)
        }) {
            return Err(format!("'{}' is already used in this document", r.new));
        }
        for u in &ix.uses {
            let written = raw(text, u.span);
            match u.kind {
                UseKind::Term { definition } | UseKind::Alias { definition }
                    if Some(definition) == id =>
                {
                    if written != Some(r.term) {
                        return Err(changed("in a use"));
                    }
                    edits.push((u.span, new.clone()));
                }
                UseKind::Compact { prefix, .. } if Some(prefix) == id => {
                    if !written.is_some_and(|w| prefixed(w, r.term)) {
                        return Err(changed("in a compact IRI"));
                    }
                    edits.push((prefix_part(u.span), new.clone()));
                }
                // A use of a compact-IRI term (`"ex:knows"` defined as
                // a key) written with this prefix, in the same context.
                UseKind::Term { definition } | UseKind::Alias { definition }
                    if r.prefix
                        && prefixed(&ix.definition(definition).term, r.term)
                        && same_origin(&ix.definition(definition).origin) =>
                {
                    if !written.is_some_and(|w| prefixed(w, r.term)) {
                        return Err(changed("in a use"));
                    }
                    edits.push((prefix_part(u.span), new.clone()));
                }
                _ => {}
            }
        }
    }
    // What the contexts write with the term.
    let mut scan = Scan {
        r,
        text,
        refs: Vec::new(),
        defs: 0,
        err: None,
    };
    if let Some(root) = &json::parse(text).root {
        scan.walk(root);
    }
    if let Some(e) = scan.err {
        return Err(e);
    }
    let own = usize::from(key.is_some());
    if !scan.refs.is_empty() && scan.defs > own {
        return Err(format!(
            "'{}' is defined more than once and its contexts refer to it; rename it by hand",
            r.term
        ));
    }
    edits.extend(scan.refs.into_iter().map(|s| (s, new.clone())));
    edits.sort_by_key(|(s, _)| (s.start, s.end));
    edits.dedup_by_key(|(s, _)| *s);
    check_duplicate_keys(text, &edits)?;
    Ok(edits)
}

/// The places the `@context`s of a text write a term: as an IRI (a term
/// definition's string, `@id`, `@reverse`, `@type`, `@index`, `@nest`, or
/// `@vocab`) or, for a prefix, at the start of a compact IRI (there, or as
/// a key).
struct Scan<'a> {
    r: &'a TermRename<'a>,
    text: &'a str,
    /// The spans to replace with the new name.
    refs: Vec<Span>,
    /// How many definitions of the term the contexts write. Known
    /// limitation: scoping is not followed. A reference outside the scope of
    /// the definition being renamed (say, in a context processed before it)
    /// is rewritten too; only a term defined more than once is refused.
    defs: usize,
    err: Option<String>,
}

impl Scan<'_> {
    fn walk(&mut self, v: &Json) {
        match &v.value {
            Value::Object(o) => {
                for m in &o.members {
                    if &*m.key == "@context" {
                        self.context(&m.value);
                    } else {
                        self.walk(&m.value);
                    }
                }
            }
            Value::Array(a) => a.items.iter().for_each(|i| self.walk(i)),
            _ => {}
        }
    }

    fn context(&mut self, v: &Json) {
        match &v.value {
            Value::Array(a) => a.items.iter().for_each(|i| self.context(i)),
            Value::Object(o) => self.map(o),
            _ => {}
        }
    }

    fn map(&mut self, o: &Object) {
        for m in &o.members {
            let k = &*m.key;
            if k == "@vocab" {
                self.reference(&m.value);
                continue;
            }
            if k.starts_with('@') {
                continue;
            }
            if k == self.r.term {
                self.defs += 1;
            } else {
                self.name(k, unquoted(m.key_span), false);
            }
            match &m.value.value {
                Value::String(_) => self.reference(&m.value),
                Value::Object(d) => {
                    for dm in &d.members {
                        match &*dm.key {
                            "@id" | "@reverse" | "@type" | "@index" | "@nest" => {
                                self.reference(&dm.value)
                            }
                            "@context" => self.context(&dm.value),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn reference(&mut self, v: &Json) {
        if let Some(s) = v.as_str() {
            self.name(s, unquoted(v.span), true);
        }
    }

    /// A string `s` written at `span`, read as an IRI (`whole`: the term
    /// alone refers to it too).
    fn name(&mut self, s: &str, span: Span, whole: bool) {
        let (term, new) = (self.r.term, self.r.new);
        let after = |name: &str| s.strip_prefix(name).and_then(|r| r.strip_prefix(':'));
        if s == new || after(new).is_some_and(|r| !r.starts_with("//")) {
            self.err = Some(format!("'{new}' is already used in a context"));
            return;
        }
        let target = if whole && s == term {
            span
        } else if self.r.prefix && after(term).is_some_and(|r| !r.starts_with("//")) {
            Span::new(span.start, span.start + term.len() as u32)
        } else {
            return;
        };
        if raw(self.text, target) == Some(term) {
            self.refs.push(target);
        } else {
            self.err = Some(format!(
                "'{term}' is written with escapes in a context; rename it by hand"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonld::{NoNetwork, parse_document};
    use crate::syntax::{GraphName, Node, Severity};
    use oxrdf::dataset::CanonicalizationAlgorithm;
    use oxrdf::{
        BlankNode, Dataset, GraphName as G, Literal, NamedNode, NamedOrBlankNode, Quad, Term,
    };

    const BASE: &str = "file:///ws/doc.jsonld";
    const CTX_URL: &str = "file:///ws/people.context.jsonld";

    // ---- the graph check ----

    /// The toRdf of `text` as a dataset with canonical blank nodes, each
    /// IRI `old` written `new` (`map`).
    fn graph(text: &str, base: &str, loader: &NoNetwork, map: Option<(&str, &str)>) -> Dataset {
        let parsed = parse_document(text, Some(base), loader).parsed;
        let errors: Vec<_> = parsed
            .errors
            .iter()
            .filter(|e| e.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}\n{text}");
        let iri = |i: &str| {
            let i = match map {
                Some((old, new)) if i == old => new,
                _ => i,
            };
            NamedNode::new_unchecked(i)
        };
        let blank = |b: u32| BlankNode::new_unchecked(format!("b{b}"));
        let term = |n: &Node| -> Term {
            match n {
                Node::Iri(i) => iri(i.as_str()).into(),
                Node::Blank(b) => blank(b.0).into(),
                Node::Literal {
                    value,
                    datatype,
                    lang: Some(l),
                } => {
                    let _ = datatype;
                    Literal::new_language_tagged_literal_unchecked(&**value, l.as_str()).into()
                }
                Node::Literal {
                    value, datatype, ..
                } => Literal::new_typed_literal(&**value, iri(datatype.as_str())).into(),
                Node::Triple(_) => panic!("JSON-LD has no triple terms"),
            }
        };
        let subject = |n: &Node| -> NamedOrBlankNode {
            match term(n) {
                Term::NamedNode(n) => n.into(),
                Term::BlankNode(b) => b.into(),
                t => panic!("not a subject: {t}"),
            }
        };
        let mut d: Dataset = parsed
            .triples
            .iter()
            .map(|t| {
                let g = match t.graph {
                    None => G::DefaultGraph,
                    Some(GraphName::Iri(i)) => iri(i.as_str()).into(),
                    Some(GraphName::Blank(b)) => blank(b.0).into(),
                };
                Quad::new(subject(&t.s), iri(t.p.as_str()), term(&t.o), g)
            })
            .collect();
        d.canonicalize(CanonicalizationAlgorithm::Unstable);
        d
    }

    /// Checks that `after` (read with `after_loader`) has the graph of
    /// `before` with IRI `old` renamed `new` (`map`), and nothing else
    /// changed.
    fn same_graph(
        before: &str,
        after: &str,
        base: &str,
        loaders: (&NoNetwork, &NoNetwork),
        map: Option<(&str, &str)>,
    ) -> Result<(), String> {
        let want = graph(before, base, loaders.0, map);
        assert!(!want.is_empty(), "no triples in\n{before}");
        let got = graph(after, base, loaders.1, None);
        if want == got {
            return Ok(());
        }
        let only =
            |a: &Dataset, b: &Dataset| a.iter().find(|q| !b.contains(*q)).map(|q| q.to_string());
        Err(format!(
            "graphs differ ({} quads expected, {} got); expected only: {:?}; got only: {:?}\n--- after:\n{after}",
            want.len(),
            got.len(),
            only(&want, &got),
            only(&got, &want),
        ))
    }

    /// `text` with `edits` applied.
    fn apply(text: &str, edits: impl IntoIterator<Item = (Span, String)>) -> String {
        let mut edits: Vec<_> = edits.into_iter().collect();
        edits.sort_by_key(|(s, _)| std::cmp::Reverse(s.start));
        let mut out = text.to_string();
        for (s, t) in edits {
            out.replace_range(s.start as usize..s.end as usize, &t);
        }
        out
    }

    /// Renames IRI `old` to `new` in `text` (and the context file `ctx`);
    /// checks the graph; returns the new document and context file texts.
    fn rename_iri(text: &str, ctx: &str, old: &str, new: &str) -> (String, String) {
        let loader = NoNetwork::default().with(CTX_URL, ctx);
        let ix = parse_document(text, Some(BASE), &loader).index;
        let ctx_text: Arc<str> = Arc::from(ctx);
        let edits = iri_edits(text, &ix, old, new, &mut |url| {
            (url == CTX_URL).then(|| ctx_text.clone())
        })
        .unwrap_or_else(|e| panic!("{old} -> {new}: {e}"));
        let (mut doc, mut file) = (Vec::new(), Vec::new());
        for e in edits {
            match e.origin {
                Origin::Document => doc.push((e.span, e.text)),
                Origin::File(u) if &*u == CTX_URL => file.push((e.span, e.text)),
                o => panic!("edit in {o:?}"),
            }
        }
        let (after, ctx_after) = (apply(text, doc), apply(ctx, file));
        let after_loader = NoNetwork::default().with(CTX_URL, ctx_after.as_str());
        same_graph(
            text,
            &after,
            BASE,
            (&loader, &after_loader),
            Some((old, new)),
        )
        .unwrap_or_else(|e| panic!("{old} -> {new}: {e}"));
        (after, ctx_after)
    }

    /// Renames the term defined (or used) at the first `"<at>"` of `text`
    /// to `new`, in this document; checks the graph is the same.
    fn rename_term(text: &str, at: &str, new: &str) -> Result<String, String> {
        let loader = NoNetwork::default().with(CTX_URL, CTX);
        let ix = parse_document(text, Some(BASE), &loader).index;
        let offset = text.find(&format!("\"{at}\"")).unwrap() as u32 + 1;
        let id = ix
            .definition_at(offset)
            .or_else(|| ix.use_at(offset).and_then(Use::definition))
            .expect("a term");
        let d = ix.definition(id);
        check_term_name(new)?;
        check_renamable(d)?;
        let r = TermRename {
            term: &d.term,
            new,
            prefix: term_definition(&ix, d).is_some_and(|t| t.prefix),
        };
        let document = |o: &Origin| *o == Origin::Document;
        let edits = term_edits(&r, text, Some(&ix), Some(id), &document, Some(d.key))?;
        let after = apply(text, edits);
        same_graph(text, &after, BASE, (&loader, &loader), None).unwrap_or_else(|e| panic!("{e}"));
        Ok(after)
    }

    const CTX: &str = r#"{"@context": {"ex": "https://ex.example/", "nickname": "ex:nickname", "homepage": {"@id": "https://ex.example/homepage", "@type": "@id"}}}"#;

    const DOC: &str = r##"{
  "@context": [
    "people.context.jsonld",
    {
      "@vocab": "https://ex.example/",
      "@base": "https://data.example/people/",
      "foaf": "http://xmlns.com/foaf/0.1/",
      "name": "foaf:name",
      "knows": {"@id": "http://xmlns.com/foaf/0.1/knows", "@type": "@id"},
      "colleague": {"@id": "ex:colleague", "@type": "@vocab"},
      "id": "@id"
    }
  ],
  "@graph": [
    {"id": "ann", "@type": "Person", "name": "Ann", "knows": "bob", "nickname": "Annie",
     "foaf:age": 41, "ex:weight": 3, "https://ex.example/colour": "red", "colleague": "Person"},
    {"@id": "#bob", "@type": "foaf:Person", "foaf:name": "Bob", "homepage": "https://ex.example/bob"},
    {"@id": "https://ex.example/Person", "http://www.w3.org/2000/01/rdf-schema#label": "Person"}
  ]
}"##;

    #[test]
    fn a_vocab_relative_iri_stays_relative_or_becomes_compact_or_absolute() {
        let person = "https://ex.example/Person";
        // Still under @vocab: relative; the @id and the vocab-coerced value
        // keep their own forms.
        let (after, _) = rename_iri(DOC, CTX, person, "https://ex.example/Human");
        assert!(after.contains(r#""@type": "Human""#), "{after}");
        assert!(after.contains(r#""colleague": "Human""#), "{after}");
        assert!(
            after.contains(r#""@id": "https://ex.example/Human""#),
            "{after}"
        );
        // Out of @vocab, under a prefix: compact.
        let (after, _) = rename_iri(DOC, CTX, person, "http://xmlns.com/foaf/0.1/Human");
        assert!(after.contains(r#""@type": "foaf:Human""#), "{after}");
        assert!(after.contains(r#""colleague": "foaf:Human""#), "{after}");
        // Under neither: absolute, without angle brackets.
        let (after, _) = rename_iri(DOC, CTX, person, "http://other.example/Human");
        assert!(
            after.contains(r#""@type": "http://other.example/Human""#),
            "{after}"
        );
        assert!(!after.contains('<'), "{after}");
        // A word that is a term is not written relative.
        let (after, _) = rename_iri(DOC, CTX, person, "https://ex.example/knows");
        assert!(after.contains(r#""@type": "ex:knows""#), "{after}");
    }

    #[test]
    fn compact_and_absolute_iris_keep_their_form() {
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "https://ex.example/weight",
            "https://ex.example/mass",
        );
        assert!(after.contains(r#""ex:mass": 3"#), "{after}");
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "http://xmlns.com/foaf/0.1/age",
            "https://ex.example/years",
        );
        assert!(after.contains(r#""ex:years": 41"#), "{after}");
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "http://xmlns.com/foaf/0.1/age",
            "http://other.example/age",
        );
        assert!(
            after.contains(r#""http://other.example/age": 41"#),
            "{after}"
        );
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "https://ex.example/colour",
            "https://ex.example/color",
        );
        assert!(
            after.contains(r#""https://ex.example/color": "red""#),
            "{after}"
        );
    }

    #[test]
    fn a_relative_id_stays_relative_to_the_base_while_it_can() {
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "https://data.example/people/ann",
            "https://data.example/people/anne",
        );
        assert!(after.contains(r#""id": "anne""#), "{after}");
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "https://data.example/people/bob",
            "https://data.example/people/robert",
        );
        assert!(after.contains(r#""knows": "robert""#), "{after}");
        assert!(after.contains(r##""@id": "#bob""##), "{after}");
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "https://data.example/people/#bob",
            "https://data.example/people/#robert",
        );
        assert!(after.contains(r##""@id": "#robert""##), "{after}");
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "https://data.example/people/ann",
            "https://elsewhere.example/ann",
        );
        assert!(
            after.contains(r#""id": "https://elsewhere.example/ann""#),
            "{after}"
        );
    }

    #[test]
    fn a_use_through_a_term_rewrites_the_definition_once_and_keeps_the_keys() {
        // Compact in the definition, two uses.
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "http://xmlns.com/foaf/0.1/name",
            "http://xmlns.com/foaf/0.1/fullName",
        );
        assert!(after.contains(r#""name": "foaf:fullName""#), "{after}");
        assert!(after.contains(r#""name": "Ann""#), "{after}");
        assert!(after.contains(r#""foaf:fullName": "Bob""#), "{after}");
        // Absolute in the definition.
        let (after, _) = rename_iri(
            DOC,
            CTX,
            "http://xmlns.com/foaf/0.1/knows",
            "http://xmlns.com/foaf/0.1/met",
        );
        assert!(
            after.contains(r#""@id": "http://xmlns.com/foaf/0.1/met""#),
            "{after}"
        );
        assert!(after.contains(r#""knows": "bob""#), "{after}");
        // In the workspace context file.
        let (after, ctx) = rename_iri(
            DOC,
            CTX,
            "https://ex.example/nickname",
            "https://ex.example/alias",
        );
        assert_eq!(after, DOC);
        assert!(ctx.contains(r#""nickname": "ex:alias""#), "{ctx}");
        let (_, ctx) = rename_iri(
            DOC,
            CTX,
            "https://ex.example/homepage",
            "http://xmlns.com/foaf/0.1/homepage",
        );
        assert!(
            ctx.contains(r#""@id": "http://xmlns.com/foaf/0.1/homepage""#),
            "{ctx}"
        );
    }

    #[test]
    fn a_term_whose_key_is_its_iri_is_renamed_with_its_uses() {
        let text = r#"{"@context": {"ex": "https://ex.example/", "ex:knows": {"@type": "@id"}},
  "@id": "ex:a", "ex:knows": "ex:b"}"#;
        let (after, _) = rename_iri(
            text,
            CTX,
            "https://ex.example/knows",
            "https://ex.example/met",
        );
        assert!(after.contains(r#""ex:met": {"@type": "@id"}"#), "{after}");
        assert!(after.contains(r#""ex:met": "ex:b""#), "{after}");
    }

    #[test]
    fn renaming_a_term_changes_only_the_term() {
        let after = rename_term(DOC, "name", "fullName").unwrap();
        assert_eq!(after, DOC.replace(r#""name":"#, r#""fullName":"#));
        let after = rename_term(DOC, "id", "ident").unwrap();
        assert_eq!(after, DOC.replace(r#""id":"#, r#""ident":"#));
        // A prefix: its compact IRIs, in uses and in the context, too.
        let after = rename_term(DOC, "foaf", "f").unwrap();
        assert!(
            after.contains(r#""f": "http://xmlns.com/foaf/0.1/""#),
            "{after}"
        );
        assert!(after.contains(r#""name": "f:name""#), "{after}");
        assert!(after.contains(r#""f:age": 41"#), "{after}");
        assert!(after.contains(r#""@type": "f:Person""#), "{after}");
        assert!(!after.contains("\"foaf"), "{after}");
        let text = r#"{"@context": {"@vocab": "ex:", "ex": "https://ex.example/", "ex:knows": {"@type": "ex:T"}},
  "@id": "ex:a", "ex:knows": "x", "p": 1}"#;
        let after = rename_term(text, "ex", "e").unwrap();
        assert_eq!(after, text.replace("\"ex", "\"e"));
    }

    #[test]
    fn renaming_a_term_is_refused_where_it_would_change_the_graph() {
        for bad in ["", "@x", "a b", "a:b", "a/b", "a\"b"] {
            assert!(rename_term(DOC, "name", bad).is_err(), "{bad:?}");
        }
        // Already a term, here or in the context file; already used.
        assert!(rename_term(DOC, "name", "knows").is_err());
        assert!(rename_term(DOC, "name", "nickname").is_err());
        assert!(rename_term(DOC, "name", "Person").is_err());
        assert!(rename_term(DOC, "foaf", "ex").is_err());
        // A term whose key is its IRI.
        let text =
            r#"{"@context": {"@vocab": "https://ex.example/", "p": {"@type": "@id"}}, "p": "x"}"#;
        assert!(rename_term(text, "p", "q").is_err());
        // Defined twice, and referred to in a context.
        let text = r#"{"@context": {"ex": "https://ex.example/", "p": "ex:p"},
  "ex:q": {"@context": {"ex": "https://other.example/", "r": "ex:r"}, "r": 1}, "p": 2}"#;
        assert!(rename_term(text, "ex", "e").is_err());
    }

    /// `iri_edits` of `text` (a remote context at `REMOTE`).
    fn try_rename_iri(text: &str, old: &str, new: &str) -> Result<Vec<Edit>, String> {
        let loader = NoNetwork::default().with(CTX_URL, CTX).with(
            "https://remote.example/ctx",
            r#"{"@context": {"far": "https://far.example/far", "xsd": "http://www.w3.org/2001/XMLSchema#"}}"#,
        );
        let ix = parse_document(text, Some(BASE), &loader).index;
        iri_edits(text, &ix, old, new, &mut |_| None)
    }

    #[test]
    fn an_iri_rename_that_cannot_reach_every_use_is_refused() {
        // Through a term of a remote context.
        let text = r#"{"@context": "https://remote.example/ctx", "@id": "https://ex.example/a", "far": 1}"#;
        let e = try_rename_iri(text, "https://far.example/far", "https://far.example/near")
            .unwrap_err();
        assert!(e.contains("https://remote.example/ctx"), "{e}");
        // The datatype of a term.
        let text = r#"{"@context": {"ex": "https://ex.example/", "d": {"@id": "ex:d", "@type": "ex:T"}},
  "@id": "ex:a", "d": "x", "ex:T": 1}"#;
        assert!(try_rename_iri(text, "https://ex.example/T", "https://ex.example/U").is_err());
        // A term that is also a prefix.
        let text =
            r#"{"@context": {"ex": "https://ex.example/"}, "@id": "ex:a", "ex": {"@id": "ex:b"}}"#;
        assert!(try_rename_iri(text, "https://ex.example/", "https://other.example/").is_err());
        // Nothing is refused where every use is written in the document.
        let text = r#"{"@context": {"ex": "https://ex.example/"}, "@id": "ex:a", "ex:p": 1}"#;
        assert_eq!(
            try_rename_iri(text, "https://ex.example/p", "https://ex.example/q")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_rename_that_would_repeat_a_key_in_an_object_is_refused() {
        let text = r#"{"@context": {"ex": "https://ex.example/"}, "@id": "ex:a", "ex:p": 1, "ex:q": 2, "ex:r": {"ex:s": 3}}"#;
        let e = try_rename_iri(text, "https://ex.example/q", "https://ex.example/p").unwrap_err();
        assert!(e.contains("ex:p"), "{e}");
        // Another object may have the key.
        assert!(try_rename_iri(text, "https://ex.example/s", "https://ex.example/p").is_ok());
        // A term renamed to a key written in the same object (not a use).
        let text =
            r#"{"@context": {"ex": "https://ex.example/", "p": "ex:p"}, "p": 1, "@id": "ex:a"}"#;
        assert!(
            check_duplicate_keys(
                text,
                &[(
                    Span::new(
                        text.rfind("\"p\"").unwrap() as u32 + 1,
                        text.rfind("\"p\"").unwrap() as u32 + 2
                    ),
                    "@id".into()
                )]
            )
            .is_err()
        );
    }

    // ---- the corpus ----

    /// Renames an IRI used in each form in a sample of the corpus (at most 120
    /// documents, spread over its sources; those that read without errors),
    /// and each term written in them;
    /// every graph must stay the same apart from the renamed IRI.
    #[test]
    fn corpus_renames_keep_the_graph() {
        use std::path::Path;
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus/jsonld");
        let Ok(index) = std::fs::read_to_string(dir.join("index.tsv")) else {
            eprintln!("skipping: test/corpus/jsonld not present");
            return;
        };
        let mut loader = NoNetwork::default();
        let mut docs = Vec::new();
        for line in index.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            match cols[..] {
                ["doc", path, base, ..] => docs.push((path, base)),
                ["context", path, iri, ..] => {
                    let text = std::fs::read_to_string(dir.join(path)).unwrap();
                    loader = loader.with(iri, text.as_str());
                }
                _ => {}
            }
        }
        type IsForm = fn(&UseKind) -> bool;
        let forms: [(&str, IsForm); 5] = [
            ("compact", |k| matches!(k, UseKind::Compact { .. })),
            ("vocab", |k| matches!(k, UseKind::VocabRelative)),
            ("absolute", |k| matches!(k, UseKind::Absolute)),
            ("relative", |k| matches!(k, UseKind::Relative)),
            ("term", |k| matches!(k, UseKind::Term { .. })),
        ];
        let (mut read, mut iris, mut terms, mut refused) = (0, 0, 0, 0);
        let mut by_form = [0; 5];
        let mut failures = Vec::new();
        let step = docs.len().div_ceil(120).max(1);
        for (path, base) in docs.into_iter().step_by(step) {
            let text = std::fs::read_to_string(dir.join(path)).unwrap();
            let doc = parse_document(&text, Some(base), &loader);
            if doc
                .parsed
                .errors
                .iter()
                .any(|e| e.severity == Severity::Error)
            {
                continue;
            }
            read += 1;
            let ix = &doc.index;
            // The datatypes: a term's coercion is not a use, so not renamed.
            let datatypes: Vec<&str> = doc
                .parsed
                .triples
                .iter()
                .filter_map(|t| match &t.o {
                    Node::Literal { datatype, .. } => Some(datatype.as_str()),
                    _ => None,
                })
                .collect();
            // Every use of the IRI is one this rename edits.
            let renamable = |iri: &str| {
                oxiri::Iri::parse(iri).is_ok()
                    && !datatypes.contains(&iri)
                    && ix
                        .uses
                        .iter()
                        .filter(|u| u.iri.as_deref() == Some(iri))
                        .all(|u| {
                            let Some(id) = u.definition() else {
                                return true;
                            };
                            let d = ix.definition(id);
                            d.origin == Origin::Document
                                && !ix
                                    .uses_of(id)
                                    .any(|x| matches!(x.kind, UseKind::Compact { .. }))
                        })
            };
            for (i, (form, is)) in forms.iter().enumerate() {
                let Some(old) = ix
                    .uses
                    .iter()
                    .filter(|u| is(&u.kind))
                    .filter_map(|u| u.iri.as_deref())
                    .find(|iri| renamable(iri))
                else {
                    continue;
                };
                let cut = old.rfind(['/', '#', ':']).map_or(0, |i| i + 1);
                for new in [
                    format!("{}Renamed", old),
                    format!("{}renamed{}", &old[..cut], "Thing"),
                    "http://renamed.example/Thing".to_string(),
                ] {
                    let edits = iri_edits(&text, ix, old, &new, &mut |_| None)
                        .unwrap_or_else(|e| panic!("{path}: {old} -> {new}: {e}"));
                    assert!(edits.iter().all(|e| e.origin == Origin::Document));
                    let after = apply(&text, edits.into_iter().map(|e| (e.span, e.text)));
                    iris += 1;
                    by_form[i] += 1;
                    if let Err(e) =
                        same_graph(&text, &after, base, (&loader, &loader), Some((old, &new)))
                    {
                        failures.push(format!(
                            "{path}: {form} {old} -> {new}: {}",
                            e.lines().next().unwrap_or("")
                        ));
                    }
                }
            }
            // Each term (and alias) written in the document and used.
            for (n, d) in ix.definitions.iter().enumerate() {
                let id = DefId(n as u32);
                if d.origin != Origin::Document
                    || d.iri_span.is_none()
                    || ix.uses_of(id).next().is_none()
                {
                    continue;
                }
                let r = TermRename {
                    term: &d.term,
                    new: "zzRenamed",
                    prefix: term_definition(ix, d).is_some_and(|t| t.prefix),
                };
                let document = |o: &Origin| *o == Origin::Document;
                match term_edits(&r, &text, Some(ix), Some(id), &document, Some(d.key)) {
                    Err(_) => refused += 1,
                    Ok(edits) => {
                        terms += 1;
                        let after = apply(&text, edits);
                        if let Err(e) = same_graph(&text, &after, base, (&loader, &loader), None) {
                            failures.push(format!(
                                "{path}: term {}: {}",
                                d.term,
                                e.lines().next().unwrap_or("")
                            ));
                        }
                    }
                }
            }
        }
        eprintln!(
            "{read} documents: {iris} IRI renames (compact {}, vocab {}, absolute {}, relative {}, term {}), {terms} term renames, {refused} refused",
            by_form[0], by_form[1], by_form[2], by_form[3], by_form[4]
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
