// Every edit re-parses the document. With the system allocator, editing a
// 10 MB TriG file grew to 1.2 GB on macOS (freed pages kept); with mimalloc it
// stays under 600 MB, and musl builds (Alpine, Linux arm64) avoid musl's slow
// malloc.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check" | "format") => std::process::exit(turtle_lsp::cli::run(&args)),
        Some("help" | "--help" | "-h") => {
            print!("{}", turtle_lsp::cli::USAGE);
            return;
        }
        _ => {}
    }
    // As the language server, stdout carries the protocol; anything else must
    // go to stderr.
    if args.iter().any(|a| a == "--version") {
        eprintln!("turtle-lsp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match turtle_lsp::server::run() {
        Ok(true) => {}
        // `exit` without `shutdown`, or the client went away.
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("turtle-lsp: {e}");
            std::process::exit(1);
        }
    }
}
