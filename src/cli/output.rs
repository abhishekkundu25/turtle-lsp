//! Reports of `turtle-lsp check`, in each output format.

use crate::batch::Problem;
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Concise,
    Json,
    Github,
    Sarif,
}

/// One checked file: its path as shown (relative to the current directory
/// when under it, `/` separators), its path relative to the workspace root
/// (for SARIF), and its problems with code-point columns, sorted by
/// position then code.
pub struct CheckedFile<'a> {
    pub path: &'a str,
    pub root_relative: &'a str,
    pub problems: &'a [Problem],
}

/// Counts over every problem found, before `--max-diagnostics` trimmed the
/// output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub hints: usize,
    pub files_with_problems: usize,
    pub files_checked: usize,
}

/// Writes the report for `files` in `format` to `out`.
pub fn render(
    format: Format,
    files: &[CheckedFile],
    summary: &Summary,
    out: &mut dyn Write,
) -> io::Result<()> {
    let _ = (format, files, summary, out);
    todo!("output formats slice")
}

/// `--statistics`: problem counts per code, most frequent first.
pub fn statistics(files: &[CheckedFile], out: &mut dyn Write) -> io::Result<()> {
    let _ = (files, out);
    todo!("output formats slice")
}

/// The one-line summary for stderr, e.g. "3 errors, 1 warning in 2 files".
pub fn summary_line(summary: &Summary) -> String {
    let _ = summary;
    todo!("output formats slice")
}
