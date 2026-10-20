//! The command line: `turtle-lsp check` and `turtle-lsp format`.

mod args;
mod check;
mod diff;
mod files;
mod format;
pub mod output;

use args::Parsed;

pub const USAGE: &str = "\
Usage:
  turtle-lsp                      run the language server (stdio)
  turtle-lsp check [PATHS...]     report problems in RDF and SPARQL files
  turtle-lsp format [PATHS...]    format files in place
  turtle-lsp --version            print the version

Run 'turtle-lsp check --help' or 'turtle-lsp format --help' for options.
";

const CHECK_USAGE: &str = "\
Usage: turtle-lsp check [OPTIONS] [PATHS...]

Report every problem the editor would show, SHACL validation included, for
the RDF and SPARQL files under PATHS (default: the workspace root). Every
RDF file under the root is loaded as context.

Options:
  --root DIR            the workspace root (default: the nearest folder with
                        a .git, else the current folder)
  --format F            concise (default), json, github or sarif
  --output-file PATH    write the report to a file instead of stdout
  --fix                 apply the safe fixes, then report what is left
  --unsafe-fixes        with --fix, also apply \"did you mean\" renames
  --max-warnings N      exit 1 when there are more than N warnings
  --exit-zero           exit 0 when the run completes
  --select CODE,...     report only these diagnostic codes
  --ignore CODE,...     do not report these codes
  --no-shacl            skip SHACL validation
  --network             fetch owl:imports that are not in the cache
  --max-diagnostics N   print at most N problems per file
  --statistics          print a count per code instead of the problems

Exit codes: 0 clean, 1 errors or too many warnings, 2 the tool failed.
";

const FORMAT_USAGE: &str = "\
Usage: turtle-lsp format [OPTIONS] [PATHS...]

Format the Turtle, TriG, N-Triples, N-Quads and SPARQL files under PATHS
(default: the workspace root) in place. A file with syntax errors is left
alone and reported.

Options:
  --root DIR    the workspace root (default: the nearest folder with a .git,
                else the current folder)
  --check       change nothing; list the files that would change
  --diff        change nothing; print a unified diff

Exit codes: 0 done, 1 with --check or --diff when a file would change,
2 the tool failed or a file could not be formatted.
";

/// Run the subcommand in `args[0]` (`check` or `format`) with the rest of
/// `args`; the process exit code.
pub fn run(args: &[String]) -> i32 {
    // Parsing runs on rayon threads; give them room beyond the 2 MB default,
    // as the server does.
    let _ = rayon::ThreadPoolBuilder::new()
        .stack_size(8 << 20)
        .build_global();
    let (command, rest) = match args.split_first() {
        Some((c, rest)) => (c.as_str(), rest),
        None => ("", args),
    };
    let result = match command {
        "check" => args::parse_check(rest).and_then(|p| match p {
            Parsed::Run(a) => check::run(a),
            Parsed::Help => help(CHECK_USAGE),
        }),
        "format" => args::parse_format(rest).and_then(|p| match p {
            Parsed::Run(a) => format::run(a),
            Parsed::Help => help(FORMAT_USAGE),
        }),
        _ => Err(format!("unknown command '{command}'\n\n{USAGE}")),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("turtle-lsp: {e}");
            2
        }
    }
}

fn help(text: &str) -> Result<i32, String> {
    print!("{text}");
    Ok(0)
}
