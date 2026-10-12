//! Hover: what a term is, where it is described, and its label/comment.

use crate::analysis::{Analysis, Encoding};
use crate::syntax::Form;
use crate::vocab::{TermKind, VOCABS};
use crate::workspace::uri_to_path;
use lsp_types::*;
use std::sync::Arc;

pub fn hover(a: &Analysis, all: &[Arc<Analysis>], enc: Encoding, pos: Position) -> Option<Hover> {
    let offset = a.offset(enc, pos)?;

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
                    .map(|u| u.name.clone())
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
    let label = text_label.unwrap_or_else(|| {
        crate::features::completion::shorten(&r.iri, &a.prefix_map())
            .unwrap_or_else(|| format!("<{}>", r.iri))
    });
    let body = describe_iri(&r.iri, &label, all)?;
    Some(md(body, a.range(enc, r.span)))
}

/// Markdown description of an IRI from the workspace and bundled vocabularies.
pub fn describe_iri(iri: &str, label: &str, all: &[Arc<Analysis>]) -> Option<String> {
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

    let mut defined_in = Vec::new();
    let mut types: Vec<String> = Vec::new();
    for a in all {
        if let Some(spans) = a.defs.get(iri) {
            let file = uri_to_path(&a.uri)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| a.uri.clone());
            let line = a.position(Encoding::Utf8, spans[0].start).line + 1;
            defined_in.push(format!("{file}:{line}"));
        }
        text_label = text_label.or_else(|| a.labels.get(iri).cloned());
        comment = comment.or_else(|| a.comments.get(iri).cloned());
        if let Some(t) = a.types.get(iri) {
            for ty in t {
                let short = crate::features::completion::shorten(ty, &a.prefix_map())
                    .unwrap_or_else(|| format!("<{ty}>"));
                if !types.contains(&short) {
                    types.push(short);
                }
            }
        }
    }
    if !types.is_empty() {
        out.push_str(&format!(" · {}", types.join(", ")));
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
    if !defined_in.is_empty() {
        defined_in.sort();
        defined_in.truncate(5);
        out.push_str(&format!("\n\n_Described in {}_", defined_in.join(", ")));
    } else if let Some((v, _)) = VOCABS.lookup(iri) {
        out.push_str(&format!("\n\n_From the {} vocabulary_", v.prefix));
    }
    Some(out)
}

fn md(value: String, range: Range) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    }
}
