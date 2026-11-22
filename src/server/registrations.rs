//! The features that write a document or offer text to type: formatting,
//! rename, completion and code actions. RDF/XML files (`.owl`, `.rdf`) are
//! read-only, and their editor keeps its own XML language, so the editor
//! should not offer these features there. Nor in OWL Functional files
//! (`.ofn`, the `ofn` language of VS Code and Neovim), which are read-only
//! too: [`LANGUAGES`] leaves `ofn` out.
//!
//! By default the `initialize` result announces each one statically, for
//! every document, and the server answers nothing that writes an RDF/XML
//! document (no edits, no completions; rename says why). A client opts in to
//! more with `"registerOutsideRdfXml": true` at the top level of its
//! `initializationOptions` ([`OPT_IN`]): each of the four features that the
//! client can register dynamically (`dynamicRegistration`) is then left out
//! of the result and registered once the client is initialized, for
//! [`selector`], which leaves RDF/XML and OWL Functional files out. VS Code opts in, so that it
//! lists no turtle-lsp formatter for XML files. The option is read once, at
//! `initialize`, and is not a setting: the settings ignore the key, and a
//! configuration section that replaces `initializationOptions` later does
//! not change it.
//!
//! The opt-in is needed because a dynamic registration is a change for any
//! client: one whose language ids are not [`LANGUAGES`] loses the features,
//! and Neovim 0.11 sends no `prepareRename` for a dynamically registered
//! rename.

use lsp_types::{
    CodeActionKind, CodeActionOptions, CompletionOptions, DocumentFilter, Registration,
    RenameOptions,
};
use serde_json::{Value, json};

/// The `initializationOptions` key with which a client asks for the
/// dynamic registrations.
pub(crate) const OPT_IN: &str = "registerOutsideRdfXml";

/// The language ids of the documents the server writes: the languages of
/// VS Code's extension and Neovim's file types (`lsp/turtle_lsp.lua`), which
/// tests check.
pub(crate) const LANGUAGES: [&str; 6] =
    ["turtle", "trig", "ntriples", "nquads", "sparql", "jsonld"];

/// JSON-LD files keep the editor's JSON language: they are matched by name.
pub(crate) const JSONLD_PATTERN: &str = "**/*.jsonld";

/// The documents the registered features apply to: every language of
/// [`LANGUAGES`] and every `.jsonld` file, whatever its URI scheme.
pub(crate) fn selector() -> Vec<DocumentFilter> {
    let filter = |language: Option<&str>, pattern: Option<&str>| DocumentFilter {
        language: language.map(str::to_string),
        scheme: None,
        pattern: pattern.map(str::to_string),
    };
    LANGUAGES
        .iter()
        .map(|l| filter(Some(l), None))
        .chain([filter(None, Some(JSONLD_PATTERN))])
        .collect()
}

pub(crate) fn completion() -> CompletionOptions {
    CompletionOptions {
        trigger_characters: Some(vec![":".into(), "@".into(), "?".into(), "$".into()]),
        resolve_provider: Some(true),
        ..Default::default()
    }
}

pub(crate) fn rename() -> RenameOptions {
    RenameOptions {
        prepare_provider: Some(true),
        work_done_progress_options: Default::default(),
    }
}

pub(crate) fn code_action() -> CodeActionOptions {
    CodeActionOptions {
        code_action_kinds: Some(vec![
            CodeActionKind::QUICKFIX,
            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
            CodeActionKind::REFACTOR_REWRITE,
        ]),
        resolve_provider: Some(true),
        ..Default::default()
    }
}

/// Which of the features the server registers dynamically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Dynamic {
    pub(crate) formatting: bool,
    pub(crate) rename: bool,
    pub(crate) completion: bool,
    pub(crate) code_action: bool,
}

impl Dynamic {
    /// As the client's `initialize` params (`raw`) declare: none without
    /// the opt-in ([`OPT_IN`] set to `true`), else those the client can
    /// register dynamically.
    pub(crate) fn of(raw: &Value) -> Dynamic {
        let opted_in = raw
            .pointer(&format!("/initializationOptions/{OPT_IN}"))
            .and_then(Value::as_bool)
            == Some(true);
        if !opted_in {
            return Dynamic::default();
        }
        let dynamic = |feature: &str| {
            raw.pointer(&format!(
                "/capabilities/textDocument/{feature}/dynamicRegistration"
            ))
            .and_then(Value::as_bool)
                == Some(true)
        };
        Dynamic {
            formatting: dynamic("formatting"),
            rename: dynamic("rename"),
            completion: dynamic("completion"),
            code_action: dynamic("codeAction"),
        }
    }

    /// The registrations to send once the client is initialized: one per
    /// dynamic feature, each with [`selector`] and the options its static
    /// capability would have.
    pub(crate) fn registrations(self) -> Vec<Registration> {
        [
            (
                self.formatting,
                "turtle-formatting",
                "textDocument/formatting",
                json!({}),
            ),
            (
                self.rename,
                "turtle-rename",
                "textDocument/rename",
                json!(rename()),
            ),
            (
                self.completion,
                "turtle-completion",
                "textDocument/completion",
                json!(completion()),
            ),
            (
                self.code_action,
                "turtle-code-action",
                "textDocument/codeAction",
                json!(code_action()),
            ),
        ]
        .into_iter()
        .filter(|(on, ..)| *on)
        .map(|(_, id, method, mut options)| {
            options["documentSelector"] = json!(selector());
            Registration {
                id: id.into(),
                method: method.into(),
                register_options: Some(options),
            }
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_feature_registers_only_when_the_client_opts_in_and_registers_it() {
        let caps = json!({"textDocument": {
            "formatting": {"dynamicRegistration": true},
            "rename": {"dynamicRegistration": false},
            "completion": {"dynamicRegistration": true},
        }});
        let raw = json!({"capabilities": caps, "initializationOptions": {OPT_IN: true}});
        let d = Dynamic::of(&raw);
        assert_eq!(
            d,
            Dynamic {
                formatting: true,
                rename: false,
                completion: true,
                code_action: false
            }
        );
        let methods: Vec<_> = d.registrations().into_iter().map(|r| r.method).collect();
        assert_eq!(
            methods,
            ["textDocument/formatting", "textDocument/completion"]
        );
        assert!(Dynamic::default().registrations().is_empty());
    }

    /// Without the opt-in, or with anything but `true` for it, nothing is
    /// registered, whatever the client can register.
    #[test]
    fn without_the_opt_in_nothing_is_registered() {
        let caps = json!({"textDocument": {
            "formatting": {"dynamicRegistration": true},
            "rename": {"dynamicRegistration": true},
            "completion": {"dynamicRegistration": true},
            "codeAction": {"dynamicRegistration": true},
        }});
        for options in [
            None,
            Some(json!(null)),
            Some(json!({})),
            Some(json!({OPT_IN: false})),
            Some(json!({OPT_IN: "true"})),
            Some(json!({"turtle": {OPT_IN: true}})),
            Some(json!({"inlayHints.labels": "all"})),
        ] {
            let mut raw = json!({"capabilities": caps});
            if let Some(o) = options {
                raw["initializationOptions"] = o;
            }
            assert_eq!(Dynamic::of(&raw), Dynamic::default(), "{raw}");
        }
    }

    /// The opt-in sits among the settings in `initializationOptions`, which
    /// read it as no setting.
    #[test]
    fn the_settings_ignore_the_opt_in() {
        use crate::settings::{Layer, Settings};
        let settings = json!({"inlayHints": {"labels": "all"}, "shacl.enabled": false});
        let mut with = settings.clone();
        with[OPT_IN] = json!(true);
        let (plain, ignored) = Settings::layered([(Layer::Client, &settings)]);
        let (opted, ignored_with) = Settings::layered([(Layer::Client, &with)]);
        assert_eq!(opted, plain);
        assert_eq!(ignored_with, ignored);
        assert!(ignored_with.is_empty());
    }

    #[test]
    fn the_selector_leaves_rdfxml_files_out() {
        let all = Dynamic {
            formatting: true,
            rename: true,
            completion: true,
            code_action: true,
        };
        for r in all.registrations() {
            let options = r.register_options.unwrap();
            let filters = options["documentSelector"].as_array().unwrap();
            assert_eq!(filters.len(), LANGUAGES.len() + 1, "{}", r.method);
            for f in filters {
                let text = f.to_string();
                assert!(!text.contains("owl") && !text.contains("rdf") && !text.contains("xml"));
                assert!(f.get("scheme").is_none(), "{text}");
            }
        }
        // The options are those of the static capabilities.
        let rename = all.registrations().remove(1).register_options.unwrap();
        assert_eq!(rename["prepareProvider"], true);
        let completion = all.registrations().remove(2).register_options.unwrap();
        assert_eq!(completion["resolveProvider"], true);
        assert_eq!(completion["triggerCharacters"], json!([":", "@", "?", "$"]));
        let actions = all.registrations().remove(3).register_options.unwrap();
        assert_eq!(actions["resolveProvider"], true);
    }

    /// The language ids of the documents the server reads and never
    /// writes: `ofn`, OWL Functional syntax, a language of VS Code's
    /// extension and a Neovim file type. [`selector`] leaves them out.
    const READ_ONLY_LANGUAGES: [&str; 1] = ["ofn"];

    #[test]
    fn the_languages_are_neovims_file_types() {
        let config = include_str!("../../lsp/turtle_lsp.lua");
        let line = config
            .lines()
            .find(|l| l.trim_start().starts_with("filetypes"))
            .unwrap();
        let mut filetypes: Vec<_> = line.split('"').skip(1).step_by(2).collect();
        let mut languages = [&LANGUAGES[..], &READ_ONLY_LANGUAGES[..]].concat();
        // The read-only ones are in no registration.
        let registered = serde_json::to_string(&selector()).unwrap();
        for id in READ_ONLY_LANGUAGES {
            assert!(!registered.contains(&format!("\"{id}\"")), "{registered}");
            assert_eq!(
                crate::syntax::Syntax::from_language_id(id),
                Some(crate::syntax::Syntax::OwlFunctional)
            );
        }
        filetypes.sort_unstable();
        languages.sort_unstable();
        assert_eq!(filetypes, languages);
    }
}
