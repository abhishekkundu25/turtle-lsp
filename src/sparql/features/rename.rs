//! Rename in SPARQL documents: a variable in its scope (each occurrence
//! keeps its sigil), a prefix in its document (the `PREFIX` lines and every
//! prefixed name), or a term across the workspace (the RDF documents as the
//! Turtle rename does, and every SPARQL document); and the SPARQL half of a
//! term rename started in an RDF document.

use super::navigation::{
    Target, name_only, occurrences, prefix_of, scope_at, target_at, with_others,
};
use crate::analysis::{Analysis, Encoding, SparqlAnalysis};
use crate::features::navigation::{self as rdf, shown, uri};
use crate::knowledge::Knowledge;
use crate::sparql::{Form, PrefixSource, VarOcc};
use crate::syntax::names::shorten;
use crate::syntax::names::{is_pn_chars_u, is_pn_prefix};
use lsp_types::*;
use std::collections::HashMap;
use std::sync::Arc;

pub fn prepare_rename(
    q: &SparqlAnalysis,
    enc: Encoding,
    pos: Position,
) -> Option<PrepareRenameResponse> {
    let offset = q.offset(enc, pos)?;
    let span = match target_at(q, offset)? {
        // The name: the sigil stays.
        Target::Var(v) => crate::syntax::Span::new(v.span.start + 1, v.span.end),
        Target::Prefix { span, .. } => span,
        Target::Term(r) if r.form == Form::A || r.iri.is_none() => return None,
        Target::Term(r) => r.span,
    };
    Some(PrepareRenameResponse::Range(q.range(enc, span)))
}

pub fn rename(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    new_name: &str,
    prefix_file: Option<Arc<Analysis>>,
    others: &[Arc<SparqlAnalysis>],
) -> Result<WorkspaceEdit, String> {
    let offset = q.offset(enc, pos).ok_or("invalid position")?;
    let target = target_at(q, offset).ok_or("nothing to rename here")?;
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    match target {
        Target::Var(v) => {
            changes.insert(uri(&q.uri), rename_var(q, enc, v, new_name)?);
        }
        Target::Prefix { name, .. } => {
            changes.insert(
                uri(&q.uri),
                rename_prefix(q, enc, &name, new_name, prefix_file)?,
            );
        }
        Target::Term(r) => {
            let old = r
                .iri
                .as_deref()
                .filter(|_| r.form != Form::A)
                .ok_or("nothing to rename here")?;
            let new_iri = rdf::renamed_iri(old, new_name, &scope_at(q, r.span.start))?;
            rdf::rename_term(k, enc, old, &new_iri, &mut changes)?;
            sparql_term_edits(&with_others(q, others), old, &new_iri, enc, &mut changes);
        }
    }
    Ok(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

pub fn rdf_term_rename(
    old: &str,
    new_iri: &str,
    docs: &[Arc<SparqlAnalysis>],
    enc: Encoding,
    edit: &mut WorkspaceEdit,
) -> Result<(), String> {
    let docs: Vec<&SparqlAnalysis> = docs.iter().map(|d| &**d).collect();
    sparql_term_edits(
        &docs,
        old,
        new_iri,
        enc,
        edit.changes.get_or_insert_with(HashMap::new),
    );
    Ok(())
}

/// A SPARQL VARNAME.
fn is_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| is_pn_chars_u(c) || c.is_ascii_digit())
        && chars.all(|c| {
            is_pn_chars_u(c)
                || c.is_ascii_digit()
                || matches!(c, '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
        })
}

fn rename_var(
    q: &SparqlAnalysis,
    enc: Encoding,
    v: &VarOcc,
    new_name: &str,
) -> Result<Vec<TextEdit>, String> {
    let new = new_name.strip_prefix(['?', '$']).unwrap_or(new_name).trim();
    if !is_var_name(new) {
        return Err(format!("'{new_name}' is not a valid variable name"));
    }
    let occs = occurrences(q, v);
    if new == v.name {
        return Ok(Vec::new());
    }
    // The scopes the renamed variable is visible in: its own, and each
    // subquery it is written in (where it would meet a variable projected
    // from there).
    let mut scopes = vec![v.scope];
    scopes.extend(occs.iter().map(|o| o.raw_scope));
    let taken = q
        .doc
        .structure()
        .vars
        .iter()
        .any(|o| o.name == new && (scopes.contains(&o.scope) || scopes.contains(&o.raw_scope)));
    if taken {
        return Err(format!("?{new} is already used in this scope"));
    }
    let text = q.text();
    Ok(occs
        .iter()
        .map(|o| TextEdit {
            range: q.range(enc, o.span),
            new_text: format!("{}{new}", &text[o.span.start as usize..][..1]),
        })
        .collect())
}

fn rename_prefix(
    q: &SparqlAnalysis,
    enc: Encoding,
    old: &str,
    new_name: &str,
    prefix_file: Option<Arc<Analysis>>,
) -> Result<Vec<TextEdit>, String> {
    let st = q.doc.structure();
    let new = new_name.trim_end_matches(':');
    if !is_pn_prefix(new) {
        return Err(format!("'{new_name}' is not a valid prefix name"));
    }
    let declared = st.prefixes.iter().any(|p| p.name == old);
    if !declared && q.external.contains_key(old) {
        return Err(
            match prefix_file.filter(|pf| pf.parsed.prefixes.iter().any(|p| p.name == old)) {
                Some(pf) => format!(
                    "prefix '{old}:' is declared in {}; rename it there",
                    shown(&pf.uri)
                ),
                None => format!(
                    "prefix '{old}:' comes from the turtle.prefixes setting; rename it there"
                ),
            },
        );
    }
    if new == old {
        return Ok(Vec::new());
    }
    if st.prefixes.iter().any(|p| p.name == new) {
        return Err(format!(
            "prefix '{new}:' is already declared in {}",
            shown(&q.uri)
        ));
    }
    if st.refs.iter().any(|r| prefix_of(q, r) == Some(new)) {
        return Err(format!(
            "prefix '{new}:' is already used in {}",
            shown(&q.uri)
        ));
    }
    let edit = |span| TextEdit {
        range: q.range(enc, span),
        new_text: new.to_string(),
    };
    let mut edits: Vec<TextEdit> = st
        .prefixes
        .iter()
        .filter(|p| p.name == old)
        .map(|p| edit(name_only(p.name_span.start, old)))
        .collect();
    edits.extend(
        st.refs
            .iter()
            .filter(|r| {
                prefix_of(q, r) == Some(old)
                    && (declared || r.prefix != Some(PrefixSource::External))
            })
            .map(|r| edit(name_only(r.span.start, old))),
    );
    Ok(edits)
}

/// Adds the edits renaming IRI `old` to `new_iri` in SPARQL documents
/// `docs`: a full IRI stays one; a prefixed name takes the prefixes in
/// effect there when they shorten the new IRI, else becomes a full IRI.
fn sparql_term_edits(
    docs: &[&SparqlAnalysis],
    old: &str,
    new_iri: &str,
    enc: Encoding,
    changes: &mut HashMap<Uri, Vec<TextEdit>>,
) {
    for d in docs {
        let edits: Vec<TextEdit> = d
            .doc
            .structure()
            .refs
            .iter()
            .filter(|r| r.form != Form::A && r.iri.as_deref() == Some(old))
            .map(|r| {
                let short = match r.form {
                    Form::PName { .. } => shorten(new_iri, &scope_at(d, r.span.start)),
                    _ => None,
                };
                TextEdit {
                    range: d.range(enc, r.span),
                    new_text: short.unwrap_or_else(|| format!("<{new_iri}>")),
                }
            })
            .collect();
        if !edits.is_empty() {
            changes.entry(uri(&d.uri)).or_default().extend(edits);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::PrefixMap;

    #[test]
    fn a_renamed_term_is_never_written_as_an_unreadable_prefixed_name() {
        let text = "PREFIX ex: <http://example.org/>\nSELECT * { ex:a ex:p ?o . ex:b ex:p ?o }\n";
        let q = SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()));
        let renamed = |new_iri: &str| {
            let mut changes = HashMap::new();
            sparql_term_edits(
                &[&q],
                "http://example.org/a",
                new_iri,
                Encoding::Utf16,
                &mut changes,
            );
            changes
                .into_values()
                .flatten()
                .map(|e| e.new_text)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            renamed("http://example.org/-a"),
            ["<http://example.org/-a>"]
        );
        assert_eq!(renamed("http://example.org/a-b"), ["ex:a-b"]);
    }

    #[test]
    fn a_prefix_is_renamed_only_to_a_valid_prefix_name() {
        let text = "PREFIX ex: <http://example.org/>\nSELECT * { ex:a ex:p ?o }\n";
        let q = SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()));
        for bad in ["1x", "a.", "a b", "-a"] {
            assert!(
                rename_prefix(&q, Encoding::Utf16, "ex", bad, None).is_err(),
                "{bad}"
            );
        }
        assert!(rename_prefix(&q, Encoding::Utf16, "ex", "eg:", None).is_ok());
    }
}
