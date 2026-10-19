//! The operations the toolbox asks for, on a workspace made of its input:
//! every file open under `file:///work/`, so knowledge crosses files as it
//! does in an editor. Each call builds the workspace anew; the input is a
//! handful of files and analysis is fast.

use lsp_types::{
    CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams, Diagnostic,
    DiagnosticSeverity, NumberOrString, Position, TextDocumentIdentifier, TextEdit, Uri,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use turtle_lsp::analysis::Encoding;
use turtle_lsp::features::shacl_validate;
use turtle_lsp::features::{diagnostics, format};
use turtle_lsp::imports;
use turtle_lsp::knowledge::Source;
use turtle_lsp::settings::Settings;
use turtle_lsp::sparql::features as sparql_features;
use turtle_lsp::syntax::{PrefixMap, Severity, Syntax};
use turtle_lsp::workspace::{DocKind, Workspace};

/// JavaScript strings count UTF-16 units.
const ENC: Encoding = Encoding::Utf16;
/// Rounds of "fix all" before giving up on fixes that keep producing fixes.
const MAX_FIX_ROUNDS: usize = 20;

pub fn handle(request: &str) -> String {
    let req: Value = match serde_json::from_str(request) {
        Ok(v) => v,
        Err(e) => return error(&format!("the request is not JSON: {e}")),
    };
    let files = || -> Result<Vec<File>, String> {
        let list = req["files"].as_array().ok_or("the request has no files")?;
        list.iter()
            .map(|f| match (f["name"].as_str(), f["text"].as_str()) {
                (Some(name), Some(text)) => Ok(File {
                    name: name.to_string(),
                    text: text.to_string(),
                }),
                _ => Err("each file needs a name and a text".to_string()),
            })
            .collect()
    };
    let answer = match req["op"].as_str().unwrap_or("") {
        "version" => Ok(json!({ "version": turtle_lsp::VERSION })),
        "analyse" => files().map(|f| analyse(&f)),
        "fix_all" => files().map(fix_all),
        op => Err(format!("unknown operation '{op}'")),
    };
    match answer {
        Ok(v) => json!({ "ok": v }).to_string(),
        Err(e) => error(&e),
    }
}

pub fn error(message: &str) -> String {
    json!({ "error": message }).to_string()
}

#[derive(Clone)]
struct File {
    name: String,
    text: String,
}

/// The input as an editor workspace.
struct Input {
    ws: Workspace,
    /// (name, URI) per file, in input order.
    uris: Vec<(String, String)>,
    settings: Settings,
}

impl Input {
    fn new(files: &[File]) -> Input {
        let mut ws = Workspace::new(Vec::new(), PrefixMap::default());
        let uris: Vec<(String, String)> = files
            .iter()
            .map(|f| (f.name.clone(), format!("file:///work/{}", encode(&f.name))))
            .collect();
        for (f, (_, uri)) in files.iter().zip(&uris) {
            ws.open_doc(uri.clone(), f.text.clone(), 1);
        }
        Input {
            ws,
            uris,
            settings: Settings::default(),
        }
    }

    fn name_of(&self, uri: &str) -> Option<&str> {
        self.uris
            .iter()
            .find(|(_, u)| u == uri)
            .map(|(n, _)| n.as_str())
    }

    /// The imports the input does not declare itself, none of which is
    /// loaded here, each with its note.
    fn unresolved(&mut self) -> HashMap<String, imports::Status> {
        imports::unresolved_without_loading(self.ws.knowledge())
            .into_iter()
            .map(|iri| {
                let note = format!(
                    "Import <{iri}> is not loaded: the toolbox does not follow imports. Add the imported ontology's file to the input to use it."
                );
                (iri, imports::Status::Unresolved(note))
            })
            .collect()
    }

    /// Diagnostics of `uri` as the server computes them once indexed,
    /// without SHACL (see [`Input::shacl`]).
    fn diagnostics(&mut self, uri: &str) -> Vec<Diagnostic> {
        let opts = diagnostics::Options::from_settings(&self.settings);
        if self.ws.is_sparql(uri) {
            let Some(q) = self.ws.sparql(uri) else {
                return Vec::new();
            };
            return sparql_features::diagnostics::diagnostics(&q, self.ws.knowledge(), ENC, &opts);
        }
        let Some(a) = self.ws.analysis(uri) else {
            return Vec::new();
        };
        let mut d = diagnostics::diagnostics_with(&a, self.ws.knowledge(), ENC, &opts);
        let unresolved = self.unresolved();
        d.extend(imports::diagnostics_with(&a, ENC, |iri| {
            unresolved.get(iri)
        }));
        d
    }

    /// Whether some file declares shapes, and if so the SHACL results by URI.
    fn shacl(&mut self) -> (bool, shacl_validate::ByUri) {
        let docs: Vec<_> = self
            .ws
            .knowledge()
            .docs()
            .map(|(_, s, a)| (s, a.clone()))
            .collect();
        let shapes = docs
            .iter()
            .any(|(s, a)| *s != Source::External && shacl_validate::has_shapes(a));
        if !shapes || !self.settings.shacl_enabled {
            return (shapes, Default::default());
        }
        let job = shacl_validate::Job {
            generation: 0,
            docs,
            max_triples: self.settings.shacl_max_triples,
            enc: ENC,
            // Nothing is loaded here, so every import the input does not
            // declare itself is missing, and results it could change soften.
            unresolved: imports::unresolved_without_loading(self.ws.knowledge()),
        };
        (
            true,
            shacl_validate::run(&job, &|| false).unwrap_or_default(),
        )
    }

    /// The quick fixes for `d` in `uri`, preferred first.
    fn fixes(&mut self, uri: &str, d: &Diagnostic) -> Vec<Value> {
        let Ok(doc_uri) = uri.parse::<Uri>() else {
            return Vec::new();
        };
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri: doc_uri },
            range: d.range,
            context: CodeActionContext {
                diagnostics: vec![d.clone()],
                only: Some(vec![CodeActionKind::QUICKFIX]),
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let actions = if self.ws.is_sparql(uri) {
            let Some(q) = self.ws.sparql(uri) else {
                return Vec::new();
            };
            sparql_features::code_actions(&q, self.ws.knowledge(), ENC, &params, false)
        } else {
            let Some(a) = self.ws.analysis(uri) else {
                return Vec::new();
            };
            diagnostics::code_actions(&a, self.ws.knowledge(), &a.text, ENC, &params)
        };
        let mut out: Vec<(bool, Value)> = Vec::new();
        for action in actions {
            let CodeActionOrCommand::CodeAction(c) = action else {
                continue;
            };
            // A fix for this diagnostic, not "add all missing prefixes".
            let mine = c
                .diagnostics
                .as_ref()
                .is_some_and(|ds| ds.len() == 1 && ds[0] == *d);
            let Some(changes) = c.edit.and_then(|e| e.changes) else {
                continue;
            };
            if !mine || c.kind != Some(CodeActionKind::QUICKFIX) {
                continue;
            }
            let mut edits = serde_json::Map::new();
            for (u, list) in changes {
                let Some(name) = self.name_of(u.as_str()) else {
                    continue;
                };
                edits.insert(name.to_string(), json!(list));
            }
            let preferred = c.is_preferred == Some(true);
            out.push((
                preferred,
                json!({ "title": c.title, "preferred": preferred, "edits": edits }),
            ));
        }
        out.sort_by_key(|(p, _)| !p);
        out.into_iter().map(|(_, v)| v).collect()
    }

    /// The formatted text of `uri`, or why there is none.
    fn format(&mut self, uri: &str) -> Value {
        let opts = format::Options::default();
        if self.ws.is_sparql(uri) {
            let Some(q) = self.ws.sparql(uri) else {
                return Value::Null;
            };
            let text = q.text();
            let guarantee = "same SPARQL algebra";
            // The formatter refuses a query with errors; say how many.
            if q.doc.has_errors() {
                let errors = self
                    .diagnostics(uri)
                    .iter()
                    .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
                    .count()
                    .max(1);
                let (s, it) = if errors == 1 {
                    ("", "it")
                } else {
                    ("s", "them")
                };
                let reason = format!("the query has {errors} error{s}; fix {it} first");
                return json!({ "status": "refused", "guarantee": guarantee, "reason": reason });
            }
            return match turtle_lsp::sparql::format(text, &q.external, Some(q.base()), &opts) {
                Ok(t) if t == text => json!({ "status": "unchanged", "guarantee": guarantee }),
                Ok(t) => json!({ "status": "changed", "guarantee": guarantee, "text": t }),
                Err(reason) => {
                    json!({ "status": "refused", "guarantee": guarantee, "reason": reason })
                }
            };
        }
        let Some(a) = self.ws.analysis(uri) else {
            return Value::Null;
        };
        let guarantee = "same graph";
        let errors = a
            .parsed
            .errors
            .iter()
            .filter(|e| e.severity == Severity::Error)
            .count();
        if errors > 0 {
            let (s, it) = if errors == 1 {
                ("", "it")
            } else {
                ("s", "them")
            };
            let reason = format!("the file has {errors} syntax error{s}; fix {it} first");
            return json!({ "status": "refused", "guarantee": guarantee, "reason": reason });
        }
        match format::format(&a.text, &a.parsed, &a.external, &opts) {
            Some(t) if t == *a.text => json!({ "status": "unchanged", "guarantee": guarantee }),
            Some(t) => json!({ "status": "changed", "guarantee": guarantee, "text": t }),
            None => json!({
                "status": "refused",
                "guarantee": guarantee,
                "reason": "the formatted text would not have the same graph, so it is not formatted",
            }),
        }
    }

    fn kind(&self, uri: &str) -> &'static str {
        match self.ws.open.get(uri).map(|d| d.kind) {
            Some(DocKind::Sparql) => "sparql",
            Some(DocKind::Rdf(Syntax::TriG)) => "trig",
            Some(DocKind::Rdf(Syntax::NTriples)) => "ntriples",
            Some(DocKind::Rdf(Syntax::NQuads)) => "nquads",
            _ => "turtle",
        }
    }
}

fn analyse(files: &[File]) -> Value {
    let mut input = Input::new(files);
    let (has_shapes, mut shacl) = input.shacl();
    let uris = input.uris.clone();
    let mut out = Vec::new();
    for (name, uri) in &uris {
        let mut diags = input.diagnostics(uri);
        diags.extend(shacl.remove(uri).unwrap_or_default());
        let diagnostics: Vec<Value> = diags
            .iter()
            .map(|d| {
                let code = match &d.code {
                    Some(NumberOrString::String(s)) => json!(s),
                    Some(NumberOrString::Number(n)) => json!(n.to_string()),
                    None => Value::Null,
                };
                json!({
                    "range": d.range,
                    "severity": d.severity,
                    "code": code,
                    "message": d.message,
                    "fixes": input.fixes(uri, d),
                })
            })
            .collect();
        out.push(json!({
            "name": name,
            "kind": input.kind(uri),
            "diagnostics": diagnostics,
            "format": input.format(uri),
        }));
    }
    json!({ "files": out, "hasShapes": has_shapes })
}

/// Apply every preferred fix, round after round, until none is left. A fix
/// that touches text another fix of the round touches (or inserts at the
/// same place) waits for the next round, analysed afresh.
fn fix_all(mut files: Vec<File>) -> Value {
    let mut applied = 0;
    for _ in 0..MAX_FIX_ROUNDS {
        let mut input = Input::new(&files);
        let uris = input.uris.clone();
        // (file index, start, end, new text) of the edits taken this round.
        let mut taken: Vec<(usize, usize, usize, String)> = Vec::new();
        for (_, uri) in &uris {
            for d in input.diagnostics(uri) {
                let Some(fix) = input
                    .fixes(uri, &d)
                    .into_iter()
                    .find(|f| f["preferred"] == true)
                else {
                    continue;
                };
                let Some(edits) = edits_of(&fix, &files) else {
                    continue;
                };
                let clash = edits.iter().any(|(i, s, e, _)| {
                    taken
                        .iter()
                        .any(|(j, ts, te, _)| i == j && ((s < te && ts < e) || s == ts))
                });
                if clash {
                    continue;
                }
                taken.extend(edits);
                applied += 1;
            }
        }
        if taken.is_empty() {
            break;
        }
        taken.sort_by_key(|t| std::cmp::Reverse((t.0, t.1)));
        for (i, s, e, new) in taken {
            files[i].text.replace_range(s..e, &new);
        }
    }
    let files: Vec<Value> = files
        .iter()
        .map(|f| json!({ "name": f.name, "text": f.text }))
        .collect();
    json!({ "files": files, "applied": applied })
}

/// A fix's edits as (file index, start byte, end byte, new text).
fn edits_of(fix: &Value, files: &[File]) -> Option<Vec<(usize, usize, usize, String)>> {
    let mut out = Vec::new();
    for (name, list) in fix["edits"].as_object()? {
        let i = files.iter().position(|f| f.name == *name)?;
        for e in serde_json::from_value::<Vec<TextEdit>>(list.clone()).ok()? {
            let s = byte_offset(&files[i].text, e.range.start)?;
            let end = byte_offset(&files[i].text, e.range.end)?;
            out.push((i, s, end.max(s), e.new_text));
        }
    }
    Some(out)
}

/// The byte offset of a UTF-16 position in `text`.
fn byte_offset(text: &str, pos: Position) -> Option<usize> {
    let mut start = 0;
    for _ in 0..pos.line {
        start += text[start..].find('\n')? + 1;
    }
    let line = &text[start..];
    let line = &line[..line.find('\n').unwrap_or(line.len())];
    let mut units = 0;
    for (i, c) in line.char_indices() {
        if units >= pos.character as usize {
            return Some(start + i);
        }
        units += c.len_utf16();
    }
    Some(start + line.len())
}

/// `name` as a URI path segment.
fn encode(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
