//! Type hierarchy for classes (`rdfs:subClassOf`) and properties
//! (`rdfs:subPropertyOf`): super terms from the workspace, external sources
//! and well-known vocabularies; sub terms from the workspace and external
//! sources.
//!
//! Each item carries its IRI (and the document that asked) in `data`, so
//! `typeHierarchy/supertypes` and `/subtypes` answer one level at a time from
//! the index. A cycle in the data (`A ⊑ B ⊑ A`) is therefore harmless: the
//! client asks one level at a time and a term is never listed as its own
//! super or sub term.

use crate::analysis::{Analysis, Encoding};
use crate::features::completion::shorten;
use crate::features::hover::short;
use crate::features::navigation::uri;
use crate::knowledge::{Knowledge, Source};
use crate::syntax::{Iri, PrefixMap, Span};
use crate::vocab::{TermKind, VOCABS};
use lsp_types::*;
use serde_json::json;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Class,
    Property,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Class => "class",
            Kind::Property => "property",
        }
    }

    fn symbol(self) -> SymbolKind {
        match self {
            Kind::Class => SymbolKind::CLASS,
            Kind::Property => SymbolKind::PROPERTY,
        }
    }
}

fn vocab_kind(iri: &str) -> Option<TermKind> {
    let (v, local) = VOCABS.lookup(iri)?;
    v.term(local).map(|t| t.kind)
}

fn is_property(k: &Knowledge, iri: Iri) -> bool {
    vocab_kind(&iri) == Some(TermKind::Property)
        || k.used_as_predicate(&iri)
        || k.types(&iri).iter().any(|t| t.ends_with("Property"))
}

/// Whether `iri` is a class or a property, from what the index and the
/// vocabularies say about it; `None` when it looks like neither.
fn kind_of(k: &Knowledge, iri: Iri) -> Option<Kind> {
    if is_property(k, iri) {
        return Some(Kind::Property);
    }
    let supers = k.supers(&iri);
    // A term whose super term is a property is a property too.
    if supers.iter().any(|s| is_property(k, *s)) {
        return Some(Kind::Property);
    }
    let class = vocab_kind(&iri) == Some(TermKind::Class)
        || !supers.is_empty()
        || !k.subs(&iri).is_empty()
        || !k.instances(&iri).is_empty()
        || k.types(&iri).iter().any(|t| t.ends_with("Class"));
    class.then_some(Kind::Class)
}

/// Where to show `iri`: its first description in the workspace (the whole
/// statement, selecting the subject), else its first reference in the asking
/// document, else its first reference anywhere in the workspace.
fn locate(
    k: &Knowledge,
    enc: Encoding,
    iri: Iri,
    asking: Option<&Analysis>,
) -> Option<(Uri, Range, Range)> {
    let at = |a: &Analysis, range: Span, sel: Span| {
        Some((uri(&a.uri), a.range(enc, range), a.range(enc, sel)))
    };
    if let Some(d) = k
        .descriptions(&iri)
        .into_iter()
        .find(|d| d.source != Source::External)
    {
        let sel = *d.spans.first()?;
        let statements = &d.analysis.parsed.statements;
        let i = statements.partition_point(|s| s.span.end < sel.start);
        let range = statements
            .get(i)
            .map(|s| s.span)
            .filter(|s| s.start <= sel.start && sel.end <= s.end)
            .unwrap_or(sel);
        return at(d.analysis, range, sel);
    }
    if let Some(a) = asking
        && let Some(r) = a.refs_of(&iri).next()
    {
        return at(a, r.span, r.span);
    }
    k.docs_referencing(&iri)
        .filter(|(_, s, _)| *s != Source::External)
        .find_map(|(_, _, a)| a.refs_of(&iri).next().map(|r| (a, r.span)))
        .and_then(|(a, span)| at(a, span, span))
}

/// The asking document's prefixes: those it declares win over those it gets
/// from outside (prefixes.ttl), which may bind the same namespace.
#[derive(Default)]
struct Names {
    declared: PrefixMap,
    all: PrefixMap,
}

impl Names {
    fn of(a: Option<&Analysis>) -> Names {
        a.map_or_else(Names::default, |a| Names {
            declared: a
                .parsed
                .prefixes
                .iter()
                .map(|p| (p.name.clone(), p.iri.to_string()))
                .collect(),
            all: a.prefix_map(),
        })
    }

    fn short(&self, iri: &str) -> String {
        shorten(iri, &self.declared).unwrap_or_else(|| short(iri, &self.all))
    }
}

fn item(
    k: &Knowledge,
    enc: Encoding,
    iri: Iri,
    kind: Kind,
    asking: Option<&Analysis>,
    names: &Names,
) -> Option<TypeHierarchyItem> {
    let (uri, range, selection_range) = locate(k, enc, iri, asking)?;
    let borrowed = VOCABS
        .lookup(&iri)
        .filter(|_| !k.is_described(&iri))
        .map(|(v, _)| format!("from the {} vocabulary", v.prefix));
    let detail = borrowed.or_else(|| k.label(&iri).map(str::to_string));
    Some(TypeHierarchyItem {
        name: names.short(&iri),
        kind: kind.symbol(),
        tags: None,
        detail,
        uri,
        range,
        selection_range,
        data: Some(json!({
            "iri": iri.as_str(),
            "kind": kind.name(),
            "uri": asking.map(|a| a.uri.as_str()),
        })),
    })
}

/// `textDocument/prepareTypeHierarchy`: the class or property under the cursor.
pub fn prepare(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
) -> Option<Vec<TypeHierarchyItem>> {
    let offset = a.offset(enc, pos)?;
    let iri = a.ref_at(offset)?.iri;
    let kind = kind_of(k, iri)?;
    let it = item(k, enc, iri, kind, Some(a), &Names::of(Some(a)))?;
    Some(vec![it])
}

/// The document a hierarchy item was asked from (its `data.uri`).
pub fn asking_uri(item: &TypeHierarchyItem) -> Option<&str> {
    item.data.as_ref()?.get("uri")?.as_str()
}

fn related(
    item: &TypeHierarchyItem,
    k: &Knowledge,
    enc: Encoding,
    asking: Option<&Analysis>,
    get: impl Fn(&Knowledge, Iri) -> Vec<Iri>,
) -> Vec<TypeHierarchyItem> {
    let Some(data) = item.data.as_ref() else {
        return vec![];
    };
    let Some(iri) = data["iri"].as_str().and_then(Iri::get) else {
        return vec![];
    };
    let kind = match data["kind"].as_str() {
        Some("property") => Kind::Property,
        Some("class") => Kind::Class,
        _ => match kind_of(k, iri) {
            Some(kind) => kind,
            None => return vec![],
        },
    };
    let names = Names::of(asking);
    get(k, iri)
        .into_iter()
        .filter(|t| *t != iri)
        .filter_map(|t| self::item(k, enc, t, kind, asking, &names))
        .collect()
}

/// `typeHierarchy/supertypes`: direct super classes / properties.
pub fn supertypes(
    item: &TypeHierarchyItem,
    k: &Knowledge,
    enc: Encoding,
    asking: Option<&Analysis>,
) -> Vec<TypeHierarchyItem> {
    related(item, k, enc, asking, |k, i| k.supers(&i))
}

/// `typeHierarchy/subtypes`: direct sub classes / properties.
pub fn subtypes(
    item: &TypeHierarchyItem,
    k: &Knowledge,
    enc: Encoding,
    asking: Option<&Analysis>,
) -> Vec<TypeHierarchyItem> {
    related(item, k, enc, asking, |k, i| k.subs(&i))
}
