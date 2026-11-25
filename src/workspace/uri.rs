//! URIs and paths: a `file:` URI and the file it names, one spelling per
//! file, and paths with symbolic links resolved.

use std::path::{Path, PathBuf};

/// The file a `file:` URL names. None in the browser build, which has no files.
pub(crate) fn file_path_of(u: &url::Url) -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    return u.to_file_path().ok();
    #[cfg(target_arch = "wasm32")]
    {
        let _ = u;
        None
    }
}

/// The `file:` URL of `path` (of the directory `path` when `dir`). None in
/// the browser build.
pub(crate) fn file_url_of(path: &Path, dir: bool) -> Option<url::Url> {
    #[cfg(not(target_arch = "wasm32"))]
    return if dir {
        url::Url::from_directory_path(path).ok()
    } else {
        url::Url::from_file_path(path).ok()
    };
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (path, dir);
        None
    }
}

/// The file name a `file:` URI ends in, decoded, without reading any file
/// system (the browser build has none).
pub(crate) fn file_uri_name(uri: &str) -> Option<String> {
    let u = url::Url::parse(uri).ok().filter(|u| u.scheme() == "file")?;
    let last = u.path_segments()?.next_back().filter(|s| !s.is_empty())?;
    let bytes = last.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = file_path_of(&url::Url::parse(uri).ok()?)?;
    Some(normalize_drive(path))
}

/// Windows: `c:\x` and `C:\x` are the same file; use the upper-case drive letter.
fn normalize_drive(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    let b = s.as_bytes();
    if cfg!(windows) && b.len() >= 2 && b[1] == b':' && b[0].is_ascii_lowercase() {
        let mut fixed = s.into_owned();
        fixed[..1].make_ascii_uppercase();
        return PathBuf::from(fixed);
    }
    path
}

/// One spelling per file. Clients differ (`file:///c%3A/x` from VS Code on
/// Windows, `file:///C:/x` elsewhere); every document is keyed by this form.
pub(crate) fn normalize_uri(uri: &str) -> String {
    if !uri.starts_with("file:") {
        return uri.to_string();
    }
    uri_to_path(uri)
        .and_then(|p| path_to_uri(&p))
        .unwrap_or_else(|| uri.to_string())
}

/// The `file:` URI of `path`, one the protocol's `Uri` parses.
pub(crate) fn path_to_uri(path: &Path) -> Option<String> {
    file_url_of(path, false).map(|u| as_uri(u.as_str()).into_owned())
}

/// `s`, a URL or an IRI, as a URI (RFC 3986): each character a URI may not
/// hold becomes the percent escapes of its UTF-8 bytes (RFC 3987, 3.1). The
/// `url` crate leaves the `[`, `]`, `^` and `|` of a path as they are, which
/// the URL standard allows, and an IRI may hold letters outside ASCII. The
/// protocol's `Uri` refuses both, and VS Code sends them escaped. `[` and
/// `]` stay where an IPv6 host has them, and `%` where it starts an escape.
pub(crate) fn as_uri(s: &str) -> std::borrow::Cow<'_, str> {
    use std::fmt::Write;
    let host_end = s.find("://").map_or(0, |i| {
        let rest = &s[i + 3..];
        i + 3 + rest.find(['/', '?', '#']).unwrap_or(rest.len())
    });
    let kept = |i: usize, c: char| match c {
        '[' | ']' => i < host_end,
        '%' => s
            .as_bytes()
            .get(i + 1..i + 3)
            .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)),
        c => c.is_ascii_alphanumeric() || "-._~:/?#@!$&'()*+,;=".contains(c),
    };
    if s.char_indices().all(|(i, c)| kept(i, c)) {
        return s.into();
    }
    let mut out = String::with_capacity(s.len() + 16);
    for (i, c) in s.char_indices() {
        if kept(i, c) {
            out.push(c);
        } else {
            for b in c.encode_utf8(&mut [0; 4]).bytes() {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out.into()
}

/// `path` without symbolic links (and on Windows without `\\?\`), or as it is
/// when it cannot be resolved.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn real(path: &Path) -> PathBuf {
    path.canonicalize()
        .map_or_else(|_| path.to_path_buf(), without_verbatim)
}

/// On Windows, `canonicalize` writes `\\?\C:\...`; paths from URIs are
/// `C:\...`. A `\\?\UNC\` share keeps its form.
fn without_verbatim(p: PathBuf) -> PathBuf {
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

/// `path` with symlinks resolved, as far as it exists (`/var` vs `/private/var`).
pub(crate) fn canonical(path: &Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return without_verbatim(p);
    }
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) => canonical(dir).join(name),
        _ => path.to_path_buf(),
    }
}
