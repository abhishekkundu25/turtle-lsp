// Every edit re-parses the document. With the system allocator, editing a
// 10 MB TriG file grew to 1.2 GB on macOS (freed pages kept); with mimalloc it
// stays under 600 MB, and musl builds (Alpine, Linux arm64) avoid musl's slow
// malloc.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Everything runs on a thread with a large stack: the main thread has the
    // OS default (1 MiB on Windows), a deeply nested document comes within a
    // factor of two of it, and a stack overflow cannot be caught.
    let run = std::thread::Builder::new()
        .name("turtle-lsp".into())
        .stack_size(turtle_lsp::server::MAIN_STACK)
        .spawn(move || run(&args));
    let code = match run.map(|t| t.join()) {
        Ok(Ok(code)) => code,
        // A panic (its message is printed already), as the main thread's
        // would have been.
        Ok(Err(_)) => 101,
        Err(e) => {
            eprintln!("turtle-lsp: {e}");
            1
        }
    };
    std::process::exit(code);
}

/// The process's exit code.
fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print!("{}", turtle_lsp::cli::USAGE);
            return 0;
        }
        // A command, or a word that is none: that fails instead of starting
        // the server and waiting on stdin. Flags (`--stdio`) start the server.
        Some(word) if !word.starts_with('-') => return turtle_lsp::cli::run(args),
        _ => {}
    }
    // The server is not running yet, so stdout is free for the answer.
    if args.iter().any(|a| a == "--version") {
        println!("turtle-lsp {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    match turtle_lsp::server::run() {
        Ok(true) => 0,
        // `exit` without `shutdown`, or the client went away.
        Ok(false) => 1,
        Err(e) => {
            eprintln!("turtle-lsp: {e}");
            1
        }
    }
}
