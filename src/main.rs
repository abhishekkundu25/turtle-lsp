fn main() {
    // stdout carries the protocol; anything else must go to stderr.
    if std::env::args().any(|a| a == "--version") {
        eprintln!("turtle-lsp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let Err(e) = turtle_lsp::server::run() {
        eprintln!("turtle-lsp: {e}");
        std::process::exit(1);
    }
}
