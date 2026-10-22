//! The command line: `turtle-lsp check`, `turtle-lsp format` and
//! `turtle-lsp query`.

mod args;
mod check;
mod diff;
mod files;
mod format;
pub(crate) mod output;
mod query;

use args::Parsed;

pub const USAGE: &str = "\
Usage:
  turtle-lsp                      run the language server (stdio)
  turtle-lsp check [PATHS...]     report problems in RDF and SPARQL files
  turtle-lsp format [PATHS...]    format files in place
  turtle-lsp query (FILE.rq | -e QUERY) [PATHS...]
                                  run a SPARQL query over RDF files
  turtle-lsp help [COMMAND]       show this text, or the options of a command
  turtle-lsp --version            print the version

Run 'turtle-lsp help check', 'turtle-lsp help format' or
'turtle-lsp help query' (or give the command --help) for options.
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
  --select CODE,...     report only these diagnostic codes (an unknown code
                        is an error)
  --ignore CODE,...     do not report these codes
  --no-shacl            skip SHACL validation
  --network             fetch owl:imports and JSON-LD contexts not in the cache
                        (public addresses only, unless
                        TURTLE_LSP_ALLOW_PRIVATE_HOSTS=1); turtle-lsp.json
                        cannot turn this on
  --max-diagnostics N   print at most N problems per file
  --statistics          print a count per code instead of the problems
                        (not with --format)

A file that cannot be read or is not UTF-8 is reported as an error with the
code io or encoding. A folder scan takes at most 20000 files; a file over
50 MB is skipped. TURTLE_LSP_MAX_FILES=N changes the first limit. --fix
replaces a file through a temporary file in its folder, and only if it has
not changed since it was read.

Exit codes: 0 clean, 1 errors or too many warnings, 2 the tool failed or
did not cover everything: a file could not be read or written (--fix), a
file was skipped, or the scan stopped at its limit.
";

const FORMAT_USAGE: &str = "\
Usage: turtle-lsp format [OPTIONS] [PATHS...]

Format the Turtle (.ttl, .turtle), TriG, N-Triples, N-Quads, JSON-LD and
SPARQL files under PATHS (default: the workspace root) in place. A file with
syntax errors is left alone and reported. A file is replaced through a
temporary file in its folder, and only if it has not changed since it was
read. A folder scan takes at most 20000 files (TURTLE_LSP_MAX_FILES=N changes
that); a file over 50 MB is skipped.

Options:
  --root DIR    the workspace root (default: the nearest folder with a .git,
                else the current folder)
  --check       change nothing; list the files that would change
  --diff        change nothing; print a unified diff

Exit codes: 0 done, 1 with --check or --diff when a file would change,
2 the tool failed, or a file could not be read, formatted or written, or was
left out.
";

const QUERY_USAGE: &str = "\
Usage: turtle-lsp query [OPTIONS] (FILE.rq | -e QUERY) [PATHS...]

Run a SPARQL query (SELECT, ASK, CONSTRUCT or DESCRIBE) over the RDF files
under PATHS (default: the workspace root). The default graph is the union of
every triple of every file, named graphs included; GRAPH and FROM NAMED match
nothing and FROM is ignored. SPARQL files are not data. prefixes.ttl and the
'prefixes' setting apply to the query and to the data files, as in the
editor. UPDATE is not run. A query that joins more than 200 patterns or
path steps at once is refused before it runs.

Options:
  -e, --execute QUERY   the query text, instead of a query file
  --root DIR            the workspace root (default: the nearest folder with
                        a .git, else the current folder)
  --format F            table, json, csv, tsv or turtle (see below)
  --timeout SECONDS     stop the query after this long (default 30; 0: never)
  --limit ROWS          print at most ROWS rows of a table
  --max-buffered-rows N stop the query once it keeps more than N rows in
                        memory to sort, group or join them (default
                        10000000; counts rows, not bytes; 0: never)

Formats:
  SELECT                table (default), json, csv or tsv
  ASK                   table (default; prints true or false) or json
  CONSTRUCT, DESCRIBE   turtle (default)
The table and Turtle show IRIs with the project's and the query's prefixes;
json (SPARQL 1.1 results JSON), csv and tsv give full IRIs.

Exit codes: 0 the query ran (an ASK answered true), 1 an ASK answered false,
2 a syntax error (printed as FILE:LINE:COL: message), a time-out, an
unsupported query or format, or another failure.
";

/// Run the subcommand in `args[0]` (`check`, `format`, `query` or `help`) with
/// the rest of `args`; the process exit code. Any other word is an error.
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
        "query" => args::parse_query(rest).and_then(|p| match p {
            Parsed::Run(a) => query::run(a),
            Parsed::Help => help(QUERY_USAGE),
        }),
        "help" => match rest.first().map(String::as_str) {
            None => help(USAGE),
            Some("check") => help(CHECK_USAGE),
            Some("format") => help(FORMAT_USAGE),
            Some("query") => help(QUERY_USAGE),
            Some(other) => Err(format!("no help for '{other}'\n\n{USAGE}")),
        },
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
