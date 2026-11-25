//! The features that write a document or offer text to type: formatting,
//! rename, completion and code actions. RDF/XML files (`.owl`, `.rdf`) keep
//! their editor's own XML language, and of the features that write only
//! rename (`features::renames`) and completion (`features::completes`)
//! edit them, so the editor should not offer
//! formatting or code actions there. Nor in OWL/XML files (`.owx`), which
//! editors keep in their XML language too, nor in OWL Functional files
//! (`.ofn`, the `ofn` language of VS Code and Neovim), which are read-only
//! too: [`LANGUAGES`] leaves `xml` and `ofn` out, and rename refuses a term
//! a Functional or OWL/XML document uses (one under a `.owl` or `.rdf` name
//! is in [`rename_selector`]'s pattern, and is refused there). Completion
//! is asked in all of them: its selector ([`completion_selector`]) adds
//! [`COMPLETED_LANGUAGES`], [`RDFXML_PATTERN`] and [`OWX_PATTERN`], and the
//! server answers in each by the syntax it reads (`features::completes`), so
//! a `.owl` that holds Turtle or OWL Functional syntax gets that syntax's
//! completion.
//!
//! By default the `initialize` result announces each one statically, for
//! every document. Of the features that write, the server answers only a
//! rename in an RDF/XML document and completion in an RDF/XML, OWL
//! Functional or OWL/XML document: no formatting edits and no code action
//! that edits one.
//! A client opts in to more with `"registerOutsideRdfXml": true` at the top
//! level of its `initializationOptions` ([`OPT_IN`]): each of the four
//! features that the client can register dynamically (`dynamicRegistration`)
//! is then left out of the result and registered once the client is
//! initialized, for [`selector`], which leaves RDF/XML, OWL/XML and OWL
//! Functional files out, rename for [`rename_selector`], which takes
//! RDF/XML files in, and completion for [`completion_selector`], which
//! takes all three in. VS Code opts in, so that it lists no turtle-lsp
//! formatter for XML files. The option is read once, at
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

/// The language ids of the documents the server reads, never writes, and
/// completes: `ofn`, OWL Functional syntax, where an accepted term may add
/// its `Prefix( … )` declaration (`features::completes`).
pub(crate) const COMPLETED_LANGUAGES: [&str; 1] = ["ofn"];

/// JSON-LD files keep the editor's JSON language: they are matched by name.
pub(crate) const JSONLD_PATTERN: &str = "**/*.jsonld";

/// RDF/XML files keep the editor's XML language: they are matched by name.
/// VS Code's client keeps a registered filter by this pattern
/// (`RDFXML_PATTERN` in `editors/vscode/src/client.ts`, which a test checks).
pub(crate) const RDFXML_PATTERN: &str = "**/*.{owl,rdf}";

/// OWL/XML files keep the editor's XML language too: they are matched by
/// name. VS Code's client keeps a registered filter by this pattern
/// (`OWX_PATTERN` in `editors/vscode/src/client.ts`, which a test checks).
pub(crate) const OWX_PATTERN: &str = "**/*.owx";

/// The documents the registered features apply to: every language of
/// [`LANGUAGES`] and every `.jsonld` file, whatever its URI scheme.
pub(crate) fn selector() -> Vec<DocumentFilter> {
    LANGUAGES
        .iter()
        .map(|l| filter(Some(l), None))
        .chain([filter(None, Some(JSONLD_PATTERN))])
        .collect()
}

/// The documents completion is registered for: [`selector`]'s, those of
/// [`COMPLETED_LANGUAGES`], and RDF/XML and OWL/XML files by name.
pub(crate) fn completion_selector() -> Vec<DocumentFilter> {
    let mut out = selector();
    out.extend(COMPLETED_LANGUAGES.iter().map(|l| filter(Some(l), None)));
    out.extend([RDFXML_PATTERN, OWX_PATTERN].map(|p| filter(None, Some(p))));
    out
}

/// The documents rename applies to: [`selector`]'s, and RDF/XML files,
/// which a rename edits too (`rename.rdfXml`).
pub(crate) fn rename_selector() -> Vec<DocumentFilter> {
    let mut out = selector();
    out.push(filter(None, Some(RDFXML_PATTERN)));
    out
}

fn filter(language: Option<&str>, pattern: Option<&str>) -> DocumentFilter {
    DocumentFilter {
        language: language.map(str::to_string),
        scheme: None,
        pattern: pattern.map(str::to_string),
    }
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
    /// dynamic feature, each with [`selector`] ([`completion_selector`] for
    /// completion, [`rename_selector`] for rename) and the options its
    /// static capability would have.
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
            options["documentSelector"] = match method {
                "textDocument/completion" => json!(completion_selector()),
                "textDocument/rename" => json!(rename_selector()),
                _ => json!(selector()),
            };
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

    /// Formatting and code actions leave RDF/XML and OWL/XML files out;
    /// rename takes RDF/XML files too, and completion both, by VS Code's
    /// patterns for them.
    #[test]
    fn the_selector_leaves_xml_files_out_but_for_rename_and_completion() {
        let all = Dynamic {
            formatting: true,
            rename: true,
            completion: true,
            code_action: true,
        };
        for r in all.registrations() {
            let options = r.register_options.unwrap();
            let filters = options["documentSelector"].as_array().unwrap();
            // Completion's adds the OWL Functional language and both XML
            // patterns, rename's the RDF/XML pattern.
            let completion = r.method == "textDocument/completion";
            let rename = r.method == "textDocument/rename";
            let rdfxml = json!({"pattern": RDFXML_PATTERN});
            let owx = json!({"pattern": OWX_PATTERN});
            assert_eq!(
                filters.contains(&rdfxml),
                rename || completion,
                "{}",
                r.method
            );
            assert_eq!(filters.contains(&owx), completion, "{}", r.method);
            let added = if completion {
                COMPLETED_LANGUAGES.len() + 2
            } else {
                usize::from(rename)
            };
            assert_eq!(filters.len(), LANGUAGES.len() + 1 + added, "{}", r.method);
            for f in filters.iter().filter(|f| **f != rdfxml && **f != owx) {
                let text = f.to_string();
                assert!(!text.contains("owl") && !text.contains("rdf") && !text.contains("xml"));
                assert!(f.get("scheme").is_none(), "{text}");
            }
        }
        // The patterns VS Code's client matches.
        let client = include_str!("../../editors/vscode/src/client.ts");
        for (name, pattern) in [
            ("RDFXML_PATTERN", RDFXML_PATTERN),
            ("OWX_PATTERN", OWX_PATTERN),
        ] {
            assert!(
                client.contains(&format!("export const {name} = \"{pattern}\"")),
                "editors/vscode/src/client.ts: {name}"
            );
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
    /// extension and a Neovim file type. [`selector`] leaves them out;
    /// [`completion_selector`] has those that complete.
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
        // The read-only ones are in no registration but completion's, and
        // there only when they complete.
        let registered = serde_json::to_string(&selector()).unwrap();
        let completed = serde_json::to_string(&completion_selector()).unwrap();
        for id in READ_ONLY_LANGUAGES {
            assert!(!registered.contains(&format!("\"{id}\"")), "{registered}");
            let syntax = crate::syntax::Syntax::from_language_id(id).unwrap();
            assert_eq!(
                completed.contains(&format!("\"{id}\"")),
                crate::features::completes(syntax),
                "{completed}"
            );
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
