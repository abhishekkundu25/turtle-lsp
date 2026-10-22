//! Reports of `turtle-lsp check`, in each output format.
//!
//! Problems arrive with 0-based lines and code-point columns (LSP ranges,
//! end exclusive). Every format prints lines and columns from 1.

use crate::batch::{Fix, Problem};
use lsp_types::{DiagnosticSeverity, NumberOrString, Position, Range, TextEdit};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Concise,
    Json,
    Github,
    Sarif,
}

/// One checked file: its path as shown (relative to the current directory
/// when under it, `/` separators), where SARIF finds it, and its problems
/// with code-point columns, sorted by position then code.
pub(crate) struct CheckedFile<'a> {
    pub(crate) path: &'a str,
    /// Relative to the workspace root (`/` separators); None outside it.
    pub(crate) root_relative: Option<&'a str>,
    /// The file's `file:` URI, for a file outside the root.
    pub(crate) uri: &'a str,
    pub(crate) problems: &'a [Problem],
}

/// Counts over every problem found, before `--max-diagnostics` trimmed the
/// output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
    pub(crate) infos: usize,
    pub(crate) hints: usize,
    pub(crate) files_with_problems: usize,
    pub(crate) files_checked: usize,
}

/// The SARIF 2.1.0 schema the `sarif` format declares.
pub(crate) const SARIF_SCHEMA: &str =
    "https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/schemas/sarif-schema-2.1.0.json";

/// The repository, as the SARIF tool's `informationUri`.
pub(crate) const INFORMATION_URI: &str = "https://github.com/abhishekkundu25/turtle-lsp";

/// Writes the report for `files` in `format` to `out`.
///
/// - `concise`: one line per problem, `path:line:col: severity[code] message`
///   (`severity: message` without a code), newlines in the message replaced
///   by spaces. The summary is not included; see [`summary_line`].
/// - `github`: one workflow command per problem,
///   `::error file=…,line=…,col=…,endLine=…,endColumn=…,title=turtle-lsp CODE::message`.
///   Warnings are `::warning`, infos and hints `::notice`.
/// - `json`: one line holding a version 1 object:
///
///   ```text
///   {
///     "version": 1,
///     "files": [{
///       "path": "data/a.ttl",
///       "diagnostics": [{
///         "range": {"start": {"line": 1, "column": 1}, "end": {"line": 1, "column": 4}},
///         "severity": "error" | "warning" | "info" | "hint",
///         "code": "undeclared-prefix" | null,
///         "message": "...",
///         "fixes": [{
///           "title": "...",
///           "safe": true,
///           "edits": [{"path": "data/a.ttl", "range": {...}, "newText": "..."}]
///         }]
///       }]
///     }],
///     "summary": {"errors": 0, "warnings": 0, "infos": 0, "hints": 0,
///                 "filesWithProblems": 0, "filesChecked": 0}
///   }
///   ```
///
///   Lines and columns start at 1 and count code points; a range's end is
///   exclusive. `files` lists only files with problems to show; `summary`
///   counts every problem. An edit's `path` is the file name the analysis
///   gave it. Within version 1, keys are only added, never renamed or removed.
/// - `sarif`: a SARIF 2.1.0 log with one run, a rule per code, the safe
///   fixes, and artifact URIs: percent-encoded paths relative to `%SRCROOT%`
///   (the workspace root) for files under it, `file:` URIs without a base
///   for the others.
pub(crate) fn render(
    format: Format,
    files: &[CheckedFile],
    summary: &Summary,
    out: &mut dyn Write,
) -> io::Result<()> {
    match format {
        Format::Concise => concise(files, out),
        Format::Github => github(files, out),
        Format::Json => {
            serde_json::to_writer(&mut *out, &json_report(files, summary))?;
            writeln!(out)
        }
        Format::Sarif => {
            serde_json::to_writer_pretty(&mut *out, &sarif_report(files))?;
            writeln!(out)
        }
    }
}

/// `--statistics`: problem counts per code, most frequent first.
///
/// ```text
///   12  unknown-term
///    3  syntax
///   15  total
/// ```
pub(crate) fn statistics(files: &[CheckedFile], out: &mut dyn Write) -> io::Result<()> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for p in files.iter().flat_map(|f| f.problems) {
        *counts.entry(code(p).unwrap_or("(no code)")).or_default() += 1;
    }
    let mut rows: Vec<(&str, usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let total: usize = rows.iter().map(|r| r.1).sum();
    let width = total.to_string().len();
    for (code, n) in rows {
        writeln!(out, "  {n:>width$}  {code}")?;
    }
    writeln!(out, "  {total:>width$}  total")
}

/// The one-line summary for stderr, e.g. "3 errors, 1 warning in 2 files".
/// Infos and hints appear only when there are some.
pub(crate) fn summary_line(summary: &Summary) -> String {
    if summary.errors + summary.warnings + summary.infos + summary.hints == 0 {
        return format!("No problems in {}", plural(summary.files_checked, "file"));
    }
    let mut parts = vec![
        plural(summary.errors, "error"),
        plural(summary.warnings, "warning"),
    ];
    for (n, word) in [(summary.infos, "info"), (summary.hints, "hint")] {
        if n > 0 {
            parts.push(plural(n, word));
        }
    }
    format!(
        "{} in {}",
        parts.join(", "),
        plural(summary.files_with_problems, "file")
    )
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn code(p: &Problem) -> Option<&str> {
    match &p.diagnostic.code {
        Some(NumberOrString::String(s)) => Some(s),
        // turtle-lsp produces no numeric codes.
        Some(NumberOrString::Number(_)) | None => None,
    }
}

/// error, warning, info or hint. A diagnostic without a severity is an
/// error, as LSP clients treat it.
fn severity(p: &Problem) -> &'static str {
    match p.diagnostic.severity {
        Some(DiagnosticSeverity::WARNING) => "warning",
        Some(DiagnosticSeverity::INFORMATION) => "info",
        Some(DiagnosticSeverity::HINT) => "hint",
        _ => "error",
    }
}

/// A 0-based position as 1-based (line, column).
fn one_based(p: Position) -> (u32, u32) {
    (p.line + 1, p.character + 1)
}

fn one_line(message: &str) -> String {
    message.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

// --- concise ---------------------------------------------------------------

fn concise(files: &[CheckedFile], out: &mut dyn Write) -> io::Result<()> {
    for f in files {
        for p in f.problems {
            let (line, col) = one_based(p.diagnostic.range.start);
            let sev = severity(p);
            let message = one_line(&p.diagnostic.message);
            match code(p) {
                Some(c) => writeln!(out, "{}:{line}:{col}: {sev}[{c}] {message}", f.path)?,
                None => writeln!(out, "{}:{line}:{col}: {sev}: {message}", f.path)?,
            }
        }
    }
    Ok(())
}

// --- github ----------------------------------------------------------------

/// Escapes a workflow command's message, as @actions/core does.
fn escape_data(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escapes a workflow command's property value, as @actions/core does.
fn escape_property(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

fn github(files: &[CheckedFile], out: &mut dyn Write) -> io::Result<()> {
    for f in files {
        for p in f.problems {
            let command = match severity(p) {
                "error" => "error",
                "warning" => "warning",
                _ => "notice",
            };
            let Range { start, end } = p.diagnostic.range;
            let (line, col) = one_based(start);
            let (end_line, end_col) = one_based(end);
            let title = match code(p) {
                Some(c) => format!("turtle-lsp {c}"),
                None => "turtle-lsp".to_string(),
            };
            writeln!(
                out,
                "::{command} file={},line={line},col={col},endLine={end_line},endColumn={end_col},title={}::{}",
                escape_property(f.path),
                escape_property(&title),
                escape_data(&p.diagnostic.message),
            )?;
        }
    }
    Ok(())
}

// --- json ------------------------------------------------------------------

fn json_position(p: Position) -> Value {
    let (line, column) = one_based(p);
    json!({ "line": line, "column": column })
}

fn json_range(r: Range) -> Value {
    json!({ "start": json_position(r.start), "end": json_position(r.end) })
}

fn json_fix(fix: &Fix) -> Value {
    let edits: Vec<Value> = fix
        .edits
        .iter()
        .flat_map(|(path, edits)| {
            edits.iter().map(move |e| {
                json!({ "path": path, "range": json_range(e.range), "newText": e.new_text })
            })
        })
        .collect();
    json!({ "title": fix.title, "safe": fix.safe, "edits": edits })
}

fn json_report(files: &[CheckedFile], summary: &Summary) -> Value {
    let files: Vec<Value> = files
        .iter()
        .filter(|f| !f.problems.is_empty())
        .map(|f| {
            let diagnostics: Vec<Value> = f
                .problems
                .iter()
                .map(|p| {
                    json!({
                        "range": json_range(p.diagnostic.range),
                        "severity": severity(p),
                        "code": code(p),
                        "message": p.diagnostic.message,
                        "fixes": p.fixes.iter().map(json_fix).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({ "path": f.path, "diagnostics": diagnostics })
        })
        .collect();
    json!({
        "version": 1,
        "files": files,
        "summary": {
            "errors": summary.errors,
            "warnings": summary.warnings,
            "infos": summary.infos,
            "hints": summary.hints,
            "filesWithProblems": summary.files_with_problems,
            "filesChecked": summary.files_checked,
        },
    })
}

// --- sarif -----------------------------------------------------------------

/// The rule id of a problem without a code.
const NO_CODE_RULE: &str = "diagnostic";

/// The codes turtle-lsp reports, each with a one-line description: what
/// `--select` and `--ignore` accept, and the SARIF rules.
pub(crate) const CODES: &[(&str, &str)] = &[
    ("syntax", "Syntax error"),
    ("undeclared-prefix", "A prefix is used but not declared"),
    ("unused-prefix", "A declared prefix is never used"),
    ("duplicate-prefix", "A prefix is declared more than once"),
    (
        "redefined-prefix",
        "A prefix is declared again with another namespace",
    ),
    (
        "unknown-term",
        "A term is not defined in its well-known vocabulary",
    ),
    ("deprecated-term", "A term is deprecated in its vocabulary"),
    (
        "undescribed-term",
        "A term is not described anywhere in the workspace",
    ),
    ("invalid-literal", "A literal is not valid for its datatype"),
    (
        "unresolved-import",
        "An owl:imports target could not be loaded",
    ),
    (
        "unbound-projection",
        "A projected SPARQL variable is never bound",
    ),
    ("json-syntax", "The JSON is malformed"),
    ("json-lone-surrogate", "A JSON string has a lone surrogate"),
    ("json-duplicate-key", "A JSON object repeats a key"),
    (
        "jsonld-context",
        "A JSON-LD context is invalid or could not be loaded",
    ),
    (
        "jsonld-undefined-term",
        "A JSON-LD key is not defined in the context",
    ),
    (
        "jsonld-undefined-prefix",
        "A JSON-LD value uses a prefix the context does not define",
    ),
    ("invalid-iri", "A JSON-LD IRI is not valid"),
    (
        "invalid-language-tag",
        "A JSON-LD language tag is not valid",
    ),
    (
        "shacl-violation",
        "The data does not conform to a SHACL shape",
    ),
    ("shacl-skipped", "SHACL validation was skipped"),
    (
        "shacl-sparql-timeout",
        "A SHACL-SPARQL query ran out of time",
    ),
    (
        "shacl-failure",
        "A shape or query could not be validated to the end",
    ),
    ("shacl-invalid-path", "A shape was skipped for its sh:path"),
    (
        "shacl-missing-path",
        "A SHACL property shape's sh:path is not described",
    ),
    (
        "shacl-unknown-class",
        "A SHACL shape names a class that is not described",
    ),
    (
        "shacl-bad-datatype",
        "A SHACL shape names an unknown datatype",
    ),
    (
        "shacl-dangling-shape",
        "A SHACL shape is referenced but not defined",
    ),
    (
        "shacl-misplaced",
        "A SHACL term is used where it does not belong",
    ),
    ("shacl-count-conflict", "SHACL cardinalities that conflict"),
    (
        "shacl-sparql-syntax",
        "A SHACL-SPARQL query has a syntax error",
    ),
    (
        "shacl-sparql-prefix",
        "A SHACL-SPARQL query uses a prefix it does not declare",
    ),
    (
        "shacl-sparql-unsupported",
        "A SHACL-SPARQL feature is not supported",
    ),
    ("internal-error", "The analysis of the file failed"),
    (IO_CODE, "The file could not be read"),
    (ENCODING_CODE, "The file is not valid UTF-8"),
];

/// A file that could not be read (`check` reports it; exit 2).
pub(crate) const IO_CODE: &str = "io";
/// A file that is not UTF-8 (`check` reports it; exit 2).
pub(crate) const ENCODING_CODE: &str = "encoding";

/// Whether `code` is one turtle-lsp reports.
pub(crate) fn is_known_code(code: &str) -> bool {
    CODES.iter().any(|(c, _)| *c == code)
}

/// The known codes, comma separated, for a message.
pub(crate) fn known_codes() -> String {
    CODES.iter().map(|(c, _)| *c).collect::<Vec<_>>().join(", ")
}

/// The description of `code`; other codes are described by their name.
fn rule_description(code: &str) -> String {
    CODES
        .iter()
        .find(|(c, _)| *c == code)
        .map_or_else(|| format!("turtle-lsp {code}"), |(_, d)| (*d).to_string())
}

fn sarif_level(p: &Problem) -> &'static str {
    match severity(p) {
        "error" => "error",
        "warning" => "warning",
        _ => "note",
    }
}

fn sarif_region(r: Range) -> Value {
    let (start_line, start_column) = one_based(r.start);
    let (end_line, end_column) = one_based(r.end);
    json!({
        "startLine": start_line,
        "startColumn": start_column,
        "endLine": end_line,
        "endColumn": end_column,
    })
}

/// `path` (`/` separators) as a relative URI reference: every character
/// outside the unreserved set, `/` kept, percent-encoded as UTF-8.
fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Where SARIF finds a file: a path relative to `%SRCROOT%` under the root,
/// else its absolute `file:` URI.
fn sarif_location(f: &CheckedFile) -> Value {
    match f.root_relative {
        Some(rel) => json!({ "uri": percent_encode_path(rel), "uriBaseId": "%SRCROOT%" }),
        None => json!({ "uri": f.uri }),
    }
}

/// A safe fix as a SARIF fix. Edit file names that are the shown path of a
/// checked file become its location; others are taken as paths below the root.
fn sarif_fix(fix: &Fix, locations: &HashMap<&str, Value>) -> Value {
    let changes: Vec<Value> = fix
        .edits
        .iter()
        .map(|(name, edits)| {
            let location = locations.get(name.as_str()).cloned().unwrap_or_else(
                || json!({ "uri": percent_encode_path(name), "uriBaseId": "%SRCROOT%" }),
            );
            let replacements: Vec<Value> = edits
                .iter()
                .map(|e: &TextEdit| {
                    json!({
                        "deletedRegion": sarif_region(e.range),
                        "insertedContent": { "text": e.new_text },
                    })
                })
                .collect();
            json!({ "artifactLocation": location, "replacements": replacements })
        })
        .collect();
    json!({ "description": { "text": fix.title }, "artifactChanges": changes })
}

fn sarif_report(files: &[CheckedFile]) -> Value {
    let problems = || {
        files
            .iter()
            .flat_map(|f| f.problems.iter().map(move |p| (f, p)))
    };
    let rule_id = |p: &Problem| code(p).unwrap_or(NO_CODE_RULE).to_string();
    // Rules sorted by id; a result points at its rule by index.
    let mut rule_index: BTreeMap<String, usize> =
        problems().map(|(_, p)| (rule_id(p), 0)).collect();
    for (i, v) in rule_index.values_mut().enumerate() {
        *v = i;
    }
    let rules: Vec<Value> = rule_index
        .keys()
        .map(|id| json!({ "id": id, "shortDescription": { "text": rule_description(id) } }))
        .collect();
    let locations: HashMap<&str, Value> =
        files.iter().map(|f| (f.path, sarif_location(f))).collect();
    let results: Vec<Value> = problems()
        .map(|(f, p)| {
            let id = rule_id(p);
            let mut result = Map::new();
            result.insert("ruleIndex".into(), json!(rule_index[&id]));
            result.insert("ruleId".into(), json!(id));
            result.insert("level".into(), json!(sarif_level(p)));
            result.insert("message".into(), json!({ "text": p.diagnostic.message }));
            result.insert(
                "locations".into(),
                json!([{
                    "physicalLocation": {
                        "artifactLocation": sarif_location(f),
                        "region": sarif_region(p.diagnostic.range),
                    }
                }]),
            );
            let fixes: Vec<Value> = p
                .fixes
                .iter()
                .filter(|x| x.safe)
                .map(|x| sarif_fix(x, &locations))
                .collect();
            if !fixes.is_empty() {
                result.insert("fixes".into(), Value::Array(fixes));
            }
            Value::Object(result)
        })
        .collect();
    json!({
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "turtle-lsp",
                    "version": crate::VERSION,
                    "informationUri": INFORMATION_URI,
                    "rules": rules,
                }
            },
            "columnKind": "unicodeCodePoints",
            "results": results,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Diagnostic;

    fn range(l0: u32, c0: u32, l1: u32, c1: u32) -> Range {
        Range::new(Position::new(l0, c0), Position::new(l1, c1))
    }

    fn problem(r: Range, sev: DiagnosticSeverity, code: Option<&str>, message: &str) -> Problem {
        Problem {
            diagnostic: Diagnostic {
                range: r,
                severity: Some(sev),
                code: code.map(|c| NumberOrString::String(c.into())),
                message: message.into(),
                ..Default::default()
            },
            fixes: Vec::new(),
        }
    }

    fn fix(title: &str, safe: bool, file: &str, r: Range, text: &str) -> Fix {
        Fix {
            title: title.into(),
            preferred: safe,
            safe,
            edits: vec![(file.into(), vec![TextEdit::new(r, text.into())])],
        }
    }

    fn render_to_string(format: Format, files: &[CheckedFile], summary: &Summary) -> String {
        let mut out = Vec::new();
        render(format, files, summary, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Two files: one with a warning (with a safe and an unsafe fix), one
    /// with an error, an info and a codeless hint.
    fn sample() -> (Vec<Problem>, Vec<Problem>) {
        let mut w = problem(
            range(2, 4, 2, 12),
            DiagnosticSeverity::WARNING,
            Some("undescribed-term"),
            "'ex:lable' is not described anywhere in the workspace. Did you mean 'ex:label'?",
        );
        w.fixes = vec![
            fix(
                "Add prefix ex:",
                true,
                "data/people.ttl",
                range(0, 0, 0, 0),
                "@prefix ex: <http://ex.com/> .\n",
            ),
            fix(
                "Change to 'ex:label'",
                false,
                "data/people.ttl",
                range(2, 4, 2, 12),
                "ex:label",
            ),
        ];
        let e = problem(
            range(6, 21, 6, 34),
            DiagnosticSeverity::ERROR,
            Some("shacl-violation"),
            "obj:mill-0517 (sd:fillsPosition of obj:pump-07) is not an instance of sd:Position",
        );
        let i = problem(
            range(0, 0, 0, 1),
            DiagnosticSeverity::INFORMATION,
            Some("shacl-skipped"),
            "SHACL skipped",
        );
        let h = problem(range(1, 0, 1, 1), DiagnosticSeverity::HINT, None, "a hint");
        (vec![w], vec![e, i, h])
    }

    fn checked<'a>(people: &'a [Problem], pumps: &'a [Problem]) -> Vec<CheckedFile<'a>> {
        vec![
            CheckedFile {
                path: "data/people.ttl",
                root_relative: Some("repo/data/people.ttl"),
                uri: "file:///work/repo/data/people.ttl",
                problems: people,
            },
            CheckedFile {
                path: "data/pumps.ttl",
                root_relative: Some("repo/data/pumps.ttl"),
                uri: "file:///work/repo/data/pumps.ttl",
                problems: pumps,
            },
        ]
    }

    fn sample_summary() -> Summary {
        Summary {
            errors: 1,
            warnings: 1,
            infos: 1,
            hints: 1,
            files_with_problems: 2,
            files_checked: 3,
        }
    }

    #[test]
    fn concise_matches_the_readme() {
        let (a, b) = sample();
        let text = render_to_string(Format::Concise, &checked(&a, &b), &sample_summary());
        assert_eq!(
            text,
            "data/people.ttl:3:5: warning[undescribed-term] 'ex:lable' is not described anywhere in the workspace. Did you mean 'ex:label'?\n\
             data/pumps.ttl:7:22: error[shacl-violation] obj:mill-0517 (sd:fillsPosition of obj:pump-07) is not an instance of sd:Position\n\
             data/pumps.ttl:1:1: info[shacl-skipped] SHACL skipped\n\
             data/pumps.ttl:2:1: hint: a hint\n"
        );
    }

    #[test]
    fn concise_keeps_a_problem_on_one_line() {
        let p = [problem(
            range(0, 3, 0, 4),
            DiagnosticSeverity::ERROR,
            Some("syntax"),
            "expected '.'\nfound 'é'\r\nhere\rend",
        )];
        let files = [CheckedFile {
            path: "é/ü.ttl",
            root_relative: Some("é/ü.ttl"),
            uri: "file:///work/é/ü.ttl",
            problems: &p,
        }];
        let text = render_to_string(Format::Concise, &files, &Summary::default());
        assert_eq!(
            text,
            "é/ü.ttl:1:4: error[syntax] expected '.' found 'é' here end\n"
        );
    }

    #[test]
    fn empty_input_prints_nothing_or_an_empty_report() {
        let s = Summary::default();
        assert_eq!(render_to_string(Format::Concise, &[], &s), "");
        assert_eq!(render_to_string(Format::Github, &[], &s), "");
        let json: Value = serde_json::from_str(&render_to_string(Format::Json, &[], &s)).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["files"], json!([]));
        assert_eq!(json["summary"]["filesChecked"], 0);
        let sarif: Value = serde_json::from_str(&render_to_string(Format::Sarif, &[], &s)).unwrap();
        assert_eq!(sarif["runs"][0]["results"], json!([]));
        assert_eq!(sarif["runs"][0]["tool"]["driver"]["rules"], json!([]));
        let mut out = Vec::new();
        statistics(&[], &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "  0  total\n");
    }

    #[test]
    fn github_annotations() {
        let (a, b) = sample();
        let text = render_to_string(Format::Github, &checked(&a, &b), &sample_summary());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "::warning file=data/people.ttl,line=3,col=5,endLine=3,endColumn=13,title=turtle-lsp undescribed-term::'ex:lable' is not described anywhere in the workspace. Did you mean 'ex:label'?"
        );
        assert!(lines[1].starts_with(
            "::error file=data/pumps.ttl,line=7,col=22,endLine=7,endColumn=35,title=turtle-lsp shacl-violation::obj:mill-0517"
        ));
        assert!(lines[2].starts_with("::notice file=data/pumps.ttl,line=1,col=1,"));
        assert_eq!(
            lines[3],
            "::notice file=data/pumps.ttl,line=2,col=1,endLine=2,endColumn=2,title=turtle-lsp::a hint"
        );
    }

    #[test]
    fn github_escapes_properties_and_data() {
        let p = [problem(
            range(0, 0, 0, 1),
            DiagnosticSeverity::ERROR,
            Some("a:b,c"),
            "100% wrong: a,b\r\nnext ü",
        )];
        let files = [CheckedFile {
            path: "dir,1/a:b%.ttl",
            root_relative: Some("x"),
            uri: "file:///work/x",
            problems: &p,
        }];
        let text = render_to_string(Format::Github, &files, &Summary::default());
        assert_eq!(
            text,
            "::error file=dir%2C1/a%3Ab%25.ttl,line=1,col=1,endLine=1,endColumn=2,title=turtle-lsp a%3Ab%2Cc::100%25 wrong: a,b%0D%0Anext ü\n"
        );
    }

    #[test]
    fn json_report_shape() {
        let (a, b) = sample();
        let text = render_to_string(Format::Json, &checked(&a, &b), &sample_summary());
        assert_eq!(text.lines().count(), 1);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], 1);
        let d = &v["files"][0]["diagnostics"][0];
        assert_eq!(v["files"][0]["path"], "data/people.ttl");
        assert_eq!(
            d["range"],
            json!({"start": {"line": 3, "column": 5}, "end": {"line": 3, "column": 13}})
        );
        assert_eq!(d["severity"], "warning");
        assert_eq!(d["code"], "undescribed-term");
        assert_eq!(
            d["fixes"][0],
            json!({
                "title": "Add prefix ex:",
                "safe": true,
                "edits": [{
                    "path": "data/people.ttl",
                    "range": {"start": {"line": 1, "column": 1}, "end": {"line": 1, "column": 1}},
                    "newText": "@prefix ex: <http://ex.com/> .\n",
                }],
            })
        );
        assert_eq!(d["fixes"][1]["safe"], false);
        let hint = &v["files"][1]["diagnostics"][2];
        assert_eq!(hint["code"], Value::Null);
        assert_eq!(hint["severity"], "hint");
        assert_eq!(v["files"][1]["diagnostics"][1]["severity"], "info");
        assert_eq!(
            v["summary"],
            json!({"errors": 1, "warnings": 1, "infos": 1, "hints": 1,
                   "filesWithProblems": 2, "filesChecked": 3})
        );
    }

    #[test]
    fn json_keeps_non_ascii_and_newlines() {
        let p = [problem(
            range(0, 2, 0, 3),
            DiagnosticSeverity::ERROR,
            Some("syntax"),
            "bad 'ü'\n\"quoted\"",
        )];
        let files = [
            CheckedFile {
                path: "ä.ttl",
                root_relative: Some("ä.ttl"),
                uri: "file:///work/ä.ttl",
                problems: &p,
            },
            CheckedFile {
                path: "clean.ttl",
                root_relative: Some("clean.ttl"),
                uri: "file:///work/clean.ttl",
                problems: &[],
            },
        ];
        let v: Value =
            serde_json::from_str(&render_to_string(Format::Json, &files, &Summary::default()))
                .unwrap();
        assert_eq!(v["files"].as_array().unwrap().len(), 1);
        assert_eq!(v["files"][0]["path"], "ä.ttl");
        assert_eq!(
            v["files"][0]["diagnostics"][0]["message"],
            "bad 'ü'\n\"quoted\""
        );
        assert_eq!(
            v["files"][0]["diagnostics"][0]["range"]["start"]["column"],
            3
        );
    }

    /// The properties SARIF 2.1.0 requires, and those GitHub code scanning
    /// reads, are present with the right types.
    #[test]
    fn sarif_structure() {
        let (a, b) = sample();
        let text = render_to_string(Format::Sarif, &checked(&a, &b), &sample_summary());
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], "2.1.0");
        assert_eq!(v["$schema"], SARIF_SCHEMA);
        let runs = v["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(run["columnKind"], "unicodeCodePoints");
        let driver = &run["tool"]["driver"];
        assert_eq!(driver["name"], "turtle-lsp");
        assert_eq!(driver["version"], crate::VERSION);
        assert_eq!(driver["informationUri"], INFORMATION_URI);
        let rules = driver["rules"].as_array().unwrap();
        let ids: Vec<&str> = rules.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            [
                "diagnostic",
                "shacl-skipped",
                "shacl-violation",
                "undescribed-term"
            ]
        );
        for r in rules {
            assert!(r["shortDescription"]["text"].is_string());
        }
        let results = run["results"].as_array().unwrap();
        assert_eq!(results.len(), 4);
        for r in results {
            assert!(r["message"]["text"].is_string());
            let idx = r["ruleIndex"].as_u64().unwrap() as usize;
            assert_eq!(rules[idx]["id"], r["ruleId"]);
            let loc = &r["locations"][0]["physicalLocation"];
            assert_eq!(loc["artifactLocation"]["uriBaseId"], "%SRCROOT%");
            for k in ["startLine", "startColumn", "endLine", "endColumn"] {
                assert!(loc["region"][k].as_u64().unwrap() >= 1);
            }
        }
        let levels: Vec<&str> = results
            .iter()
            .map(|r| r["level"].as_str().unwrap())
            .collect();
        assert_eq!(levels, ["warning", "error", "note", "note"]);
        let w = &results[0];
        assert_eq!(
            w["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "repo/data/people.ttl"
        );
        assert_eq!(
            w["locations"][0]["physicalLocation"]["region"],
            json!({"startLine": 3, "startColumn": 5, "endLine": 3, "endColumn": 13})
        );
        // Only the safe fix, with the file named by its root-relative path.
        let fixes = w["fixes"].as_array().unwrap();
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0]["description"]["text"], "Add prefix ex:");
        let change = &fixes[0]["artifactChanges"][0];
        assert_eq!(change["artifactLocation"]["uri"], "repo/data/people.ttl");
        assert_eq!(
            change["replacements"][0],
            json!({
                "deletedRegion": {"startLine": 1, "startColumn": 1, "endLine": 1, "endColumn": 1},
                "insertedContent": {"text": "@prefix ex: <http://ex.com/> .\n"},
            })
        );
        assert!(results[1].get("fixes").is_none());
    }

    #[test]
    fn sarif_uris_are_percent_encoded_and_outside_files_are_file_uris() {
        let (a, b) = sample();
        let files = vec![
            CheckedFile {
                path: "data/people.ttl",
                root_relative: Some("my data/a#b %c.ttl"),
                uri: "file:///work/my%20data/a%23b%20%25c.ttl",
                problems: &a,
            },
            CheckedFile {
                path: "/elsewhere/\u{e9}.ttl",
                root_relative: None,
                uri: "file:///elsewhere/%C3%A9.ttl",
                problems: &b,
            },
        ];
        let text = render_to_string(Format::Sarif, &files, &sample_summary());
        let v: Value = serde_json::from_str(&text).unwrap();
        let results = v["runs"][0]["results"].as_array().unwrap();
        let loc =
            |i: usize| results[i]["locations"][0]["physicalLocation"]["artifactLocation"].clone();
        assert_eq!(
            loc(0),
            json!({"uri": "my%20data/a%23b%20%25c.ttl", "uriBaseId": "%SRCROOT%"})
        );
        // Inside the root, a fix names the file as the result does.
        let fix = &results[0]["fixes"][0]["artifactChanges"][0]["artifactLocation"];
        assert_eq!(fix, &loc(0));
        // Outside: an absolute file: URI, and no base.
        for i in 1..results.len() {
            assert_eq!(loc(i), json!({"uri": "file:///elsewhere/%C3%A9.ttl"}));
        }
    }

    #[test]
    fn percent_encoding_keeps_unreserved_characters_and_slashes() {
        assert_eq!(percent_encode_path("a/b-c_d.e~f/G9"), "a/b-c_d.e~f/G9");
        assert_eq!(percent_encode_path("a b/c#d?e%f"), "a%20b/c%23d%3Fe%25f");
        assert_eq!(percent_encode_path("\u{e9}"), "%C3%A9");
    }

    #[test]
    fn every_known_code_has_a_description_and_the_table_has_no_repeats() {
        let mut seen = std::collections::HashSet::new();
        for (c, d) in CODES {
            assert!(seen.insert(*c), "{c} is listed twice");
            assert!(!d.is_empty());
            assert_eq!(&rule_description(c), d);
        }
        assert!(is_known_code("syntax") && is_known_code("shacl-violation"));
        assert!(is_known_code(IO_CODE) && is_known_code(ENCODING_CODE));
        assert!(!is_known_code("sintax"));
        assert_eq!(rule_description("other"), "turtle-lsp other");
    }

    #[test]
    fn statistics_sorted_by_count_then_code() {
        let p = |c: &str| problem(range(0, 0, 0, 1), DiagnosticSeverity::WARNING, Some(c), "m");
        let mut a: Vec<Problem> = (0..12).map(|_| p("unknown-term")).collect();
        a.push(p("unused-prefix"));
        let b = vec![p("syntax"), p("unused-prefix"), p("syntax")];
        let mut out = Vec::new();
        statistics(&checked(&a, &b), &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "  12  unknown-term\n   2  syntax\n   2  unused-prefix\n  16  total\n"
        );
    }

    #[test]
    fn summary_lines() {
        let s = |errors, warnings, infos, hints, with, checked| Summary {
            errors,
            warnings,
            infos,
            hints,
            files_with_problems: with,
            files_checked: checked,
        };
        assert_eq!(
            summary_line(&s(3, 1, 0, 0, 2, 5)),
            "3 errors, 1 warning in 2 files"
        );
        assert_eq!(
            summary_line(&s(1, 0, 0, 0, 1, 1)),
            "1 error, 0 warnings in 1 file"
        );
        assert_eq!(
            summary_line(&s(0, 2, 1, 3, 2, 9)),
            "0 errors, 2 warnings, 1 info, 3 hints in 2 files"
        );
        assert_eq!(
            summary_line(&s(0, 0, 0, 1, 1, 9)),
            "0 errors, 0 warnings, 1 hint in 1 file"
        );
        assert_eq!(summary_line(&s(0, 0, 0, 0, 0, 5)), "No problems in 5 files");
        assert_eq!(summary_line(&s(0, 0, 0, 0, 0, 1)), "No problems in 1 file");
    }
}
