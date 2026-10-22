//! Hover: what a term is, where it is described, and its label/comment.

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use crate::syntax::{Form, Iri, PrefixMap};
use crate::vocab::{TermKind, VOCABS};
use crate::workspace::uri_to_path;
use lsp_types::*;

pub fn hover(a: &Analysis, k: &Knowledge, enc: Encoding, pos: Position) -> Option<Hover> {
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
                let ns = a.prefix_map().get(&name).cloned().unwrap_or_default();
                let origin = if a.parsed.prefixes.iter().any(|p| p.name == name) {
                    "declared in this file"
                } else {
                    "from the project prefixes"
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
    let prefixes = a.prefix_map();
    let label = text_label.unwrap_or_else(|| short(&r.iri, &prefixes));
    let body = describe_iri(&r.iri, &label, k, &prefixes)?;
    Some(md(body, a.range(enc, r.span)))
}

/// `iri` as `prefix:local` with the document's prefixes, else with the
/// well-known vocabulary's prefix, else `<iri>`.
pub fn short(iri: &str, prefixes: &PrefixMap) -> String {
    crate::features::completion::shorten(iri, prefixes)
        .or_else(|| {
            let (v, local) = VOCABS.lookup(iri)?;
            crate::features::completion::shorten(
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
pub fn describe_iri(iri: &str, label: &str, k: &Knowledge, prefixes: &PrefixMap) -> Option<String> {
    let mut out = format!("**{label}**");
    let mut kind = None;
    let mut text_label = None;
    let mut comment = None;

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
        if t.deprecated {
            out.push_str(" ~~deprecated~~");
        }
    }

    let mut defined_in: Vec<String> = k
        .descriptions(iri)
        .iter()
        .map(|d| {
            let a = d.analysis;
            let line = a.position(Encoding::Utf8, d.spans[0].start).line + 1;
            // A cached import is named by the URL it came from.
            if let Some(origin) = &a.origin {
                return format!("{origin}:{line}");
            }
            let file = uri_to_path(&a.uri)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .or_else(|| crate::workspace::file_uri_name(&a.uri))
                .unwrap_or_else(|| a.uri.clone());
            // A workspace file links to the line, as editors open `#L<line>`.
            if a.uri.starts_with("file:") {
                format!("[{file}:{line}]({}#L{line})", a.uri)
            } else {
                format!("{file}:{line}")
            }
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
}
