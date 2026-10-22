//! The workspace of a command-line run: its root, its settings, and the files
//! it loads and reports.

use super::output::{ENCODING_CODE, IO_CODE};
use crate::batch::BatchFile;
use crate::settings::{Ignored, Layer, SETTINGS_FILE, Settings, read_settings_file};
use crate::workspace::{
    MAX_FILE_BYTES, MAX_FILES, Scan, is_rdf_file, is_sparql_path, path_to_uri, scan_roots_capped,
};
use rayon::prelude::*;
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// The environment variable that raises (or lowers) the number of files a
/// directory scan of the command line takes. The server's cap is fixed.
pub(crate) const MAX_FILES_VAR: &str = "TURTLE_LSP_MAX_FILES";

/// The files of a run, read.
pub(crate) struct Workspace {
    pub(crate) root: PathBuf,
    pub(crate) cwd: PathBuf,
    /// Reported files first (sorted by path), then context files.
    pub(crate) files: Vec<File>,
    /// Reported files that could not be read, sorted by path.
    pub(crate) unread: Vec<Unread>,
    /// A file or folder was left out of the run, and said so on stderr: the
    /// scan stopped at its cap, a file was too large, or a context file could
    /// not be read. The run is not complete: exit 2.
    pub(crate) incomplete: bool,
}

/// A reported file that could not be read or decoded.
pub(crate) struct Unread {
    pub(crate) shown: String,
    /// Relative to the workspace root (`/` separators); None outside it.
    pub(crate) root_relative: Option<String>,
    pub(crate) uri: String,
    /// `io` or `encoding`.
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

/// What a file was like when it was read: written back only if it still is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Stamp {
        Stamp {
            len: meta.len(),
            modified: meta.modified().ok(),
        }
    }
}

pub(crate) struct File {
    pub(crate) path: PathBuf,
    pub(crate) stamp: Stamp,
    /// As shown: relative to the current directory when under it, `/` separators.
    pub(crate) shown: String,
    /// Relative to the workspace root (`/` separators); None outside it.
    pub(crate) root_relative: Option<String>,
    /// The file's `file:` URI.
    pub(crate) uri: String,
    pub(crate) text: String,
    pub(crate) reported: bool,
}

impl File {
    pub(crate) fn batch(&self) -> BatchFile {
        BatchFile::at_path(&self.path, &self.shown, self.text.clone(), !self.reported)
            .expect("an absolute path has a file URI")
    }
}

/// What a run loads besides the files it reports.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    /// Every RDF file under the root (`check`).
    Workspace,
    /// Nothing: prefixes.ttl files are read when needed (`format`).
    None,
}

/// The current directory, without symbolic links.
pub(crate) fn current_dir() -> Result<PathBuf, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    Ok(real(&cwd))
}

/// `path` without symbolic links (and on Windows without `\\?\`), or as it is
/// when it cannot be resolved.
pub(crate) fn real(path: &Path) -> PathBuf {
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
pub(crate) fn find_root(flag: Option<&Path>, cwd: &Path) -> Result<PathBuf, String> {
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
/// reported on stderr and ignored; so is what it may not set, see
/// [`Layer::File`]), then `flags`.
pub(crate) fn settings(root: &Path, flags: &Value) -> Settings {
    let (settings, ignored) = layered(root, flags);
    warn(&ignored);
    settings
}

/// [`settings`], but a number of seconds in `turtle-lsp.json` that cannot be
/// held is an error (the message for stderr), so a CI gate does not run on
/// settings it was not given.
pub(crate) fn settings_strict(root: &Path, flags: &Value) -> Result<Settings, String> {
    let (settings, mut ignored) = layered(root, flags);
    if !ignored.out_of_range.is_empty() {
        let only = Ignored {
            untrusted: Vec::new(),
            out_of_range: std::mem::take(&mut ignored.out_of_range),
        };
        warn(&ignored);
        return Err(only.message(SETTINGS_FILE).unwrap_or_default());
    }
    warn(&ignored);
    Ok(settings)
}

fn layered(root: &Path, flags: &Value) -> (Settings, Ignored) {
    let file = match read_settings_file(root) {
        Ok(v) => v.unwrap_or(Value::Null),
        Err(e) => {
            eprintln!("turtle-lsp: warning: {e}; using the default settings");
            Value::Null
        }
    };
    Settings::layered([(Layer::File, &file), (Layer::Client, flags)])
}

fn warn(ignored: &Ignored) {
    if let Some(m) = ignored.message(SETTINGS_FILE) {
        eprintln!("turtle-lsp: warning: {m}");
    }
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

/// The cap on files per directory scan: [`MAX_FILES_VAR`], else [`MAX_FILES`].
fn max_files() -> Result<usize, String> {
    match std::env::var(MAX_FILES_VAR) {
        Err(_) => Ok(MAX_FILES),
        Ok(v) => v
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("{MAX_FILES_VAR} needs a whole number above 0, not '{v}'")),
    }
}

/// [`load_lenient`] for a run that needs all of its data (`query`): a file
/// that cannot be read, or one left out, fails it.
pub(crate) fn load(
    root: PathBuf,
    cwd: PathBuf,
    paths: &[PathBuf],
    context: Context,
) -> Result<Workspace, String> {
    let ws = load_lenient(root, cwd, paths, context)?;
    if ws.unread.is_empty() && !ws.incomplete {
        return Ok(ws);
    }
    let mut messages: Vec<String> = ws
        .unread
        .iter()
        .map(|u| format!("{}: {}", u.shown, u.message))
        .collect();
    if ws.incomplete {
        messages.push("files were left out of the run (see the warnings above)".to_string());
    }
    Err(messages.join("\n"))
}

/// Load the files of a run. `paths` are what the user named (default: the
/// root); the RDF and SPARQL files under them are reported. A named path
/// that does not exist fails the run. A reported file that cannot be read
/// is in [`Workspace::unread`], and the run goes on; what a scan left out
/// is warned about on stderr and makes the workspace
/// [`incomplete`](Workspace::incomplete).
pub(crate) fn load_lenient(
    root: PathBuf,
    cwd: PathBuf,
    paths: &[PathBuf],
    context: Context,
) -> Result<Workspace, String> {
    let cap = max_files()?;
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
    let mut truncated: BTreeSet<PathBuf> = BTreeSet::new();
    let mut too_large: BTreeSet<PathBuf> = BTreeSet::new();
    let mut scan = |dir: &Path, wanted: fn(&Path) -> bool| -> Vec<PathBuf> {
        let Scan {
            paths,
            truncated: cut,
            too_large: big,
        } = scan_roots_capped(std::slice::from_ref(&dir.to_path_buf()), wanted, cap);
        if cut {
            truncated.insert(dir.to_path_buf());
        }
        too_large.extend(big);
        paths
    };
    // The root is walked once, for the context and for the named files under it.
    let root_is_target = targets.contains(&root);
    let root_files: Option<Vec<PathBuf>> =
        (context == Context::Workspace || root_is_target).then(|| {
            scan(
                &root,
                if root_is_target {
                    is_rdf_or_sparql
                } else {
                    is_rdf_file
                },
            )
        });
    let mut reported: BTreeSet<PathBuf> = BTreeSet::new();
    for t in &targets {
        if t.is_dir() {
            match &root_files {
                Some(found) if *t == root => reported.extend(found.iter().cloned()),
                _ => reported.extend(scan(t, is_rdf_or_sparql)),
            }
        } else if is_rdf_or_sparql(t) {
            reported.insert(t.clone());
        } else {
            eprintln!(
                "turtle-lsp: warning: {}: not an RDF or SPARQL file, skipped",
                shown(t, &cwd)
            );
        }
    }
    let mut context_paths: Vec<PathBuf> = match (context, root_files) {
        (Context::Workspace, Some(found)) => found
            .into_iter()
            .filter(|p| is_rdf_file(p) && !reported.contains(p))
            .collect(),
        _ => Vec::new(),
    };
    context_paths.sort();
    let mut incomplete = false;
    for dir in &truncated {
        incomplete = true;
        eprintln!(
            "turtle-lsp: warning: {}: more than {cap} RDF and SPARQL files; the scan stopped at \
             the first {cap} in path order (raise the limit with {MAX_FILES_VAR})",
            shown(dir, &cwd)
        );
    }
    for path in &too_large {
        incomplete = true;
        eprintln!(
            "turtle-lsp: warning: {}: larger than {} MB, skipped",
            shown(path, &cwd),
            MAX_FILE_BYTES / (1024 * 1024)
        );
    }
    let jobs: Vec<(PathBuf, bool)> = reported
        .into_iter()
        .map(|p| (p, true))
        .chain(context_paths.into_iter().map(|p| (p, false)))
        .collect();
    let read: Vec<Result<File, (bool, Unread)>> = jobs
        .into_par_iter()
        .map(|(path, reported)| {
            let shown = shown(&path, &cwd);
            let root_relative = relative(&path, &root);
            let uri = path_to_uri(&path).unwrap_or_default();
            match read_text(&path) {
                Ok((text, stamp)) => Ok(File {
                    root_relative,
                    uri,
                    shown,
                    path,
                    stamp,
                    text,
                    reported,
                }),
                Err((code, message)) => Err((
                    reported,
                    Unread {
                        uri,
                        root_relative,
                        shown,
                        code,
                        message,
                    },
                )),
            }
        })
        .collect();
    let mut files = Vec::with_capacity(read.len());
    let mut unread = Vec::new();
    for r in read {
        match r {
            Ok(f) => files.push(f),
            Err((true, u)) => unread.push(u),
            Err((false, u)) => {
                incomplete = true;
                eprintln!(
                    "turtle-lsp: warning: {}: not loaded as context: {}",
                    u.shown, u.message
                );
            }
        }
    }
    Ok(Workspace {
        root,
        cwd,
        files,
        unread,
        incomplete,
    })
}

fn is_rdf_or_sparql(path: &Path) -> bool {
    is_rdf_file(path) || is_sparql_path(path)
}

/// The text of the file at `path` and its [`Stamp`], or the code
/// (`io` or `encoding`) and message of what went wrong.
fn read_text(path: &Path) -> Result<(String, Stamp), (&'static str, String)> {
    let io = |e: std::io::Error| (IO_CODE, e.to_string());
    let mut file = std::fs::File::open(path).map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    if meta.len() > MAX_FILE_BYTES {
        return Err((
            IO_CODE,
            format!(
                "larger than {} MB, not read",
                MAX_FILE_BYTES / (1024 * 1024)
            ),
        ));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    std::io::Read::read_to_end(&mut file, &mut bytes).map_err(io)?;
    match String::from_utf8(bytes) {
        Ok(text) => Ok((text, Stamp::of(&meta))),
        Err(e) => Err((
            ENCODING_CODE,
            format!(
                "not valid UTF-8 (first bad byte at offset {})",
                e.utf8_error().valid_up_to()
            ),
        )),
    }
}

/// Replace the contents of the file at `path` with `new`, if it is still as
/// it was when it was read (`read`: its length and modification time). The
/// text goes to a temporary file in the same folder, with the mode of the
/// original, which is renamed over the file, so a reader sees the old text or
/// the new and a failed write leaves the file alone. A symbolic link is
/// followed: the file it points at is replaced and the link stays. A file
/// without write permission is not replaced.
pub(crate) fn write_back(path: &Path, read: Stamp, new: &str) -> Result<(), String> {
    let target = path
        .canonicalize()
        .map_err(|e| format!("cannot resolve the path: {e}"))?;
    let meta = std::fs::metadata(&target).map_err(|e| e.to_string())?;
    if Stamp::of(&meta) != read {
        return Err("changed since it was read; not written".to_string());
    }
    if meta.permissions().readonly() {
        return Err("is read-only; not written".to_string());
    }
    let dir = target.parent().ok_or("has no folder")?;
    let name = target
        .file_name()
        .ok_or("has no name")?
        .to_string_lossy()
        .into_owned();
    let tmp = dir.join(format!(".{name}.turtle-lsp-{}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(new.as_bytes())?;
        f.set_permissions(meta.permissions())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &target)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let d = std::env::temp_dir()
                .join(format!("turtle-lsp-files-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(real(&d))
        }

        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        }

        /// The folder's entries, to see that no temporary file stays.
        fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn stamp(path: &Path) -> Stamp {
        Stamp::of(&std::fs::metadata(path).unwrap())
    }

    #[test]
    fn cli_paths_are_shown_relative_with_slashes() {
        let base = std::env::temp_dir();
        assert_eq!(shown(&base.join("a").join("b.ttl"), &base), "a/b.ttl");
        assert_eq!(relative(&base, &base).as_deref(), Some("."));
    }

    #[test]
    fn write_back_replaces_the_text_and_leaves_no_temporary_file() {
        let d = Dir::new("replace");
        let f = d.write("a.ttl", b"old\r\n");
        write_back(&f, stamp(&f), "new\r\n").unwrap();
        // Bytes as given: line endings are not touched.
        assert_eq!(std::fs::read(&f).unwrap(), b"new\r\n");
        assert_eq!(d.names(), ["a.ttl"]);
    }

    #[test]
    fn write_back_refuses_a_file_that_changed_since_it_was_read() {
        let d = Dir::new("stale");
        let f = d.write("a.ttl", b"read this");
        let read = stamp(&f);
        // Another writer: same length, a later modification time.
        std::fs::write(&f, b"read that").unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let e = write_back(&f, read, "ours").unwrap_err();
        assert!(e.contains("changed since it was read"), "{e}");
        assert_eq!(std::fs::read(&f).unwrap(), b"read that");
        // Another length, the same time.
        let g = d.write("b.ttl", b"x");
        let read = stamp(&g);
        std::fs::write(&g, b"xyz").unwrap();
        assert!(write_back(&g, read, "ours").is_err());
        assert_eq!(std::fs::read(&g).unwrap(), b"xyz");
        assert_eq!(d.names(), ["a.ttl", "b.ttl"]);
    }

    #[cfg(unix)]
    #[test]
    fn write_back_keeps_the_mode_follows_a_link_and_leaves_a_read_only_file() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let d = Dir::new("modes");
        let real_file = d.write("real.ttl", b"old");
        std::fs::set_permissions(&real_file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = d.0.join("link.ttl");
        symlink(&real_file, &link).unwrap();
        write_back(&link, stamp(&link), "new").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read(&real_file).unwrap(), b"new");
        let mode = std::fs::metadata(&real_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
        // No write permission: not replaced, although the folder is writable.
        let ro = d.write("ro.ttl", b"old");
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444)).unwrap();
        let e = write_back(&ro, stamp(&ro), "new").unwrap_err();
        assert!(e.contains("read-only"), "{e}");
        assert_eq!(std::fs::read(&ro).unwrap(), b"old");
        assert_eq!(d.names(), ["link.ttl", "real.ttl", "ro.ttl"]);
    }

    #[cfg(unix)]
    #[test]
    fn write_back_in_a_folder_that_cannot_be_written_fails_without_a_trace() {
        use std::os::unix::fs::PermissionsExt;
        let d = Dir::new("folder");
        let f = d.write("a.ttl", b"old");
        let read = stamp(&f);
        std::fs::set_permissions(&d.0, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = write_back(&f, read, "new");
        std::fs::set_permissions(&d.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        // (A superuser can write anywhere; then there is nothing to check.)
        if let Err(e) = result {
            assert!(e.contains("cannot write"), "{e}");
            assert_eq!(std::fs::read(&f).unwrap(), b"old");
            assert_eq!(d.names(), ["a.ttl"]);
        }
    }

    #[test]
    fn read_text_tells_io_from_encoding() {
        let d = Dir::new("read");
        let ok = d.write("ok.ttl", "caf\u{e9}".as_bytes());
        assert_eq!(read_text(&ok).unwrap().0, "caf\u{e9}");
        let bad = d.write("bad.ttl", b"ok \xff\xfe");
        let (code, message) = read_text(&bad).unwrap_err();
        assert_eq!(code, ENCODING_CODE);
        assert!(message.contains("offset 3"), "{message}");
        let (code, _) = read_text(&d.0.join("missing.ttl")).unwrap_err();
        assert_eq!(code, IO_CODE);
        let (code, _) = read_text(&d.0).unwrap_err();
        assert_eq!(code, IO_CODE);
    }

    #[test]
    fn a_lenient_load_goes_on_past_a_file_that_cannot_be_read() {
        let d = Dir::new("lenient");
        d.write("a.ttl", b"<a> <b> <c> .\n");
        d.write("bad.ttl", b"<a> <b> \xff .\n");
        d.write("z.ttl", b"<d> <e> <f> .\n");
        let ws = load_lenient(d.0.clone(), d.0.clone(), &[], Context::Workspace).unwrap();
        let shown: Vec<&str> = ws.files.iter().map(|f| f.shown.as_str()).collect();
        assert_eq!(shown, ["a.ttl", "z.ttl"]);
        assert_eq!(ws.unread.len(), 1);
        assert_eq!(ws.unread[0].shown, "bad.ttl");
        assert_eq!(ws.unread[0].code, ENCODING_CODE);
        assert!(!ws.incomplete);
        // The strict load, for `query`, fails on it and says which.
        let e = load(d.0.clone(), d.0.clone(), &[], Context::None)
            .err()
            .unwrap();
        assert!(e.contains("bad.ttl") && e.contains("UTF-8"), "{e}");
    }
}
