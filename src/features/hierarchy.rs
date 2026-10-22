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
use crate::features::hover::short;
use crate::features::navigation::uri;
use crate::knowledge::{Knowledge, Source};
use crate::syntax::names::shorten;
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
        let parsed = &d.analysis.parsed;
        let range = parsed
            .statement_at(sel)
            .map(|i| parsed.statements[i].span)
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
pub(crate) fn asking_uri(item: &TypeHierarchyItem) -> Option<&str> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;

    /// JSON-LD node objects nest, and so do their statements: a class
    /// described in a nested node object is shown as that object, not as
    /// the one around it.
    const VOCAB: &str = r#"{
  "@context": {
    "ex": "http://ex.org/",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#"
  },
  "@id": "ex:vocab",
  "ex:defines": {
    "@id": "ex:Gadget",
    "@type": "rdfs:Class",
    "rdfs:subClassOf": { "@id": "ex:Thing", "@type": "rdfs:Class" }
  }
}
"#;

    /// The node object whose `@id` is `id`, braces included.
    fn object(text: &str, id: &str) -> Span {
        let at = text.find(&format!("\"@id\": \"{id}\"")).unwrap();
        let start = text[..at].rfind('{').unwrap();
        let mut depth = 0;
        for (i, c) in text[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Span::new(start as u32, (start + i + 1) as u32);
                    }
                }
                _ => {}
            }
        }
        unreachable!()
    }

    #[test]
    fn the_innermost_json_ld_statement_contains_a_span() {
        let p = crate::jsonld::parse(VOCAB, None, &crate::jsonld::NoNetwork::default());
        let at = |id: &str| {
            let i = VOCAB.find(&format!("\"{id}\"")).unwrap() as u32 + 1;
            p.statement_at(Span::new(i, i + id.len() as u32))
                .map(|i| p.statements[i].span)
        };
        assert_eq!(at("ex:Gadget"), Some(object(VOCAB, "ex:Gadget")));
        assert_eq!(at("ex:Thing"), Some(object(VOCAB, "ex:Thing")));
        assert_eq!(at("ex:vocab").map(|s| s.start), Some(0));
        let around = p.statements_around(object(VOCAB, "ex:Thing")).count();
        assert_eq!(around, 3, "{:?}", p.statements);
    }

    #[test]
    fn a_class_described_in_a_nested_json_ld_object_is_shown_as_that_object() {
        let mut ws = Workspace::default();
        let (ld, ttl) = ("file:///w/vocab.jsonld", "file:///w/uses.ttl");
        let uses = "@prefix ex: <http://ex.org/> .\nex:x a ex:Gadget , ex:Thing .\n";
        ws.open_doc(ld.into(), VOCAB.into(), 1);
        ws.open_doc(ttl.into(), uses.into(), 1);
        let a = ws.analysis(ttl).unwrap();
        let k = ws.knowledge();
        let vocab = k.get(ld).unwrap().clone();
        for id in ["ex:Gadget", "ex:Thing"] {
            let col = uses.lines().nth(1).unwrap().find(id).unwrap() as u32 + 1;
            let it = prepare(&a, k, Encoding::Utf8, Position::new(1, col)).unwrap()[0].clone();
            assert_eq!(it.uri.as_str(), ld);
            assert_eq!(
                it.range,
                vocab.range(Encoding::Utf8, object(VOCAB, id)),
                "{id}"
            );
        }
    }
}
