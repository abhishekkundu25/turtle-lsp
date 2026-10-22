//! Client settings: the `turtle` section of the editor's configuration.
//!
//! Read from `initializationOptions` and from `workspace/didChangeConfiguration`,
//! on top of an optional `turtle-lsp.json` at the workspace root (see
//! [`read_settings_file`]). Both nested (`{"inlayHints": {"labels": "all"}}`) and dotted
//! (`{"inlayHints.labels": "all"}`) keys are accepted. Unknown keys and values
//! of the wrong type are ignored, so a bad setting never stops the server.
//!
//! The settings file is untrusted: a cloned repository ships it. It cannot turn
//! on network fetching or lift a resource limit above the defaults (see
//! [`Layer::File`]); what it tried is reported in [`Ignored`].

use crate::syntax::PrefixMap;
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

/// The workspace settings file, read from the workspace root.
pub const SETTINGS_FILE: &str = "turtle-lsp.json";

/// The largest `turtle-lsp.json` that is read.
pub const SETTINGS_FILE_MAX_BYTES: u64 = 1 << 20;

/// The settings in `<root>/turtle-lsp.json`: a JSON object with the keys of the
/// editor's `turtle` section, nested or dotted, optionally wrapped in a
/// top-level `"turtle"` object. `Ok(None)` when there is no such file; `Err`
/// with a message for the user when it cannot be read, is larger than 1 MB, is
/// not JSON, or is not an object.
pub fn read_settings_file(root: &Path) -> Result<Option<Value>, String> {
    let path = root.join(SETTINGS_FILE);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(SETTINGS_FILE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > SETTINGS_FILE_MAX_BYTES {
        return Err(format!(
            "{}: larger than {} MB",
            path.display(),
            SETTINGS_FILE_MAX_BYTES >> 20
        ));
    }
    let text =
        String::from_utf8(bytes).map_err(|_| format!("{}: not valid UTF-8", path.display()))?;
    match serde_json::from_str::<Value>(&text) {
        Ok(v @ Value::Object(_)) => Ok(Some(v)),
        Ok(_) => Err(format!("{}: expected a JSON object", path.display())),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `n` seconds as a [`Duration`]; `None` when `n` is negative, NaN, infinite
/// or too large to hold (`Duration::from_secs_f64` panics on those).
pub fn seconds(n: f64) -> Option<Duration> {
    Duration::try_from_secs_f64(n).ok()
}

/// Where a settings layer comes from, which decides how far it is trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// `turtle-lsp.json`: any repository can ship one, so it cannot turn on
    /// `imports.network` or set `query.timeoutSeconds`, `query.maxRows`,
    /// `query.maxBufferedRows` or `shacl.maxTriples` beyond their defaults
    /// (0, "no limit", counts as beyond). A value past the default is
    /// replaced by the default.
    File,
    /// The editor (`initializationOptions`, the configuration section) and
    /// the command-line flags: the user's own choice.
    Client,
}

/// What the layers asked for and did not get.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ignored {
    /// Keys the settings file may not set that way.
    pub untrusted: Vec<&'static str>,
    /// Keys whose number of seconds is negative or too large to hold, with
    /// the value as written.
    pub out_of_range: Vec<(&'static str, String)>,
}

impl Ignored {
    pub fn is_empty(&self) -> bool {
        self.untrusted.is_empty() && self.out_of_range.is_empty()
    }

    /// One sentence for the user, naming the keys; `None` when nothing was
    /// ignored. `source` is the file name.
    pub fn message(&self, source: &str) -> Option<String> {
        let mut parts = Vec::new();
        if !self.untrusted.is_empty() {
            parts.push(format!(
                "ignored {}: a settings file cannot turn on network access or raise a limit \
                 past its default; set them in the editor or on the command line",
                self.untrusted.join(", ")
            ));
        }
        if !self.out_of_range.is_empty() {
            let keys: Vec<String> = self
                .out_of_range
                .iter()
                .map(|(k, v)| format!("{k} needs a number of seconds, not {v}"))
                .collect();
            parts.push(keys.join("; "));
        }
        (!parts.is_empty()).then(|| format!("{source}: {}", parts.join("; ")))
    }

    fn untrusted(&mut self, key: &'static str) {
        if !self.untrusted.contains(&key) {
            self.untrusted.push(key);
        }
    }

    fn out_of_range(&mut self, key: &'static str, v: &Value) {
        if !self.out_of_range.iter().any(|(k, _)| *k == key) {
            self.out_of_range.push((key, v.to_string()));
        }
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
/// The default `query.timeoutSeconds`.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// The default `query.maxRows`.
const QUERY_MAX_ROWS: usize = 1000;
/// The default `shacl.maxTriples`.
const SHACL_MAX_TRIPLES: u64 = 2_000_000;

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            prefixes: PrefixMap::default(),
            inlay_hint_labels: InlayLabels::Opaque,
            shacl_enabled: true,
            shacl_max_triples: SHACL_MAX_TRIPLES,
            shacl_on_type: true,
            lint_undescribed_terms: true,
            imports_network: false,
            imports_unresolved: UnresolvedImports::Soften,
            format_line_width: 100,
            code_lens_references: true,
            diagnostics_workspace: false,
            query_timeout: Some(QUERY_TIMEOUT),
            query_max_rows: Some(QUERY_MAX_ROWS),
            query_max_buffered_rows: Some(QUERY_MAX_BUFFERED_ROWS),
        }
    }
}

impl Settings {
    /// Defaults, overridden by each layer in turn (the settings file, then
    /// initializationOptions, then the configuration section).
    pub fn from_layers<'a>(layers: impl IntoIterator<Item = (Layer, &'a Value)>) -> Settings {
        Settings::layered(layers).0
    }

    /// [`Settings::from_layers`], and what the layers asked for and did not get.
    pub fn layered<'a>(
        layers: impl IntoIterator<Item = (Layer, &'a Value)>,
    ) -> (Settings, Ignored) {
        let mut s = Settings::default();
        let mut ignored = Ignored::default();
        for (layer, v) in layers {
            s.apply_layer(layer, v, &mut ignored);
        }
        (s, ignored)
    }

    /// Override the settings present in `v`, as the editor would; the rest
    /// keep their value.
    pub fn apply(&mut self, v: &Value) {
        self.apply_layer(Layer::Client, v, &mut Ignored::default());
    }

    fn apply_layer(&mut self, layer: Layer, v: &Value, ignored: &mut Ignored) {
        if let Value::Object(map) = v {
            for (k, v) in map {
                let path: Vec<&str> = k.split('.').collect();
                self.apply_at(layer, ignored, &path, v);
            }
        }
    }

    fn apply_at(&mut self, layer: Layer, ig: &mut Ignored, path: &[&str], v: &Value) {
        let file = layer == Layer::File;
        match path {
            [] => self.apply_layer(layer, v, ig),
            ["turtle", rest @ ..] => self.apply_at(layer, ig, rest, v),
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
                    if file && n > SHACL_MAX_TRIPLES {
                        ig.untrusted("shacl.maxTriples");
                    }
                    self.shacl_max_triples = if file { n.min(SHACL_MAX_TRIPLES) } else { n };
                }
            }
            ["shacl", "onType"] => set_bool(&mut self.shacl_on_type, v),
            ["lint", "undescribedTerms"] => set_bool(&mut self.lint_undescribed_terms, v),
            ["imports", "network"] => {
                if file {
                    if v.as_bool() == Some(true) {
                        ig.untrusted("imports.network");
                    }
                } else {
                    set_bool(&mut self.imports_network, v);
                }
            }
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
                let Some(n) = v.as_f64() else { return };
                let Some(d) = seconds(n) else {
                    ig.out_of_range("query.timeoutSeconds", v);
                    return;
                };
                let mut t = (!d.is_zero()).then_some(d);
                if file && t.is_none_or(|t| t > QUERY_TIMEOUT) {
                    ig.untrusted("query.timeoutSeconds");
                    t = Some(QUERY_TIMEOUT);
                }
                self.query_timeout = t;
            }
            ["query", "maxRows"] => {
                if let Some(n) = v.as_u64().and_then(|n| usize::try_from(n).ok()) {
                    let mut n = (n > 0).then_some(n);
                    if file && n.is_none_or(|n| n > QUERY_MAX_ROWS) {
                        ig.untrusted("query.maxRows");
                        n = Some(QUERY_MAX_ROWS);
                    }
                    self.query_max_rows = n;
                }
            }
            ["query", "maxBufferedRows"] => {
                if let Some(n) = v.as_u64().and_then(|n| usize::try_from(n).ok()) {
                    let mut n = (n > 0).then_some(n);
                    if file && n.is_none_or(|n| n > QUERY_MAX_BUFFERED_ROWS) {
                        ig.untrusted("query.maxBufferedRows");
                        n = Some(QUERY_MAX_BUFFERED_ROWS);
                    }
                    self.query_max_buffered_rows = n;
                }
            }
            [head, rest @ ..] => {
                // A nested group, e.g. {"shacl": {"enabled": false}}.
                if let Value::Object(map) = v {
                    for (k, v) in map {
                        let mut p: Vec<&str> = vec![head];
                        p.extend_from_slice(rest);
                        p.extend(k.split('.'));
                        self.apply_at(layer, ig, &p, v);
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
        let a = Settings::from_layers([(Layer::Client, &nested)]);
        assert_eq!(a, Settings::from_layers([(Layer::Client, &dotted)]));
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
        let off = Settings::from_layers([(
            Layer::Client,
            &json!({"query": {"timeoutSeconds": 0, "maxRows": 5, "maxBufferedRows": 500}}),
        )]);
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
        let s = Settings::from_layers([(Layer::Client, &init), (Layer::Client, &config)]);
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
            let s = Settings::from_layers([(
                Layer::Client,
                &json!({"query": {"maxBufferedRows": bad}}),
            )]);
            assert_eq!(s.query_max_buffered_rows, Some(10_000_000), "{bad}");
        }
        assert_eq!(
            Settings::from_layers([(Layer::Client, &Value::Null)]),
            Settings::default()
        );
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
        let s = Settings::from_layers([(Layer::File, &v)]);
        assert!(!s.lint_undescribed_terms);
        assert_eq!(s.prefixes["ex"], "http://example.org/");

        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"turtle": {"format": {"lineWidth": 80}}}"#,
        )
        .unwrap();
        let v = read_settings_file(&dir).unwrap().unwrap();
        assert_eq!(
            Settings::from_layers([(Layer::File, &v)]).format_line_width,
            80
        );
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
        let s = Settings::from_layers([(Layer::File, &file), (Layer::Client, &init)]);
        assert!(s.lint_undescribed_terms);
        assert_eq!(s.format_line_width, 80);
    }

    fn layered(layer: Layer, v: Value) -> (Settings, Ignored) {
        Settings::layered([(layer, &v)])
    }

    #[test]
    fn seconds_rejects_what_a_duration_cannot_hold() {
        assert_eq!(seconds(2.5), Some(Duration::from_millis(2500)));
        assert_eq!(seconds(0.0), Some(Duration::ZERO));
        assert_eq!(seconds(1e300), None);
        assert_eq!(seconds(f64::MAX), None);
        assert_eq!(seconds(f64::INFINITY), None);
        assert_eq!(seconds(f64::NAN), None);
        assert_eq!(seconds(-1.0), None);
    }

    #[test]
    fn an_out_of_range_timeout_is_ignored_and_named_not_a_panic() {
        for layer in [Layer::File, Layer::Client] {
            for bad in [json!(1e300), json!(-5), json!(1.9e19)] {
                let (s, ig) = layered(layer, json!({"query": {"timeoutSeconds": bad}}));
                assert_eq!(s.query_timeout, Some(Duration::from_secs(30)), "{bad}");
                assert_eq!(ig.untrusted, Vec::<&str>::new());
                assert_eq!(ig.out_of_range.len(), 1, "{bad}");
                assert_eq!(ig.out_of_range[0].0, "query.timeoutSeconds");
                let m = ig.message("turtle-lsp.json").unwrap();
                assert!(
                    m.contains("query.timeoutSeconds needs a number of seconds"),
                    "{m}"
                );
            }
        }
        // 0 is "no limit" for the editor.
        let (s, ig) = layered(Layer::Client, json!({"query.timeoutSeconds": 0}));
        assert_eq!(s.query_timeout, None);
        assert!(ig.is_empty());
    }

    #[test]
    fn the_settings_file_cannot_turn_on_the_network() {
        let (s, ig) = layered(Layer::File, json!({"imports": {"network": true}}));
        assert!(!s.imports_network);
        assert_eq!(ig.untrusted, ["imports.network"]);
        let (s, ig) = layered(Layer::File, json!({"turtle": {"imports.network": true}}));
        assert!(!s.imports_network && ig.untrusted == ["imports.network"]);
        // `false` asks for nothing.
        let (s, ig) = layered(Layer::File, json!({"imports": {"network": false}}));
        assert!(!s.imports_network && ig.is_empty());
        // The editor can.
        let (s, ig) = layered(Layer::Client, json!({"imports": {"network": true}}));
        assert!(s.imports_network && ig.is_empty());
    }

    #[test]
    fn the_settings_file_cannot_raise_a_limit_past_the_defaults() {
        let file = json!({
            "query": {"timeoutSeconds": 0, "maxRows": 0, "maxBufferedRows": 99_999_999_999u64},
            "shacl": {"maxTriples": 50_000_000},
        });
        let (s, ig) = layered(Layer::File, file.clone());
        let d = Settings::default();
        assert_eq!(s.query_timeout, d.query_timeout);
        assert_eq!(s.query_max_rows, d.query_max_rows);
        assert_eq!(s.query_max_buffered_rows, d.query_max_buffered_rows);
        assert_eq!(s.shacl_max_triples, d.shacl_max_triples);
        let mut keys = ig.untrusted.clone();
        keys.sort();
        assert_eq!(
            keys,
            [
                "query.maxBufferedRows",
                "query.maxRows",
                "query.timeoutSeconds",
                "shacl.maxTriples"
            ]
        );
        let m = ig.message("turtle-lsp.json").unwrap();
        assert!(m.starts_with("turtle-lsp.json: ignored "), "{m}");
        assert!(
            m.contains("shacl.maxTriples") && m.contains("query.maxRows"),
            "{m}"
        );
        // Past the defaults but not "unlimited".
        let (s, _) = layered(
            Layer::File,
            json!({"query.timeoutSeconds": 31, "query.maxRows": 1001}),
        );
        assert_eq!(
            (s.query_timeout, s.query_max_rows),
            (d.query_timeout, d.query_max_rows)
        );
        // The same values from the editor are honoured.
        let (s, ig) = layered(Layer::Client, file);
        assert_eq!((s.query_timeout, s.query_max_rows), (None, None));
        assert_eq!(s.query_max_buffered_rows, Some(99_999_999_999));
        assert_eq!(s.shacl_max_triples, 50_000_000);
        assert!(ig.is_empty());
    }

    #[test]
    fn the_settings_file_can_tighten_limits_and_the_editor_overrides_it() {
        let file = json!({
            "query": {"timeoutSeconds": 5, "maxRows": 10, "maxBufferedRows": 100},
            "shacl": {"maxTriples": 1000},
        });
        let (s, ig) = layered(Layer::File, file.clone());
        assert_eq!(s.query_timeout, Some(Duration::from_secs(5)));
        assert_eq!(
            (s.query_max_rows, s.query_max_buffered_rows),
            (Some(10), Some(100))
        );
        assert_eq!(s.shacl_max_triples, 1000);
        assert!(ig.is_empty());
        let editor = json!({"query": {"maxRows": 50_000}, "imports": {"network": true}});
        let s = Settings::from_layers([(Layer::File, &file), (Layer::Client, &editor)]);
        assert_eq!(s.query_max_rows, Some(50_000));
        assert!(s.imports_network);
    }

    #[test]
    fn the_settings_file_is_read_up_to_one_megabyte() {
        let dir = temp_root("big");
        let pad = " ".repeat(SETTINGS_FILE_MAX_BYTES as usize - 2);
        std::fs::write(dir.join(SETTINGS_FILE), format!("{{{pad}}}")).unwrap();
        assert!(read_settings_file(&dir).unwrap().is_some());
        std::fs::write(dir.join(SETTINGS_FILE), format!("{{ {pad}}}")).unwrap();
        let e = read_settings_file(&dir).unwrap_err();
        assert!(e.contains("larger than 1 MB"), "{e}");
        // Not UTF-8: an error, not a panic.
        std::fs::write(dir.join(SETTINGS_FILE), [b'{', 0xff, b'}']).unwrap();
        assert!(read_settings_file(&dir).unwrap_err().contains("UTF-8"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
