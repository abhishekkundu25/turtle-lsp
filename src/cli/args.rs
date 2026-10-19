//! Command-line arguments of `check` and `format`, parsed by hand.

use super::output::Format;
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

fn codes(v: &str) -> impl Iterator<Item = String> + '_ {
    v.split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
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
            "--select" => out.select.extend(codes(&it.value(f)?)),
            "--ignore" => out.ignore.extend(codes(&it.value(f)?)),
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
    Ok(Parsed::Run(out))
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
        let a =
            check("a.ttl --format=json --max-warnings 3 --select x,y --select z --fix b").unwrap();
        assert_eq!(a.paths, [PathBuf::from("a.ttl"), PathBuf::from("b")]);
        assert_eq!(a.format, Some(Format::Json));
        assert_eq!(a.max_warnings, Some(3));
        assert_eq!(a.select, ["x", "y", "z"]);
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
    fn cli_format_flags() {
        let Parsed::Run(a) = parse_format(&args("--check x.ttl --diff")).unwrap() else {
            panic!()
        };
        assert!(a.check && a.diff);
        assert_eq!(a.paths, [PathBuf::from("x.ttl")]);
        assert!(parse_format(&args("--fix")).is_err());
    }
}
