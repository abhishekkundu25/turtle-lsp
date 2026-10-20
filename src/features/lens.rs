//! Reference lenses: "N references" above each subject a document describes.
//!
//! `textDocument/codeLens` lists one lens per IRI the document describes (a
//! top-level subject, a TriG or N-Quads graph name, a JSON-LD node's `@id`),
//! at its first description, with no title or command; `codeLens/resolve`
//! counts the references then, so a document's lenses cost one pass over its
//! definitions and each count is paid only when the editor shows it.
//!
//! The count is the workspace's RDF references with the declarations left
//! out: what "Find References" lists without its declarations, minus the
//! uses in SPARQL queries (finding those reads every query file from disk,
//! too slow to do once per lens). A description of the IRI in another file
//! is a declaration too, so it is not counted.

use crate::analysis::{Analysis, Encoding};
use crate::features::navigation::term_references;
use crate::knowledge::Knowledge;
use lsp_types::{CodeLens, Command};
use serde_json::{Value, json};

/// The client-side command a resolved lens runs, with arguments
/// `[uri, position, locations]`: the document, the lens's position and the
/// references. The server does not offer it as an `executeCommand`.
pub const SHOW_REFERENCES: &str = "turtle.showReferences";

/// Documents larger than this get no lenses: a lens per subject of a huge
/// generated file is noise, and the list is rebuilt on every edit.
pub const MAX_LENS_BYTES: usize = 5 * 1024 * 1024;

/// The document's lenses, in text order, each with `data`
/// `{"uri": <document>, "iri": <subject IRI>}` for [`resolve`]. None when
/// `enabled` is false or the document is over [`MAX_LENS_BYTES`].
pub fn code_lenses(a: &Analysis, enc: Encoding, enabled: bool) -> Vec<CodeLens> {
    if !enabled || a.text.len() > MAX_LENS_BYTES {
        return vec![];
    }
    let mut firsts: Vec<_> = a
        .defs
        .iter()
        .filter_map(|(iri, spans)| Some((spans.iter().min_by_key(|s| s.start)?, iri)))
        .collect();
    firsts.sort_unstable_by_key(|(span, _)| span.start);
    firsts
        .into_iter()
        .map(|(span, iri)| CodeLens {
            range: a.range(enc, *span),
            command: None,
            data: Some(json!({ "uri": a.uri, "iri": iri.to_string() })),
        })
        .collect()
}

/// `lens` with its title and [`SHOW_REFERENCES`] command; unchanged when its
/// `data` is not one [`code_lenses`] made.
pub fn resolve(mut lens: CodeLens, k: &Knowledge, enc: Encoding) -> CodeLens {
    let Some(data) = &lens.data else {
        return lens;
    };
    let (Some(uri), Some(iri)) = (data["uri"].as_str(), data["iri"].as_str()) else {
        return lens;
    };
    let locations = term_references(k, enc, iri, false);
    let n = locations.len();
    lens.command = Some(Command {
        title: format!("{n} reference{}", if n == 1 { "" } else { "s" }),
        command: SHOW_REFERENCES.into(),
        arguments: Some(vec![
            Value::String(uri.to_string()),
            json!(lens.range.start),
            json!(locations),
        ]),
    });
    lens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::Source;
    use crate::syntax::{PrefixMap, Syntax};
    use lsp_types::Position;
    use std::sync::Arc;

    fn analysis(uri: &str, text: &str) -> Arc<Analysis> {
        Arc::new(Analysis::with_syntax(
            uri.into(),
            text,
            Arc::new(PrefixMap::default()),
            false,
            Syntax::from_extension(uri).unwrap_or_default(),
        ))
    }

    fn title(l: &CodeLens) -> &str {
        &l.command.as_ref().unwrap().title
    }

    #[test]
    fn one_lens_per_subject_counted_on_resolve() {
        let a = analysis(
            "file:///a.ttl",
            "@prefix ex: <http://example.org/> .\n\
             ex:a ex:p ex:b .\n\
             ex:b ex:p ex:a, ex:c .\n\
             ex:a ex:q ex:b .\n\
             ex:c ex:p \"x\" .\n",
        );
        let b = analysis(
            "file:///b.ttl",
            "@prefix ex: <http://example.org/> .\nex:d ex:p ex:b .\n",
        );
        let mut k = Knowledge::default();
        k.set("file:///a.ttl", Source::Open, a.clone());
        k.set("file:///b.ttl", Source::Open, b);
        let lenses = code_lenses(&a, Encoding::Utf16, true);
        let lines: Vec<u32> = lenses.iter().map(|l| l.range.start.line).collect();
        assert_eq!(lines, [1, 2, 4], "{lenses:?}");
        assert!(lenses.iter().all(|l| l.command.is_none()));
        let resolved: Vec<CodeLens> = lenses
            .into_iter()
            .map(|l| resolve(l, &k, Encoding::Utf16))
            .collect();
        // Objects: ex:a on line 2; ex:b on lines 1 and 3 and in b.ttl; ex:c on line 2.
        let titles: Vec<&str> = resolved.iter().map(title).collect();
        assert_eq!(titles, ["1 reference", "3 references", "1 reference"]);
        let c = resolved[1].command.as_ref().unwrap();
        assert_eq!(c.command, SHOW_REFERENCES);
        let args = c.arguments.as_ref().unwrap();
        assert_eq!(args[0], "file:///a.ttl");
        assert_eq!(args[1], json!(Position::new(2, 0)));
        assert_eq!(args[2].as_array().unwrap().len(), 3);
    }

    #[test]
    fn off_and_too_large_give_none() {
        let a = analysis("file:///a.ttl", "<http://x/a> <http://x/p> 1 .\n");
        assert_eq!(code_lenses(&a, Encoding::Utf16, true).len(), 1);
        assert!(code_lenses(&a, Encoding::Utf16, false).is_empty());
        let big = "<http://x/a> <http://x/p> 1 .\n".repeat(MAX_LENS_BYTES / 29 + 1);
        let a = analysis("file:///big.nt", &big);
        assert!(code_lenses(&a, Encoding::Utf16, true).is_empty());
    }

    #[test]
    fn foreign_data_is_left_alone() {
        let lens = CodeLens {
            range: Default::default(),
            command: None,
            data: Some(json!({"other": 1})),
        };
        let k = Knowledge::default();
        assert_eq!(resolve(lens.clone(), &k, Encoding::Utf16), lens);
    }
}
