//! Command-line arguments of `check`, `format` and `query`, parsed by hand.

use super::output::{self, Format};
use crate::settings::Reasoning;
use crate::workspace::{is_rdf_file, is_sparql_path};
use std::path::PathBuf;

#[derive(Debug, Default)]
pub(crate) struct CheckArgs {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) root: Option<PathBuf>,
    /// `-`: check standard input as the file at this path.
    pub(crate) stdin_filename: Option<PathBuf>,
    pub(crate) format: Option<Format>,
    pub(crate) output_file: Option<PathBuf>,
    pub(crate) fix: bool,
    pub(crate) unsafe_fixes: bool,
    pub(crate) max_warnings: Option<usize>,
    pub(crate) exit_zero: bool,
    pub(crate) select: Vec<String>,
    pub(crate) ignore: Vec<String>,
    pub(crate) no_shacl: bool,
    pub(crate) network: bool,
    pub(crate) max_diagnostics: Option<usize>,
    /// The problems below it are not reported.
    pub(crate) min_severity: Severity,
    /// None: the format's default ([`DEFAULT_MAX_PROBLEMS`] for the concise
    /// format, else no cap); `Some(0)`: no cap.
    pub(crate) max_problems: Option<usize>,
    pub(crate) statistics: bool,
}

#[derive(Debug, Default)]
pub(crate) struct FormatArgs {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) root: Option<PathBuf>,
    /// `-`: format standard input as the file at this path, to stdout.
    pub(crate) stdin_filename: Option<PathBuf>,
    pub(crate) check: bool,
    pub(crate) diff: bool,
}

/// The output of `query`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueryFormat {
    Table,
    Json,
    Csv,
    Tsv,
    Turtle,
}

impl QueryFormat {
    pub(crate) fn name(self) -> &'static str {
        match self {
            QueryFormat::Table => "table",
            QueryFormat::Json => "json",
            QueryFormat::Csv => "csv",
            QueryFormat::Tsv => "tsv",
            QueryFormat::Turtle => "turtle",
        }
    }
}

/// Where the query comes from.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum QuerySource {
    File(PathBuf),
    Inline(String),
}

#[derive(Debug)]
pub(crate) struct QueryArgs {
    pub(crate) query: QuerySource,
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) root: Option<PathBuf>,
    /// None: the answer's own (a table, or Turtle for a graph).
    pub(crate) format: Option<QueryFormat>,
    /// None: no time limit.
    pub(crate) timeout: Option<std::time::Duration>,
    pub(crate) limit: Option<usize>,
    /// None: no limit.
    pub(crate) max_buffered_rows: Option<usize>,
    /// The rules whose inferred triples the query also runs over; None:
    /// `query.reasoning` decides.
    pub(crate) reasoning: Option<Reasoning>,
}

/// The default `--timeout`, in seconds.
pub(crate) const QUERY_TIMEOUT_SECONDS: u64 = 30;

/// `check --min-severity`, from the most severe to the least.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Severity {
    Error,
    Warning,
    Info,
    #[default]
    Hint,
}

/// The problems the concise format prints at most without `--max-problems`.
pub(crate) const DEFAULT_MAX_PROBLEMS: usize = 100;

/// What the command line asks for.
#[derive(Debug)]
pub(crate) enum Parsed<T> {
    Run(T),
    Help,
}

/// The arguments one by one, with `--name=value` split into name and value.
struct Args<'a> {
    items: std::slice::Iter<'a, String>,
    /// The value of the last `--name=value`, not yet taken.
    inline: Option<String>,
    /// After `--`, everything is a path.
    only_paths: bool,
}

enum Item {
    Flag(String),
    Path(String),
}

impl<'a> Args<'a> {
    fn new(args: &'a [String]) -> Args<'a> {
        Args {
            items: args.iter(),
            inline: None,
            only_paths: false,
        }
    }

    fn next(&mut self) -> Result<Option<Item>, String> {
        if let Some(v) = self.inline.take() {
            let flag = "the previous flag";
            return Err(format!("{flag} takes no value (got '{v}')"));
        }
        let Some(a) = self.items.next().cloned() else {
            return Ok(None);
        };
        if self.only_paths || a == "-" || !a.starts_with('-') {
            return Ok(Some(Item::Path(a)));
        }
        if a == "--" {
            self.only_paths = true;
            return self.next();
        }
        match a.split_once('=') {
            Some((name, value)) if name.starts_with("--") => {
                self.inline = Some(value.to_string());
                Ok(Some(Item::Flag(name.to_string())))
            }
            _ => Ok(Some(Item::Flag(a))),
        }
    }

    /// The value of `flag`: after `=`, or the next argument.
    fn value(&mut self, flag: &str) -> Result<String, String> {
        if let Some(v) = self.inline.take() {
            return Ok(v);
        }
        self.items
            .next()
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    }

    fn number(&mut self, flag: &str) -> Result<usize, String> {
        let v = self.value(flag)?;
        v.parse()
            .map_err(|_| format!("{flag} needs a whole number, not '{v}'"))
    }

    /// A flag without a value must not get one with `=`.
    fn no_value(&mut self, flag: &str) -> Result<(), String> {
        match self.inline.take() {
            Some(v) => Err(format!("{flag} takes no value (got '{v}')")),
            None => Ok(()),
        }
    }
}

/// The diagnostic codes in a `--select` or `--ignore` value; a code
/// turtle-lsp never reports is an error, so a typo does not filter everything.
fn codes(flag: &str, v: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for c in v.split(',').map(str::trim).filter(|c| !c.is_empty()) {
        if !output::is_known_code(c) {
            return Err(format!(
                "{flag}: unknown diagnostic code '{c}' (known codes: {})",
                output::known_codes()
            ));
        }
        out.push(c.to_string());
    }
    Ok(out)
}

/// `-` would be standard input, which `query` does not read.
fn no_stdin(paths: &[PathBuf]) -> Result<(), String> {
    if paths.iter().any(|p| p.as_os_str() == "-") {
        return Err("'-' (standard input) is not supported; name a file or folder".to_string());
    }
    Ok(())
}

/// The name of the text on standard input when `paths` is `-` (then taken
/// out of `paths`): `name` (`--stdin-filename`), else `stdin.ttl`.
fn stdin(paths: &mut Vec<PathBuf>, name: Option<PathBuf>) -> Result<Option<PathBuf>, String> {
    if !paths.iter().any(|p| p.as_os_str() == "-") {
        return match name {
            Some(_) => Err("--stdin-filename works only with '-' (standard input)".to_string()),
            None => Ok(None),
        };
    }
    if paths.len() > 1 {
        return Err("'-' (standard input) cannot be combined with other paths".to_string());
    }
    paths.clear();
    let name = name.unwrap_or_else(|| PathBuf::from("stdin.ttl"));
    if !is_rdf_file(&name) && !is_sparql_path(&name) {
        return Err(format!(
            "--stdin-filename {}: not the name of an RDF or SPARQL file",
            name.display()
        ));
    }
    Ok(Some(name))
}

fn unknown(flag: &str, command: &str) -> String {
    format!("unknown option '{flag}' (see 'turtle-lsp {command} --help')")
}

/// Arguments of `check`, without the subcommand itself.
pub(crate) fn parse_check(args: &[String]) -> Result<Parsed<CheckArgs>, String> {
    let mut out = CheckArgs::default();
    let mut stdin_filename = None;
    let mut it = Args::new(args);
    while let Some(item) = it.next()? {
        let flag = match item {
            Item::Path(p) => {
                out.paths.push(PathBuf::from(p));
                continue;
            }
            Item::Flag(f) => f,
        };
        let f = flag.as_str();
        match f {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--root" => out.root = Some(PathBuf::from(it.value(f)?)),
            "--stdin-filename" => stdin_filename = Some(PathBuf::from(it.value(f)?)),
            "--format" => {
                let v = it.value(f)?;
                out.format = Some(match v.as_str() {
                    "concise" => Format::Concise,
                    "json" => Format::Json,
                    "github" => Format::Github,
                    "sarif" => Format::Sarif,
                    _ => {
                        return Err(format!(
                            "--format must be concise, json, github or sarif, not '{v}'"
                        ));
                    }
                });
            }
            "--output-file" => out.output_file = Some(PathBuf::from(it.value(f)?)),
            "--max-warnings" => out.max_warnings = Some(it.number(f)?),
            "--max-diagnostics" => out.max_diagnostics = Some(it.number(f)?),
            "--max-problems" => out.max_problems = Some(it.number(f)?),
            "--min-severity" => {
                let v = it.value(f)?;
                out.min_severity = match v.as_str() {
                    "error" => Severity::Error,
                    "warning" => Severity::Warning,
                    "info" => Severity::Info,
                    "hint" => Severity::Hint,
                    _ => {
                        return Err(format!(
                            "--min-severity must be error, warning, info or hint, not '{v}'"
                        ));
                    }
                };
            }
            "--select" => out.select.extend(codes(f, &it.value(f)?)?),
            "--ignore" => out.ignore.extend(codes(f, &it.value(f)?)?),
            _ => {
                let slot = match f {
                    "--fix" => &mut out.fix,
                    "--unsafe-fixes" => &mut out.unsafe_fixes,
                    "--exit-zero" => &mut out.exit_zero,
                    "--no-shacl" => &mut out.no_shacl,
                    "--network" => &mut out.network,
                    "--statistics" => &mut out.statistics,
                    _ => return Err(unknown(f, "check")),
                };
                *slot = true;
                it.no_value(f)?;
            }
        }
    }
    if out.unsafe_fixes && !out.fix {
        return Err("--unsafe-fixes works only with --fix".to_string());
    }
    if out.statistics && out.format.is_some() {
        return Err("--statistics prints counts in its own layout; drop --format".to_string());
    }
    out.stdin_filename = stdin(&mut out.paths, stdin_filename)?;
    if out.fix && out.stdin_filename.is_some() {
        return Err("--fix writes files; it does not work with '-' (standard input)".to_string());
    }
    Ok(Parsed::Run(out))
}

/// Arguments of `format`, without the subcommand itself.
pub(crate) fn parse_format(args: &[String]) -> Result<Parsed<FormatArgs>, String> {
    let mut out = FormatArgs::default();
    let mut stdin_filename = None;
    let mut it = Args::new(args);
    while let Some(item) = it.next()? {
        let flag = match item {
            Item::Path(p) => {
                out.paths.push(PathBuf::from(p));
                continue;
            }
            Item::Flag(f) => f,
        };
        let f = flag.as_str();
        match f {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--root" => out.root = Some(PathBuf::from(it.value(f)?)),
            "--stdin-filename" => stdin_filename = Some(PathBuf::from(it.value(f)?)),
            "--check" | "--diff" => {
                if f == "--check" {
                    out.check = true;
                } else {
                    out.diff = true;
                }
                it.no_value(f)?;
            }
            _ => return Err(unknown(f, "format")),
        }
    }
    out.stdin_filename = stdin(&mut out.paths, stdin_filename)?;
    Ok(Parsed::Run(out))
}

/// Arguments of `query`, without the subcommand itself. The first path is
/// the query file unless `-e` gives the query.
pub(crate) fn parse_query(args: &[String]) -> Result<Parsed<QueryArgs>, String> {
    let mut inline = None;
    let mut paths = Vec::new();
    let mut root = None;
    let mut format = None;
    let mut timeout = Some(std::time::Duration::from_secs(QUERY_TIMEOUT_SECONDS));
    let mut limit = None;
    let mut max_buffered_rows = Some(crate::settings::QUERY_MAX_BUFFERED_ROWS);
    let mut reasoning = None;
    let mut it = Args::new(args);
    while let Some(item) = it.next()? {
        let flag = match item {
            Item::Path(p) => {
                paths.push(PathBuf::from(p));
                continue;
            }
            Item::Flag(f) => f,
        };
        let f = flag.as_str();
        match f {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-e" | "--execute" => {
                if inline.is_some() {
                    return Err(format!("{f} is given twice"));
                }
                inline = Some(it.value(f)?);
            }
            "--root" => root = Some(PathBuf::from(it.value(f)?)),
            "--format" => {
                let v = it.value(f)?;
                format = Some(match v.as_str() {
                    "table" => QueryFormat::Table,
                    "json" => QueryFormat::Json,
                    "csv" => QueryFormat::Csv,
                    "tsv" => QueryFormat::Tsv,
                    "turtle" => QueryFormat::Turtle,
                    _ => {
                        return Err(format!(
                            "--format must be table, json, csv, tsv or turtle, not '{v}'"
                        ));
                    }
                });
            }
            "--timeout" => {
                let v = it.value(f)?;
                let secs = v
                    .parse::<f64>()
                    .ok()
                    .and_then(crate::settings::seconds)
                    .ok_or_else(|| format!("{f} needs a number of seconds, not '{v}'"))?;
                timeout = (!secs.is_zero()).then_some(secs);
            }
            "--limit" => limit = Some(it.number(f)?),
            "--max-buffered-rows" => {
                let n: usize = it.number(f)?;
                max_buffered_rows = (n > 0).then_some(n);
            }
            "--reasoning" => {
                let v = it.value(f)?;
                reasoning = Some(match v.as_str() {
                    "off" => Reasoning::Off,
                    "default" => Reasoning::Default,
                    "complete" => Reasoning::Complete,
                    _ => {
                        return Err(format!(
                            "--reasoning must be off, default or complete, not '{v}'"
                        ));
                    }
                });
            }
            _ => return Err(unknown(f, "query")),
        }
    }
    no_stdin(&paths)?;
    if limit.is_some() && format.is_some_and(|f| f != QueryFormat::Table) {
        return Err("--limit works only with the table format".to_string());
    }
    let query = match inline {
        Some(text) => QuerySource::Inline(text),
        None if paths.is_empty() => {
            return Err(
                "no query: give a query file or -e QUERY (see 'turtle-lsp query --help')"
                    .to_string(),
            );
        }
        None => QuerySource::File(paths.remove(0)),
    };
    Ok(Parsed::Run(QueryArgs {
        query,
        paths,
        root,
        format,
        timeout,
        limit,
        max_buffered_rows,
        reasoning,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn check(s: &str) -> Result<CheckArgs, String> {
        match parse_check(&args(s))? {
            Parsed::Run(a) => Ok(a),
            Parsed::Help => Err("help".into()),
        }
    }

    #[test]
    fn cli_check_flags_and_paths() {
        let a = check(
            "a.ttl --format=json --max-warnings 3 --select syntax,io --select encoding --fix b",
        )
        .unwrap();
        assert_eq!(a.paths, [PathBuf::from("a.ttl"), PathBuf::from("b")]);
        assert_eq!(a.format, Some(Format::Json));
        assert_eq!(a.max_warnings, Some(3));
        assert_eq!(a.select, ["syntax", "io", "encoding"]);
        assert!(a.fix && !a.unsafe_fixes);
        let a = check("-- --fix").unwrap();
        assert_eq!(a.paths, [PathBuf::from("--fix")]);
        assert!(!a.fix);
    }

    #[test]
    fn cli_check_output_budget() {
        let a = check("").unwrap();
        assert_eq!((a.min_severity, a.max_problems), (Severity::Hint, None));
        let a = check("--min-severity warning --max-problems=0").unwrap();
        assert_eq!(
            (a.min_severity, a.max_problems),
            (Severity::Warning, Some(0))
        );
        let a = check("--min-severity=error --max-problems 10").unwrap();
        assert_eq!(
            (a.min_severity, a.max_problems),
            (Severity::Error, Some(10))
        );
        assert_eq!(
            check("--min-severity info").unwrap().min_severity,
            Severity::Info
        );
        assert!(Severity::Error < Severity::Warning && Severity::Info < Severity::Hint);
        let e = check("--min-severity fatal").unwrap_err();
        assert!(
            e.contains("error, warning, info or hint") && e.contains("'fatal'"),
            "{e}"
        );
        assert!(check("--min-severity").is_err());
        assert!(check("--max-problems -1").is_err());
        assert!(check("--max-problems lots").is_err());
    }

    #[test]
    fn cli_check_bad_flags() {
        assert!(check("--bogus").unwrap_err().contains("--bogus"));
        assert!(check("--format xml").is_err());
        assert!(check("--max-warnings").is_err());
        assert!(check("--max-warnings x").is_err());
        assert!(check("--fix=yes").is_err());
        assert!(check("--unsafe-fixes").is_err());
        assert_eq!(check("--help").unwrap_err(), "help");
    }

    #[test]
    fn cli_check_rejects_unknown_codes_and_what_does_not_combine() {
        for flag in ["--select", "--ignore"] {
            let e = check(&format!("{flag} syntax,sintax")).unwrap_err();
            assert!(
                e.contains("'sintax'") && e.contains("undeclared-prefix"),
                "{e}"
            );
        }
        // Every code the output knows is accepted, and so are io and encoding.
        assert!(check("--select io,encoding,shacl-violation --ignore unused-prefix").is_ok());
        assert!(check("--statistics --format json").is_err());
        assert!(check("--statistics").is_ok());
    }

    fn format(s: &str) -> Result<FormatArgs, String> {
        match parse_format(&args(s))? {
            Parsed::Run(a) => Ok(a),
            Parsed::Help => Err("help".into()),
        }
    }

    #[test]
    fn cli_standard_input_is_rejected_by_query() {
        for e in [
            query("q.rq -").unwrap_err(),
            query("-e ASK{} -").unwrap_err(),
        ] {
            assert!(e.contains("standard input"), "{e}");
        }
    }

    #[test]
    fn cli_check_and_format_read_standard_input_alone() {
        let a = check("-").unwrap();
        assert_eq!(a.stdin_filename, Some(PathBuf::from("stdin.ttl")));
        assert!(a.paths.is_empty());
        let a = format("--check - --stdin-filename=data/x.trig").unwrap();
        assert_eq!(a.stdin_filename, Some(PathBuf::from("data/x.trig")));
        assert!(a.paths.is_empty() && a.check);
        // After --, `-` is still standard input.
        assert!(check("-- -").unwrap().stdin_filename.is_some());
        let named = |s: &str| check(s).unwrap().stdin_filename.unwrap();
        assert_eq!(named("- --stdin-filename q.rq"), PathBuf::from("q.rq"));
        assert_eq!(
            named("--stdin-filename d.jsonld -"),
            PathBuf::from("d.jsonld")
        );
        // Without `-`, nothing is read.
        assert_eq!(check("a.ttl").unwrap().stdin_filename, None);
        assert_eq!(format("a.ttl").unwrap().stdin_filename, None);
    }

    #[test]
    fn cli_standard_input_combinations_are_errors() {
        for e in [
            check("- a.ttl").unwrap_err(),
            check("a.ttl -").unwrap_err(),
            check("- -").unwrap_err(),
            format("- a.ttl").unwrap_err(),
        ] {
            assert!(e.contains("cannot be combined with other paths"), "{e}");
        }
        assert!(check("- --fix").unwrap_err().contains("--fix"));
        for e in [
            check("- --stdin-filename").unwrap_err(),
            format("- --stdin-filename").unwrap_err(),
        ] {
            assert!(e.contains("--stdin-filename needs a value"), "{e}");
        }
        for e in [
            check("--stdin-filename x.ttl a.ttl").unwrap_err(),
            format("--stdin-filename x.ttl").unwrap_err(),
        ] {
            assert!(e.contains("only with '-'"), "{e}");
        }
        for e in [
            check("- --stdin-filename notes.txt").unwrap_err(),
            format("- --stdin-filename data").unwrap_err(),
        ] {
            assert!(e.contains("not the name of an RDF or SPARQL file"), "{e}");
        }
    }

    #[test]
    fn cli_format_flags() {
        let Parsed::Run(a) = parse_format(&args("--check x.ttl --diff")).unwrap() else {
            panic!()
        };
        assert!(a.check && a.diff);
        assert_eq!(a.paths, [PathBuf::from("x.ttl")]);
        assert!(parse_format(&args("--fix")).is_err());
    }

    fn query(s: &str) -> Result<QueryArgs, String> {
        match parse_query(&args(s))? {
            Parsed::Run(a) => Ok(a),
            Parsed::Help => Err("help".into()),
        }
    }

    #[test]
    fn cli_query_flags() {
        let a = query("q.rq a.ttl --format=csv --timeout 1.5 data").unwrap();
        assert_eq!(a.query, QuerySource::File(PathBuf::from("q.rq")));
        assert_eq!(a.paths, [PathBuf::from("a.ttl"), PathBuf::from("data")]);
        assert_eq!(a.format, Some(QueryFormat::Csv));
        assert_eq!(a.timeout, Some(std::time::Duration::from_millis(1500)));
        let a = query("-e ASK{} a.ttl --limit 3").unwrap();
        assert_eq!(a.query, QuerySource::Inline("ASK{}".into()));
        assert_eq!(a.paths, [PathBuf::from("a.ttl")]);
        assert_eq!(a.limit, Some(3));
        assert_eq!(a.timeout, Some(std::time::Duration::from_secs(30)));
        assert_eq!(query("q.rq --timeout 0").unwrap().timeout, None);
        assert_eq!(a.max_buffered_rows, Some(10_000_000));
        let rows = |s: &str| query(s).unwrap().max_buffered_rows;
        assert_eq!(rows("q.rq --max-buffered-rows 500"), Some(500));
        assert_eq!(rows("q.rq --max-buffered-rows 0"), None);
        assert_eq!(a.reasoning, None);
        let rules = |s: &str| query(s).unwrap().reasoning;
        assert_eq!(rules("q.rq --reasoning default"), Some(Reasoning::Default));
        assert_eq!(
            rules("q.rq --reasoning=complete"),
            Some(Reasoning::Complete)
        );
        assert_eq!(
            rules("q.rq --reasoning complete --reasoning off"),
            Some(Reasoning::Off)
        );
    }

    #[test]
    fn cli_query_bad_flags() {
        assert!(query("").unwrap_err().contains("no query"));
        assert!(query("q.rq --format xml").is_err());
        assert!(query("q.rq --timeout -1").is_err());
        // A number a Duration cannot hold is an error, not a panic.
        for bad in ["1e300", "1.9e19", "inf", "NaN"] {
            let e = query(&format!("q.rq --timeout {bad}")).unwrap_err();
            assert!(e.contains("needs a number of seconds"), "{bad}: {e}");
        }
        assert!(query("q.rq --max-buffered-rows -1").is_err());
        assert!(query("q.rq --max-buffered-rows lots").is_err());
        assert!(query("q.rq --limit 3 --format json").is_err());
        assert!(query("-e a -e b").is_err());
        let e = query("q.rq --reasoning nope").unwrap_err();
        assert!(e.contains("off, default or complete"), "{e}");
        assert!(query("q.rq --reasoning").is_err());
        assert_eq!(query("--help").unwrap_err(), "help");
    }
}
