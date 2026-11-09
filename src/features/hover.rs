//! Hover: what a term is, where it is described, and its label/comment.

use super::prefixes::{Naming, Scope, declaration_at};
use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use crate::reasoning::{HOVER_WAIT, Inferred, Names, NoProof, Origin, Shared, Shown, Types};
use crate::syntax::names::shorten;
use crate::syntax::{Form, Iri, PrefixMap};
use crate::vocab::{TermKind, VOCABS};
use crate::workspace::uri_to_path;
use lsp_types::*;
use std::sync::Arc;

/// Inferred types a hover lists; the rest are counted.
const MAX_INFERRED: usize = 20;

pub fn hover(a: &Analysis, k: &Knowledge, enc: Encoding, pos: Position) -> Option<Hover> {
    hover_reasoned(a, k, enc, pos, None)
}

/// [`hover`], and on a resource, with `reasoning` on, the types reasoning
/// infers for it after what [`hover`] says (see [`inferred`]); `reasoning`
/// holds the project's prefixes for `a` too, which the proofs are named
/// with after `a`'s own ([`Naming`]).
pub(crate) fn hover_reasoned(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    reasoning: Option<(&Arc<Shared>, Arc<PrefixMap>)>,
) -> Option<Hover> {
    let offset = a.offset(enc, pos)?;
    if let Some(h) = super::embedded::hover(a, k, enc, offset) {
        return h;
    }

    // On a prefix declaration's name: show the namespace.
    if let Some(p) = a
        .parsed
        .prefixes
        .iter()
        .find(|p| p.name_span.contains(offset))
    {
        return Some(md(
            format!("**{}:** → `<{}>`", p.name, p.iri),
            a.range(enc, p.name_span),
        ));
    }

    let r = a.ref_at(offset)?;
    let text_label = match r.form {
        Form::PName { prefix_len } => {
            // Cursor on the prefix part: describe the prefix.
            if offset <= r.span.start + prefix_len {
                let name = a
                    .parsed
                    .prefix_uses
                    .iter()
                    .find(|u| u.span == r.span)
                    .map(|u| u.name.to_string())
                    .unwrap_or_default();
                // The declaration in scope, as go-to-definition finds it.
                let (ns, origin) = match declaration_at(a, &name, offset) {
                    Some(p) => (p.iri.to_string(), "declared in this file"),
                    None => (
                        a.external.get(&name).cloned().unwrap_or_default(),
                        "from the project prefixes",
                    ),
                };
                return Some(md(
                    format!("**{name}:** → `<{ns}>`  \n_{origin}_"),
                    a.range(enc, r.span),
                ));
            }
            None
        }
        Form::A => Some("a (rdf:type)".to_string()),
        Form::Iri => None,
    };
    let prefixes = Scope::at(a, r.span.start);
    let label = text_label.unwrap_or_else(|| short(&r.iri, &prefixes));
    let mut body = describe_iri(&r.iri, &label, k, &prefixes)?;
    if let Some((shared, project)) = reasoning {
        let types = shared.inferred_types(&r.iri, HOVER_WAIT);
        let naming = Naming::at(a, r.span.start, project);
        body.push_str(&inferred(types, &r.iri, k, &naming));
    }
    Some(md(body, a.range(enc, r.span)))
}

/// The "Inferred" section of a hover on `iri`: each type reasoning infers
/// and no document asserts, with its proof folded under it; while a run is
/// writing the state, or the proofs are still being searched for, a line
/// that says so. Empty when there is nothing to add. IRIs are written by
/// `naming`.
///
/// The closest types come first: those whose proofs have fewer steps, then
/// less depth, then by name. A step an earlier type's proof wrote is not
/// written again: a later proof refers to it ([`Shown`]).
///
/// A type whose proof rests on no triple of a document (an axiom, as
/// `dt-type1` types `xsd:integer` an `rdfs:Datatype`) says nothing of the
/// text: such types are named on one line after the others, without proofs.
fn inferred(types: Types, iri: &str, k: &Knowledge, naming: &Naming) -> String {
    let asserted = k.types(iri);
    let new = |class: &str| !asserted.iter().any(|t| &**t == class);
    let iri_name = |i: &str| naming.name(i);
    let place = |o: &Origin| place(&o.uri, o.imported_from.as_deref(), o.line);
    let names = Names {
        iri: &iri_name,
        place: &place,
    };
    let title = |class: &str| format!("**{}**", iri_name(class));
    let mut built_in = Vec::new();
    let (items, note): (Vec<String>, _) = match types {
        Types::Unknown => return String::new(),
        Types::Running => return "\n\n**Inferred:** _reasoning is running_".into(),
        Types::Proved(v) => {
            let mut listed: Vec<(Inferred, String)> = Vec::new();
            for t in v.into_iter().filter(|t| new(&t.class)) {
                match &t.proof {
                    Ok(p) if !p.rests_on_documents() => built_in.push(iri_name(&t.class)),
                    _ => {
                        let title = title(&t.class);
                        listed.push((t, title));
                    }
                }
            }
            listed.sort_by_cached_key(|(t, title)| {
                let size = t
                    .proof
                    .as_ref()
                    .map_or((usize::MAX, usize::MAX), |p| p.size());
                (size, title.clone())
            });
            let mut shown = Shown::default();
            let written = listed.iter().enumerate().map(|(n, (t, title))| {
                match &t.proof {
                    // Not written, only counted.
                    _ if n >= MAX_INFERRED => String::new(),
                    Ok(p) => p.markdown(title, &names, &mut shown),
                    Err(NoProof::NotFound) => format!("- {title} · _no proof found_"),
                    Err(NoProof::Stopped) => {
                        format!("- {title} · _proof search stopped at its time limit_")
                    }
                }
            });
            (written.collect(), "")
        }
        Types::Pending(classes) => (
            classes
                .iter()
                .filter(|c| new(c))
                .map(|c| format!("- {}", title(c)))
                .collect(),
            "\n\n_The proof is still being worked out; hover again._",
        ),
    };
    if items.is_empty() && built_in.is_empty() {
        return String::new();
    }
    let more = items.len().saturating_sub(MAX_INFERRED);
    let mut lines = items[..items.len() - more].to_vec();
    if more > 0 {
        lines.push(format!("- _and {more} more_"));
    }
    if !built_in.is_empty() {
        built_in.sort();
        lines.push(format!(
            "- _From the built-in vocabulary:_ {}",
            built_in.join(", ")
        ));
    }
    let mut out = format!("\n\n**Inferred**\n\n{}", lines.join("\n"));
    out.push_str(note);
    out
}

/// Line `line` (from 1) of the document at `uri`, as hover names it: a
/// cached import by the URL it came from (`imported_from`), `url:line`; a
/// workspace file as a link to the line, as editors open `#L<line>`; any
/// other document by its name.
pub(crate) fn place(uri: &str, imported_from: Option<&str>, line: u32) -> String {
    if let Some(origin) = imported_from {
        return format!("{origin}:{line}");
    }
    let file = file_label(uri);
    if uri.starts_with("file:") {
        format!("[{file}:{line}]({uri}#L{line})")
    } else {
        format!("{file}:{line}")
    }
}

/// The name of the document at `uri`: its file's name, else the URI.
pub(crate) fn file_label(uri: &str) -> String {
    uri_to_path(uri)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .or_else(|| crate::workspace::file_uri_name(uri))
        .unwrap_or_else(|| uri.to_string())
}

/// `iri` as `prefix:local` with the document's prefixes, else with the
/// well-known vocabulary's prefix, else `<iri>`.
pub(crate) fn short(iri: &str, prefixes: &PrefixMap) -> String {
    shorten(iri, prefixes)
        .or_else(|| {
            let (v, local) = VOCABS.lookup(iri)?;
            shorten(
                iri,
                &PrefixMap::from_iter([(v.prefix.to_string(), v.namespace.to_string())]),
            )
            .filter(|_| !local.is_empty())
        })
        .unwrap_or_else(|| format!("<{iri}>"))
}

fn shorts(iris: &[Iri], prefixes: &PrefixMap) -> String {
    iris.iter()
        .map(|i| short(i, prefixes))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Markdown description of an IRI from the workspace and bundled vocabularies.
pub(crate) fn describe_iri(
    iri: &str,
    label: &str,
    k: &Knowledge,
    prefixes: &PrefixMap,
) -> Option<String> {
    let mut out = format!("**{label}**");
    let mut kind = None;
    let mut text_label = None;
    let mut comment = None;
    // A workspace file's `owl:deprecated true`, or the vocabulary's flag.
    let mut deprecated = k.deprecated_here(iri);

    if let Some((v, local)) = VOCABS.lookup(iri)
        && let Some(t) = v.term(local)
    {
        kind = Some(match t.kind {
            TermKind::Class => "class",
            TermKind::Property => "property",
            TermKind::Individual => "individual",
        });
        if !t.label.is_empty() {
            text_label = Some(t.label.clone());
        }
        if !t.comment.is_empty() {
            comment = Some(t.comment.clone());
        }
        deprecated |= t.deprecated;
    }
    if deprecated {
        out.push_str(" ~~deprecated~~");
    }

    let mut defined_in: Vec<String> = k
        .descriptions(iri)
        .iter()
        .map(|d| {
            let a = d.analysis;
            let line = a.position(Encoding::Utf8, d.spans[0].start).line + 1;
            place(&a.uri, a.origin.as_deref(), line)
        })
        .collect();
    text_label = text_label.or_else(|| k.label(iri).map(str::to_string));
    comment = comment.or_else(|| k.comment(iri).map(str::to_string));
    let types = k.types(iri);
    if !types.is_empty() {
        out.push_str(&format!(" · {}", shorts(&types, prefixes)));
    } else if let Some(k) = kind {
        out.push_str(&format!(" · {k}"));
    }
    out.push_str(&format!("  \n`<{iri}>`"));
    if let Some(l) = text_label
        && l != label
    {
        out.push_str(&format!("\n\n**{l}**"));
    }
    if let Some(c) = comment {
        out.push_str(&format!("\n\n{c}"));
    }
    let (domains, ranges) = (k.domains(iri), k.ranges(iri));
    let mut dr = Vec::new();
    if !domains.is_empty() {
        dr.push(format!("Domain: {}", shorts(&domains, prefixes)));
    }
    if !ranges.is_empty() {
        dr.push(format!("Range: {}", shorts(&ranges, prefixes)));
    }
    if !dr.is_empty() {
        out.push_str(&format!("\n\n{}", dr.join(" · ")));
    }
    if !defined_in.is_empty() {
        defined_in.sort();
        defined_in.truncate(5);
        out.push_str(&format!("\n\n_Described in {}_", defined_in.join(", ")));
    } else if let Some((v, _)) = VOCABS.lookup(iri) {
        out.push_str(&format!("\n\n_From the {} vocabulary_", v.prefix));
    }
    Some(out)
}

pub(crate) fn md(value: String, range: Range) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn hover_names_the_url_a_cached_import_came_from() {
        let text = "\n<http://ex.com/x#T> a <http://www.w3.org/2002/07/owl#Class> .\n";
        let mut a = Analysis::new(
            "file:///cache/imports/0123456789abcdef.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            false,
        );
        a.origin = Some("http://ex.com/x".into());
        let mut k = Knowledge::default();
        k.add_external("import:http://ex.com/x", Arc::new(a));
        let md = describe_iri("http://ex.com/x#T", "T", &k, &PrefixMap::default()).unwrap();
        assert!(md.contains("_Described in http://ex.com/x:2_"), "{md}");
        assert!(!md.contains("0123456789abcdef"), "{md}");
    }

    #[test]
    fn the_inferred_section_says_what_reasoning_has_and_skips_asserted_types() {
        let text = "<http://example.org/rex> a <http://example.org/Dog> .\n";
        let a = Analysis::new("file:///w/a.ttl".into(), text, Arc::default(), false);
        let mut k = Knowledge::default();
        k.set(
            "file:///w/a.ttl",
            crate::knowledge::Source::Open,
            Arc::new(a),
        );
        let ex = |l: &str| format!("http://example.org/{l}");
        let prefixes = PrefixMap::from_iter([("ex".to_string(), ex(""))]);
        let a = Analysis::new("file:///w/a.ttl".into(), text, Arc::default(), false);
        let naming = Naming::at(&a, 0, Arc::new(prefixes));
        let section = |t: Types| inferred(t, &ex("rex"), &k, &naming);
        assert_eq!(section(Types::Unknown), "");
        assert_eq!(section(Types::Proved(Vec::new())), "");
        assert_eq!(
            section(Types::Running),
            "\n\n**Inferred:** _reasoning is running_"
        );
        // A type a document asserts is not inferred news, though the last
        // run, older than the text, inferred it.
        assert_eq!(
            section(Types::Pending(vec![ex("Animal"), ex("Dog")])),
            "\n\n**Inferred**\n\n- **ex:Animal**\n\n\
             _The proof is still being worked out; hover again._"
        );
        assert_eq!(section(Types::Pending(vec![ex("Dog")])), "");
        let many = (0..25).map(|i| ex(&format!("C{i:02}"))).collect();
        let md = section(Types::Pending(many));
        assert!(
            md.contains("- **ex:C19**\n- _and 5 more_\n\n_The proof"),
            "{md}"
        );
        assert!(!md.contains("ex:C20"), "{md}");
        let none = section(Types::Proved(vec![crate::reasoning::Inferred {
            class: ex("Animal"),
            proof: Err(crate::reasoning::NoProof::NotFound),
        }]));
        assert_eq!(
            none,
            "\n\n**Inferred**\n\n- **ex:Animal** · _no proof found_"
        );
        let stopped = section(Types::Proved(vec![crate::reasoning::Inferred {
            class: ex("Animal"),
            proof: Err(crate::reasoning::NoProof::Stopped),
        }]));
        assert_eq!(
            stopped,
            "\n\n**Inferred**\n\n- **ex:Animal** · _proof search stopped at its time limit_"
        );
    }

    #[test]
    fn hover_says_an_own_deprecated_term_is_deprecated() {
        let text = "<http://ex.com/x#T> <http://www.w3.org/2002/07/owl#deprecated> true .\n";
        let a = Analysis::new(
            "file:///work/onto.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            false,
        );
        let mut k = Knowledge::default();
        k.set(
            "file:///work/onto.ttl",
            crate::knowledge::Source::Disk,
            Arc::new(a),
        );
        let md = describe_iri("http://ex.com/x#T", "T", &k, &PrefixMap::default()).unwrap();
        assert!(md.starts_with("**T** ~~deprecated~~"), "{md}");
        // An import's deprecation is not the workspace's.
        let mut k = Knowledge::default();
        let a = Analysis::new("file:///c.ttl".into(), text, Arc::default(), false);
        k.add_external("import:http://ex.com/x", Arc::new(a));
        let md = describe_iri("http://ex.com/x#T", "T", &k, &PrefixMap::default()).unwrap();
        assert!(!md.contains("deprecated"), "{md}");
    }

    #[test]
    fn hover_links_a_workspace_description_to_its_line() {
        let text = "\n<http://ex.com/x#T> a <http://www.w3.org/2002/07/owl#Class> .\n";
        let a = Analysis::new(
            "file:///work/my%20onto.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            false,
        );
        let mut k = Knowledge::default();
        k.add_external("file:///work/my%20onto.ttl", Arc::new(a));
        let md = describe_iri("http://ex.com/x#T", "T", &k, &PrefixMap::default()).unwrap();
        assert!(
            md.contains("_Described in [my onto.ttl:2](file:///work/my%20onto.ttl#L2)_"),
            "{md}"
        );
    }

    #[test]
    fn hover_in_a_shacl_query() {
        use crate::syntax::Span;
        let text = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
                    @prefix ex: <http://example.com/> .\n\
                    ex:P sh:declare [ sh:prefix \"my\" ; sh:namespace \"http://example.com/my#\" ] .\n\
                    ex:S sh:sparql [ sh:prefixes ex:P ; sh:select \"SELECT $this WHERE { $this ex:p \\\"a\\\" ; my:q ?v }\" ] .\n";
        let a = Analysis::new(
            "file:///w/shapes.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        );
        let at = text.find("my:q").unwrap() as u32;
        let h = hover(
            &a,
            &Knowledge::default(),
            Encoding::Utf16,
            a.position(Encoding::Utf16, at + 1),
        )
        .expect("a hover on the prefix");
        let HoverContents::Markup(m) = &h.contents else {
            panic!()
        };
        assert!(m.value.contains("<http://example.com/my#>"), "{}", m.value);
        assert_eq!(
            h.range,
            Some(a.range(Encoding::Utf16, Span::new(at, at + 4)))
        );
    }

    #[test]
    fn hover_and_definition_agree_on_the_declaration_in_scope() {
        let text = "@prefix ex: <http://a/> .\nex:x ex:p ex:o .\n@prefix ex: <http://b/> .\nex:y ex:p ex:o .\n";
        let external: PrefixMap =
            PrefixMap::from_iter([("ex".to_string(), "http://ext/".to_string())]);
        let a = Analysis::new("file:///w/doc.ttl".into(), text, Arc::new(external), true);
        let k = Knowledge::default();
        let at = |needle: &str| a.position(Encoding::Utf16, text.find(needle).unwrap() as u32 + 1);
        let hover_text = |needle: &str| {
            let h = hover(&a, &k, Encoding::Utf16, at(needle)).unwrap();
            let HoverContents::Markup(m) = h.contents else {
                panic!()
            };
            m.value
        };
        for (needle, ns, line) in [("ex:x", "http://a/", 0), ("ex:y", "http://b/", 2)] {
            let defs =
                crate::features::navigation::definition(&a, &k, Encoding::Utf16, at(needle), None);
            assert_eq!(defs.len(), 1, "{needle}");
            assert_eq!(defs[0].range.start.line, line, "{needle}");
            let md = hover_text(needle);
            assert!(md.contains(&format!("<{ns}>")), "{needle}: {md}");
            assert!(md.contains("declared in this file"), "{needle}: {md}");
        }
    }

    #[test]
    fn a_prefix_used_above_its_declaration_comes_from_outside() {
        let text = "ex:x ex:p ex:o .\n@prefix ex: <http://b/> .\n";
        let external: PrefixMap =
            PrefixMap::from_iter([("ex".to_string(), "http://ext/".to_string())]);
        let a = Analysis::new("file:///w/doc.ttl".into(), text, Arc::new(external), true);
        let pos = a.position(Encoding::Utf16, 1);
        let h = hover(&a, &Knowledge::default(), Encoding::Utf16, pos).unwrap();
        let HoverContents::Markup(m) = h.contents else {
            panic!()
        };
        assert!(m.value.contains("<http://ext/>"), "{}", m.value);
        assert!(m.value.contains("from the project prefixes"), "{}", m.value);
        let defs = crate::features::navigation::definition(
            &a,
            &Knowledge::default(),
            Encoding::Utf16,
            pos,
            None,
        );
        assert!(defs.is_empty(), "{defs:?}");
    }
}
