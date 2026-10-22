//! JSON-LD 1.1 expansion (JSON-LD 1.1 Processing Algorithms §5.1 Expansion
//! and §5.3 Value Expansion) of a [`Json`] document, keeping the span of
//! every key, `@id`, `@type` and value it reads.
//!
//! The result is a tree of [`Item`]s: node objects, value objects and list
//! objects, as the spec's expanded document holds them, minus what RDF
//! cannot carry (`@index` entries) and plus where each part is written:
//! - a [`NodeObject`] keeps the JSON that produced it (the object, or the
//!   string an `@id`-coerced value is written as), its `@id` and every
//!   `@type` with their spans, and each property with the key that named it;
//! - a [`ValueObject`] keeps its JSON value (borrowed from the document) and
//!   where its `@language` and `@type` are written, when they are;
//! - every IRI keeps the [`Form`] it is written in: a compact IRI with a
//!   defined prefix is a prefixed name, anything else (a term, an absolute or
//!   relative IRI, a `@vocab`-relative word) an IRI.
//!
//! Spans follow the crate's JSON-LD convention: an IRI or term written in a
//! JSON string covers the string's content (no quotes); a literal covers the
//! whole JSON token.
//!
//! Contexts are applied where they occur (embedded, property-scoped and
//! type-scoped, reverting to the previous context for new node objects as
//! step 7 says) through a [`Processor`]. A context that fails to process is
//! reported and the context in force before it stays.
//!
//! Expansion also records the document's [`SourceIndex`]: each key,
//! `@type` and `@id` value it reads, the term definitions they resolve to,
//! the active context inside each object, and the strings of the contexts it
//! processes with the definitions they name where they are written.
//!
//! Errors are the spec's, reported as `jsonld-context` diagnostics whose
//! message starts with the W3C error code. The spec stops at the first one;
//! here the entry (or object) in error is left out and expansion goes on.
//! Keys and values the spec drops silently (an undefined term, a
//! keyword-like key) are [`Dropped`] records, for lints.

use super::context::{
    ActiveContext, Container, ContextError, Direction, Options, Processor, ScopedContext,
    expand_iri, has_keyword_form, is_absolute_iri, is_blank_node, is_keyword, resolve_iri,
};
use super::json::{Json, Member, Object, Value};
use super::source::{Builder, SourceIndex, Use, UseKind, UseRole};
use super::{DropReason, Dropped, UsedTerm, unquoted};
use crate::syntax::{Form, Severity, Span, SyntaxError};
use rustc_hash::FxHashMap;
use std::borrow::Cow;
use std::sync::Arc;

/// An expanded element.
#[derive(Clone, Debug)]
pub(crate) enum Item<'a> {
    Node(Box<NodeObject<'a>>),
    Value(ValueObject<'a>),
    List(ListObject<'a>),
}

impl Item<'_> {
    /// Where the item is written: a node object's JSON object (or the
    /// `@id` of one written only as a reference), a value's JSON, a list's
    /// array or `@list` object.
    pub(crate) fn span(&self) -> Span {
        match self {
            Item::Node(n) => n.reference_span(),
            Item::Value(v) => v.span,
            Item::List(l) => l.span,
        }
    }
}

/// An IRI (or blank node identifier) as written in the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IriUse {
    /// The expanded value: an absolute IRI, a blank node identifier, or a
    /// relative IRI when no base resolved it.
    pub(crate) iri: Arc<str>,
    /// The string it is written in, without quotes.
    pub(crate) span: Span,
    pub(crate) form: Form,
}

/// A `@type` of a node object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TypeUse {
    pub(crate) iri: IriUse,
    /// The key that gave it: `@type` (or an alias), or the property of a
    /// `@type` map.
    pub(crate) key: Span,
}

/// A property and its values, as one key gives them.
#[derive(Clone, Debug)]
pub(crate) struct Property<'a> {
    pub(crate) iri: Arc<str>,
    /// The key, without quotes.
    pub(crate) key: Span,
    pub(crate) form: Form,
    pub(crate) items: Vec<Item<'a>>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct NodeObject<'a> {
    /// The JSON object, or the string a value coerced to `@id` is written
    /// in (without quotes).
    pub(crate) span: Span,
    pub(crate) id: Option<IriUse>,
    pub(crate) types: Vec<TypeUse>,
    pub(crate) props: Vec<Property<'a>>,
    /// `@reverse` properties: each item is a node object whose property
    /// points at this node.
    pub(crate) reverse: Vec<Property<'a>>,
    /// `@graph`: this node names a graph holding these items.
    pub(crate) graph: Option<Vec<Item<'a>>>,
    pub(crate) included: Vec<Item<'a>>,
    /// Written as an object with entries besides `@id` (and `@index`): it
    /// describes the node, rather than only referring to it.
    pub(crate) described: bool,
}

impl NodeObject<'_> {
    /// The node's `@id` when it is only referred to, else its span.
    pub(crate) fn reference_span(&self) -> Span {
        match &self.id {
            Some(id) if !self.described => id.span,
            _ => self.span,
        }
    }

    /// A graph object: `@graph`, and nothing but `@id` and `@index` beside it.
    pub(crate) fn is_graph_object(&self) -> bool {
        self.graph.is_some()
            && self.types.is_empty()
            && self.props.is_empty()
            && self.reverse.is_empty()
            && self.included.is_empty()
    }
}

/// The value of a value object, borrowed from the document.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Literal<'a> {
    String(&'a str),
    /// A JSON number as written.
    Number(&'a str),
    Bool(bool),
    /// A `@json` value: any JSON.
    Json(&'a Json),
}

#[derive(Clone, Debug)]
pub(crate) struct ValueObject<'a> {
    /// The JSON scalar, or the `{"@value": ...}` object.
    pub(crate) span: Span,
    pub(crate) value: Literal<'a>,
    /// The datatype IRI, where it is written (`None`: from a term's type
    /// mapping in the context) and how.
    pub(crate) datatype: Option<(Arc<str>, Option<Span>, Form)>,
    /// The language tag, and where it is written (`None`: from the context).
    pub(crate) language: Option<(Arc<str>, Option<Span>)>,
    pub(crate) direction: Option<Direction>,
}

#[derive(Clone, Debug)]
pub(crate) struct ListObject<'a> {
    /// The array or `{"@list": ...}` object.
    pub(crate) span: Span,
    pub(crate) items: Vec<Item<'a>>,
}

/// An expanded document.
#[derive(Debug, Default)]
pub(crate) struct Expanded<'a> {
    /// The top-level node objects (the content of a top-level object that
    /// has only `@graph`).
    pub(crate) items: Vec<Item<'a>>,
    /// `jsonld-context` errors.
    pub(crate) errors: Vec<SyntaxError>,
    pub(crate) dropped: Vec<Dropped>,
    /// Keys, types and `@vocab`-coerced values that are terms of the
    /// active context, in the order they are read.
    pub(crate) used_terms: Vec<UsedTerm>,
    pub(crate) index: SourceIndex,
}

/// Expand the document `root`, whose URL is `base`, with contexts from
/// `processor`.
pub(crate) fn expand<'a>(
    root: &'a Json,
    base: Option<&str>,
    processor: &mut Processor,
) -> Expanded<'a> {
    let mut x = Expander {
        processor,
        base,
        errors: Vec::new(),
        dropped: Vec::new(),
        used_terms: Vec::new(),
        iris: FxHashMap::default(),
        index: Builder::default(),
    };
    let initial = ActiveContext::new(base);
    let items = match x.element(&initial, None, root, false) {
        Ex::One(Item::Node(n)) if n.is_graph_object() && n.id.is_none() => {
            n.graph.unwrap_or_default()
        }
        ex => ex.into_vec(),
    };
    let index = x.index.finish(x.processor.take_written());
    Expanded {
        items,
        errors: x.errors,
        dropped: x.dropped,
        used_terms: x.used_terms,
        index,
    }
}

/// What expanding an element gives: the spec's `null`, a map, or an array.
#[derive(Debug)]
enum Ex<'a> {
    Null,
    One(Item<'a>),
    Many(Vec<Item<'a>>),
}

impl<'a> Ex<'a> {
    fn into_vec(self) -> Vec<Item<'a>> {
        match self {
            Ex::Null => Vec::new(),
            Ex::One(i) => vec![i],
            Ex::Many(v) => v,
        }
    }
}

/// The active property: the key (a term, a compact IRI or a keyword) and
/// where it is written.
#[derive(Clone, Copy, Debug)]
struct Prop<'a> {
    key: &'a str,
    span: Span,
}

/// An error already reported: leave out what is in error.
struct Skip;

// Keywords present in a result map.
const K_ID: u32 = 1;
const K_TYPE: u32 = 2;
const K_GRAPH: u32 = 4;
const K_INCLUDED: u32 = 8;
const K_VALUE: u32 = 16;
const K_LANGUAGE: u32 = 32;
const K_DIRECTION: u32 = 64;
const K_INDEX: u32 = 128;
const K_LIST: u32 = 256;
const K_SET: u32 = 512;
const K_REVERSE: u32 = 1024;

fn keyword_bit(k: &str) -> u32 {
    match k {
        "@id" => K_ID,
        "@type" => K_TYPE,
        "@graph" => K_GRAPH,
        "@included" => K_INCLUDED,
        "@value" => K_VALUE,
        "@language" => K_LANGUAGE,
        "@direction" => K_DIRECTION,
        "@index" => K_INDEX,
        "@list" => K_LIST,
        "@set" => K_SET,
        "@reverse" => K_REVERSE,
        _ => 0,
    }
}

/// The result map of step 13, as it fills.
#[derive(Default)]
struct Map<'a> {
    keys: u32,
    id: Option<IriUse>,
    types: Vec<TypeUse>,
    graph: Vec<Item<'a>>,
    included: Vec<Item<'a>>,
    /// The `@value` JSON (`null` included).
    value: Option<&'a Json>,
    language: Option<(&'a str, Span)>,
    direction: Option<Direction>,
    list: Vec<Item<'a>>,
    set: Option<Ex<'a>>,
    reverse: Vec<Property<'a>>,
    props: Vec<Property<'a>>,
}

struct Expander<'a, 'p, 'l, 'b> {
    processor: &'p mut Processor<'l>,
    base: Option<&'b str>,
    errors: Vec<SyntaxError>,
    dropped: Vec<Dropped>,
    used_terms: Vec<UsedTerm>,
    /// IRI expansions without the base IRI, and how the value is written,
    /// by (context content, `@vocab`, value): keys and types repeat across
    /// node objects.
    iris: FxHashMap<(u64, bool, &'a str), (Expansion, Option<UseKind>)>,
    index: Builder,
}

/// An IRI expansion (`None`: `null`) and the form the value is written in.
type Expansion = Option<(Arc<str>, Form)>;

/// An object's entries as a map: with duplicate keys, the last one.
fn map_entries(o: &Object) -> Vec<&Member> {
    let mut entries = o.entries();
    entries.retain(|m| !matches!(m.value.value, Value::Invalid));
    entries
}

/// A value, or each item of an array.
fn items_of(v: &Json) -> &[Json] {
    match &v.value {
        Value::Array(a) => &a.items,
        _ => std::slice::from_ref(v),
    }
}

/// Whether `key` IRI-expands (with `@vocab`) to the keyword `kw`: it is the
/// keyword, or an alias of it. [`Expander::is_alias`] caches it.
fn is_alias_with(ctx: &ActiveContext, key: &str, kw: &str) -> bool {
    if key.starts_with('@') {
        return key == kw;
    }
    ctx.term(key).and_then(|d| d.iri.as_deref()) == Some(kw)
}

/// How `s` is written: a compact IRI whose prefix is a defined prefix term
/// is a prefixed name.
fn form_of(ctx: &ActiveContext, s: &str) -> Form {
    if let Some(colon) = s.find(':').filter(|&i| i > 0) {
        let (prefix, suffix) = (&s[..colon], &s[colon + 1..]);
        if prefix != "_"
            && !suffix.starts_with("//")
            && ctx.term(s).is_none()
            && ctx
                .term(prefix)
                .is_some_and(|d| d.prefix && d.iri.is_some())
        {
            return Form::PName {
                prefix_len: colon as u32,
            };
        }
    }
    Form::Iri
}

impl<'a> Expander<'a, '_, '_, '_> {
    /// IRI Expansion (§4.3) of `s`, and the form it is written in.
    fn expand_iri(
        &mut self,
        ctx: &ActiveContext,
        s: &'a str,
        document_relative: bool,
        vocab: bool,
    ) -> Expansion {
        self.expand_kind(ctx, s, document_relative, vocab).0
    }

    /// [`Self::expand_iri`], and how `s` is written.
    fn expand_kind(
        &mut self,
        ctx: &ActiveContext,
        s: &'a str,
        document_relative: bool,
        vocab: bool,
    ) -> (Expansion, Option<UseKind>) {
        let key = (ctx.content_id(), vocab, s);
        let (hit, kind) = match self.iris.get(&key) {
            Some(hit) => hit.clone(),
            None => {
                let hit = expand_iri(ctx, s, false, vocab).map(|x| (Arc::from(x), form_of(ctx, s)));
                let kind = self.index.classify(ctx, s, vocab);
                self.iris.insert(key, (hit.clone(), kind));
                (hit, kind)
            }
        };
        // Only a value nothing else expands is resolved against the base.
        let hit = match (hit, ctx.base()) {
            (Some((x, form)), Some(base))
                if document_relative
                    && !is_keyword(&x)
                    && !is_absolute_iri(&x)
                    && !is_blank_node(&x) =>
            {
                Some((Arc::from(resolve_iri(base, s)), form))
            }
            (hit, _) => hit,
        };
        (hit, kind)
    }

    /// [`Self::expand_iri`] of `s`, written at `span` as a `role`, recorded
    /// in the index.
    fn expand_use(
        &mut self,
        ctx: &ActiveContext,
        s: &'a str,
        span: Span,
        role: UseRole,
        document_relative: bool,
        vocab: bool,
    ) -> Expansion {
        let (hit, kind) = self.expand_kind(ctx, s, document_relative, vocab);
        if let Some(kind) = kind {
            self.index.push_use(Use {
                span,
                role,
                iri: hit.as_ref().map(|(x, _)| x.clone()),
                kind,
            });
        }
        hit
    }

    /// Whether `key` IRI-expands (with `@vocab`) to the keyword `kw`.
    fn is_alias(&mut self, ctx: &ActiveContext, key: &'a str, kw: &str) -> bool {
        if key.starts_with('@') {
            return key == kw;
        }
        self.expand_iri(ctx, key, false, true)
            .is_some_and(|(x, _)| &*x == kw)
    }

    fn error(&mut self, code: &str, span: Span, message: impl std::fmt::Display) -> Skip {
        self.errors.push(SyntaxError::new(
            span,
            format!("{code}: {message}"),
            Severity::Error,
            super::context::DIAGNOSTIC_CODE,
        ));
        Skip
    }

    fn context_error(&mut self, e: ContextError) {
        self.errors.push(e.to_syntax_error());
    }

    fn drop(&mut self, span: Span, reason: DropReason) {
        self.dropped.push(Dropped { span, reason });
    }

    /// Records `s`, written at `span`, when it is a term of `ctx` (not a
    /// keyword alias) whose IRI is not spelled by the term itself.
    fn term_use(&mut self, ctx: &ActiveContext, s: &str, span: Span) {
        let Some(d) = ctx
            .term(s)
            .filter(|d| d.iri.as_deref().is_some_and(|i| !is_keyword(i)))
        else {
            return;
        };
        // Written in the document with no IRI of its own: the key spells it.
        if d.is_local() && d.iri_span.is_none() {
            return;
        }
        self.used_terms.push(UsedTerm {
            span,
            definition: d.iri_span.filter(|_| d.is_local()),
        });
    }

    /// Apply a term's scoped context (`property`: a property-scoped one,
    /// else type-scoped); on an error, report it and keep `ctx`.
    fn scoped(
        &mut self,
        ctx: &ActiveContext,
        sc: &ScopedContext,
        at: Span,
        property: bool,
    ) -> ActiveContext {
        match self
            .processor
            .process_scoped(ctx, sc, at, property, property)
        {
            Ok(c) => c,
            Err(e) => {
                self.context_error(e);
                ctx.clone()
            }
        }
    }

    /// The Expansion algorithm (§5.1.2) for `el`.
    fn element(
        &mut self,
        ctx: &ActiveContext,
        prop: Option<Prop<'a>>,
        el: &'a Json,
        from_map: bool,
    ) -> Ex<'a> {
        match &el.value {
            // 1
            Value::Null | Value::Invalid => Ex::Null,
            // 4
            Value::String(_) | Value::Number(_) | Value::Bool(_) => {
                let Some(p) = prop.filter(|p| p.key != "@graph") else {
                    return Ex::Null;
                };
                let lit = match &el.value {
                    Value::String(s) => Literal::String(s),
                    Value::Number(n) => Literal::Number(n),
                    Value::Bool(b) => Literal::Bool(*b),
                    _ => unreachable!(),
                };
                match ctx.term(p.key).and_then(|d| d.context.clone()) {
                    Some(sc) => {
                        let c = self.scoped(ctx, &sc, p.span, true);
                        Ex::One(self.value_expansion(&c, Some(p.key), lit, el.span))
                    }
                    None => Ex::One(self.value_expansion(ctx, Some(p.key), lit, el.span)),
                }
            }
            // 5
            Value::Array(a) => {
                let list = prop
                    .and_then(|p| ctx.term(p.key))
                    .is_some_and(|d| d.container.contains(Container::LIST));
                let mut out = Vec::with_capacity(a.items.len());
                for item in &a.items {
                    match self.element(ctx, prop, item, from_map) {
                        Ex::Many(v) if list => out.push(Item::List(ListObject {
                            span: item.span,
                            items: v,
                        })),
                        Ex::Many(v) => out.extend(v),
                        Ex::One(i) => out.push(i),
                        Ex::Null => {}
                    }
                }
                Ex::Many(out)
            }
            Value::Object(o) => self.object(ctx, prop, el, o, from_map),
        }
    }

    /// Steps 6 to 20, for a map.
    fn object(
        &mut self,
        active: &ActiveContext,
        prop: Option<Prop<'a>>,
        el: &'a Json,
        o: &'a Object,
        from_map: bool,
    ) -> Ex<'a> {
        let entries = map_entries(o);
        let mut ctx: Cow<ActiveContext> = Cow::Borrowed(active);
        // 7
        if !from_map && let Some(previous) = active.previous() {
            let mut keep = entries.len() == 1 && self.is_alias(active, &entries[0].key, "@id");
            for m in &entries {
                keep = keep || self.is_alias(active, &m.key, "@value");
            }
            if !keep {
                ctx = Cow::Borrowed(previous);
            }
        }
        // 3, 8
        if let Some(p) = prop
            && let Some(sc) = active.term(p.key).and_then(|d| d.context.clone())
        {
            ctx = Cow::Owned(self.scoped(&ctx, &sc, p.span, true));
        }
        // 9
        if let Some(m) = entries.iter().find(|m| &*m.key == "@context") {
            match self
                .processor
                .process(&ctx, &m.value, self.base, Options::default())
            {
                Ok(c) => {
                    self.index.context_strings(&m.value, &ctx, &c);
                    ctx = Cow::Owned(c);
                }
                Err(e) => {
                    self.index.context_strings(&m.value, &ctx, &ctx);
                    self.context_error(e);
                }
            }
        }
        // 10, 11
        let type_scoped = ctx.clone();
        let mut type_keys: Vec<&'a Member> = Vec::new();
        for &m in &entries {
            if self.is_alias(&ctx, &m.key, "@type") {
                type_keys.push(m);
            }
        }
        type_keys.sort_by(|a, b| a.key.cmp(&b.key));
        for m in &type_keys {
            let mut terms: Vec<(&str, Span)> = items_of(&m.value)
                .iter()
                .filter_map(|t| Some((t.as_str()?, unquoted(t.span))))
                .collect();
            terms.sort_by(|a, b| a.0.cmp(b.0));
            for (t, span) in terms {
                if let Some(sc) = type_scoped.term(t).and_then(|d| d.context.clone()) {
                    ctx = Cow::Owned(self.scoped(&ctx, &sc, span, false));
                }
            }
        }
        // 12
        let input_json = type_keys
            .first()
            .and_then(|m| items_of(&m.value).last())
            .and_then(Json::as_str)
            .is_some_and(|t| self.is_alias(&ctx, t, "@json"));
        // 13, 14
        self.index.scope(el.span, o.close.is_some(), &ctx);
        let mut r = Map::default();
        self.entries(&ctx, &type_scoped, prop, &entries, input_json, &mut r);
        self.finish(prop, el, o, r)
    }

    /// Steps 13 and 14: `entries` into `r`.
    fn entries(
        &mut self,
        ctx: &ActiveContext,
        type_scoped: &ActiveContext,
        prop: Option<Prop<'a>>,
        entries: &[&'a Member],
        input_json: bool,
        r: &mut Map<'a>,
    ) {
        let mut nests: Vec<&'a Member> = Vec::new();
        for &m in entries {
            let key: &'a str = &m.key;
            let kspan = unquoted(m.key_span);
            // 13.1
            if key == "@context" {
                self.index.push_use(Use {
                    span: kspan,
                    role: UseRole::Key,
                    iri: Some(Arc::from(key)),
                    kind: UseKind::Keyword,
                });
                continue;
            }
            // 13.2, 13.3
            self.term_use(ctx, key, kspan);
            let Some((expanded, form)) =
                self.expand_use(ctx, key, kspan, UseRole::Key, false, true)
            else {
                let reason = if has_keyword_form(key) {
                    DropReason::KeywordLike
                } else {
                    DropReason::NullTerm
                };
                self.drop(kspan, reason);
                continue;
            };
            if is_keyword(&expanded) {
                // 13.4
                if &*expanded == "@nest" {
                    if prop.is_some_and(|p| p.key == "@reverse") {
                        self.error(
                            "invalid reverse property map",
                            kspan,
                            "a @reverse map cannot hold keywords",
                        );
                    } else {
                        nests.push(m);
                    }
                    continue;
                }
                let _ = self.keyword(ctx, type_scoped, prop, m, &expanded, input_json, r);
                continue;
            }
            if !expanded.contains(':') {
                self.drop(kspan, DropReason::UndefinedTerm);
                continue;
            }
            let _ = self.property(ctx, m, expanded, form, r);
        }
        // 14
        for m in nests {
            let scoped = ctx.term(&m.key).and_then(|d| d.context.clone());
            let nctx = match &scoped {
                Some(sc) => self.scoped(ctx, sc, unquoted(m.key_span), true),
                None => ctx.clone(),
            };
            for nested in items_of(&m.value) {
                let Some(no) = nested.as_object() else {
                    if !matches!(nested.value, Value::Invalid) {
                        self.error(
                            "invalid @nest value",
                            nested.span,
                            "a @nest value must be an object",
                        );
                    }
                    continue;
                };
                self.index.scope(nested.span, no.close.is_some(), &nctx);
                let inner = map_entries(no);
                if let Some(v) = inner
                    .iter()
                    .find(|e| is_alias_with(&nctx, &e.key, "@value"))
                {
                    self.error(
                        "invalid @nest value",
                        unquoted(v.key_span),
                        "a @nest object cannot be a value object",
                    );
                    continue;
                }
                self.entries(&nctx, type_scoped, prop, &inner, input_json, r);
            }
        }
    }

    /// Step 13.4: a keyword entry.
    #[allow(clippy::too_many_arguments)]
    fn keyword(
        &mut self,
        ctx: &ActiveContext,
        type_scoped: &ActiveContext,
        prop: Option<Prop<'a>>,
        m: &'a Member,
        kw: &str,
        input_json: bool,
        r: &mut Map<'a>,
    ) -> Result<(), Skip> {
        let kspan = unquoted(m.key_span);
        let v = &m.value;
        // 13.4.1
        if prop.is_some_and(|p| p.key == "@reverse") {
            return Err(self.error(
                "invalid reverse property map",
                kspan,
                format!("a @reverse map cannot hold {kw}"),
            ));
        }
        let bit = keyword_bit(kw);
        // 13.4.2
        if r.keys & bit != 0 && kw != "@included" && kw != "@type" {
            return Err(self.error(
                "colliding keywords",
                kspan,
                format!("{kw} is given more than once (through an alias)"),
            ));
        }
        match kw {
            "@id" => {
                let Some(s) = v.as_str() else {
                    return Err(self.error("invalid @id value", v.span, "@id must be a string"));
                };
                let span = unquoted(v.span);
                r.id = Some(self.id(ctx, s, span, false));
                r.keys |= bit;
            }
            "@type" => {
                let all_strings = match &v.value {
                    Value::String(_) => true,
                    Value::Array(a) => a.items.iter().all(|i| i.as_str().is_some()),
                    _ => false,
                };
                if !all_strings {
                    return Err(self.error(
                        "invalid type value",
                        v.span,
                        "@type must be a string or an array of strings",
                    ));
                }
                for t in items_of(v) {
                    let s = t.as_str().unwrap_or_default();
                    let span = unquoted(t.span);
                    self.term_use(type_scoped, s, span);
                    match self.expand_use(type_scoped, s, span, UseRole::Type, true, true) {
                        Some((iri, form)) => r.types.push(TypeUse {
                            iri: IriUse { iri, span, form },
                            key: kspan,
                        }),
                        None => self.drop(span, DropReason::KeywordLike),
                    }
                }
                r.keys |= bit;
            }
            "@graph" => {
                let p = Prop {
                    key: "@graph",
                    span: kspan,
                };
                let items = self.element(ctx, Some(p), v, false).into_vec();
                r.graph.extend(items);
                r.keys |= bit;
            }
            "@included" => {
                let ex = self.element(ctx, prop, v, false);
                let bad = match &ex {
                    Ex::Null => Some(v.span),
                    Ex::One(i) => (!matches!(i, Item::Node(_))).then(|| i.span()),
                    Ex::Many(items) => items
                        .iter()
                        .find(|i| !matches!(i, Item::Node(_)))
                        .map(Item::span),
                };
                if let Some(at) = bad {
                    return Err(self.error(
                        "invalid @included value",
                        at,
                        "@included must hold node objects",
                    ));
                }
                r.included.extend(ex.into_vec());
                r.keys |= bit;
            }
            "@value" => {
                if !input_json && matches!(v.value, Value::Array(_) | Value::Object(_)) {
                    return Err(self.error(
                        "invalid value object value",
                        v.span,
                        "@value must be a string, a number, true, false or null",
                    ));
                }
                r.value = Some(v);
                r.keys |= bit;
            }
            "@language" => {
                let Some(s) = v.as_str() else {
                    return Err(self.error(
                        "invalid language-tagged string",
                        v.span,
                        "@language must be a string",
                    ));
                };
                r.language = Some((s, unquoted(v.span)));
                r.keys |= bit;
            }
            "@direction" => {
                r.direction = match v.as_str() {
                    Some("ltr") => Some(Direction::Ltr),
                    Some("rtl") => Some(Direction::Rtl),
                    _ => {
                        return Err(self.error(
                            "invalid base direction",
                            v.span,
                            "@direction must be \"ltr\" or \"rtl\"",
                        ));
                    }
                };
                r.keys |= bit;
            }
            "@index" => {
                if v.as_str().is_none() {
                    return Err(self.error(
                        "invalid @index value",
                        v.span,
                        "@index must be a string",
                    ));
                }
                r.keys |= bit;
            }
            "@list" => {
                // 13.4.11.1: a free-floating list is dropped.
                let Some(p) = prop.filter(|p| p.key != "@graph") else {
                    return Ok(());
                };
                r.list = self.element(ctx, Some(p), v, false).into_vec();
                r.keys |= bit;
            }
            "@set" => {
                r.set = Some(self.element(ctx, prop, v, false));
                r.keys |= bit;
            }
            "@reverse" => {
                if v.as_object().is_none() {
                    return Err(self.error(
                        "invalid @reverse value",
                        v.span,
                        "@reverse must be an object",
                    ));
                }
                let p = Prop {
                    key: "@reverse",
                    span: kspan,
                };
                let Ex::One(Item::Node(n)) = self.element(ctx, Some(p), v, false) else {
                    return Ok(());
                };
                let n = *n;
                // 13.4.13.3: reversed twice is forward.
                r.props.extend(n.reverse);
                for p in n.props {
                    if let Some(bad) = p
                        .items
                        .iter()
                        .find(|i| matches!(i, Item::Value(_) | Item::List(_)))
                    {
                        self.error(
                            "invalid reverse property value",
                            bad.span(),
                            "a reverse property's value must be a node object",
                        );
                        continue;
                    }
                    r.reverse.push(p);
                    r.keys |= bit;
                }
            }
            // Other keywords mean nothing in a node object.
            _ => {}
        }
        Ok(())
    }

    /// Steps 13.5 to 13.14: a property entry.
    fn property(
        &mut self,
        ctx: &ActiveContext,
        m: &'a Member,
        iri: Arc<str>,
        form: Form,
        r: &mut Map<'a>,
    ) -> Result<(), Skip> {
        let key: &'a str = &m.key;
        let kspan = unquoted(m.key_span);
        let v = &m.value;
        let def = ctx.term(key);
        let container = def.map_or(Container::default(), |d| d.container);
        let reverse = def.is_some_and(|d| d.reverse);
        let p = Prop { key, span: kspan };
        // Language and index maps are read with the key's scoped context
        // applied, as jsonld.js does (the spec does not say which context
        // expands their keys; IIIF aliases `@none` this way).
        let map_ctx = |x: &mut Self| match def.and_then(|d| d.context.clone()) {
            Some(sc) => x.scoped(ctx, &sc, kspan, true),
            None => ctx.clone(),
        };
        let ex = if def.and_then(|d| d.type_mapping.as_deref()) == Some("@json") {
            // 13.6
            Ex::One(Item::Value(ValueObject {
                span: v.span,
                value: Literal::Json(v),
                datatype: None,
                language: None,
                direction: None,
            }))
        } else if container.contains(Container::LANGUAGE)
            && let Some(map) = v.as_object()
        {
            // 13.7
            let ctx = &map_ctx(self);
            let direction = match def.and_then(|d| d.direction) {
                Some(d) => d,
                None => ctx.default_direction(),
            };
            let mut out = Vec::new();
            for lm in map_entries(map) {
                let lang: &'a str = &lm.key;
                let none = lang == "@none"
                    || expand_iri(ctx, lang, false, true).as_deref() == Some("@none");
                let language = (!none).then(|| (Arc::from(lang), Some(unquoted(lm.key_span))));
                for item in items_of(&lm.value) {
                    match &item.value {
                        Value::Null | Value::Invalid => {}
                        Value::String(s) => out.push(Item::Value(ValueObject {
                            span: item.span,
                            value: Literal::String(s),
                            datatype: None,
                            language: language.clone(),
                            direction,
                        })),
                        _ => {
                            self.error(
                                "invalid language map value",
                                item.span,
                                "a language map value must be a string",
                            );
                        }
                    }
                }
            }
            Ex::Many(out)
        } else if (container.contains(Container::INDEX)
            || container.contains(Container::TYPE)
            || container.contains(Container::ID))
            && let Some(map) = v.as_object()
        {
            // 13.8
            let ctx = &map_ctx(self);
            Ex::Many(self.index_map(ctx, p, container, map))
        } else {
            // 13.9
            self.element(ctx, Some(p), v, false)
        };
        // 13.10
        let mut items = match ex {
            Ex::Null => return Ok(()),
            // 13.11
            Ex::One(Item::List(l)) => vec![Item::List(l)],
            ex if container.contains(Container::LIST) => vec![Item::List(ListObject {
                span: v.span,
                items: ex.into_vec(),
            })],
            ex => ex.into_vec(),
        };
        // 13.12
        if container.contains(Container::GRAPH)
            && !container.contains(Container::ID)
            && !container.contains(Container::INDEX)
        {
            items = items.into_iter().map(graph_object).collect();
        }
        let prop = Property {
            iri,
            key: kspan,
            form,
            items,
        };
        if reverse {
            // 13.13
            if let Some(bad) = prop
                .items
                .iter()
                .find(|i| matches!(i, Item::Value(_) | Item::List(_)))
            {
                return Err(self.error(
                    "invalid reverse property value",
                    bad.span(),
                    "a reverse property's value must be a node object",
                ));
            }
            r.reverse.push(prop);
            r.keys |= K_REVERSE;
        } else {
            r.props.push(prop);
        }
        Ok(())
    }

    /// Step 13.8: an index, `@id` or `@type` map.
    fn index_map(
        &mut self,
        ctx: &ActiveContext,
        p: Prop<'a>,
        container: Container,
        map: &'a Object,
    ) -> Vec<Item<'a>> {
        let def = ctx.term(p.key);
        let index_key: Arc<str> = def
            .and_then(|d| d.index.clone())
            .unwrap_or_else(|| Arc::from("@index"));
        let by_id_or_type =
            container.contains(Container::ID) || container.contains(Container::TYPE);
        let mut out = Vec::new();
        for im in map_entries(map) {
            let index: &'a str = &im.key;
            let ispan = unquoted(im.key_span);
            // 13.8.3.1 - 13.8.3.3
            let mut map_ctx = if by_id_or_type {
                ctx.previous().unwrap_or(ctx).clone()
            } else {
                ctx.clone()
            };
            if container.contains(Container::TYPE)
                && let Some(sc) = map_ctx.term(index).and_then(|d| d.context.clone())
            {
                map_ctx = self.scoped(&map_ctx, &sc, ispan, false);
            }
            let expanded_index = expand_iri(ctx, index, false, true);
            let none = expanded_index.as_deref() == Some("@none");
            if container.contains(Container::TYPE) {
                self.expand_use(ctx, index, ispan, UseRole::Type, false, true);
            }
            let values = self.element(&map_ctx, Some(p), &im.value, true).into_vec();
            for mut item in values {
                // 13.8.3.7.1
                if container.contains(Container::GRAPH)
                    && !matches!(&item, Item::Node(n) if n.is_graph_object())
                {
                    item = graph_object(item);
                }
                if none {
                    // No index to apply.
                } else if container.contains(Container::INDEX) && &*index_key != "@index" {
                    // 13.8.3.7.2
                    let re = self.value_expansion(
                        ctx,
                        Some(&index_key),
                        Literal::String(index),
                        im.key_span,
                    );
                    let Some(prop_iri) = expand_iri(ctx, &index_key, false, true) else {
                        continue;
                    };
                    match &mut item {
                        Item::Node(n) => n.props.insert(
                            0,
                            Property {
                                iri: Arc::from(prop_iri),
                                key: ispan,
                                form: Form::Iri,
                                items: vec![re],
                            },
                        ),
                        _ => {
                            self.error(
                                "invalid value object",
                                ispan,
                                "a property-valued index cannot index a value",
                            );
                            continue;
                        }
                    }
                } else if container.contains(Container::INDEX) {
                    // 13.8.3.7.3: @index is not RDF.
                } else if container.contains(Container::ID) {
                    // 13.8.3.7.4
                    if let Item::Node(n) = &mut item
                        && n.id.is_none()
                    {
                        n.id = Some(self.id(ctx, index, ispan, false));
                    }
                } else if container.contains(Container::TYPE)
                    && let Some(iri) = &expanded_index
                    && let Item::Node(n) = &mut item
                {
                    // 13.8.3.7.5
                    n.types.insert(
                        0,
                        TypeUse {
                            iri: IriUse {
                                iri: Arc::from(iri.as_str()),
                                span: ispan,
                                form: form_of(ctx, index),
                            },
                            key: p.span,
                        },
                    );
                }
                out.push(item);
            }
        }
        out
    }

    /// Steps 15 to 20: the result map as an item.
    fn finish(
        &mut self,
        prop: Option<Prop<'a>>,
        el: &'a Json,
        o: &'a Object,
        r: Map<'a>,
    ) -> Ex<'a> {
        let free = prop.is_none_or(|p| p.key == "@graph");
        let open = o.open;
        // 15
        if r.keys & K_VALUE != 0 {
            let allowed = K_VALUE | K_LANGUAGE | K_TYPE | K_DIRECTION | K_INDEX;
            if r.keys & !allowed != 0 || !r.props.is_empty() {
                self.error(
                    "invalid value object",
                    open,
                    "a value object can only have @value, @type, @language, @direction and @index",
                );
                return Ex::Null;
            }
            if r.keys & K_TYPE != 0 && r.keys & (K_LANGUAGE | K_DIRECTION) != 0 {
                self.error(
                    "invalid value object",
                    open,
                    "a value object cannot have both @type and @language or @direction",
                );
                return Ex::Null;
            }
            let json_type = r.types.len() == 1 && &*r.types[0].iri.iri == "@json";
            let Some(value) = r.value else {
                return Ex::Null;
            };
            let lit = if json_type {
                Literal::Json(value)
            } else {
                match &value.value {
                    Value::Null | Value::Invalid => return Ex::Null,
                    Value::String(s) => Literal::String(s),
                    Value::Number(n) => Literal::Number(n),
                    Value::Bool(b) => Literal::Bool(*b),
                    // Rejected at @value.
                    Value::Array(_) | Value::Object(_) => return Ex::Null,
                }
            };
            if r.language.is_some() && !matches!(lit, Literal::String(_)) {
                self.error(
                    "invalid language-tagged value",
                    value.span,
                    "a value with @language must be a string",
                );
                return Ex::Null;
            }
            let mut datatype = None;
            if !json_type && r.keys & K_TYPE != 0 {
                match r.types.as_slice() {
                    [t] if is_absolute_iri(&t.iri.iri)
                        && oxiri::Iri::parse(&*t.iri.iri).is_ok() =>
                    {
                        datatype = Some((t.iri.iri.clone(), Some(t.iri.span), t.iri.form));
                    }
                    _ => {
                        let at = r.types.first().map_or(open, |t| t.iri.span);
                        self.error(
                            "invalid typed value",
                            at,
                            "the @type of a value must be one IRI",
                        );
                        return Ex::Null;
                    }
                }
            }
            if free {
                return Ex::Null;
            }
            return Ex::One(Item::Value(ValueObject {
                span: el.span,
                value: lit,
                datatype,
                language: r.language.map(|(l, s)| (Arc::from(l), Some(s))),
                direction: r.direction,
            }));
        }
        // 17
        if r.keys & (K_LIST | K_SET) != 0 {
            let both = r.keys & (K_LIST | K_SET) == K_LIST | K_SET;
            if both || r.keys & !(K_LIST | K_SET | K_INDEX) != 0 || !r.props.is_empty() {
                self.error(
                    "invalid set or list object",
                    open,
                    "a @list or @set object can only have @index beside it",
                );
                return Ex::Null;
            }
            if r.keys & K_SET != 0 {
                return r.set.unwrap_or(Ex::Null);
            }
            if free {
                return Ex::Null;
            }
            return Ex::One(Item::List(ListObject {
                span: el.span,
                items: r.list,
            }));
        }
        // 18
        if r.keys == K_LANGUAGE && r.props.is_empty() {
            return Ex::Null;
        }
        // 19
        let rest = r.keys & !K_INDEX;
        if free && (rest == 0 || rest == K_ID) && r.props.is_empty() {
            return Ex::Null;
        }
        let described = r.keys & !(K_ID | K_INDEX) != 0 || !r.props.is_empty();
        Ex::One(Item::Node(Box::new(NodeObject {
            span: el.span,
            id: r.id,
            types: r.types,
            props: r.props,
            reverse: r.reverse,
            graph: (r.keys & K_GRAPH != 0).then_some(r.graph),
            included: r.included,
            described,
        })))
    }

    /// An `@id` (document-relative; with `@vocab` for a `@vocab`-coerced
    /// value). One that looks like a keyword expands to `null`: it keeps
    /// what is written (which no IRI starts with: `@`), so the node converts
    /// to nothing.
    fn id(&mut self, ctx: &ActiveContext, s: &'a str, span: Span, vocab: bool) -> IriUse {
        if vocab {
            self.term_use(ctx, s, span);
        }
        match self.expand_use(ctx, s, span, UseRole::Id, true, vocab) {
            Some((iri, form)) => IriUse { iri, span, form },
            None => {
                self.drop(span, DropReason::KeywordLike);
                IriUse {
                    iri: Arc::from(s),
                    span,
                    form: Form::Iri,
                }
            }
        }
    }

    /// Value Expansion (§5.3) of a scalar written at `span`.
    fn value_expansion(
        &mut self,
        ctx: &ActiveContext,
        prop: Option<&str>,
        lit: Literal<'a>,
        span: Span,
    ) -> Item<'a> {
        let def = prop.and_then(|p| ctx.term(p));
        let mapping = def.and_then(|d| d.type_mapping.clone());
        if let Literal::String(s) = lit
            && let Some(vocab) = match mapping.as_deref() {
                Some("@id") => Some(false),
                Some("@vocab") => Some(true),
                _ => None,
            }
        {
            let at = unquoted(span);
            return Item::Node(Box::new(NodeObject {
                span: at,
                id: Some(self.id(ctx, s, at, vocab)),
                ..NodeObject::default()
            }));
        }
        let mut v = ValueObject {
            span,
            value: lit,
            datatype: None,
            language: None,
            direction: None,
        };
        match mapping {
            Some(t) if !matches!(&*t, "@id" | "@vocab" | "@none") => {
                v.datatype = Some((t, None, Form::Iri));
            }
            _ if matches!(lit, Literal::String(_)) => {
                v.language = match def.and_then(|d| d.language.clone()) {
                    Some(l) => l,
                    None => ctx.default_language().map(Arc::from),
                }
                .map(|l| (l, None));
                v.direction = match def.and_then(|d| d.direction) {
                    Some(d) => d,
                    None => ctx.default_direction(),
                };
            }
            _ => {}
        }
        Item::Value(v)
    }
}

/// A graph object holding `item` (step 13.12).
fn graph_object(item: Item<'_>) -> Item<'_> {
    let span = item.span();
    Item::Node(Box::new(NodeObject {
        span,
        graph: Some(vec![item]),
        ..NodeObject::default()
    }))
}
