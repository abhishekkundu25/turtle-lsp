fn main() {
    // stdout carries the protocol; anything else must go to stderr.
    if std::env::args().any(|a| a == "--version") {
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
