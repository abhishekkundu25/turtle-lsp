//! What kind of file a path names, from one table ([`ROWS`]).
//!
//! A row is an extension or a whole file name, the kind it gives and its
//! letter-case rule: RDF, import-source and JSON extensions are read in any
//! case (`Data.TTL` is Turtle), SPARQL extensions and the catalog's name only
//! as written (`a.RQ` is not SPARQL). The extension is what follows the last
//! `.` of the last part of a path or URI (after the last `/` or `\`). Every
//! row is a file the server watches: `server::WATCHED_FILES` are the globs
//! of all rows, and a test checks them. `Syntax::from_extension` keeps its own
//! list, as the syntax core cannot read the workspace; a test checks it
//! against the RDF rows.

use crate::imports::CATALOG_FILE;
use crate::syntax::Syntax;
use std::path::Path;

/// What a file is to this server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileKind {
    /// An RDF document in this syntax.
    Rdf(Syntax),
    /// A SPARQL query or update.
    Sparql,
    /// An ontology read only as an `owl:imports` target (`.rdf`, `.owl`).
    ImportSource,
    /// A catalog mapping import IRIs to files ([`CATALOG_FILE`]).
    Catalog,
    /// A `.json` file: a JSON-LD context, or the settings file.
    Json,
}

/// What a row matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pattern {
    /// The text after the last `.` of the file name.
    Extension(&'static str),
    /// The whole file name.
    Name(&'static str),
}

/// How a row's text is compared with a path's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Case {
    /// In any letter case.
    Any,
    /// As written (every row is written in lower case).
    Exact,
}

/// One row of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) pattern: Pattern,
    pub(crate) kind: FileKind,
    pub(crate) case: Case,
}

const fn ext(ext: &'static str, kind: FileKind, case: Case) -> Row {
    Row {
        pattern: Pattern::Extension(ext),
        kind,
        case,
    }
}

/// Every kind of file the server reads, in the order of `WATCHED_FILES`.
pub(crate) const ROWS: &[Row] = &[
    ext("ttl", FileKind::Rdf(Syntax::Turtle), Case::Any),
    ext("turtle", FileKind::Rdf(Syntax::Turtle), Case::Any),
    ext("trig", FileKind::Rdf(Syntax::TriG), Case::Any),
    ext("nt", FileKind::Rdf(Syntax::NTriples), Case::Any),
    ext("nq", FileKind::Rdf(Syntax::NQuads), Case::Any),
    ext("jsonld", FileKind::Rdf(Syntax::JsonLd), Case::Any),
    ext("json", FileKind::Json, Case::Any),
    ext("rdf", FileKind::ImportSource, Case::Any),
    ext("owl", FileKind::ImportSource, Case::Any),
    ext("rq", FileKind::Sparql, Case::Exact),
    ext("sparql", FileKind::Sparql, Case::Exact),
    ext("ru", FileKind::Sparql, Case::Exact),
    Row {
        pattern: Pattern::Name(CATALOG_FILE),
        kind: FileKind::Catalog,
        case: Case::Exact,
    },
];

impl FileKind {
    /// The kind of file a path or URI names, by the first row that matches.
    pub(crate) fn of(path: &str) -> Option<FileKind> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, ext)| ext);
        ROWS.iter()
            .find(|row| {
                let seen = match row.pattern {
                    Pattern::Extension(_) => ext,
                    Pattern::Name(_) => Some(name),
                };
                seen.is_some_and(|s| row.matches(s))
            })
            .map(|row| row.kind)
    }

    /// [`FileKind::of`] a path on disk.
    pub(crate) fn of_path(path: &Path) -> Option<FileKind> {
        FileKind::of(&path.to_string_lossy())
    }
}

impl Row {
    fn matches(&self, seen: &str) -> bool {
        let (Pattern::Extension(text) | Pattern::Name(text)) = self.pattern;
        match self.case {
            Case::Any => seen.eq_ignore_ascii_case(text),
            Case::Exact => seen == text,
        }
    }
}

/// A file this server reads as RDF: `.ttl`, `.turtle`, `.trig`, `.nt`, `.nq`
/// or `.jsonld`, in any letter case.
pub(crate) fn is_rdf_file(path: &Path) -> bool {
    matches!(FileKind::of_path(path), Some(FileKind::Rdf(_)))
}

/// Whether a path or URI names a SPARQL file: `.rq`, `.sparql` or `.ru`, in
/// lower case.
pub(crate) fn is_sparql_name(path: &str) -> bool {
    FileKind::of(path) == Some(FileKind::Sparql)
}

/// A SPARQL file: `.rq`, `.sparql` or `.ru`, in lower case.
pub(crate) fn is_sparql_path(path: &Path) -> bool {
    FileKind::of_path(path) == Some(FileKind::Sparql)
}

/// Files that feed `owl:imports` without being documents themselves: a
/// catalog, an `.rdf` or `.owl` ontology.
pub(crate) fn is_import_source(path: &Path) -> bool {
    matches!(
        FileKind::of_path(path),
        Some(FileKind::ImportSource | FileKind::Catalog)
    )
}

/// Whether `path` names a kind of file the server watches (any row): if it
/// was deleted, it was a file, not a folder.
pub(super) fn is_watched_file(path: &Path) -> bool {
    FileKind::of_path(path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::WATCHED_FILES;
    use crate::workspace::DocKind;

    /// The predicates as they were before the table (the RDF one under its
    /// new name), kept to check that every path keeps its kind.
    mod before {
        use crate::syntax::Syntax;
        use std::path::Path;

        pub(crate) fn is_sparql_name(path: &str) -> bool {
            let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
            name.rsplit_once('.')
                .is_some_and(|(_, ext)| matches!(ext, "rq" | "sparql" | "ru"))
        }

        pub(crate) fn is_rdf_file(path: &Path) -> bool {
            Syntax::from_extension(&path.to_string_lossy()).is_some()
        }

        pub(crate) fn is_import_source(path: &Path) -> bool {
            path.file_name().is_some_and(|f| f == "catalog-v001.xml")
                || path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("rdf") || e.eq_ignore_ascii_case("owl"))
        }

        pub(crate) fn is_watched_file(path: &Path) -> bool {
            is_rdf_file(path)
                || is_import_source(path)
                || is_sparql_name(&path.to_string_lossy())
                || path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        }
    }

    /// The globs of the rows, as `WATCHED_FILES` writes them: the extensions
    /// in one, then each file name.
    fn globs() -> Vec<String> {
        let exts: Vec<&str> = ROWS
            .iter()
            .filter_map(|row| match row.pattern {
                Pattern::Extension(ext) => Some(ext),
                Pattern::Name(_) => None,
            })
            .collect();
        let names = ROWS.iter().filter_map(|row| match row.pattern {
            Pattern::Name(name) => Some(format!("**/{name}")),
            Pattern::Extension(_) => None,
        });
        std::iter::once(format!("**/*.{{{}}}", exts.join(",")))
            .chain(names)
            .collect()
    }

    /// The row's text in another letter case.
    fn upper(row: &Row) -> String {
        match row.pattern {
            Pattern::Extension(ext) => format!("a.{}", ext.to_ascii_uppercase()),
            Pattern::Name(name) => name.to_ascii_uppercase(),
        }
    }

    fn lower(row: &Row) -> String {
        match row.pattern {
            Pattern::Extension(ext) => format!("a.{ext}"),
            Pattern::Name(name) => name.to_string(),
        }
    }

    #[test]
    fn watched_files_is_the_glob_of_the_table() {
        assert_eq!(WATCHED_FILES.to_vec(), globs());
        // No `{...}` group with a `*` in it, which Neovim cannot match.
        for glob in WATCHED_FILES {
            let groups = glob.split('{').skip(1).map(|g| g.split('}').next());
            assert!(groups.flatten().all(|g| !g.contains('*')), "{glob}");
        }
    }

    #[test]
    fn the_rdf_rows_are_what_from_extension_reads() {
        for row in ROWS {
            let rdf = match row.kind {
                FileKind::Rdf(s) => Some(s),
                _ => None,
            };
            for name in [lower(row), upper(row)] {
                for path in [
                    name.clone(),
                    format!("file:///w/{name}"),
                    format!("C:\\w\\{name}"),
                ] {
                    assert_eq!(Syntax::from_extension(&path), rdf, "{path}");
                }
            }
        }
    }

    #[test]
    fn every_row_keeps_its_kind_and_case_rule() {
        for row in ROWS {
            let name = lower(row);
            assert_eq!(FileKind::of(&name), Some(row.kind), "{name}");
            let other = upper(row);
            let expected = (row.case == Case::Any).then_some(row.kind);
            assert_eq!(FileKind::of(&other), expected, "{other}");
        }
    }

    #[test]
    fn doc_kind_reads_the_table() {
        for row in ROWS {
            for name in [lower(row), upper(row)] {
                let uri = format!("file:///w/{name}");
                let expected = match FileKind::of(&uri) {
                    Some(FileKind::Rdf(s)) => DocKind::Rdf(s),
                    Some(FileKind::Sparql) => DocKind::Sparql,
                    _ => DocKind::Rdf(Syntax::Turtle),
                };
                assert_eq!(DocKind::detect(None, &uri), expected, "{uri}");
                assert_eq!(DocKind::detect(Some("turtle"), &uri), expected, "{uri}");
            }
        }
    }

    #[test]
    fn paths_keep_the_kind_the_old_predicates_gave() {
        use FileKind::*;
        let cases: &[(&str, Option<FileKind>)] = &[
            ("a.ttl", Some(Rdf(Syntax::Turtle))),
            ("a.TTL", Some(Rdf(Syntax::Turtle))),
            ("/w/x.Turtle", Some(Rdf(Syntax::Turtle))),
            ("/w/g.trig", Some(Rdf(Syntax::TriG))),
            ("/w/a.NT", Some(Rdf(Syntax::NTriples))),
            ("/w/a.nq", Some(Rdf(Syntax::NQuads))),
            ("/w/ctx.jsonld", Some(Rdf(Syntax::JsonLd))),
            ("/w/.ttl", Some(Rdf(Syntax::Turtle))),
            ("/w/prefixes.ttl", Some(Rdf(Syntax::Turtle))),
            ("/w/a.b.ttl", Some(Rdf(Syntax::Turtle))),
            ("a.rq", Some(Sparql)),
            ("/w/q.sparql", Some(Sparql)),
            ("/w/u.ru", Some(Sparql)),
            ("a.RQ", None),
            ("/w/q.Sparql", None),
            ("/w/u.RU", None),
            ("a.json", Some(Json)),
            ("/w/A.JSON", Some(Json)),
            ("/w/turtle-lsp.json", Some(Json)),
            ("catalog-v001.xml", Some(Catalog)),
            ("/w/.ontologies/catalog-v001.xml", Some(Catalog)),
            ("/w/Catalog-v001.xml", None),
            ("/w/catalog.xml", None),
            ("/w/x.owl", Some(ImportSource)),
            ("x.OWL", Some(ImportSource)),
            ("/w/x.rdf", Some(ImportSource)),
            ("/w/x.Rdf", Some(ImportSource)),
            ("/w/notes.txt", None),
            ("/w/x.xml", None),
            ("/w/ttl", None),
            ("/w/folder", None),
            ("/w/a.ttl.bak", None),
            ("", None),
        ];
        for &(path, kind) in cases {
            assert_eq!(FileKind::of(path), kind, "{path}");
            let p = Path::new(path);
            assert_eq!(is_rdf_file(p), before::is_rdf_file(p), "{path}");
            assert_eq!(is_rdf_file(p), matches!(kind, Some(Rdf(_))), "{path}");
            assert_eq!(is_sparql_name(path), before::is_sparql_name(path), "{path}");
            assert_eq!(is_sparql_path(p), before::is_sparql_name(path), "{path}");
            assert_eq!(is_import_source(p), before::is_import_source(p), "{path}");
            assert_eq!(is_watched_file(p), before::is_watched_file(p), "{path}");
        }
    }
}
