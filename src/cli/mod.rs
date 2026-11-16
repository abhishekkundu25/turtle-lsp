//! The command line: `turtle-lsp check`, `format`, `query`, `codes` and
//! `explain`.

mod args;
mod check;
mod codes;
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
  turtle-lsp codes                list the diagnostic codes
  turtle-lsp explain CODE         describe a code and how to turn it on or off
  turtle-lsp help [COMMAND]       show this text, or the options of a command
  turtle-lsp --version            print the version

Run 'turtle-lsp help COMMAND' (or give the command --help) for its
options.
";

const CHECK_USAGE: &str = "\
Usage: turtle-lsp check [OPTIONS] [PATHS...]

Report every problem the editor would show, SHACL validation included, for
the RDF and SPARQL files under PATHS (default: the workspace root). Every
RDF file under the root is loaded as context.

Options:
  --root DIR            the workspace root (default: the nearest folder with
                        a .git, else the current folder)
  -                     as the only path: check the text on standard input
                        (not with --fix)
  --stdin-filename PATH with -, the file the text stands for (default:
                        stdin.ttl): its syntax, its root and the path its
                        problems are reported under; a file there is
                        replaced by the text
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
  --max-diagnostics N   print at most N problems per file, the most severe
                        first
  --max-problems N      print at most N problems in all, the most severe
                        first (default: 50 in the concise format on stdout,
                        no cap otherwise; 0: no cap); a line on stderr says
                        how many were not printed
  --min-severity S      error, warning, info or hint: report only problems at
                        least this severe (default: hint, every problem); a
                        skipped SHACL run is always reported
  --statistics          print a count per code instead of the problems
                        (not with --format; no cap applies)

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
  --root DIR            the workspace root (default: the nearest folder with
                        a .git, else the current folder)
  -                     as the only path: print the text on standard input
                        formatted (as it is when already formatted)
  --stdin-filename PATH with -, the file the text stands for (default:
                        stdin.ttl): its syntax, and the label of --diff
  --check               change nothing; list the files that would change
                        (with -, print nothing)
  --diff                change nothing; print a unified diff

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
editor. UPDATE is not run. A query that joins more than 70 patterns or
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
  --reasoning R         off, default or complete: also query the triples
                        TurbOWL infers with those rules (OWL 2 RL); a line
                        on stderr says how many (default: query.reasoning,
                        else off)

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

const CODES_USAGE: &str = "\
Usage: turtle-lsp codes [OPTIONS]

List every diagnostic code turtle-lsp reports, one per line: the code, on or
off (whether it is reported by default) and what it means. These are the
codes that --select and --ignore of 'turtle-lsp check', and lint.ignore and
lint.enable in turtle-lsp.json, take.

Options:
  --format F    text (default) or json: {\"version\": 1, \"codes\": [{\"code\",
                \"description\", \"default\": \"on\" or \"off\"}]}

Exit codes: 0 done, 2 a bad option.
";

const EXPLAIN_USAGE: &str = "\
Usage: turtle-lsp explain [OPTIONS] CODE

Describe one diagnostic code: what it means, whether it is reported by
default, and how to turn it on (lint.enable in turtle-lsp.json, for a code
that is off by default), turn it off (lint.ignore in turtle-lsp.json, or
'turtle-lsp check --ignore CODE') or show only it ('turtle-lsp check
--select CODE'). 'turtle-lsp codes' lists the codes.

Options:
  --format F    text (default) or json: the fields of one 'codes --format
                json' entry, and \"enable\" and \"disable\"

Exit codes: 0 done, 2 an unknown code (the closest known code is named) or
a bad option.
";

/// Run the subcommand in `args[0]` (`check`, `format`, `query`, `codes`,
/// `explain` or `help`) with the rest of `args`; the process exit code. Any
/// other word is an error.
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
        "codes" => codes::parse_codes(rest).and_then(|p| match p {
            Parsed::Run(f) => codes::run_codes(f),
            Parsed::Help => help(CODES_USAGE),
        }),
        "explain" => codes::parse_explain(rest, EXPLAIN_USAGE).and_then(|p| match p {
            Parsed::Run(a) => codes::run_explain(a),
            Parsed::Help => help(EXPLAIN_USAGE),
        }),
        "help" => match rest.first().map(String::as_str) {
            None => help(USAGE),
            Some("check") => help(CHECK_USAGE),
            Some("format") => help(FORMAT_USAGE),
            Some("query") => help(QUERY_USAGE),
            Some("codes") => help(CODES_USAGE),
            Some("explain") => help(EXPLAIN_USAGE),
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
