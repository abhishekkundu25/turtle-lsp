//! The settings and the layers they are made of: `turtle-lsp.json` at the
//! first workspace root, `initializationOptions` and the last `turtle`
//! configuration section, in that order over the defaults.
//!
//! The first configuration section the client sends (in
//! `didChangeConfiguration` or a `workspace/configuration` answer) replaces
//! `initializationOptions`: the client's latest view of its settings
//! supersedes its start-up snapshot, so a key the user removed falls back to
//! `turtle-lsp.json` or the default.
//!
//! A change to a layer is acted on in two steps: [`Layers::relayer`] says
//! what the new settings change, the server acts on that, and
//! [`Layers::commit`] makes them current.

use super::client::Client;
use crate::settings::{Layer, SETTINGS_FILE, Settings, read_settings_file};
use lsp_server::RequestId;
use lsp_types::request::Request as _;
use lsp_types::{ConfigurationItem, ConfigurationParams};
use serde_json::Value;
use std::path::PathBuf;

pub(super) struct Layers {
    /// `turtle-lsp.json` (Null when absent or invalid).
    file: Value,
    /// The warning about `turtle-lsp.json` at startup (unusable, or keys it
    /// may not set), shown once started.
    file_error: Option<String>,
    init_options: Value,
    /// The last `turtle` configuration section.
    config: Value,
    /// The defaults overridden by the layers.
    settings: Settings,
    /// The client can answer `workspace/configuration`.
    config_pull: bool,
    /// The pending `workspace/configuration` request.
    config_request: Option<RequestId>,
}

/// What new settings change, before they are made current.
pub(super) struct SettingsChange {
    settings: Settings,
    prefixes: bool,
    network: bool,
}

impl SettingsChange {
    /// The new settings.
    pub(super) fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The `prefixes` setting changed.
    pub(super) fn prefixes(&self) -> bool {
        self.prefixes
    }

    /// `imports.network` changed.
    pub(super) fn network(&self) -> bool {
        self.network
    }
}

impl Layers {
    /// The layers at startup: the settings file (and its warning) and the
    /// client's `initializationOptions`.
    pub(super) fn new(
        (file, file_error): (Value, Option<String>),
        init_options: Value,
        config_pull: bool,
    ) -> Layers {
        let settings =
            Settings::from_layers([(Layer::File, &file), (Layer::Client, &init_options)]);
        Layers {
            file,
            file_error,
            init_options,
            config: Value::Null,
            settings,
            config_pull,
            config_request: None,
        }
    }

    pub(super) fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The startup warning about `turtle-lsp.json`, once.
    pub(super) fn take_file_error(&mut self) -> Option<String> {
        self.file_error.take()
    }

    /// The client's `turtle` configuration section changed. It replaces
    /// `initializationOptions` as well as the previous section. False when
    /// there is no section (Null), and nothing changed.
    pub(super) fn set_config(&mut self, section: Value) -> bool {
        if section.is_null() {
            return false;
        }
        self.init_options = Value::Null;
        self.config = section;
        true
    }

    /// `turtle-lsp.json` changed on disk: read it again from `root`. An
    /// invalid file counts as absent; the warning to show, if any.
    pub(super) fn reload_file(&mut self, root: Option<&PathBuf>) -> Option<String> {
        let (file, error) = read_file(root);
        self.file = file;
        error
    }

    /// Ask the client for its `turtle` section, when it can answer.
    pub(super) fn request_config(&mut self, client: &mut Client) {
        if !self.config_pull {
            return;
        }
        let params = ConfigurationParams {
            items: vec![ConfigurationItem {
                scope_uri: None,
                section: Some("turtle".into()),
            }],
        };
        let id = client.request(lsp_types::request::WorkspaceConfiguration::METHOD, params);
        self.config_request = Some(id);
    }

    /// The client answered request `id`: whether it is the pending
    /// `workspace/configuration` request (which is no longer pending).
    pub(super) fn config_answered(&mut self, id: &RequestId) -> bool {
        if self.config_request.as_ref() != Some(id) {
            return false;
        }
        self.config_request = None;
        true
    }

    /// The settings recomputed from the layers, when they differ from the
    /// current ones: what they change. Nothing is current until
    /// [`Layers::commit`].
    pub(super) fn relayer(&self) -> Option<SettingsChange> {
        let new = Settings::from_layers([
            (Layer::File, &self.file),
            (Layer::Client, &self.init_options),
            (Layer::Client, &self.config),
        ]);
        if new == self.settings {
            return None;
        }
        Some(SettingsChange {
            prefixes: new.prefixes != self.settings.prefixes,
            network: new.imports_network != self.settings.imports_network,
            settings: new,
        })
    }

    /// Make the settings of `change` current.
    pub(super) fn commit(&mut self, change: SettingsChange) {
        self.settings = change.settings;
    }
}

/// The settings layer from `turtle-lsp.json` at `root`, and the warning to
/// show for it, if any: Null when there is no root, no file, or a bad file;
/// the warning names the keys of a good file that it may not set.
pub(super) fn read_file(root: Option<&PathBuf>) -> (Value, Option<String>) {
    match root.map(|r| read_settings_file(r)) {
        Some(Ok(Some(v))) => {
            let (_, ignored) = Settings::layered([(Layer::File, &v)]);
            let warning = ignored
                .message(SETTINGS_FILE)
                .map(|m| format!("turtle-lsp: {m}"));
            (v, warning)
        }
        Some(Err(e)) => (
            Value::Null,
            Some(format!("turtle-lsp: {e}; its settings are ignored")),
        ),
        _ => (Value::Null, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Set `section` and make what it changes current; what it changed.
    fn configure(l: &mut Layers, section: Value) -> Option<(bool, bool)> {
        if !l.set_config(section) {
            return None;
        }
        let change = l.relayer()?;
        let what = (change.prefixes(), change.network());
        l.commit(change);
        Some(what)
    }

    #[test]
    fn a_configuration_section_replaces_the_initialization_options() {
        let mut l = Layers::new(
            (Value::Null, None),
            json!({"format": {"lineWidth": 80}}),
            false,
        );
        assert_eq!(l.settings().format_line_width, 80);
        configure(&mut l, json!({"codeLens": {"references": false}}));
        assert_eq!(l.settings().format_line_width, 100);
        assert!(!l.settings().code_lens_references);

        // Under the section, the settings file shows through.
        let file = (json!({"format": {"lineWidth": 90}}), None);
        let mut l = Layers::new(file, json!({"format": {"lineWidth": 80}}), false);
        assert_eq!(l.settings().format_line_width, 80);
        configure(&mut l, json!({}));
        assert_eq!(l.settings().format_line_width, 90);
        // No section: the client keeps what it sent.
        let mut l = Layers::new(
            (Value::Null, None),
            json!({"format": {"lineWidth": 80}}),
            false,
        );
        assert_eq!(configure(&mut l, Value::Null), None);
        assert_eq!(l.settings().format_line_width, 80);
    }

    #[test]
    fn relayer_reports_what_changed_and_commits_nothing_itself() {
        let mut l = Layers::new((Value::Null, None), Value::Null, false);
        assert!(l.relayer().is_none(), "nothing changed");
        assert!(l.set_config(json!({"prefixes": {"ex": "http://example.org/"}})));
        let change = l.relayer().expect("a change");
        assert!(change.prefixes() && !change.network());
        assert_eq!(
            change.settings().prefixes.get("ex").map(String::as_str),
            Some("http://example.org/")
        );
        assert!(l.settings().prefixes.is_empty(), "not current yet");
        l.commit(change);
        assert!(!l.settings().prefixes.is_empty());
        assert!(l.relayer().is_none());
        // Another setting is a change that is neither.
        let change = configure(
            &mut l,
            json!({"prefixes": {"ex": "http://example.org/"}, "format": {"lineWidth": 70}}),
        );
        assert_eq!(change, Some((false, false)));
        let change = configure(&mut l, json!({"imports": {"network": true}}));
        assert_eq!(change, Some((true, true)));
    }

    #[test]
    fn the_configuration_request_is_sent_only_to_a_client_that_answers_it() {
        let (tx, rx) = crate::server::wire::memory();
        let mut client = Client::new(tx);
        let mut l = Layers::new((Value::Null, None), Value::Null, false);
        l.request_config(&mut client);
        assert!(rx.try_iter().next().is_none());
        let mut l = Layers::new((Value::Null, None), Value::Null, true);
        l.request_config(&mut client);
        let id = match rx.try_iter().next().unwrap() {
            lsp_server::Message::Request(r) => {
                assert_eq!(r.method, "workspace/configuration");
                r.id
            }
            other => panic!("{other:?}"),
        };
        assert!(!l.config_answered(&RequestId::from(99)));
        assert!(l.config_answered(&id));
        assert!(!l.config_answered(&id), "answered once");
    }
}
