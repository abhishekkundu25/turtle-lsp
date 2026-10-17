//! Client settings: the `turtle` section of the editor's configuration.
//!
//! Read from `initializationOptions` and from `workspace/didChangeConfiguration`.
//! Both nested (`{"inlayHints": {"labels": "all"}}`) and dotted
//! (`{"inlayHints.labels": "all"}`) keys are accepted. Unknown keys and values
//! of the wrong type are ignored, so a bad setting never stops the server.

use crate::syntax::PrefixMap;
use serde_json::Value;

/// Which terms get a label inlay hint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InlayLabels {
    /// Only terms whose IRI does not say what they are (e.g. `ex:P1234`).
    #[default]
    Opaque,
    All,
    Off,
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
    pub format_line_width: u32,
}

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
            format_line_width: 100,
        }
    }
}

impl Settings {
    /// Defaults, overridden by each layer in turn (initializationOptions, then
    /// the configuration section).
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
            ["format", "lineWidth"] => {
                if let Some(n) = v.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    self.format_line_width = n;
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
        assert_eq!(s.shacl_max_triples, 2_000_000);
        assert_eq!(s.format_line_width, 100);
        assert!(s.prefixes.is_empty());
    }

    #[test]
    fn nested_and_dotted_keys() {
        let nested = json!({
            "prefixes": {"ex:": "http://example.org/", "bad": 3},
            "inlayHints": {"labels": "all"},
            "shacl": {"enabled": false, "maxTriples": 10, "onType": false},
            "lint": {"undescribedTerms": false},
            "imports": {"network": true},
            "format": {"lineWidth": 80},
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
            "turtle.format.lineWidth": 80,
        });
        let a = Settings::from_layers([&nested]);
        assert_eq!(a, Settings::from_layers([&dotted]));
        assert_eq!(a.prefixes.len(), 1);
        assert_eq!(a.prefixes["ex"], "http://example.org/");
        assert_eq!(a.inlay_hint_labels, InlayLabels::All);
        assert!(!a.shacl_enabled && !a.shacl_on_type);
        assert!(!a.lint_undescribed_terms && a.imports_network);
        assert_eq!((a.shacl_max_triples, a.format_line_width), (10, 80));
    }

    #[test]
    fn later_layers_override_and_bad_values_are_ignored() {
        let init = json!({"prefixes": {"ex": "http://a/"}, "format": {"lineWidth": 120}});
        let config = json!({
            "inlayHints": {"labels": "sometimes"},
            "shacl": {"enabled": "no", "maxTriples": -1},
            "format": {"lineWidth": 90},
        });
        let s = Settings::from_layers([&init, &config]);
        assert_eq!(s.prefixes["ex"], "http://a/");
        assert_eq!(s.format_line_width, 90);
        assert_eq!(s.inlay_hint_labels, InlayLabels::Opaque);
        assert!(s.shacl_enabled);
        assert_eq!(s.shacl_max_triples, 2_000_000);
        assert_eq!(Settings::from_layers([&Value::Null]), Settings::default());
    }
}
