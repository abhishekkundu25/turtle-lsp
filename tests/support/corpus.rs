// The public corpus, for the tests that read it. Included with
// `include!("support/corpus.rs")` (it is not a test crate of its own).

/// `rel` under `test/corpus`, when the corpus has it. Without it the test
/// says `skipped` on stderr and gets `None`; in CI (`CI=true`) a missing
/// corpus fails the test instead, so a run that checked nothing is never
/// reported `ok`.
fn corpus_path(rel: &str) -> Option<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus");
    corpus_in(&root, rel, std::env::var("CI").is_ok_and(|v| v == "true"))
}

/// [`corpus_path`] for the corpus at `root`, in CI or not.
fn corpus_in(root: &std::path::Path, rel: &str, ci: bool) -> Option<std::path::PathBuf> {
    let path = root.join(rel);
    if path.exists() {
        return Some(path);
    }
    let message = format!("test/corpus/{rel} is absent (run tools/corpus/fetch.sh, or npm run corpus)");
    assert!(!ci, "{message}; CI=true does not skip the corpus tests");
    eprintln!("skipped: {message}");
    None
}
