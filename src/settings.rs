//! Client settings: the `turtle` section of the editor's configuration.
//!
//! Read from `initializationOptions` and from `workspace/didChangeConfiguration`,
//! on top of an optional `turtle-lsp.json` at the workspace root (see
//! [`read_settings_file`]). Both nested (`{"inlayHints": {"labels": "all"}}`) and dotted
//! (`{"inlayHints.labels": "all"}`) keys are accepted. Unknown keys and values
//! of the wrong type are ignored, so a bad setting never stops the server.

use crate::syntax::PrefixMap;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// The workspace settings file, read from the workspace root.
pub const SETTINGS_FILE: &str = "turtle-lsp.json";

/// The settings in `<root>/turtle-lsp.json`: a JSON object with the keys of the
/// editor's `turtle` section, nested or dotted, optionally wrapped in a
/// top-level `"turtle"` object. `Ok(None)` when there is no such file; `Err`
/// with a message for the user when it cannot be read, is not JSON, or is not
/// an object.
pub fn read_settings_file(root: &Path) -> Result<Option<Value>, String> {
    let path = root.join(SETTINGS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(v @ Value::Object(_)) => Ok(Some(v)),
        Ok(_) => Err(format!("{}: expected a JSON object", path.display())),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Which terms get a label inlay hint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InlayLabels {
    /// Only terms whose IRI does not say what they are (e.g. `ex:P1234`).
    #[default]
    Opaque,
    All,
    Off,
}

/// How SHACL results treat `owl:imports` that are not loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnresolvedImports {
    /// Results the missing data could change are lowered to warnings and
    /// say so.
    #[default]
    Soften,
    /// As if the imports were empty: full severity, no note.
    Ignore,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Prefixes available in every file (on top of any `prefixes.ttl`).
    pub prefixes: PrefixMap,
    pub inlay_hint_labels: InlayLabels,
    pub shacl_enabled: bool,
    /// SHACL validation is skipped when the data has more triples than this.
    pub shacl_max_triples: u64,
    /// Validate about a second after typing stops, not only on save.
    pub shacl_on_type: bool,
    pub lint_undescribed_terms: bool,
    /// Fetch `owl:imports` over the network.
    pub imports_network: bool,
    pub imports_unresolved: UnresolvedImports,
    pub format_line_width: u32,
    /// "N references" lenses above the subjects a document describes.
    pub code_lens_references: bool,
    /// Diagnose the workspace files that are not open, in the background.
    pub diagnostics_workspace: bool,
    /// "Run query" stops a query after this long; `None` (0) for no limit.
    pub query_timeout: Option<Duration>,
    /// "Run query" keeps at most this many rows (or triples) of an answer;
    /// `None` (0) keeps all.
    pub query_max_rows: Option<usize>,
    /// "Run query" stops a query that keeps more than this many rows in
    /// memory (to sort, deduplicate, group or join them); `None` (0) for no
    /// limit.
    pub query_max_buffered_rows: Option<usize>,
}

/// The default `query.maxBufferedRows`: about 1 GB, at about 100 bytes a row.
pub const QUERY_MAX_BUFFERED_ROWS: usize = 10_000_000;

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            prefixes: PrefixMap::default(),
            inlay_hint_labels: InlayLabels::Opaque,
            shacl_enabled: true,
            shacl_max_triples: 2_000_000,
            shacl_on_type: true,
            lint_undescribed_terms: true,
            imports_network: false,
            imports_unresolved: UnresolvedImports::Soften,
            format_line_width: 100,
            code_lens_references: true,
            diagnostics_workspace: false,
            query_timeout: Some(Duration::from_secs(30)),
            query_max_rows: Some(1000),
            query_max_buffered_rows: Some(QUERY_MAX_BUFFERED_ROWS),
        }
    }
}

impl Settings {
    /// Defaults, overridden by each layer in turn (the settings file, then
    /// initializationOptions, then the configuration section).
    pub fn from_layers<'a>(layers: impl IntoIterator<Item = &'a Value>) -> Settings {
        let mut s = Settings::default();
        for layer in layers {
            s.apply(layer);
        }
        s
    }

    /// Override the settings present in `v`; the rest keep their value.
    pub fn apply(&mut self, v: &Value) {
        if let Value::Object(map) = v {
            for (k, v) in map {
                let path: Vec<&str> = k.split('.').collect();
                self.apply_at(&path, v);
            }
        }
    }

    fn apply_at(&mut self, path: &[&str], v: &Value) {
        match path {
            [] => self.apply(v),
            ["turtle", rest @ ..] => self.apply_at(rest, v),
            ["prefixes"] => {
                if let Value::Object(p) = v {
                    self.prefixes = p
                        .iter()
                        .filter_map(|(k, v)| Some((prefix_name(k), v.as_str()?.to_string())))
                        .collect();
                }
            }
            ["prefixes", name @ ..] => {
                if let Some(iri) = v.as_str() {
                    self.prefixes
                        .insert(prefix_name(&name.join(".")), iri.to_string());
                }
            }
            ["inlayHints", "labels"] => {
                if let Some(l) = v.as_str().and_then(parse_labels) {
                    self.inlay_hint_labels = l;
                }
            }
            ["shacl", "enabled"] => set_bool(&mut self.shacl_enabled, v),
            ["shacl", "maxTriples"] => {
                if let Some(n) = v.as_u64() {
                    self.shacl_max_triples = n;
                }
            }
            ["shacl", "onType"] => set_bool(&mut self.shacl_on_type, v),
            ["lint", "undescribedTerms"] => set_bool(&mut self.lint_undescribed_terms, v),
            ["imports", "network"] => set_bool(&mut self.imports_network, v),
            ["imports", "unresolved"] => {
                if let Some(u) = v.as_str().and_then(parse_unresolved) {
                    self.imports_unresolved = u;
                }
            }
            ["diagnostics", "workspace"] => set_bool(&mut self.diagnostics_workspace, v),
            ["format", "lineWidth"] => {
                if let Some(n) = v.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    self.format_line_width = n;
                }
            }
            ["codeLens", "references"] => set_bool(&mut self.code_lens_references, v),
            ["query", "timeoutSeconds"] => {
                if let Some(n) = v.as_f64().filter(|n| *n >= 0.0 && n.is_finite()) {
                    self.query_timeout = (n > 0.0).then(|| Duration::from_secs_f64(n));
                }
            }
            ["query", "maxRows"] => {
                if let Some(n) = v.as_u64().and_then(|n| usize::try_from(n).ok()) {
                    self.query_max_rows = (n > 0).then_some(n);
                }
            }
            ["query", "maxBufferedRows"] => {
                if let Some(n) = v.as_u64().and_then(|n| usize::try_from(n).ok()) {
                    self.query_max_buffered_rows = (n > 0).then_some(n);
                }
            }
            [head, rest @ ..] => {
                // A nested group, e.g. {"shacl": {"enabled": false}}.
                if let Value::Object(map) = v {
                    for (k, v) in map {
                        let mut p: Vec<&str> = vec![head];
                        p.extend_from_slice(rest);
                        p.extend(k.split('.'));
                        self.apply_at(&p, v);
                    }
                }
            }
        }
    }
}

fn prefix_name(k: &str) -> String {
    k.trim_end_matches(':').to_string()
}

fn parse_labels(s: &str) -> Option<InlayLabels> {
    match s {
        "opaque" => Some(InlayLabels::Opaque),
        "all" => Some(InlayLabels::All),
        "off" => Some(InlayLabels::Off),
        _ => None,
    }
}

fn parse_unresolved(s: &str) -> Option<UnresolvedImports> {
    match s {
        "soften" => Some(UnresolvedImports::Soften),
        "ignore" => Some(UnresolvedImports::Ignore),
        _ => None,
    }
}

fn set_bool(slot: &mut bool, v: &Value) {
    if let Some(b) = v.as_bool() {
        *slot = b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults() {
        let s = Settings::from_layers([]);
        assert_eq!(s.inlay_hint_labels, InlayLabels::Opaque);
        assert!(s.shacl_enabled && s.shacl_on_type);
        assert!(s.lint_undescribed_terms && !s.imports_network);
        assert_eq!(s.imports_unresolved, UnresolvedImports::Soften);
        assert_eq!(s.shacl_max_triples, 2_000_000);
        assert_eq!(s.format_line_width, 100);
        assert!(s.code_lens_references);
        assert!(!s.diagnostics_workspace);
        assert_eq!(s.query_timeout, Some(Duration::from_secs(30)));
        assert_eq!(s.query_max_rows, Some(1000));
        assert_eq!(s.query_max_buffered_rows, Some(10_000_000));
        assert!(s.prefixes.is_empty());
    }

    #[test]
    fn nested_and_dotted_keys() {
        let nested = json!({
            "prefixes": {"ex:": "http://example.org/", "bad": 3},
            "inlayHints": {"labels": "all"},
            "shacl": {"enabled": false, "maxTriples": 10, "onType": false},
            "lint": {"undescribedTerms": false},
            "imports": {"network": true, "unresolved": "ignore"},
            "format": {"lineWidth": 80},
            "codeLens": {"references": false},
            "diagnostics": {"workspace": true},
            "query": {"timeoutSeconds": 2.5, "maxRows": 0, "maxBufferedRows": 0},
            "server": {"path": "/ignored"},
        });
        let dotted = json!({
            "prefixes.ex": "http://example.org/",
            "inlayHints.labels": "all",
            "shacl.enabled": false,
            "shacl.maxTriples": 10,
            "shacl.onType": false,
            "lint": {"undescribedTerms": false},
            "imports.network": true,
            "imports.unresolved": "ignore",
            "turtle.format.lineWidth": 80,
            "codeLens.references": false,
            "diagnostics.workspace": true,
            "query.timeoutSeconds": 2.5,
            "query.maxRows": 0,
            "query.maxBufferedRows": 0,
        });
        let a = Settings::from_layers([&nested]);
        assert_eq!(a, Settings::from_layers([&dotted]));
        assert_eq!(a.prefixes.len(), 1);
        assert_eq!(a.prefixes["ex"], "http://example.org/");
        assert_eq!(a.inlay_hint_labels, InlayLabels::All);
        assert!(!a.shacl_enabled && !a.shacl_on_type);
        assert!(!a.lint_undescribed_terms && a.imports_network);
        assert_eq!(a.imports_unresolved, UnresolvedImports::Ignore);
        assert!(!a.code_lens_references);
        assert_eq!((a.shacl_max_triples, a.format_line_width), (10, 80));
        assert!(a.diagnostics_workspace);
        assert_eq!(a.query_timeout, Some(Duration::from_millis(2500)));
        assert_eq!((a.query_max_rows, a.query_max_buffered_rows), (None, None));
        let off = Settings::from_layers([
            &json!({"query": {"timeoutSeconds": 0, "maxRows": 5, "maxBufferedRows": 500}}),
        ]);
        assert_eq!((off.query_timeout, off.query_max_rows), (None, Some(5)));
        assert_eq!(off.query_max_buffered_rows, Some(500));
    }

    #[test]
    fn later_layers_override_and_bad_values_are_ignored() {
        let init = json!({"prefixes": {"ex": "http://a/"}, "format": {"lineWidth": 120}});
        let config = json!({
            "inlayHints": {"labels": "sometimes"},
            "shacl": {"enabled": "no", "maxTriples": -1},
            "imports": {"unresolved": "drop"},
            "format": {"lineWidth": 90},
            "query": {"timeoutSeconds": -1, "maxRows": 2.5, "maxBufferedRows": -1},
        });
        let s = Settings::from_layers([&init, &config]);
        assert_eq!(s.prefixes["ex"], "http://a/");
        assert_eq!(s.format_line_width, 90);
        assert_eq!(s.inlay_hint_labels, InlayLabels::Opaque);
        assert!(s.shacl_enabled);
        assert_eq!(s.shacl_max_triples, 2_000_000);
        assert_eq!(s.imports_unresolved, UnresolvedImports::Soften);
        assert_eq!(s.query_timeout, Some(Duration::from_secs(30)));
        assert_eq!(s.query_max_rows, Some(1000));
        assert_eq!(s.query_max_buffered_rows, Some(10_000_000));
        for bad in [json!(2.5), json!("lots"), json!(true)] {
            let s = Settings::from_layers([&json!({"query": {"maxBufferedRows": bad}})]);
            assert_eq!(s.query_max_buffered_rows, Some(10_000_000), "{bad}");
        }
        assert_eq!(Settings::from_layers([&Value::Null]), Settings::default());
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("turtle-lsp-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn settings_file_absent_valid_and_wrapped() {
        let dir = temp_root("ok");
        assert_eq!(read_settings_file(&dir), Ok(None));

        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"lint.undescribedTerms": false, "prefixes": {"ex": "http://example.org/"}}"#,
        )
        .unwrap();
        let v = read_settings_file(&dir).unwrap().unwrap();
        let s = Settings::from_layers([&v]);
        assert!(!s.lint_undescribed_terms);
        assert_eq!(s.prefixes["ex"], "http://example.org/");

        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"turtle": {"format": {"lineWidth": 80}}}"#,
        )
        .unwrap();
        let v = read_settings_file(&dir).unwrap().unwrap();
        assert_eq!(Settings::from_layers([&v]).format_line_width, 80);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_file_invalid() {
        let dir = temp_root("bad");
        std::fs::write(dir.join(SETTINGS_FILE), "{ not json").unwrap();
        let e = read_settings_file(&dir).unwrap_err();
        assert!(e.contains(SETTINGS_FILE), "{e}");
        std::fs::write(dir.join(SETTINGS_FILE), "[1, 2]").unwrap();
        let e = read_settings_file(&dir).unwrap_err();
        assert!(e.contains("expected a JSON object"), "{e}");
        // A directory by that name cannot be read.
        std::fs::remove_file(dir.join(SETTINGS_FILE)).unwrap();
        std::fs::create_dir(dir.join(SETTINGS_FILE)).unwrap();
        assert!(read_settings_file(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_file_is_the_lowest_layer() {
        let file = json!({"lint": {"undescribedTerms": false}, "format": {"lineWidth": 80}});
        let init = json!({"lint": {"undescribedTerms": true}});
        let s = Settings::from_layers([&file, &init]);
        assert!(s.lint_undescribed_terms);
        assert_eq!(s.format_line_width, 80);
    }
}
