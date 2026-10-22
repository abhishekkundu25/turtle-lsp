//! Command-line arguments of `check`, `format` and `query`, parsed by hand.

use super::output::{self, Format};
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct CheckArgs {
    pub paths: Vec<PathBuf>,
    pub root: Option<PathBuf>,
    pub format: Option<Format>,
    pub output_file: Option<PathBuf>,
    pub fix: bool,
    pub unsafe_fixes: bool,
    pub max_warnings: Option<usize>,
    pub exit_zero: bool,
    pub select: Vec<String>,
    pub ignore: Vec<String>,
    pub no_shacl: bool,
    pub network: bool,
    pub max_diagnostics: Option<usize>,
    pub statistics: bool,
}

#[derive(Debug, Default)]
pub struct FormatArgs {
    pub paths: Vec<PathBuf>,
    pub root: Option<PathBuf>,
    pub check: bool,
    pub diff: bool,
}

/// The output of `query`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryFormat {
    Table,
    Json,
    Csv,
    Tsv,
    Turtle,
}

impl QueryFormat {
    pub fn name(self) -> &'static str {
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
pub enum QuerySource {
    File(PathBuf),
    Inline(String),
}

#[derive(Debug)]
pub struct QueryArgs {
    pub query: QuerySource,
    pub paths: Vec<PathBuf>,
    pub root: Option<PathBuf>,
    /// None: the answer's own (a table, or Turtle for a graph).
    pub format: Option<QueryFormat>,
    /// None: no time limit.
    pub timeout: Option<std::time::Duration>,
    pub limit: Option<usize>,
    /// None: no limit.
    pub max_buffered_rows: Option<usize>,
}

/// The default `--timeout`, in seconds.
pub const QUERY_TIMEOUT_SECONDS: u64 = 30;

/// What the command line asks for.
#[derive(Debug)]
pub enum Parsed<T> {
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

/// `-` would be standard input, which no command reads.
fn no_stdin(paths: &[PathBuf]) -> Result<(), String> {
    if paths.iter().any(|p| p.as_os_str() == "-") {
        return Err("'-' (standard input) is not supported; name a file or folder".to_string());
    }
    Ok(())
}

fn unknown(flag: &str, command: &str) -> String {
    format!("unknown option '{flag}' (see 'turtle-lsp {command} --help')")
}

/// Arguments of `check`, without the subcommand itself.
pub fn parse_check(args: &[String]) -> Result<Parsed<CheckArgs>, String> {
    let mut out = CheckArgs::default();
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
    no_stdin(&out.paths)?;
    Ok(Parsed::Run(out))
}

/// Arguments of `format`, without the subcommand itself.
pub fn parse_format(args: &[String]) -> Result<Parsed<FormatArgs>, String> {
    let mut out = FormatArgs::default();
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
    no_stdin(&out.paths)?;
    Ok(Parsed::Run(out))
}

/// Arguments of `query`, without the subcommand itself. The first path is
/// the query file unless `-e` gives the query.
pub fn parse_query(args: &[String]) -> Result<Parsed<QueryArgs>, String> {
    let mut inline = None;
    let mut paths = Vec::new();
    let mut root = None;
    let mut format = None;
    let mut timeout = Some(std::time::Duration::from_secs(QUERY_TIMEOUT_SECONDS));
    let mut limit = None;
    let mut max_buffered_rows = Some(crate::settings::QUERY_MAX_BUFFERED_ROWS);
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

    #[test]
    fn cli_standard_input_is_rejected_by_every_command() {
        for e in [
            check("-").unwrap_err(),
            match parse_format(&args("-")) {
                Err(e) => e,
                Ok(_) => panic!("accepted"),
            },
            query("q.rq -").unwrap_err(),
        ] {
            assert!(e.contains("standard input"), "{e}");
        }
        // Also after --.
        assert!(check("-- -").is_err());
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
        assert_eq!(query("--help").unwrap_err(), "help");
    }
}
