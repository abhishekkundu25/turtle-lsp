//! The workspace of a command-line run: its root, its settings, and the files
//! it loads and reports.

use crate::batch::BatchFile;
use crate::settings::{Settings, read_settings_file};
use crate::workspace::{is_sparql_path, is_turtle, scan_roots_matching};
use rayon::prelude::*;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// The files of a run, read.
pub struct Workspace {
    pub root: PathBuf,
    pub cwd: PathBuf,
    /// Reported files first (sorted by path), then context files.
    pub files: Vec<File>,
}

pub struct File {
    pub path: PathBuf,
    /// As shown: relative to the current directory when under it, `/` separators.
    pub shown: String,
    /// Relative to the workspace root (`/` separators), else as shown.
    pub root_relative: String,
    pub text: String,
    pub reported: bool,
}

impl File {
    pub fn batch(&self) -> BatchFile {
        BatchFile::at_path(&self.path, &self.shown, self.text.clone(), !self.reported)
            .expect("an absolute path has a file URI")
    }
}

/// What a run loads besides the files it reports.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// Every RDF file under the root (`check`).
    Workspace,
    /// Nothing: prefixes.ttl files are read when needed (`format`).
    None,
}

/// The current directory, without symbolic links.
pub fn current_dir() -> Result<PathBuf, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    Ok(real(&cwd))
}

/// `path` without symbolic links (and on Windows without `\\?\`), or as it is
/// when it cannot be resolved.
pub fn real(path: &Path) -> PathBuf {
    let p = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC")
        {
            return PathBuf::from(rest);
        }
    }
    p
}

/// The workspace root: `flag` if given, else the nearest ancestor of `cwd`
/// (itself included) with a `.git`, else `cwd`.
pub fn find_root(flag: Option<&Path>, cwd: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = flag {
        let dir = cwd.join(dir);
        if !dir.is_dir() {
            return Err(format!("--root {}: not a directory", dir.display()));
        }
        return Ok(real(&dir));
    }
    Ok(cwd
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf())
}

/// Settings: defaults, then `turtle-lsp.json` at `root` (a bad file is
/// reported on stderr and ignored), then `flags`.
pub fn settings(root: &Path, flags: &Value) -> Settings {
    let file = match read_settings_file(root) {
        Ok(v) => v.unwrap_or(Value::Null),
        Err(e) => {
            eprintln!("turtle-lsp: warning: {e}; using the default settings");
            Value::Null
        }
    };
    Settings::from_layers([&file, flags])
}

/// `path` relative to `base` with `/` separators, if it is under `base`.
fn relative(path: &Path, base: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    Some(if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    })
}

fn shown(path: &Path, cwd: &Path) -> String {
    relative(path, cwd).unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"))
}

/// Load the files of a run. `paths` are what the user named (default: the
/// root); the RDF and SPARQL files under them are reported. A named path
/// that does not exist, or a reported file that cannot be read, fails the
/// run.
pub fn load(
    root: PathBuf,
    cwd: PathBuf,
    paths: &[PathBuf],
    context: Context,
) -> Result<Workspace, String> {
    let mut targets = Vec::new();
    for p in paths {
        let abs = cwd.join(p);
        if !abs.exists() {
            return Err(format!("{}: no such file or directory", p.display()));
        }
        targets.push(real(&abs));
    }
    if targets.is_empty() {
        targets.push(root.clone());
    }
    let mut reported: BTreeSet<PathBuf> = BTreeSet::new();
    for t in &targets {
        if t.is_dir() {
            let dir = std::slice::from_ref(t);
            reported.extend(scan_roots_matching(dir, is_turtle));
            reported.extend(scan_roots_matching(dir, is_sparql_path));
        } else if is_turtle(t) || is_sparql_path(t) {
            reported.insert(t.clone());
        } else {
            eprintln!(
                "turtle-lsp: warning: {}: not an RDF or SPARQL file, skipped",
                shown(t, &cwd)
            );
        }
    }
    let mut context_paths: Vec<PathBuf> = match context {
        Context::Workspace => scan_roots_matching(std::slice::from_ref(&root), is_turtle)
            .into_iter()
            .filter(|p| !reported.contains(p))
            .collect(),
        Context::None => Vec::new(),
    };
    context_paths.sort();
    let jobs: Vec<(PathBuf, bool)> = reported
        .into_iter()
        .map(|p| (p, true))
        .chain(context_paths.into_iter().map(|p| (p, false)))
        .collect();
    let read: Vec<Result<Option<File>, String>> = jobs
        .into_par_iter()
        .map(|(path, reported)| {
            let shown = shown(&path, &cwd);
            match std::fs::read_to_string(&path) {
                Ok(text) => Ok(Some(File {
                    root_relative: relative(&path, &root).unwrap_or_else(|| shown.clone()),
                    shown,
                    path,
                    text,
                    reported,
                })),
                Err(e) if reported => Err(format!("{shown}: {e}")),
                Err(_) => Ok(None),
            }
        })
        .collect();
    let mut files = Vec::with_capacity(read.len());
    for r in read {
        files.extend(r?);
    }
    Ok(Workspace { root, cwd, files })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_paths_are_shown_relative_with_slashes() {
        let base = std::env::temp_dir();
        assert_eq!(shown(&base.join("a").join("b.ttl"), &base), "a/b.ttl");
        assert_eq!(relative(&base, &base).as_deref(), Some("."));
    }
}
