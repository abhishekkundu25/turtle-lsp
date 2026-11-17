//! `turtle-lsp codes` and `turtle-lsp explain`: the diagnostic codes, each
//! with its description and whether it is on by default. They read the code
//! table ([`super::output::CODES`]) and, through the default settings
//! ([`Settings::lint_on`]), the off-by-default list
//! ([`crate::features::curation::OPT_IN`]); neither is copied here.

use super::args::Parsed;
use super::output::{CODES, ENCODING_CODE, IO_CODE};
use crate::settings::Settings;
use crate::vocab::EditBuf;
use serde_json::{Value, json};
use std::io::Write;

/// The output of `codes` and `explain`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CodesFormat {
    #[default]
    Text,
    Json,
}

/// One row of the code table.
type Code = (&'static str, &'static str);

#[derive(Debug)]
pub(crate) struct ExplainArgs {
    /// The row of the code asked for: a known code by construction.
    code: Code,
    format: CodesFormat,
}

/// The options both commands take, and the words that are not options.
fn parse(args: &[String], command: &str) -> Result<Parsed<(CodesFormat, Vec<String>)>, String> {
    let mut format = CodesFormat::Text;
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        match flag {
            "-h" | "--help" if inline.is_none() => return Ok(Parsed::Help),
            "--format" => {
                let v = match inline {
                    Some(v) => v,
                    None => it.next().cloned().ok_or("--format needs a value")?,
                };
                format = match v.as_str() {
                    "text" => CodesFormat::Text,
                    "json" => CodesFormat::Json,
                    _ => return Err(format!("--format must be text or json, not '{v}'")),
                };
            }
            f if f.starts_with('-') => {
                return Err(format!(
                    "unknown option '{a}' (see 'turtle-lsp {command} --help')"
                ));
            }
            _ => words.push(a.clone()),
        }
    }
    Ok(Parsed::Run((format, words)))
}

/// Arguments of `codes`, without the subcommand itself.
pub(crate) fn parse_codes(args: &[String]) -> Result<Parsed<CodesFormat>, String> {
    Ok(match parse(args, "codes")? {
        Parsed::Help => Parsed::Help,
        Parsed::Run((_, words)) if !words.is_empty() => {
            return Err(format!(
                "codes takes no arguments (got '{}'); for one code, run 'turtle-lsp explain {0}'",
                words[0]
            ));
        }
        Parsed::Run((format, _)) => Parsed::Run(format),
    })
}

/// Arguments of `explain`, without the subcommand itself. `usage` is the
/// text shown when the code is missing.
pub(crate) fn parse_explain(args: &[String], usage: &str) -> Result<Parsed<ExplainArgs>, String> {
    let (format, words) = match parse(args, "explain")? {
        Parsed::Help => return Ok(Parsed::Help),
        Parsed::Run(r) => r,
    };
    let code = match words.as_slice() {
        [] => return Err(format!("explain needs a diagnostic code\n\n{usage}")),
        [c] => find(c)?,
        [_, extra, ..] => {
            return Err(format!(
                "explain takes one code (got '{extra}' too; see 'turtle-lsp explain --help')"
            ));
        }
    };
    Ok(Parsed::Run(ExplainArgs { code, format }))
}

/// The row of `code`; an unknown code names the closest known one.
fn find(code: &str) -> Result<Code, String> {
    if let Some(row) = CODES.iter().find(|(c, _)| *c == code) {
        return Ok(*row);
    }
    let probe: Vec<char> = code.chars().collect();
    // A suggestion only when it is a likely typo: a third of the code's length.
    let max = (probe.len() / 3).max(2);
    let mut buf = EditBuf::default();
    let closest = CODES
        .iter()
        .filter_map(|(c, _)| {
            let chars: Vec<char> = c.chars().collect();
            buf.within(&probe, &chars, max).map(|d| (d, *c))
        })
        .min();
    let hint = closest.map_or(String::new(), |(_, c)| format!("; did you mean '{c}'?"));
    Err(format!(
        "unknown diagnostic code '{code}'{hint} (run 'turtle-lsp codes' for the list)"
    ))
}

/// Whether `code` is reported unless a setting turns it off. Only the
/// opt-in checks are off by default, and `lint.enable` turns those on.
fn on_by_default(code: &str) -> bool {
    Settings::default().lint_on(code)
}

fn default_word(code: &str) -> &'static str {
    if on_by_default(code) { "on" } else { "off" }
}

/// A file that could not be read: always reported, and the run exits 2.
fn always_on(code: &str) -> bool {
    code == IO_CODE || code == ENCODING_CODE
}

/// How to turn `code` on.
fn enable(code: &str) -> String {
    if on_by_default(code) {
        "It is on by default.".to_string()
    } else {
        format!(
            "Add \"{code}\" to \"lint.enable\" in turtle-lsp.json at the workspace root: \
             {{\"lint\": {{\"enable\": [\"{code}\"]}}}}."
        )
    }
}

/// How to turn `code` off.
fn disable(code: &str) -> String {
    if always_on(code) {
        return format!(
            "It cannot be turned off: a file that could not be read leaves the check \
             incomplete, and 'turtle-lsp check' exits 2. --ignore {code} hides the line only."
        );
    }
    let ignore = format!(
        "\"lint.ignore\" in turtle-lsp.json at the workspace root \
             ({{\"lint\": {{\"ignore\": [\"{code}\"]}}}}), or run 'turtle-lsp check --ignore {code}'"
    );
    if on_by_default(code) {
        format!("Add \"{code}\" to {ignore}.")
    } else {
        format!(
            "It is off unless \"lint.enable\" lists it; to turn it off even then, add it to {ignore}."
        )
    }
}

/// `codes`: one line per code, in the table's order.
fn codes_text() -> String {
    let width = CODES.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    CODES
        .iter()
        .map(|(c, d)| format!("{c:<width$}  {:<3}  {d}\n", default_word(c)))
        .collect()
}

fn entry((code, description): Code) -> Value {
    json!({ "code": code, "description": description, "default": default_word(code) })
}

fn codes_json() -> Value {
    json!({ "version": 1, "codes": CODES.iter().copied().map(entry).collect::<Vec<_>>() })
}

fn explain_text((code, description): Code) -> String {
    let select = if on_by_default(code) {
        String::new()
    } else {
        " (it must be in \"lint.enable\" too)".to_string()
    };
    format!(
        "{code}: {description}\n\
         Default: {}\n\
         Turn on: {}\n\
         Turn off: {}\n\
         Show only it: turtle-lsp check --select {code}{select}\n",
        default_word(code),
        enable(code),
        disable(code),
    )
}

fn explain_json(row: Code) -> Value {
    let mut v = entry(row);
    v["enable"] = json!(enable(row.0));
    v["disable"] = json!(disable(row.0));
    v
}

fn json_line(v: &Value) -> String {
    format!("{v}\n")
}

/// Runs `codes`; the exit code.
pub(crate) fn run_codes(format: CodesFormat) -> Result<i32, String> {
    print(&match format {
        CodesFormat::Text => codes_text(),
        CodesFormat::Json => json_line(&codes_json()),
    })
}

/// Runs `explain`; the exit code.
pub(crate) fn run_explain(a: ExplainArgs) -> Result<i32, String> {
    print(&match a.format {
        CodesFormat::Text => explain_text(a.code),
        CodesFormat::Json => json_line(&explain_json(a.code)),
    })
}

fn print(text: &str) -> Result<i32, String> {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => Ok(0),
        // A reader that stopped early (`| head`) is not a failure.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(0),
        Err(e) => Err(format!("cannot write to stdout: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::output;
    use crate::features::curation::OPT_IN;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn explain(s: &str) -> Result<ExplainArgs, String> {
        match parse_explain(&args(s), "USAGE")? {
            Parsed::Run(a) => Ok(a),
            Parsed::Help => Err("help".into()),
        }
    }

    #[test]
    fn cli_codes_flags() {
        assert!(matches!(
            parse_codes(&[]),
            Ok(Parsed::Run(CodesFormat::Text))
        ));
        assert!(matches!(
            parse_codes(&args("--format=json")),
            Ok(Parsed::Run(CodesFormat::Json))
        ));
        assert!(matches!(
            parse_codes(&args("--format text")),
            Ok(Parsed::Run(CodesFormat::Text))
        ));
        assert!(matches!(parse_codes(&args("-h")), Ok(Parsed::Help)));
        assert_eq!(
            parse_codes(&args("--format xml")).unwrap_err(),
            "--format must be text or json, not 'xml'"
        );
        assert_eq!(
            parse_codes(&args("--format")).unwrap_err(),
            "--format needs a value"
        );
        assert!(
            parse_codes(&args("--fix"))
                .unwrap_err()
                .contains("unknown option '--fix'")
        );
        assert!(
            parse_codes(&args("syntax"))
                .unwrap_err()
                .contains("explain syntax")
        );
    }

    #[test]
    fn cli_explain_flags() {
        let a = explain("--format json unused-prefix").unwrap();
        assert_eq!((a.code.0, a.format), ("unused-prefix", CodesFormat::Json));
        assert_eq!(explain("syntax --help").unwrap_err(), "help");
        assert_eq!(
            explain("").unwrap_err(),
            "explain needs a diagnostic code\n\nUSAGE"
        );
        assert!(explain("syntax io").unwrap_err().contains("'io' too"));
        assert!(
            explain("undeclard-prefix")
                .unwrap_err()
                .contains("did you mean 'undeclared-prefix'?")
        );
    }

    /// The table, in its order, and the defaults the settings apply.
    #[test]
    fn cli_codes_read_the_table_and_the_settings() {
        let text = codes_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), CODES.len());
        for (line, (c, d)) in lines.iter().zip(CODES) {
            let words: Vec<&str> = line.split_whitespace().collect();
            let on = !OPT_IN.contains(c);
            assert_eq!(words[..2], [*c, if on { "on" } else { "off" }], "{line}");
            assert!(line.ends_with(d), "{line}");
        }
        let v = codes_json();
        assert_eq!(v["version"], 1);
        assert_eq!(v["codes"].as_array().unwrap().len(), CODES.len());
        assert!(output::is_known_code(
            v["codes"][0]["code"].as_str().unwrap()
        ));
    }
}
