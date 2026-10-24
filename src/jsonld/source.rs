//! The JSON-LD source index: what expansion learns about the text of a
//! document, kept for the editor features ([`SourceIndex`]).
//!
//! - **Uses** ([`Use`]): every key, `@type` value and `@id` value (and each
//!   string a term coerces to `@id` or `@vocab`, and each key of an `@id` or
//!   `@type` map) that expansion reads, with its span (without quotes), what
//!   it expands to, and how it is written ([`UseKind`]). A key that only
//!   looks like a keyword (`@foo`) is not a use.
//! - **Definitions** ([`Definition`]): each term definition a use resolves
//!   to, and each one written in the document, applied or not. Uses refer
//!   to them by [`DefId`], so "the same definition" is an equality.
//! - **Context strings** ([`ContextString`]): each string of a context
//!   expansion processed (a term's key and IRI, `@id`, `@type`, `@vocab`...)
//!   with the term definitions it names where it is written, for the lints
//!   and for rename. Contexts that expansion does not read (inside a `@json`
//!   literal, under a key it drops) are not here.
//! - **Scopes** ([`Scope`]): each JSON object expansion reads, with the
//!   active context in force inside it ([`SourceIndex::scope_at`]).
//!
//! Spans are byte offsets into the document, except a definition's spans,
//! which are in the text of the document it is written in
//! ([`Definition::origin`]).

use super::context::{
    ActiveContext, TermDefinition, has_keyword_form, is_absolute_iri, is_keyword,
};
use super::json::{Json, Member, Object, Value};
use crate::syntax::Span;
use rustc_hash::FxHashMap;
use std::sync::Arc;

/// A term definition in [`SourceIndex::definitions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DefId(pub u32);

/// Where a term definition is written.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Origin {
    /// In this document.
    Document,
    /// In a workspace context file (a `file:` URL).
    File(Arc<str>),
    /// In a bundled or remote context (or one it `@import`s): not in the
    /// workspace.
    Remote(Arc<str>),
}

impl Origin {
    fn of(source: Option<&Arc<str>>) -> Origin {
        match source {
            None => Origin::Document,
            Some(url) if url.starts_with("file:") => Origin::File(url.clone()),
            Some(url) => Origin::Remote(url.clone()),
        }
    }

    /// The context document's URL; `None` for this document.
    #[cfg(test)]
    pub(crate) fn url(&self) -> Option<&str> {
        match self {
            Origin::Document => None,
            Origin::File(u) | Origin::Remote(u) => Some(u),
        }
    }
}

/// A term definition and where it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Definition {
    pub(crate) term: Arc<str>,
    pub(crate) origin: Origin,
    /// The term's key, without quotes, in the origin's text.
    pub(crate) key: Span,
    /// The IRI mapping's string (`@id`, `@reverse` or the whole
    /// definition), without quotes, in the origin's text; `None` when the
    /// key spells the IRI.
    pub(crate) iri_span: Option<Span>,
    /// What the term maps to: an IRI, a blank node identifier or a keyword
    /// (an alias); `None` for a term mapped to `null`.
    pub(crate) iri: Option<Arc<str>>,
}

/// What a use is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum UseRole {
    /// A key of a JSON object.
    Key,
    /// A `@type` value, or a key of a `@type` map.
    Type,
    /// An `@id` value, a key of an `@id` map, or a string a term coerces to
    /// `@id` or `@vocab`.
    Id,
}

/// How a use is written: the steps of IRI Expansion (JSON-LD 1.1
/// Processing Algorithms §4.3) that gave its IRI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum UseKind {
    /// A keyword (the use's `iri`).
    Keyword,
    /// A term aliasing a keyword (the use's `iri`).
    Alias { definition: DefId },
    /// A term of the active context, whose IRI is its definition's.
    Term { definition: DefId },
    /// `prefix:suffix` with a prefix term of the active context.
    Compact { prefix_len: u32, prefix: DefId },
    /// A word appended to `@vocab`.
    VocabRelative,
    /// An absolute IRI as written (including `scheme:rest` whose scheme is
    /// no term).
    Absolute,
    /// A blank node identifier (`_:b`).
    BlankNode,
    /// Anything else: a relative IRI (an `@id` resolved against the base,
    /// if any), or a key the context does not define.
    Relative,
}

/// A key or value expansion read, in text order in
/// [`SourceIndex::uses`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Use {
    /// The string, without quotes.
    pub(crate) span: Span,
    pub(crate) role: UseRole,
    /// What it expands to: an IRI (relative when no base resolved it), a
    /// blank node identifier or a keyword; `None` for a term mapped to
    /// `null`.
    pub(crate) iri: Option<Arc<str>>,
    pub(crate) kind: UseKind,
}

impl Use {
    /// The definition of the term (or alias) this use is.
    pub(crate) fn definition(&self) -> Option<DefId> {
        match self.kind {
            UseKind::Term { definition } | UseKind::Alias { definition } => Some(definition),
            _ => None,
        }
    }
}

/// What a [`ContextString`] is in its context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextRole {
    /// The key of a term defined by an object with no `@id` or `@reverse`:
    /// the key is the term's IRI (or compact IRI).
    KeyIsIri,
    /// The key of any other term definition.
    Key,
    /// A term definition written as a string: its IRI.
    Value,
    /// A definition's `@id` or `@reverse`.
    Id,
    /// A definition's `@type`.
    Type,
    /// A definition's `@index`.
    Index,
    /// A definition's `@nest`.
    Nest,
    /// The context's `@vocab`.
    Vocab,
}

impl ContextRole {
    /// The string may be a term as a whole (a key is only ever read for the
    /// prefix of its compact IRI).
    pub(crate) fn names_a_term(self) -> bool {
        !matches!(self, ContextRole::Key | ContextRole::KeyIsIri)
    }
}

/// A string of a context expansion processed, with the term definitions it
/// names where it is written. Only strings that are read as IRIs or terms
/// are here, not `@container` or `@language` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextString {
    /// The string, without quotes.
    pub(crate) span: Span,
    pub(crate) role: ContextRole,
    /// The definition of the term this whole string is, in the contexts in
    /// force at the string (`None` for a key, a keyword, and a string that
    /// is no term there).
    pub(crate) term: Option<DefId>,
    /// The definition of the prefix before the first colon of the string
    /// (`ex` for `ex:name`), when the string is `prefix:suffix` and the
    /// prefix is a term there.
    pub(crate) prefix: Option<DefId>,
}

/// A JSON object expansion read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    /// From the `{` to the `}`, or to the last member when unclosed.
    pub(crate) span: Span,
    /// Has its `}`.
    pub(crate) closed: bool,
    context: u32,
    /// The enclosing scope (`NONE` for a top-level one).
    parent: u32,
    /// Where it ends for [`SourceIndex::scope_at`] (exclusive): after its
    /// `}`, or, unclosed, where its parent ends.
    limit: u32,
}

const NONE: u32 = u32::MAX;

/// What expansion records about the text of a JSON-LD document.
#[derive(Clone, Debug, Default)]
pub(crate) struct SourceIndex {
    /// Sorted by span.
    pub(crate) uses: Vec<Use>,
    /// Indexed by [`DefId`].
    pub(crate) definitions: Vec<Definition>,
    /// The strings of the contexts expansion processed, in text order.
    pub(crate) context_strings: Vec<ContextString>,
    /// Sorted by start, outer before inner.
    scopes: Vec<Scope>,
    contexts: Vec<ActiveContext>,
}

impl SourceIndex {
    pub(crate) fn definition(&self, id: DefId) -> &Definition {
        &self.definitions[id.0 as usize]
    }

    /// The use whose string contains `offset` (its end included).
    pub(crate) fn use_at(&self, offset: u32) -> Option<&Use> {
        let i = self.uses.partition_point(|u| u.span.start <= offset);
        self.uses[..i].last().filter(|u| u.span.contains(offset))
    }

    /// The uses of the term (or alias) `id`, and the compact IRIs whose
    /// prefix it is.
    pub(crate) fn uses_of(&self, id: DefId) -> impl Iterator<Item = &Use> {
        self.uses.iter().filter(move |u| match u.kind {
            UseKind::Term { definition } | UseKind::Alias { definition } => definition == id,
            UseKind::Compact { prefix, .. } => prefix == id,
            _ => false,
        })
    }

    /// The definition written in this document whose key contains `offset`.
    pub(crate) fn definition_at(&self, offset: u32) -> Option<DefId> {
        self.definitions
            .iter()
            .position(|d| d.origin == Origin::Document && d.key.contains(offset))
            .map(|i| DefId(i as u32))
    }

    /// Every scope, sorted by start, outer before inner.
    pub(crate) fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// The innermost scope whose object `offset` is inside (after its `{`,
    /// before its `}`). An unclosed object reaches as far as the object
    /// around it, so a cursor after a syntax error still finds the context
    /// it is typing in.
    pub(crate) fn scope_at(&self, offset: u32) -> Option<&Scope> {
        let mut i = self.scopes.partition_point(|s| s.span.start < offset);
        while i > 0 {
            let s = &self.scopes[i - 1];
            if offset < s.limit {
                return Some(s);
            }
            i = match s.parent {
                NONE => 0,
                p => p as usize + 1,
            };
        }
        None
    }

    /// The scope around `scope`.
    pub(crate) fn parent(&self, scope: &Scope) -> Option<&Scope> {
        self.scopes.get(scope.parent as usize)
    }

    /// The active context inside `scope`.
    pub(crate) fn context(&self, scope: &Scope) -> &ActiveContext {
        &self.contexts[scope.context as usize]
    }

    /// The active context at `offset`: [`SourceIndex::scope_at`]'s.
    pub(crate) fn context_at(&self, offset: u32) -> Option<&ActiveContext> {
        self.scope_at(offset).map(|s| self.context(s))
    }
}

/// Builds a [`SourceIndex`] as expansion goes.
#[derive(Default)]
pub(crate) struct Builder {
    index: SourceIndex,
    /// By (document, key start).
    ids: FxHashMap<(Option<Arc<str>>, u32), DefId>,
    /// Context by content id: an index into `contexts`.
    by_content: FxHashMap<u64, u32>,
    /// The strings of contexts, before the definitions they name are known.
    strings: Vec<Pending>,
}

/// A [`ContextString`] whose definitions are still being found: one written
/// in the document is known by where its key starts.
struct Pending {
    span: Span,
    role: ContextRole,
    term: Target,
    prefix: Target,
}

#[derive(Clone, Copy)]
enum Target {
    None,
    Def(DefId),
    /// The term written in this document with its key at this offset.
    Written(u32),
}

impl Builder {
    /// The id of `term`'s definition `d`.
    pub(crate) fn define(&mut self, term: &str, d: &TermDefinition) -> DefId {
        let key = d.span.unwrap_or_default();
        let next = DefId(self.index.definitions.len() as u32);
        let id = *self
            .ids
            .entry((d.source.clone(), key.start))
            .or_insert(next);
        if id == next {
            self.index.definitions.push(Definition {
                term: Arc::from(term),
                origin: Origin::of(d.source.as_ref()),
                key,
                iri_span: d.iri_span,
                iri: d.iri.clone(),
            });
        }
        id
    }

    /// How `s` is written, expanded in `ctx` (`vocab`: relative to
    /// `@vocab`, as keys and types are); `None` for a keyword-like string,
    /// which expands to nothing.
    pub(crate) fn classify(
        &mut self,
        ctx: &ActiveContext,
        s: &str,
        vocab: bool,
    ) -> Option<UseKind> {
        if is_keyword(s) {
            return Some(UseKind::Keyword);
        }
        if has_keyword_form(s) {
            return None;
        }
        if let Some(d) = ctx.term(s) {
            let alias = d.iri.as_deref().is_some_and(is_keyword);
            if alias {
                return Some(UseKind::Alias {
                    definition: self.define(s, d),
                });
            }
            if vocab {
                return Some(UseKind::Term {
                    definition: self.define(s, d),
                });
            }
        }
        if let Some(colon) = s.find(':').filter(|&i| i > 0) {
            let (prefix, suffix) = (&s[..colon], &s[colon + 1..]);
            if prefix == "_" {
                return Some(UseKind::BlankNode);
            }
            if suffix.starts_with("//") {
                return Some(UseKind::Absolute);
            }
            if let Some(d) = ctx.term(prefix).filter(|d| d.prefix && d.iri.is_some()) {
                return Some(UseKind::Compact {
                    prefix_len: colon as u32,
                    prefix: self.define(prefix, d),
                });
            }
            if is_absolute_iri(s) {
                return Some(UseKind::Absolute);
            }
        }
        if vocab && ctx.vocab().is_some() {
            return Some(UseKind::VocabRelative);
        }
        Some(UseKind::Relative)
    }

    pub(crate) fn push_use(&mut self, u: Use) {
        self.index.uses.push(u);
    }

    /// The JSON object at `span` (`closed`: it has its `}`) is read with
    /// `ctx`.
    pub(crate) fn scope(&mut self, span: Span, closed: bool, ctx: &ActiveContext) {
        let id = ctx.content_id();
        let contexts = &mut self.index.contexts;
        let same = |i: u32, contexts: &[ActiveContext]| {
            let c = &contexts[i as usize];
            c.content_id() == id && c.base() == ctx.base()
        };
        let context = match self.index.scopes.last() {
            Some(s) if same(s.context, contexts) => s.context,
            _ => match self.by_content.get(&id) {
                Some(&i) if same(i, contexts) => i,
                _ => {
                    let i = contexts.len() as u32;
                    contexts.push(ctx.clone());
                    self.by_content.insert(id, i);
                    i
                }
            },
        };
        self.index.scopes.push(Scope {
            span,
            closed,
            context,
            parent: NONE,
            limit: if closed { span.end } else { u32::MAX },
        });
    }

    /// Records the strings of the context `value` (an object, a URL, an
    /// array of them, or `null`), which expansion processed in `before` to
    /// get `after`.
    pub(crate) fn context_strings(
        &mut self,
        value: &Json,
        before: &ActiveContext,
        after: &ActiveContext,
    ) {
        let mut walk = Walk {
            builder: self,
            before,
            after,
            value: value.span,
            levels: Vec::new(),
        };
        walk.context(value);
    }

    /// The index, with the definitions `written` in the document.
    pub(crate) fn finish(mut self, written: Vec<(Arc<str>, Arc<TermDefinition>)>) -> SourceIndex {
        for (term, d) in written {
            self.define(&term, &d);
        }
        let ids = &self.ids;
        let known = |t: Target| match t {
            Target::None => None,
            Target::Def(id) => Some(id),
            Target::Written(key) => ids.get(&(None, key)).copied(),
        };
        let mut strings: Vec<ContextString> = self
            .strings
            .iter()
            .map(|p| ContextString {
                span: p.span,
                role: p.role,
                term: known(p.term),
                prefix: known(p.prefix),
            })
            .collect();
        // The same context read twice.
        strings.sort_by_key(|c| (c.span.start, c.span.end));
        strings.dedup_by_key(|c| c.span);
        self.index.context_strings = strings;
        let mut index = self.index;
        index.uses.sort_by_key(|u| (u.span.start, u.span.end));
        // The same string read twice (an `@id` map key with several values).
        index.uses.dedup_by_key(|u| u.span);
        let scopes = &mut index.scopes;
        scopes.sort_by_key(|s| (s.span.start, std::cmp::Reverse(s.span.end)));
        scopes.dedup_by_key(|s| s.span);
        // Parents: the enclosing scopes of each one, on a stack. An unclosed
        // scope reaches as far as its parent.
        let mut stack: Vec<u32> = Vec::new();
        for i in 0..scopes.len() {
            let start = scopes[i].span.start;
            while stack
                .last()
                .is_some_and(|&top| start >= scopes[top as usize].limit)
            {
                stack.pop();
            }
            if let Some(&top) = stack.last() {
                let limit = scopes[top as usize].limit;
                let s = &mut scopes[i];
                s.parent = top;
                if !s.closed {
                    s.limit = limit;
                }
            }
            stack.push(i as u32);
        }
        index
    }
}

/// The context objects of one `@context` value (or of the `@context` of a
/// term definition in it), with the terms each defines.
struct Level<'j> {
    frames: Vec<Frame<'j>>,
    /// The frame being read: later ones of the array are not yet applied.
    current: usize,
}

struct Frame<'j> {
    members: &'j [Member],
    /// Term (any key but a keyword) to where its key starts; with many
    /// members only.
    map: Option<FxHashMap<&'j str, u32>>,
}

const LINEAR_FRAME: usize = 8;

impl<'j> Frame<'j> {
    fn new(o: &'j Object) -> Frame<'j> {
        let map = (o.members.len() > LINEAR_FRAME).then(|| {
            o.members
                .iter()
                .filter(|m| !m.key.starts_with('@'))
                .map(|m| (&*m.key, m.key_inner.start))
                .collect()
        });
        Frame {
            members: &o.members,
            map,
        }
    }

    /// Where the key of the (last) definition of `word` starts.
    fn key_of(&self, word: &str) -> Option<u32> {
        match &self.map {
            Some(map) => map.get(word).copied(),
            None => self
                .members
                .iter()
                .rev()
                .find(|m| &*m.key == word)
                .map(|m| m.key_inner.start),
        }
    }
}

/// Reads a context value as expansion processed it.
struct Walk<'b, 'j> {
    builder: &'b mut Builder,
    before: &'b ActiveContext,
    after: &'b ActiveContext,
    /// Where the whole `@context` value is written.
    value: Span,
    levels: Vec<Level<'j>>,
}

impl<'j> Walk<'_, 'j> {
    /// The context `v`: an object, an array of contexts, or something that
    /// holds no strings of this document (a URL, `null`).
    fn context(&mut self, v: &'j Json) {
        let objects: Vec<&'j Object> = match &v.value {
            Value::Object(o) => vec![o],
            Value::Array(a) => a.items.iter().filter_map(Json::as_object).collect(),
            _ => return,
        };
        if objects.is_empty() {
            return;
        }
        self.levels.push(Level {
            frames: objects.iter().map(|&o| Frame::new(o)).collect(),
            current: 0,
        });
        let at = self.levels.len() - 1;
        for (i, o) in objects.iter().enumerate() {
            self.levels[at].current = i;
            self.object(o);
        }
        self.levels.pop();
    }

    fn object(&mut self, o: &'j Object) {
        for m in &o.members {
            let key = &*m.key;
            if key.starts_with('@') {
                if key == "@vocab" {
                    self.string(&m.value, ContextRole::Vocab);
                }
                continue;
            }
            match &m.value.value {
                Value::String(_) => {
                    self.key(m, ContextRole::Key);
                    self.string(&m.value, ContextRole::Value);
                }
                Value::Object(def) => {
                    let own_iri = def
                        .members
                        .iter()
                        .any(|d| matches!(&*d.key, "@id" | "@reverse"));
                    let role = if own_iri {
                        ContextRole::Key
                    } else {
                        ContextRole::KeyIsIri
                    };
                    self.key(m, role);
                    for d in &def.members {
                        match &*d.key {
                            "@id" | "@reverse" => self.string(&d.value, ContextRole::Id),
                            "@type" => self.string(&d.value, ContextRole::Type),
                            "@index" => self.string(&d.value, ContextRole::Index),
                            "@nest" => self.string(&d.value, ContextRole::Nest),
                            "@context" => self.context(&d.value),
                            _ => {}
                        }
                    }
                }
                _ => self.key(m, ContextRole::Key),
            }
        }
    }

    /// The key of the term definition `m`: only its prefix can name a term.
    fn key(&mut self, m: &'j Member, role: ContextRole) {
        let prefix = self.prefix(&m.key);
        self.builder.strings.push(Pending {
            span: m.key_inner,
            role,
            term: Target::None,
            prefix,
        });
    }

    /// The string `v`, if it is one.
    fn string(&mut self, v: &'j Json, role: ContextRole) {
        let Some(s) = v.as_str() else { return };
        let term = if s.starts_with('@') || s.is_empty() {
            Target::None
        } else {
            self.resolve(s)
        };
        let prefix = self.prefix(s);
        self.builder.strings.push(Pending {
            span: v.inner,
            role,
            term,
            prefix,
        });
    }

    /// The definition of the prefix of `s` (`ex` for `ex:name`) when it has
    /// one that is not a scheme-like `//` or a blank node label.
    fn prefix(&mut self, s: &str) -> Target {
        match s.find(':') {
            Some(i) if i > 0 && &s[..i] != "_" && !s[i + 1..].starts_with("//") => {
                self.resolve(&s[..i])
            }
            _ => Target::None,
        }
    }

    /// The definition of the term `word` where the string being read is: in
    /// the context object around it, else the ones before it in the same
    /// array and those around that (all of them: a term-scoped context is
    /// applied after its whole context), else what the contexts processed
    /// before this value, and the URLs in it, defined. Known limit: the
    /// contexts of the URLs of an array are only seen through the context
    /// the whole value gives, so a term a URL defines and a *later* object
    /// of the same array defines again is not found for the strings between
    /// them.
    fn resolve(&mut self, word: &str) -> Target {
        for (n, level) in self.levels.iter().enumerate().rev() {
            let top = if n + 1 == self.levels.len() {
                level.current
            } else {
                level.frames.len().saturating_sub(1)
            };
            if let Some(key) = level.frames[..=top]
                .iter()
                .rev()
                .find_map(|f| f.key_of(word))
            {
                return Target::Written(key);
            }
        }
        // A definition this value writes that was not found above comes
        // after the string: it is not in force there.
        let after = self.after.term(word).filter(|d| {
            !(d.source.is_none() && d.span.is_some_and(|s| self.value.contains(s.start)))
        });
        match after.or_else(|| self.before.term(word)) {
            Some(d) => Target::Def(self.builder.define(word, d)),
            None => Target::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonld::{NoNetwork, parse_document};
    use UseKind::*;
    use UseRole::*;

    const BASE: &str = "file:///ws/doc.jsonld";
    const CTX_URL: &str = "file:///ws/ctx.jsonld";
    const CTX: &str = r#"{"@context": {"shared": "http://shared.org/s"}}"#;

    fn at(text: &str, span: Span) -> &str {
        &text[span.start as usize..span.end as usize]
    }

    fn index(text: &str) -> SourceIndex {
        let loader = NoNetwork::default().with(CTX_URL, CTX).with(
            "https://remote.example/ctx",
            r#"{"@context": {"far": "http://far.org/f"}}"#,
        );
        parse_document(text, Some(BASE), &loader).index
    }

    /// The term of a definition and its key and IRI as written in `text`
    /// (this document's or the context file's).
    fn def(ix: &SourceIndex, id: DefId, text: &str) -> (String, String, Option<String>) {
        let d = ix.definition(id);
        let text = match d.origin {
            Origin::Document => text,
            Origin::File(_) => CTX,
            Origin::Remote(_) => r#"{"@context": {"far": "http://far.org/f"}}"#,
        };
        (
            d.term.to_string(),
            at(text, d.key).to_string(),
            d.iri_span.map(|s| at(text, s).to_string()),
        )
    }

    const DOC: &str = r#"{
  "@context": [
    "ctx.jsonld",
    {
      "@vocab": "http://vocab.org/",
      "ex": "http://ex.org/",
      "id": "@id",
      "name": "ex:name",
      "unused": "ex:unused",
      "Person": {"@id": "ex:Person", "@context": {"nick": "ex:nick"}}
    }
  ],
  "id": "ex:a",
  "@type": "Person",
  "name": "Ann",
  "ex:age": 41,
  "colour": "red",
  "shared": "x",
  "nick": "Annie",
  "ex:knows": {"@id": "_:b", "http://abs.org/p": 1, "rel": {"@id": "other"}}
}"#;

    /// A key recovered from an unquoted word (`false`) is in no string: no
    /// use, so no hover, rename or "Change to" fix there.
    #[test]
    fn an_unquoted_key_is_no_use() {
        let text = "{\"@context\": {\"@vocab\": \"http://schema.org/\"}, false \"k\": 1, \"p\": 2}";
        let ix = parse_document(text, Some(BASE), &NoNetwork::default()).index;
        let keys: Vec<&str> = ix.uses.iter().map(|u| at(text, u.span)).collect();
        assert_eq!(keys, ["@context", "p"]);
    }

    #[test]
    fn uses_record_how_each_key_type_and_id_is_written() {
        let ix = index(DOC);
        let got: Vec<_> = ix
            .uses
            .iter()
            .map(|u| (at(DOC, u.span), u.role, u.iri.as_deref().unwrap_or("-")))
            .collect();
        assert_eq!(
            got,
            [
                ("@context", Key, "@context"),
                ("id", Key, "@id"),
                ("ex:a", Id, "http://ex.org/a"),
                ("@type", Key, "@type"),
                ("Person", Type, "http://ex.org/Person"),
                ("name", Key, "http://ex.org/name"),
                ("ex:age", Key, "http://ex.org/age"),
                ("colour", Key, "http://vocab.org/colour"),
                ("shared", Key, "http://shared.org/s"),
                ("nick", Key, "http://ex.org/nick"),
                ("ex:knows", Key, "http://ex.org/knows"),
                ("@id", Key, "@id"),
                ("_:b", Id, "_:b"),
                ("http://abs.org/p", Key, "http://abs.org/p"),
                ("rel", Key, "http://vocab.org/rel"),
                ("@id", Key, "@id"),
                ("other", Id, "file:///ws/other"),
            ]
        );
        let kind = |s: &str| ix.uses.iter().find(|u| at(DOC, u.span) == s).unwrap().kind;
        let ex = match kind("ex:a") {
            Compact {
                prefix_len: 2,
                prefix,
            } => prefix,
            k => panic!("{k:?}"),
        };
        assert_eq!(
            def(&ix, ex, DOC),
            ("ex".into(), "ex".into(), Some("http://ex.org/".into()))
        );
        assert!(matches!(kind("ex:age"), Compact { prefix_len: 2, prefix } if prefix == ex));
        assert_eq!(kind("@type"), Keyword);
        assert_eq!(kind("colour"), VocabRelative);
        assert_eq!(kind("_:b"), BlankNode);
        assert_eq!(kind("http://abs.org/p"), Absolute);
        assert_eq!(kind("other"), Relative);
        // An alias and terms, with their definitions.
        let Alias { definition: id } = kind("id") else {
            panic!("{:?}", kind("id"))
        };
        assert_eq!(
            def(&ix, id, DOC),
            ("id".into(), "id".into(), Some("@id".into()))
        );
        let term = |s: &str| match kind(s) {
            Term { definition } => definition,
            k => panic!("{s}: {k:?}"),
        };
        assert_eq!(
            def(&ix, term("Person"), DOC),
            ("Person".into(), "Person".into(), Some("ex:Person".into()))
        );
        assert_eq!(
            def(&ix, term("name"), DOC),
            ("name".into(), "name".into(), Some("ex:name".into()))
        );
        // A term of the type-scoped context, written in this document.
        let nick = term("nick");
        assert_eq!(ix.definition(nick).origin, Origin::Document);
        assert_eq!(def(&ix, nick, DOC).2.as_deref(), Some("ex:nick"));
        // A term of the workspace context file: spans in the file's text.
        let shared = term("shared");
        assert_eq!(ix.definition(shared).origin, Origin::File(CTX_URL.into()));
        assert_eq!(
            def(&ix, shared, DOC),
            (
                "shared".into(),
                "shared".into(),
                Some("http://shared.org/s".into())
            )
        );
        // Uses of one definition, and lookups by offset.
        let name_uses: Vec<_> = ix.uses_of(term("name")).map(|u| at(DOC, u.span)).collect();
        assert_eq!(name_uses, ["name"]);
        let ex_uses: Vec<_> = ix.uses_of(ex).map(|u| at(DOC, u.span)).collect();
        assert_eq!(ex_uses, ["ex:a", "ex:age", "ex:knows"]);
        let colour = DOC.find("\"colour\"").unwrap() as u32;
        assert_eq!(
            ix.use_at(colour + 3).map(|u| at(DOC, u.span)),
            Some("colour")
        );
        assert_eq!(ix.use_at(colour), None);
    }

    #[test]
    fn every_definition_written_in_the_document_is_listed() {
        let ix = index(DOC);
        let mut written: Vec<_> = ix
            .definitions
            .iter()
            .filter(|d| d.origin == Origin::Document)
            .map(|d| (at(DOC, d.key), d.iri.as_deref().unwrap_or("-")))
            .collect();
        written.sort();
        assert_eq!(
            written,
            [
                ("Person", "http://ex.org/Person"),
                ("ex", "http://ex.org/"),
                ("id", "@id"),
                ("name", "http://ex.org/name"),
                ("nick", "http://ex.org/nick"),
                ("unused", "http://ex.org/unused"),
            ]
        );
        let unused = DOC.find("\"unused\"").unwrap() as u32 + 2;
        let id = ix.definition_at(unused).unwrap();
        assert_eq!(def(&ix, id, DOC).2.as_deref(), Some("ex:unused"));
        assert!(ix.uses_of(id).next().is_none());
    }

    /// The context strings as (text, role, term's key, prefix's key) where
    /// the keys are the text at the key of the definition each names.
    fn strings(text: &str) -> Vec<(String, ContextRole, Option<String>, Option<String>)> {
        let ix = index(text);
        let key = |id: Option<DefId>| {
            id.map(|id| {
                let d = ix.definition(id);
                let origin = match d.origin {
                    Origin::Document => text,
                    Origin::File(_) => CTX,
                    Origin::Remote(_) => "",
                };
                format!("{}@{}", at(origin, d.key), d.key.start)
            })
        };
        ix.context_strings
            .iter()
            .map(|c| {
                (
                    at(text, c.span).to_string(),
                    c.role,
                    key(c.term),
                    key(c.prefix),
                )
            })
            .collect()
    }

    #[test]
    fn context_strings_name_the_definitions_in_force_where_they_are_written() {
        let got = strings(DOC);
        let find = |s: &str| got.iter().filter(|g| g.0 == s).collect::<Vec<_>>();
        let ex = DOC.find("\"ex\"").unwrap() + 1;
        let ex_key = Some(format!("ex@{ex}"));
        // A prefix defined in the same object, for a string of the object
        // and of a context nested in a term.
        for s in ["ex:name", "ex:unused", "ex:Person", "ex:nick"] {
            let g = find(s);
            assert_eq!(g.len(), 1, "{s}");
            assert_eq!(g[0].3, ex_key, "{s}");
        }
        // Not terms, not prefixes: a keyword, a URL, an absolute IRI.
        assert_eq!(find("@id")[0].2, None);
        assert_eq!(find("http://vocab.org/")[0].3, None);
        // Keys: the prefix of a compact-IRI key is a prefix; roles tell a
        // key that is the IRI from one that has its own.
        let text = r#"{"@context": {"ex": "http://ex.org/", "ex:a": {"@type": "@id"}, "b": {"@id": "ex:b"}, "c": null}}"#;
        let got = strings(text);
        let roles: Vec<_> = got.iter().map(|g| (g.0.as_str(), g.1)).collect();
        assert_eq!(
            roles,
            [
                ("ex", ContextRole::Key),
                ("http://ex.org/", ContextRole::Value),
                ("ex:a", ContextRole::KeyIsIri),
                ("@id", ContextRole::Type),
                ("b", ContextRole::Key),
                ("ex:b", ContextRole::Id),
                ("c", ContextRole::Key),
            ]
        );
        let key_ex = Some(format!("ex@{}", text.find("\"ex\"").unwrap() + 1));
        assert_eq!(got[2].3, key_ex);
        assert_eq!(got[5].3, key_ex);
    }

    #[test]
    fn a_context_string_names_the_definition_of_its_own_scope() {
        // Two definitions of `ex`: the string names the one in force where
        // it is written.
        let text = r#"{"@context": [{"ex": "http://one.org/", "a": "ex:a"}, {"ex": "http://two.org/", "b": "ex:b"}],
  "@graph": [{"@context": {"ex": "http://three.org/", "c": "ex:c"}}]}"#;
        let got = strings(text);
        let prefix = |s: &str| got.iter().find(|g| g.0 == s).unwrap().3.clone().unwrap();
        let start = |n: usize| text.match_indices("\"ex\"").nth(n).unwrap().0 + 1;
        assert_eq!(prefix("ex:a"), format!("ex@{}", start(0)));
        assert_eq!(prefix("ex:b"), format!("ex@{}", start(1)));
        assert_eq!(prefix("ex:c"), format!("ex@{}", start(2)));
        // A definition of a later object of the array is not in force yet.
        let text = r#"{"@context": [{"a": "ex:a"}, {"ex": "http://two.org/"}]}"#;
        assert_eq!(strings(text)[1].3, None);
        // A context of the file named before it is.
        let text = r#"{"@context": ["ctx.jsonld", {"a": "shared"}]}"#;
        let got = strings(text);
        assert_eq!(got[1].2.as_deref().map(|s| &s[..6]), Some("shared"));
    }

    #[test]
    fn contexts_nothing_reads_leave_no_strings() {
        // Inside a @json literal, and under a key expansion drops.
        let json = r#"{"@context": {"j": {"@id": "http://ex.org/j", "@type": "@json"}}, "j": {"@context": {"a": "b:c"}}}"#;
        let got = strings(json);
        assert!(got.iter().all(|g| g.0 != "b:c"), "{got:?}");
        let dropped =
            r#"{"@context": {"ex": "http://ex.org/"}, "nope": {"@context": {"a": "b:c"}}}"#;
        assert!(strings(dropped).iter().all(|g| g.0 != "b:c"));
        let read = r#"{"@context": {"ex": "http://ex.org/"}, "ex:p": {"@context": {"a": "b:c"}}}"#;
        assert!(strings(read).iter().any(|g| g.0 == "b:c"));
    }

    #[test]
    fn terms_of_a_remote_context_are_not_in_the_workspace() {
        let text = r#"{"@context": "https://remote.example/ctx", "far": 1}"#;
        let ix = index(text);
        let u = ix.use_at(text.find("far\"").unwrap() as u32).unwrap();
        let d = ix.definition(u.definition().unwrap());
        assert_eq!(
            d.origin,
            Origin::Remote("https://remote.example/ctx".into())
        );
        assert_eq!(d.origin.url(), Some("https://remote.example/ctx"));
        assert_eq!(u.iri.as_deref(), Some("http://far.org/f"));
    }

    #[test]
    fn the_innermost_object_gives_the_context_at_an_offset() {
        let ix = index(DOC);
        let in_top = DOC.find("\"colour\"").unwrap() as u32;
        let top = ix.scope_at(in_top).unwrap();
        assert_eq!(top.span.start, 0);
        let ctx = ix.context(top);
        assert!(ctx.term("nick").is_some() && ctx.term("name").is_some());
        // The type-scoped context does not reach the nested node.
        let in_knows = DOC.find("\"http://abs.org/p\"").unwrap() as u32;
        let knows = ix.scope_at(in_knows).unwrap();
        assert!(at(DOC, knows.span).starts_with("{\"@id\": \"_:b\""));
        assert_eq!(ix.parent(knows), Some(top));
        let ctx = ix.context(knows);
        assert!(ctx.term("nick").is_none() && ctx.term("name").is_some());
        // Just before `{`, and just after `}`: the enclosing object.
        let open = DOC.find("{\"@id\": \"_:b\"").unwrap() as u32;
        assert_eq!(ix.scope_at(open), Some(top));
        assert_eq!(ix.scope_at(knows.span.end), Some(top));
        assert_eq!(ix.scope_at(knows.span.end - 1), Some(knows));
        assert_eq!(ix.scope_at(0), None);
        assert_eq!(ix.scope_at(DOC.len() as u32), None);
    }

    #[test]
    fn scopes_survive_a_syntax_error_before_the_cursor() {
        // A missing comma, then an unclosed object being typed in.
        let text = r#"{
  "@context": {"ex": "http://ex.org/"},
  "ex:a": 1
  "ex:b": {
    "@context": {"q": "ex:q"},
    "q": 2,
    "#;
        let p = parse_document(text, Some(BASE), &NoNetwork::default());
        assert!(p.parsed.errors.iter().any(|e| e.code == "json-syntax"));
        let ix = p.index;
        let s = ix.scope_at(text.len() as u32).unwrap();
        assert!(at(text, s.span).starts_with("{\n    \"@context\": {\"q\""));
        assert!(!s.closed);
        assert!(ix.context(s).term("q").is_some());
        // A closed object after the error.
        let text = r#"{"@context": {"ex": "http://ex.org/"}, "ex:a": 1 "ex:b": {"@context": {"q": "ex:q"}, "q": 2, }, "ex:c": 3}"#;
        let ix = index(text);
        let inner = text.find("\"q\": 2").unwrap() as u32 + 8;
        assert!(ix.context_at(inner).unwrap().term("q").is_some());
        let after = text.find("\"ex:c\"").unwrap() as u32;
        let ctx = ix.context_at(after).unwrap();
        assert!(ctx.term("q").is_none() && ctx.term("ex").is_some());
    }
}
