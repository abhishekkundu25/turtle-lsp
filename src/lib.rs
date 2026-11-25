//! A language server and command-line checker for RDF 1.2 (Turtle, TriG, N-Triples, N-Quads, JSON-LD, RDF/XML), OWL 2 ontologies in Functional syntax and OWL/XML, and SPARQL, with SHACL validation.
//!
//! # Modules
//!
//! `ARCHITECTURE.md`, at the root of the repository, has the module table
//! (what each module owns and which modules call it), the layer rules that
//! the `boundaries` test checks, and the files that hold one part each of
//! the larger modules. The module doc at the top of each file says the rest.
//!
//! `commands`, `diagnose`, `extensions`, `fetch`, `ide`, `layout`, `ofn`,
//! `owx`, `rdfxml`, `reasoning`, `vocab`, `worker` and `workspace` are
//! private to the crate: no consumer uses them.
//!
//! # Public interface
//!
//! The consumers are the browser engine (`web/engine`), the examples
//! (`examples/`), the integration tests (`tests/`, with `tests/support/`) and
//! the binary (`src/main.rs`). Below is every path they use, one per line,
//! with the consumers that use it. The list comes from a grep of their
//! sources for `turtle_lsp::`, both in `use` statements and inline, and for
//! paths through a module they imported. Methods, fields and enum variants of
//! the listed types are not listed. The list is a doctest, so `cargo test`
//! fails when one of the paths stops being public. An item that only
//! consumers use stays `pub`.
//!
//! ```
//! use turtle_lsp::VERSION as _; // web/engine
//! use turtle_lsp::analysis::Analysis as _; // tests
//! use turtle_lsp::analysis::Encoding as _; // tests
//! use turtle_lsp::analysis::ParsePath as _; // tests
//! use turtle_lsp::analysis::SparqlAnalysis as _; // tests
//! use turtle_lsp::batch::BatchFile as _; // tests, web/engine
//! use turtle_lsp::batch::Columns as _; // web/engine
//! use turtle_lsp::batch::Fix as _; // web/engine
//! use turtle_lsp::batch::FormatOutcome as _; // web/engine
//! use turtle_lsp::batch::Formatted as _; // web/engine
//! use turtle_lsp::batch::Options as _; // tests, web/engine
//! use turtle_lsp::batch::analyse as _; // tests, web/engine
//! use turtle_lsp::batch::fix_all as _; // tests, web/engine
//! use turtle_lsp::batch::query as _; // web/engine
//! use turtle_lsp::cli::USAGE as _; // main
//! use turtle_lsp::cli::run as _; // main
//! use turtle_lsp::clock::Instant as _; // web/engine
//! use turtle_lsp::clock::elapsed as _; // examples, tests
//! use turtle_lsp::features::completion::complete as _; // tests
//! use turtle_lsp::features::diagnostics::Options as _; // tests
//! use turtle_lsp::features::diagnostics::diagnostics as _; // tests
//! use turtle_lsp::features::diagnostics::diagnostics_with as _; // tests
//! use turtle_lsp::features::format::Options as _; // examples, tests
//! use turtle_lsp::features::format::format as _; // examples, tests
//! use turtle_lsp::features::format::formatting as _; // tests
//! use turtle_lsp::features::hierarchy::prepare as _; // tests
//! use turtle_lsp::features::hierarchy::subtypes as _; // tests
//! use turtle_lsp::features::hierarchy::supertypes as _; // tests
//! use turtle_lsp::features::hover::hover as _; // tests
//! use turtle_lsp::features::inlay::inlay_hints as _; // tests
//! use turtle_lsp::features::navigation::PrefixScope as _; // tests
//! use turtle_lsp::features::navigation::RdfXml as _; // tests
//! use turtle_lsp::features::navigation::definition as _; // tests
//! use turtle_lsp::features::navigation::highlights as _; // tests
//! use turtle_lsp::features::navigation::implementations as _; // tests
//! use turtle_lsp::features::navigation::prepare_rename as _; // tests
//! use turtle_lsp::features::navigation::references as _; // tests
//! use turtle_lsp::features::navigation::rename as _; // tests
//! use turtle_lsp::features::navigation::type_definition as _; // tests
//! use turtle_lsp::features::quickfix::code_actions as _; // tests
//! use turtle_lsp::features::refactor::apply as _; // tests
//! use turtle_lsp::features::refactor::code_actions as _; // tests
//! use turtle_lsp::features::refactor::edits_of as _; // tests
//! use turtle_lsp::features::semantic::full as _; // tests
//! use turtle_lsp::features::semantic::range as _; // tests
//! use turtle_lsp::features::shacl_validate::Job as _; // tests
//! use turtle_lsp::features::shacl_validate::run as _; // tests
//! use turtle_lsp::features::symbols::document_symbols as _; // tests
//! use turtle_lsp::features::symbols::folding_ranges as _; // tests
//! use turtle_lsp::features::symbols::workspace_symbols as _; // tests
//! use turtle_lsp::imports::Status as _; // tests
//! use turtle_lsp::imports::diagnostics_with as _; // tests
//! use turtle_lsp::imports::functional_to_ntriples as _; // tests
//! use turtle_lsp::imports::owl_xml_to_ntriples as _; // tests
//! use turtle_lsp::imports::parse_catalog as _; // tests
//! use turtle_lsp::imports::rdf_xml_to_ntriples as _; // tests
//! use turtle_lsp::imports::sniff as _; // tests
//! use turtle_lsp::jsonld::ContextLoader as _; // tests
//! use turtle_lsp::jsonld::LoadError as _; // tests
//! use turtle_lsp::jsonld::NoNetwork as _; // tests
//! use turtle_lsp::jsonld::RemoteDocument as _; // tests
//! use turtle_lsp::jsonld::format::format as _; // tests
//! use turtle_lsp::jsonld::json::Json as _; // tests
//! use turtle_lsp::jsonld::json::TokenKind as _; // tests
//! use turtle_lsp::jsonld::json::Value as _; // tests
//! use turtle_lsp::jsonld::json::parse as _; // tests
//! use turtle_lsp::jsonld::json::tokens as _; // tests
//! use turtle_lsp::jsonld::json::valid as _; // examples
//! use turtle_lsp::jsonld::parse as _; // tests
//! use turtle_lsp::knowledge::Knowledge as _; // tests
//! use turtle_lsp::knowledge::Source as _; // tests
//! use turtle_lsp::oracle::Batch as _; // tests
//! use turtle_lsp::oracle::change as _; // tests
//! use turtle_lsp::oracle::check as _; // tests
//! use turtle_lsp::query::Answer as _; // tests, web/engine
//! use turtle_lsp::query::Inferred as _; // web/engine
//! use turtle_lsp::query::Limits as _; // tests, web/engine
//! use turtle_lsp::query::QueryError as _; // tests, web/engine
//! use turtle_lsp::query::render::term as _; // web/engine
//! use turtle_lsp::query::render::turtle as _; // web/engine
//! use turtle_lsp::query::run as _; // tests
//! use turtle_lsp::rdf::graph::Graph as _; // examples, tests
//! use turtle_lsp::rdf::graph::Id as _; // examples, tests
//! use turtle_lsp::rdf::graph::Term as _; // examples, tests
//! use turtle_lsp::server::Embedded as _; // tests, web/engine
//! use turtle_lsp::server::MAIN_STACK as _; // main
//! use turtle_lsp::server::run as _; // main
//! use turtle_lsp::settings::InlayLabels as _; // tests
//! use turtle_lsp::settings::Layer as _; // web/engine
//! use turtle_lsp::settings::Reasoning as _; // web/engine
//! use turtle_lsp::settings::Settings as _; // tests, web/engine
//! use turtle_lsp::settings::seconds as _; // web/engine
//! use turtle_lsp::shacl::Path as _; // tests
//! use turtle_lsp::shacl::SH as _; // tests
//! use turtle_lsp::shacl::validate::validate_report as _; // examples
//! use turtle_lsp::shacl::validate_report as _; // tests
//! use turtle_lsp::sparql::Document as _; // examples, tests
//! use turtle_lsp::sparql::Level as _; // examples, tests
//! use turtle_lsp::sparql::features::code_actions as _; // tests
//! use turtle_lsp::sparql::features::completion as _; // tests
//! use turtle_lsp::sparql::features::definition as _; // tests
//! use turtle_lsp::sparql::features::diagnostics as _; // tests
//! use turtle_lsp::sparql::features::document_symbols as _; // tests
//! use turtle_lsp::sparql::features::folding_ranges as _; // tests
//! use turtle_lsp::sparql::features::formatting as _; // tests
//! use turtle_lsp::sparql::features::highlights as _; // tests
//! use turtle_lsp::sparql::features::hover as _; // tests
//! use turtle_lsp::sparql::features::inlay_hints as _; // tests
//! use turtle_lsp::sparql::features::prepare_rename as _; // tests
//! use turtle_lsp::sparql::features::rdf_term_rename as _; // tests
//! use turtle_lsp::sparql::features::references as _; // tests
//! use turtle_lsp::sparql::features::rename as _; // tests
//! use turtle_lsp::sparql::features::resolve_code_action as _; // tests
//! use turtle_lsp::sparql::features::semantic_tokens_full as _; // tests
//! use turtle_lsp::sparql::features::semantic_tokens_range as _; // tests
//! use turtle_lsp::sparql::features::term_references as _; // tests
//! use turtle_lsp::sparql::format as _; // examples, tests
//! use turtle_lsp::sparql::lexer::TokenKind as _; // examples, tests
//! use turtle_lsp::sparql::lexer::tokenize as _; // examples, tests
//! use turtle_lsp::sparql::spargebra_input::text_for_spargebra as _; // examples
//! use turtle_lsp::syntax::BlankId as _; // tests
//! use turtle_lsp::syntax::Direction as _; // tests
//! use turtle_lsp::syntax::Form as _; // tests
//! use turtle_lsp::syntax::GraphBlock as _; // tests
//! use turtle_lsp::syntax::GraphName as _; // tests
//! use turtle_lsp::syntax::Iri as _; // tests
//! use turtle_lsp::syntax::Node as _; // examples, tests
//! use turtle_lsp::syntax::Parsed as _; // examples, tests
//! use turtle_lsp::syntax::PrefixDecl as _; // tests
//! use turtle_lsp::syntax::PrefixMap as _; // examples, tests
//! use turtle_lsp::syntax::PrefixUse as _; // tests
//! use turtle_lsp::syntax::RDF as _; // tests
//! use turtle_lsp::syntax::Ref as _; // tests
//! use turtle_lsp::syntax::Role as _; // tests
//! use turtle_lsp::syntax::Severity as _; // examples, tests
//! use turtle_lsp::syntax::Span as _; // tests
//! use turtle_lsp::syntax::Statement as _; // tests
//! use turtle_lsp::syntax::Syntax as _; // examples, tests
//! use turtle_lsp::syntax::SyntaxError as _; // tests
//! use turtle_lsp::syntax::Triple as _; // examples, tests
//! use turtle_lsp::syntax::XSD as _; // tests
//! use turtle_lsp::syntax::lexer::Kind as _; // examples, tests
//! use turtle_lsp::syntax::lexer::Token as _; // tests
//! use turtle_lsp::syntax::lexer::tokenize as _; // examples, tests
//! use turtle_lsp::syntax::parse as _; // examples, tests
//! use turtle_lsp::syntax::parse_with_base as _; // examples, tests
//! ```
#![deny(unsafe_code)]

pub mod analysis;
pub mod batch;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod clock;
pub(crate) mod commands;
pub(crate) mod diagnose;
pub(crate) mod extensions;
pub mod features;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod fetch;
pub(crate) mod ide;
pub mod imports;
pub mod jsonld;
pub mod knowledge;
pub(crate) mod layout;
pub(crate) mod ofn;
#[doc(hidden)]
pub mod oracle;
pub(crate) mod owx;
pub mod query;
pub mod rdf;
pub(crate) mod rdfxml;
pub(crate) mod reasoning;
pub mod server;
pub mod settings;
pub mod shacl;
pub mod sparql;
pub mod syntax;
pub(crate) mod vocab;
pub(crate) mod worker;
pub(crate) mod workspace;

/// The version of this build, as the server reports it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The module boundaries, read from the source: each library and frontend
/// names only what its layer allows (`LAYERS`) and no protocol crate; core
/// code names no `extensions` (`EXTENSION_USERS` excepted); no file outside
/// `server` and `extensions` names `crate::server`; and no product file
/// outside `src/oracle.rs` and `ORACLE_USERS` names `crate::oracle`.
/// `AGENTS.md` ("Where things live") and `ARCHITECTURE.md` ("The modules")
/// state the layer and `extensions` rules. Comments, literals, `#[cfg(test)]`
/// items and the files that a `#[cfg(test)] mod x;` declares are not checked.
/// Paths are read from the tokens: `use` groups expanded, `self` and `super`
/// resolved.
///
/// Limits: a path that a macro builds, or that a re-export reaches (`pub use`
/// in an allowed module), is not seen.
#[cfg(test)]
mod boundaries {
    const LIBRARIES: &str = "syntax rdf shacl sparql::lexer sparql::spargebra_input";
    /// The only modules a library file may name, by path prefix.
    const LAYERS: &[(&str, &str)] = &[
        ("src/syntax/", "syntax"),
        ("src/sparql/lexer.rs", "syntax"),
        ("src/sparql/spargebra_input.rs", LIBRARIES),
        ("src/rdf/", LIBRARIES),
        ("src/shacl/", LIBRARIES),
        ("src/rdfxml/", "syntax rdfxml"),
        ("src/ofn/", "syntax ofn"),
        ("src/owx/", "syntax ofn rdfxml::xml owx"),
    ];
    /// The core files that may name `extensions`: the router and the Run query lens.
    const EXTENSION_USERS: &[&str] = &["src/server.rs", "src/features/lens.rs"];

    /// The product files outside `src/oracle.rs` that may name `crate::oracle`:
    /// none.
    const ORACLE_USERS: &[&str] = &[];

    /// The protocol crates. The libraries of `LAYERS` name none of them.
    const PROTOCOL: &[&str] = &["lsp_types", "lsp_server", "serde_json"];

    #[test]
    fn each_file_names_only_what_its_layer_allows() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let (mut dirs, mut files) = (vec!["src".to_string()], Vec::new());
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(root.join(&dir)).unwrap() {
                let name = format!("{dir}/{}", entry.unwrap().file_name().to_string_lossy());
                if root.join(&name).is_dir() {
                    dirs.push(name);
                } else if name.ends_with(".rs") {
                    let text = std::fs::read_to_string(root.join(&name)).unwrap();
                    files.push((name, text));
                }
            }
        }
        let files: Vec<_> = files
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();
        let errors = check(&files);
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    /// The OWL/XML frontend names `syntax`, `ofn` and the XML scanner of
    /// `rdfxml`, and nothing else: not the RDF/XML grammar, not a feature.
    #[test]
    fn owx_names_syntax_ofn_and_the_xml_scanner_only() {
        let errors = |text: &str| check(&[("src/owx/reader.rs", text)]).len();
        let allowed = "use crate::syntax::Span;\nuse crate::ofn::tree::Document;\nuse crate::rdfxml::xml::{self, Scanner};\nuse super::source::OwlXml;\n";
        assert_eq!(errors(allowed), 0);
        assert_eq!(errors("use crate::rdfxml::parse;\n"), 1);
        assert_eq!(errors("use crate::rdfxml::xmlish::X;\n"), 1);
        assert_eq!(errors("use crate::features::writes;\n"), 1);
        assert_eq!(errors("let a = crate::analysis::Analysis::new;\n"), 1);
        // Test items are not checked.
        assert_eq!(errors("#[cfg(test)]\nuse crate::features::writes;\n"), 0);
    }

    /// How many errors `check` reports for one file.
    fn count(name: &str, text: &str) -> usize {
        check(&[(name, text)]).len()
    }

    /// `super` chains resolve against the file's module path, `mod.rs`
    /// folded; `self` names the file's own module, which is always allowed.
    #[test]
    fn super_chains_resolve_to_the_module_they_reach() {
        assert_eq!(
            count("src/syntax/parser.rs", "use super::super::features::x;\n"),
            1
        );
        assert_eq!(
            count("src/syntax/parser.rs", "use super::lexer::Token;\n"),
            0
        );
        assert_eq!(count("src/owx/mod.rs", "use super::features::x;\n"), 1);
        assert_eq!(count("src/sparql/lexer.rs", "use super::format::x;\n"), 1);
        assert_eq!(count("src/sparql/lexer.rs", "use self::kinds::X;\n"), 0);
        let up = "use self::super::super::features::x;\n";
        assert_eq!(count("src/syntax/parser.rs", up), 1);
    }

    /// A `#[cfg(test)]` item ends where its braces or parentheses balance:
    /// a struct field at its `,`, not at the next line that ends in `;`.
    #[test]
    fn a_test_item_ends_where_its_brackets_balance() {
        let field = "struct S {\n    #[cfg(test)]\n    probe: u32,\n    a: u32,\n}\nfn f() {\n    let x = crate::features::x();\n}\n";
        assert_eq!(count("src/syntax/parser.rs", field), 1);
        let arm = "fn f(k: u8) -> u8 {\n    match k {\n        #[cfg(test)]\n        0 => 1,\n        _ => {\n            crate::features::x();\n            2\n        }\n    }\n}\n";
        assert_eq!(count("src/syntax/parser.rs", arm), 1);
        // A declaration's generics may hold a `,`: the item ends at its body.
        let generic = "#[cfg(test)]\nfn f() -> Result<(), u8> {\n    crate::features::x();\n}\n";
        assert_eq!(count("src/syntax/parser.rs", generic), 0);
        // A test item that never closes reads to the end of the file: an error.
        assert_eq!(
            count("src/syntax/parser.rs", "#[cfg(test)]\nmod tests {\n"),
            1
        );
    }

    /// The text that a test file may hold, and a product file may not.
    const CROSS_LAYER: &str = "use crate::features::writes;\n";

    /// `src/syntax/parser.rs` declares `mod tests;` under `#[cfg(test)]`: the
    /// test module is `src/syntax/parser/tests.rs` or a folder of that name.
    #[test]
    fn a_test_file_under_a_file_parent_is_test_code() {
        let parent = (
            "src/syntax/parser.rs",
            "#[cfg(test)]\nmod tests;\nmod lexer;\n",
        );
        for test in [
            "src/syntax/parser/tests.rs",
            "src/syntax/parser/tests/mod.rs",
            "src/syntax/parser/tests/more.rs",
        ] {
            assert_eq!(check(&[parent, (test, CROSS_LAYER)]).len(), 0, "{test}");
        }
        for product in [
            "src/syntax/tests.rs",
            "src/syntax/parser/lexer.rs",
            "src/syntax/parser/tests_more.rs",
        ] {
            assert_eq!(
                check(&[parent, (product, CROSS_LAYER)]).len(),
                1,
                "{product}"
            );
        }
    }

    /// `src/owx/mod.rs` declares `mod tests;` under `#[cfg(test)]`: the test
    /// module is `src/owx/tests.rs` or a folder of that name.
    #[test]
    fn a_test_file_under_a_mod_rs_parent_is_test_code() {
        let parent = (
            "src/owx/mod.rs",
            "#[cfg(test)]\npub(crate) mod tests;\nmod reader;\n",
        );
        for test in [
            "src/owx/tests.rs",
            "src/owx/tests/mod.rs",
            "src/owx/tests/more.rs",
        ] {
            assert_eq!(check(&[parent, (test, CROSS_LAYER)]).len(), 0, "{test}");
        }
        for product in [
            "src/owx/reader.rs",
            "src/owx/mod/tests.rs",
            "src/ofn/tests.rs",
        ] {
            assert_eq!(
                check(&[parent, (product, CROSS_LAYER)]).len(),
                1,
                "{product}"
            );
        }
    }

    /// An `extensions` import in a group is checked at its origin, aliased or not.
    #[test]
    fn a_grouped_extensions_import_is_core_naming_extensions() {
        let core = "src/features/hover.rs";
        assert_eq!(count(core, "use crate::{extensions as ext};\n"), 1);
        assert_eq!(count(core, "use crate::{\n    extensions as ext,\n};\n"), 1);
        assert_eq!(
            count(
                core,
                "use crate::{syntax::{Span, Syntax}, extensions::{self as e}};\n"
            ),
            1
        );
        assert_eq!(
            count("src/features/lens.rs", "use crate::{extensions as ext};\n"),
            0
        );
    }

    /// A group is expanded, nested groups included, and each path is checked.
    #[test]
    fn a_grouped_import_is_checked_path_by_path() {
        let owx = "src/owx/reader.rs";
        assert_eq!(count(owx, "use crate::{syntax::Span};\n"), 0);
        let nested =
            "use crate::{\n    ofn::tree::Document,\n    rdfxml::xml::{self, Scanner},\n};\n";
        assert_eq!(count(owx, nested), 0);
        assert_eq!(count(owx, "use crate::{syntax::Span, rdfxml::parse};\n"), 1);
        // A path after `as` in an expression is a path, not an alias.
        let cast = "let c = <u8 as crate::features::Code>::f();\n";
        assert_eq!(count(owx, cast), 1);
    }

    /// String and raw-string contents are not code: a column-0 `"#;` does not
    /// end a test module, and a path in a product string is not named.
    #[test]
    fn literals_are_not_code() {
        let owx = "src/owx/reader.rs";
        let fixture = "fn f() {}\n#[cfg(test)]\nmod tests {\n    const T: &str = r#\"\n<a/>\n\"#;\n    use crate::features::writes;\n}\n";
        assert_eq!(count(owx, fixture), 0);
        assert_eq!(count(owx, "const S: &str = \"crate::features\";\n"), 0);
        let hidden = "const S: &str = r#\"\n#[cfg(test)]\n\"#;\nuse crate::features::writes;\n";
        assert_eq!(count(owx, hidden), 1);
        assert_eq!(
            count(owx, "const C: char = '{';\nuse crate::features::writes;\n"),
            1
        );
    }

    /// Below the server, no file names `crate::server`.
    #[test]
    fn only_the_listed_files_name_the_server() {
        let spawn = "let s = crate::server::WORKER_STACK;\n";
        assert_eq!(count("src/features/hover.rs", spawn), 1);
        assert_eq!(count("src/workspace.rs", spawn), 1);
        assert_eq!(
            count("src/workspace/kinds.rs", "use super::super::server::X;\n"),
            1
        );
        assert_eq!(count("src/server_x.rs", spawn), 1);
        for name in [
            "src/server.rs",
            "src/server/index.rs",
            "src/extensions/mod.rs",
        ] {
            assert_eq!(count(name, spawn), 0, "{name}");
        }
        let main = "fn main() {\n    let _ = turtle_lsp::server::run();\n}\n";
        assert_eq!(count("src/main.rs", main), 0);
    }

    /// Only `ORACLE_USERS` name `crate::oracle`.
    #[test]
    fn only_the_listed_files_name_the_oracle() {
        let call = "let x = crate::oracle::check(a, b);\n";
        assert_eq!(count("src/features/hover.rs", call), 1);
        assert_eq!(count("src/workspace.rs", call), 1);
        assert_eq!(
            count(
                "src/oracle.rs",
                "use self::x::Y;\nlet y = crate::oracle::z();\n"
            ),
            0
        );
    }

    /// The layered libraries name no protocol crate.
    #[test]
    fn libraries_name_no_protocol_crate() {
        assert_eq!(
            count("src/syntax/parser.rs", "fn f(r: lsp_types::Range) {}\n"),
            1
        );
        assert_eq!(count("src/shacl/model.rs", "use serde_json::Value;\n"), 1);
        assert_eq!(count("src/owx/reader.rs", "use lsp_server::Message;\n"), 1);
        assert_eq!(count("src/rdf/mod.rs", "use lsp_types as lt;\n"), 1);
        assert_eq!(count("src/features/hover.rs", "use lsp_types::Range;\n"), 0);
    }

    /// The boundary errors of `files`, given as (path from the crate root, text).
    /// A file that a `#[cfg(test)] mod x;` declares, and the folder below it,
    /// is test code and is not checked.
    fn check(files: &[(&str, &str)]) -> Vec<String> {
        let masked: Vec<String> = files.iter().map(|(_, text)| mask(text)).collect();
        let mut tests = Vec::new();
        for ((name, _), code) in files.iter().zip(&masked) {
            for (start, end) in test_items(code) {
                let item = &code[start + TEST.len()..end.unwrap_or(code.len())];
                if let Some(module) = declared_module(item) {
                    tests.push(format!("{}{module}", module_dir(name)));
                }
            }
        }
        let mut errors = Vec::new();
        for ((name, text), code) in files.iter().zip(masked) {
            let test =
                |t: &String| *name == format!("{t}.rs") || name.starts_with(&format!("{t}/"));
            if !tests.iter().any(test) {
                check_file(name, text, code, &mut errors);
            }
            // Paths of one group on one line give one message.
            errors.dedup();
        }
        errors
    }

    /// The attribute that starts a test item.
    const TEST: &str = "#[cfg(test)]";

    fn check_file(name: &str, text: &str, code: String, errors: &mut Vec<String>) {
        let lines: Vec<&str> = text.lines().collect();
        let breaks: Vec<usize> = code.match_indices('\n').map(|(i, _)| i).collect();
        let mut report = |offset: usize, why: String| {
            let n = breaks.partition_point(|&b| b < offset);
            let code = lines.get(n).map_or("", |l| l.trim());
            errors.push(format!("{name}:{}: {why}: {code}", n + 1));
        };
        let mut product = code.into_bytes();
        for (start, end) in test_items(std::str::from_utf8(&product).unwrap()) {
            if end.is_none() {
                report(start, "the test item does not end".into());
            }
            let end = end.unwrap_or(product.len());
            for b in &mut product[start..end] {
                if *b != b'\n' {
                    *b = b' ';
                }
            }
        }
        let product = String::from_utf8(product).unwrap();
        let layer = LAYERS.iter().find(|(p, _)| name.starts_with(p));
        let core = !name.starts_with("src/extensions/") && !EXTENSION_USERS.contains(&name);
        let below_server = !(name == "src/server.rs"
            || name.starts_with("src/server/")
            || name.starts_with("src/extensions/"));
        let outside_oracle = !(name == "src/oracle.rs"
            || name.starts_with("src/oracle/")
            || ORACLE_USERS.contains(&name));
        let module = file_module(name);
        let tokens: Vec<_> = tokens(&product).collect();
        if layer.is_some() {
            for &(offset, token) in &tokens {
                if PROTOCOL.contains(&token) {
                    report(offset, format!("a library may not name {token}"));
                }
            }
        }
        // The module path at each token: the file's, plus the inline `mod x {}` it is in.
        let (mut depth, mut inline) = (0, Vec::new());
        let mut i = 0;
        while i < tokens.len() {
            let (_, token) = tokens[i];
            let starts = i == 0 || tokens[i - 1].1 != "::";
            if starts && is_ident(token) && tokens.get(i + 1).is_some_and(|t| t.1 == "::") {
                let mut paths = Vec::new();
                i = tree(&tokens, i, Vec::new(), &mut paths);
                let mut here = module.clone();
                here.extend(inline.iter().map(|(_, m): &(usize, &str)| m.to_string()));
                for (path, offset) in paths {
                    let resolved = resolve(&path, &here);
                    let first = resolved
                        .as_ref()
                        .and_then(|r| r.first())
                        .map(String::as_str);
                    let inner = &path[..path.len() - 1];
                    if core && (first == Some("extensions") || inner.contains(&"extensions")) {
                        report(offset, "core code may not name extensions".into());
                    }
                    if below_server && first == Some("server") {
                        report(offset, "this file may not name crate::server".into());
                    }
                    if outside_oracle && first == Some("oracle") {
                        report(offset, "only ORACLE_USERS may name crate::oracle".into());
                    }
                    let (Some((_, allowed)), Some(resolved)) = (layer, resolved) else {
                        continue;
                    };
                    // A `self` path that stays in its module names that module or one below it.
                    if path[0] == "self" && resolved.starts_with(&here) {
                        continue;
                    }
                    let joined = resolved.join("::");
                    let names = |m: &str| {
                        joined
                            .strip_prefix(m)
                            .is_some_and(|r| r.is_empty() || r.starts_with("::"))
                    };
                    if !allowed.split(' ').any(names) {
                        report(offset, format!("may name only crate::{{{allowed}}}"));
                    }
                }
                continue;
            }
            match token {
                "{" => {
                    depth += 1;
                    if i >= 2 && tokens[i - 2].1 == "mod" {
                        inline.push((depth, tokens[i - 1].1));
                    }
                }
                "}" => {
                    if inline.last().is_some_and(|m| m.0 == depth) {
                        inline.pop();
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// `text` with its comments and the whole of its string, char and raw
    /// string literals turned to spaces. Line breaks stay, so lines keep
    /// their numbers.
    fn mask(text: &str) -> String {
        let c: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < c.len() {
            let rest = &c[i..];
            let after_ident = |k: usize| i >= k && is_ident_char(c[i - k]);
            let raw_prefix = rest[0] == 'r'
                && (!after_ident(1) || (matches!(c[i - 1], 'b' | 'c') && !after_ident(2)));
            let hashes = rest.iter().skip(1).take_while(|&&h| h == '#').count();
            let len = if rest.starts_with(&['/', '/']) {
                rest.iter().position(|&ch| ch == '\n').unwrap_or(rest.len())
            } else if rest.starts_with(&['/', '*']) {
                let (mut depth, mut j) = (0, 0);
                while j < rest.len() {
                    if rest[j..].starts_with(&['/', '*']) {
                        (depth, j) = (depth + 1, j + 2);
                    } else if rest[j..].starts_with(&['*', '/']) {
                        (depth, j) = (depth - 1, j + 2);
                        if depth == 0 {
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                j
            } else if raw_prefix && rest.get(1 + hashes) == Some(&'"') {
                let end: Vec<char> = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let body = 2 + hashes;
                (body..rest.len())
                    .find(|&j| rest[j..].starts_with(&end))
                    .map_or(rest.len(), |j| j + end.len())
            } else if rest[0] == '"' {
                let mut j = 1;
                while j < rest.len() && rest[j] != '"' {
                    j += if rest[j] == '\\' { 2 } else { 1 };
                }
                (j + 1).min(rest.len())
            } else if rest.starts_with(&['\'', '\\']) {
                (3..rest.len())
                    .find(|&j| rest[j] == '\'')
                    .map_or(rest.len(), |j| j + 1)
            } else if rest[0] == '\'' && rest.get(2) == Some(&'\'') {
                3
            } else {
                out.push(rest[0]);
                i += 1;
                continue;
            };
            out.extend(
                rest[..len]
                    .iter()
                    .map(|&ch| if ch == '\n' { '\n' } else { ' ' }),
            );
            i += len;
        }
        out
    }

    /// The byte ranges of the `#[cfg(test)]` items in masked code, each from
    /// its attribute to its end; `None` for an item that never ends.
    fn test_items(code: &str) -> Vec<(usize, Option<usize>)> {
        let mut items = Vec::new();
        let mut from = 0;
        while let Some(found) = code[from..].find(TEST) {
            let start = from + found;
            let end = item_end(code, start + TEST.len());
            items.push((start, end));
            let Some(end) = end else { break };
            from = end;
        }
        items
    }

    /// Where the item that starts at `from` ends: after a `;` outside
    /// brackets, after the `}` that closes its first `{`, or before a closer
    /// of the block it is in. A field, variant, match arm or expression also
    /// ends after a `,` outside brackets; a declaration does not, since its
    /// generics may hold one.
    fn item_end(code: &str, from: usize) -> Option<usize> {
        let declaration = declares(&code[from..]);
        let mut depth = 0;
        for (i, &c) in code.as_bytes().iter().enumerate().skip(from) {
            match c {
                b'{' | b'(' | b'[' => depth += 1,
                b'}' | b')' | b']' if depth == 0 => return Some(i),
                b'}' if depth == 1 => return Some(i + 1),
                b'}' | b')' | b']' => depth -= 1,
                b';' if depth == 0 => return Some(i + 1),
                b',' if depth == 0 && !declaration => return Some(i + 1),
                _ => {}
            }
        }
        None
    }

    /// The keywords that start a declaration.
    const DECLARATIONS: &[&str] = &[
        "fn",
        "mod",
        "impl",
        "struct",
        "enum",
        "union",
        "trait",
        "type",
        "const",
        "static",
        "use",
        "let",
        "macro_rules",
        "unsafe",
        "extern",
        "async",
    ];

    /// Whether the item at the start of `item` is a declaration: its first
    /// keyword after its attributes and its visibility.
    fn declares(item: &str) -> bool {
        let mut tokens = tokens(item).map(|t| t.1);
        let mut depth = 0;
        let mut next = tokens.next();
        while let Some(token) = next {
            match token {
                "[" => depth += 1,
                "]" => depth -= 1,
                "pub" | "(" | ")" | "crate" | "super" | "in" | "self" | "::" | "#" => {}
                _ if depth == 0 => return DECLARATIONS.contains(&token),
                _ => {}
            }
            next = tokens.next();
        }
        false
    }

    /// The module that a test item declares out of line: `x` in `mod x;`,
    /// with any visibility and no other attribute.
    fn declared_module(item: &str) -> Option<&str> {
        let tokens: Vec<&str> = tokens(item).map(|t| t.1).collect();
        let mut rest = &tokens[..];
        if rest.first() == Some(&"pub") {
            rest = &rest[1..];
            if rest.first() == Some(&"(") {
                rest = &rest[rest.iter().position(|&t| t == ")")? + 1..];
            }
        }
        match rest {
            ["mod", module, ";"] if is_ident(module) => Some(module),
            _ => None,
        }
    }

    /// The folder that holds the out-of-line modules of the file `name`.
    fn module_dir(name: &str) -> String {
        let root = ["src/lib.rs", "src/main.rs"].contains(&name) || name.ends_with("/mod.rs");
        match name.rsplit_once('/') {
            Some((dir, _)) if root => format!("{dir}/"),
            _ => format!("{}/", name.trim_end_matches(".rs")),
        }
    }

    /// The module path of the file `name`, `mod.rs` folded; empty at a crate root.
    fn file_module(name: &str) -> Vec<String> {
        let module = name[4..].trim_end_matches(".rs").trim_end_matches("/mod");
        if module == "lib" || module == "main" {
            return Vec::new();
        }
        module.split('/').map(String::from).collect()
    }

    /// The crate path that `path` names from the module `here`, or `None` for
    /// a path that does not start at `crate`, `self` or `super`.
    fn resolve(path: &[&str], here: &[String]) -> Option<Vec<String>> {
        let (mut resolved, mut rest) = match path.first()? {
            &"crate" => (Vec::new(), &path[1..]),
            &"self" | &"super" => (here.to_vec(), path),
            _ => return None,
        };
        while let Some((&first, tail)) = rest.split_first() {
            match first {
                "self" => {}
                "super" => {
                    resolved.pop();
                }
                _ => break,
            }
            rest = tail;
        }
        resolved.extend(rest.iter().filter(|&&s| s != "self").map(|s| s.to_string()));
        Some(resolved)
    }

    /// The paths of the tree that starts at `tokens[i]`, each after the
    /// segments in `path`: a path joined by `::`, which may end in a `{…}`
    /// group of trees. Each path
    /// comes with the offset of its last segment, and an alias (`as x`) is
    /// dropped, so a path is checked where it starts. Returns the index after
    /// the tree.
    fn tree<'a>(
        tokens: &[(usize, &'a str)],
        mut i: usize,
        mut path: Vec<&'a str>,
        paths: &mut Vec<(Vec<&'a str>, usize)>,
    ) -> usize {
        loop {
            match tokens.get(i) {
                Some(&(_, "{")) if !path.is_empty() => {
                    i += 1;
                    while let Some(&(_, token)) = tokens.get(i) {
                        if token == "}" {
                            return i + 1;
                        }
                        let next = if token == "," {
                            i + 1
                        } else {
                            tree(tokens, i, path.clone(), paths)
                        };
                        i = next.max(i + 1);
                    }
                    return i;
                }
                Some(&(offset, segment)) if is_ident(segment) || segment == "*" => {
                    path.push(segment);
                    i += 1;
                    if tokens.get(i).is_some_and(|t| t.1 == "::") {
                        i += 1;
                        continue;
                    }
                    // An alias is one name; after `as` in an expression, a path goes on.
                    let alias = !tokens.get(i + 2).is_some_and(|t| t.1 == "::");
                    if tokens.get(i).is_some_and(|t| t.1 == "as") && alias {
                        i += 2;
                    }
                    paths.push((path, offset));
                    return i;
                }
                // A path that goes on as `::<…>` ends before the generics.
                _ => {
                    if !path.is_empty() {
                        let offset = tokens[i.min(tokens.len()) - 1].0;
                        paths.push((path, offset));
                    }
                    return i;
                }
            }
        }
    }

    /// The tokens of masked code, with their byte offsets: identifiers, `::`,
    /// and every other character on its own.
    fn tokens(code: &str) -> impl Iterator<Item = (usize, &str)> {
        let mut chars = code.char_indices().peekable();
        std::iter::from_fn(move || {
            let (start, c) = chars.find(|c| !c.1.is_whitespace())?;
            let mut end = start + c.len_utf8();
            if is_ident_char(c) {
                while let Some(&(i, n)) = chars.peek().filter(|p| is_ident_char(p.1)) {
                    end = i + n.len_utf8();
                    chars.next();
                }
            } else if c == ':' && chars.peek().is_some_and(|p| p.1 == ':') {
                chars.next();
                end += 1;
            }
            Some((start, &code[start..end]))
        })
    }

    fn is_ident_char(c: char) -> bool {
        c == '_' || c.is_alphanumeric()
    }

    fn is_ident(token: &str) -> bool {
        token.starts_with(|c: char| c == '_' || c.is_alphabetic())
    }
}
