//! The background thread: it takes jobs, resolves their imports (workspace,
//! catalogs, cache, network) and sends the answers back.

use super::catalog::Catalog;
use super::load::{Budget, load};
use super::*;

// ---- background thread ----

#[derive(Default)]
pub(super) struct Memo {
    /// Path -> (modified, length, base IRI, parsed ontology).
    pub(super) files: FxHashMap<PathBuf, (SystemTime, u64, String, Ontology)>,
    /// Import IRI (see `norm`) -> when fetching it failed, from which URL,
    /// and why. Under the import, not the URL, so that fetching the import
    /// again finds it whatever a catalog maps it to.
    pub(super) failed: FxHashMap<String, (Instant, String, String)>,
}

/// A failed fetch is not tried again for this long.
pub(super) const RETRY_FETCH_AFTER: Duration = Duration::from_secs(300);
/// A job waits this long for a newer one first, so editing an import IRI
/// neither rescans the workspace nor fetches every intermediate spelling.
const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone)]
pub(super) struct Ontology {
    pub(super) analysis: Arc<Analysis>,
    pub(super) imports: Vec<String>,
}

/// An import IRI containing this makes the resolver panic, in tests.
#[cfg(test)]
const PANIC_IMPORT: &str = "turtle-lsp-test-panic";

/// Why imports are unresolved when their thread ended in the middle of a pass.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const CRASHED: &str = "the import resolver stopped after an internal error (see the server's log); change the imports or restart to try again";
/// Why imports are unresolved when no thread could be started for them.
pub(super) const NO_THREAD: &str = "the import resolver could not be started";

/// The answer of pass `generation` that resolved none of `imports`.
pub(super) fn failed(generation: u64, imports: &[String], why: &str) -> Done {
    Done {
        generation,
        status: imports
            .iter()
            .map(|i| (i.clone(), Status::Unresolved(why.to_string())))
            .collect(),
        loaded: vec![],
    }
}

pub(super) fn worker(jobs: Receiver<Job>, out: Sender<Done>) {
    let mut memo = Memo::default();
    while let Ok(mut job) = jobs.recv() {
        // Only the latest job matters.
        // A superseded job's refetch still counts.
        let take = |job: &mut Job, mut j: Job| {
            j.refetch = j.refetch.or(job.refetch.take());
            *job = j;
        };
        loop {
            while let Ok(j) = jobs.try_recv() {
                take(&mut job, j);
            }
            // Every keystroke on an import line is a new job: wait for typing
            // to pause before walking the workspace for catalogs (or fetching).
            match jobs.recv_timeout(DEBOUNCE) {
                Ok(j) => take(&mut job, j),
                Err(_) => break,
            }
        }
        let superseded = || !jobs.is_empty();
        // A bug on one pass must not leave the main loop waiting for its
        // answer: that pass is unresolved, with the reason, and the thread
        // goes on with a memo that starts over.
        let attempt = crate::server::worker::catch("resolving imports", || {
            #[cfg(test)]
            if job.req.imports.iter().any(|i| i.contains(PANIC_IMPORT)) {
                panic!("injected resolver panic");
            }
            resolve(&job, &mut memo, &superseded)
        });
        let done = match attempt {
            Ok(Some(done)) => done,
            Ok(None) => continue,
            Err(msg) => {
                memo = Memo::default();
                failed(job.generation, &job.req.imports, &msg)
            }
        };
        if out.send(done).is_err() {
            break;
        }
    }
}

/// Resolve a job's imports transitively; `None` when a newer job arrived.
pub(super) fn resolve(job: &Job, memo: &mut Memo, superseded: &dyn Fn() -> bool) -> Option<Done> {
    let req = &job.req;
    if let Some(iri) = &job.refetch {
        memo.failed.remove(norm(iri));
    }
    let catalog = Catalog::scan(&req.roots);
    let local: FxHashSet<&str> = req.local.iter().map(String::as_str).collect();
    let mut queue: VecDeque<(String, u32)> = req.imports.iter().map(|i| (i.clone(), 0)).collect();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let mut status = Vec::new();
    let mut loaded = Vec::new();
    // What this pass may fetch: more is for the next pass, with a note.
    let mut budget = Budget::pass();
    while let Some((iri, depth)) = queue.pop_front() {
        if !seen.insert(norm(&iri).to_string()) {
            continue;
        }
        if superseded() {
            return None;
        }
        let st = if local.contains(norm(&iri)) {
            Status::Local
        } else {
            match load(&iri, req, &catalog, memo, &mut budget) {
                Ok((location, onto, fresh)) => {
                    if depth < MAX_DEPTH {
                        queue.extend(onto.imports.iter().map(|i| (i.clone(), depth + 1)));
                    }
                    loaded.push(Loaded {
                        key: format!("{KEY_PREFIX}{iri}"),
                        facts: fresh.then(|| DocFacts::new(&onto.analysis)),
                        analysis: onto.analysis,
                    });
                    Status::Loaded(location)
                }
                Err(msg) => Status::Unresolved(msg),
            }
        };
        status.push((iri, st));
    }
    // `<x>` and `<x#>` are one import; each spelling gets its status.
    for i in &req.imports {
        if !status.iter().any(|(s, _)| s == i)
            && let Some(st) = status
                .iter()
                .find(|(s, _)| norm(s) == norm(i))
                .map(|(_, st)| st.clone())
        {
            status.push((i.clone(), st));
        }
    }
    Some(Done {
        generation: job.generation,
        status,
        loaded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::hash_name;
    use crate::imports::catalog::Target;
    use crate::imports::testing::*;
    use std::fs;

    fn job(imports: &[&str], local: &[&str], roots: Vec<PathBuf>, cache: Option<PathBuf>) -> Job {
        Job {
            generation: 1,
            req: Request {
                imports: imports.iter().map(|s| s.to_string()).collect(),
                local: local.iter().map(|s| s.to_string()).collect(),
                roots,
                cache,
                network: false,
                fetch: vec![],
                allow_private: true,
            },
            refetch: None,
        }
    }

    #[test]
    fn local_imports_outside_the_workspace_are_not_read() {
        let root = tmp("outside");
        let ws = root.join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(
            root.join("private.ttl"),
            "<http://ex.com/p> a <http://ex.com/C> .\n",
        )
        .unwrap();
        fs::write(
            ws.join(CATALOG_FILE),
            r#"<catalog><uri name="http://ex.com/p" uri="../private.ttl"/></catalog>"#,
        )
        .unwrap();
        let file_iri = url::Url::from_file_path(root.join("private.ttl"))
            .unwrap()
            .to_string();
        let j = job(&["http://ex.com/p", &file_iri], &[], vec![ws.clone()], None);
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert!(done.loaded.is_empty(), "{:?}", done.status);
        for (_, st) in &done.status {
            assert!(
                matches!(st, Status::Unresolved(m) if m.contains("outside the workspace")),
                "{st:?}"
            );
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn imports_resolve_transitively_through_catalog_and_cache_with_cycles() {
        let root = tmp("transitive");
        let ws = root.join("ws");
        fs::create_dir_all(ws.join("sub")).unwrap();
        let cache = root.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            ws.join("sub").join(CATALOG_FILE),
            r#"<catalog><uri name="http://ex.com/a" uri="../a.ttl"/></catalog>"#,
        )
        .unwrap();
        // a imports b (cache) and itself; b imports a (cycle) and c (missing) and the local one.
        fs::write(
            ws.join("a.ttl"),
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<> owl:imports <http://ex.com/b>, <http://ex.com/a> .\n<#T> a owl:Class .\n",
        )
        .unwrap();
        fs::write(
            cache.join(format!("{}.ttl", hash_name("http://ex.com/b"))),
            crate::fetch::with_header(
                "http://ex.com/b",
                "<http://ex.com/b> <http://www.w3.org/2002/07/owl#imports> <http://ex.com/a#>, <http://ex.com/c>, <http://ex.com/local> .\n",
            ),
        )
        .unwrap();
        let j = job(
            &["http://ex.com/a"],
            &["http://ex.com/local"],
            vec![ws.clone()],
            Some(cache.clone()),
        );
        let mut memo = Memo::default();
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        let st: FxHashMap<_, _> = done.status.iter().cloned().collect();
        assert!(matches!(st["http://ex.com/a"], Status::Loaded(_)), "{st:?}");
        assert!(matches!(st["http://ex.com/b"], Status::Loaded(_)), "{st:?}");
        assert_eq!(st["http://ex.com/local"], Status::Local);
        assert!(
            matches!(&st["http://ex.com/c"], Status::Unresolved(m) if m.contains("turtle.imports.network")),
            "{st:?}"
        );
        // The cycle back to a#: visited once.
        assert_eq!(done.status.len(), 4, "{st:?}");
        assert_eq!(done.loaded.len(), 2);
        // a imports <http://ex.com/a#> (via b): the same import, same status.
        assert!(
            done.status
                .iter()
                .all(|(_, s)| !matches!(s, Status::Unresolved(m) if m.contains("http://ex.com/a")))
        );
        // Relative IRIs resolve against the import IRI.
        let a = &done
            .loaded
            .iter()
            .find(|l| l.key.ends_with("/a"))
            .unwrap()
            .analysis;
        assert!(a.defs.contains_key("http://ex.com/a#T"));
        assert!(a.uri.starts_with("file:"), "{}", a.uri);
        assert!(done.loaded.iter().all(|l| l.facts.is_some()));
        // Unchanged files are not parsed again.
        let again = resolve(&j, &mut memo, &|| false).unwrap();
        assert!(again.loaded.iter().all(|l| l.facts.is_none()));
        assert!(Arc::ptr_eq(
            &again.loaded[0].analysis,
            &done.loaded[0].analysis
        ));
        // A newer job supersedes this one.
        assert!(resolve(&j, &mut memo, &|| true).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn depth_is_limited() {
        let root = tmp("depth");
        let mut catalog = String::from("<catalog>");
        for i in 0..10 {
            fs::write(
                root.join(format!("o{i}.ttl")),
                format!(
                    "<http://ex.com/o{i}> <{OWL_IMPORTS}> <http://ex.com/o{}> .\n",
                    i + 1
                ),
            )
            .unwrap();
            catalog.push_str(&format!(
                "<uri name=\"http://ex.com/o{i}\" uri=\"o{i}.ttl\"/>"
            ));
        }
        catalog.push_str("</catalog>");
        fs::write(root.join(CATALOG_FILE), catalog).unwrap();
        let j = job(&["http://ex.com/o0"], &[], vec![root.clone()], None);
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        // o0 (direct) plus 5 levels below it.
        assert_eq!(done.loaded.len(), 1 + MAX_DEPTH as usize);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fetch_loads_only_the_listed_import_and_names_it_by_its_cache_file() {
        let root = tmp("fetch");
        let cache = root.join("cache");
        let (base, log) = serve();
        let (a, b) = (format!("{base}/ok-a"), format!("{base}/ok-b"));
        let mut j = job(&[&a, &b], &[], vec![root.clone()], Some(cache.clone()));
        // Listed as `<a#>`: the same import.
        j.req.fetch = vec![format!("{a}#")];
        let mut memo = Memo::default();
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        let st: FxHashMap<_, _> = done.status.iter().cloned().collect();
        assert_eq!(st[&a], Status::Loaded(a.clone()));
        assert!(
            matches!(&st[&b], Status::Unresolved(m) if m.contains("quick fix") && m.contains("turtle.imports.network")),
            "{st:?}"
        );
        assert_eq!(*log.lock().unwrap(), ["/ok-a"]);
        let check = |done: &Done| {
            let l = &done.loaded[0].analysis;
            let path = crate::workspace::uri_to_path(&l.uri).unwrap();
            assert_eq!(path, cache.join(format!("{}.ttl", hash_name(&a))));
            assert_eq!(l.origin.as_deref(), Some(a.as_str()));
            assert!(l.defs.contains_key(format!("{a}#T").as_str()));
        };
        check(&done);
        // Next session: a cache hit, named the same way, without a request.
        let j = job(&[&a], &[], vec![root.clone()], Some(cache.clone()));
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert_eq!(done.status, [(a.clone(), Status::Loaded(a.clone()))]);
        check(&done);
        assert_eq!(log.lock().unwrap().len(), 1);
        // No cache directory: named by the URL, as before.
        let mut j = job(&[&a], &[], vec![root.clone()], None);
        j.req.fetch = vec![a.clone()];
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert_eq!(done.loaded[0].analysis.uri, a);
        assert_eq!(done.loaded[0].analysis.origin, None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn refetch_tries_a_recently_failed_import_again() {
        let root = tmp("refetch");
        let (base, log) = serve();
        let c = format!("{base}/missing");
        let mut j = job(&[&c], &[], vec![root.clone()], None);
        j.req.fetch = vec![c.clone()];
        let mut memo = Memo::default();
        memo.failed.insert(
            norm(&c).to_string(),
            (Instant::now(), c.clone(), "earlier".into()),
        );
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        assert_eq!(
            done.status,
            [(c.clone(), Status::Unresolved("earlier".into()))]
        );
        assert!(log.lock().unwrap().is_empty());
        j.refetch = Some(c.clone());
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        assert!(
            matches!(&done.status[0].1, Status::Unresolved(m) if m.contains("could not be fetched")),
            "{:?}",
            done.status
        );
        assert_eq!(*log.lock().unwrap(), ["/missing"]);
        let _ = fs::remove_dir_all(&root);
    }

    const OWL: &str = "http://www.w3.org/2002/07/owl#";

    fn statuses(done: &Done) -> FxHashMap<String, Status> {
        done.status.iter().cloned().collect()
    }

    fn unresolved_with<'a>(st: &'a FxHashMap<String, Status>, iri: &str, what: &str) -> &'a Status {
        let s = &st[iri];
        assert!(
            matches!(s, Status::Unresolved(m) if m.contains(what)),
            "{iri}: {s:?} (wanted {what:?})"
        );
        s
    }

    #[test]
    fn a_trig_and_a_jsonld_import_are_read_in_their_own_syntax() {
        use crate::syntax::Syntax;
        let root = tmp("syntaxes");
        fs::write(
            root.join("g.trig"),
            format!(
                "@prefix owl: <{OWL}> .\n<http://ex.com/g> owl:imports <http://ex.com/j> .\n<http://ex.com/graph> {{ <http://ex.com/g> a owl:Ontology . }}\n"
            ),
        )
        .unwrap();
        fs::write(
            root.join("j.jsonld"),
            format!(r#"{{"@id": "http://ex.com/j", "@type": "{OWL}Ontology", "{OWL}imports": {{"@id": "http://ex.com/q"}}}}"#),
        )
        .unwrap();
        fs::write(
            root.join("q.nq"),
            "<http://ex.com/q> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Ontology> <http://ex.com/graph> .\n",
        )
        .unwrap();
        // The import IRIs say nothing about syntax: no extension.
        fs::write(
            root.join(CATALOG_FILE),
            r#"<catalog><uri name="http://ex.com/g" uri="g.trig"/><uri name="http://ex.com/j" uri="j.jsonld"/><uri name="http://ex.com/q" uri="q.nq"/></catalog>"#,
        )
        .unwrap();
        let j = job(&["http://ex.com/g"], &[], vec![root.clone()], None);
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        let st = statuses(&done);
        for iri in ["http://ex.com/g", "http://ex.com/j", "http://ex.com/q"] {
            assert!(matches!(st[iri], Status::Loaded(_)), "{iri}: {st:?}");
        }
        let syntax = |key: &str| {
            done.loaded
                .iter()
                .find(|l| l.key.ends_with(key))
                .unwrap()
                .analysis
                .parsed
                .syntax
        };
        assert_eq!(syntax("/g"), Syntax::TriG);
        assert_eq!(syntax("/j"), Syntax::JsonLd);
        assert_eq!(syntax("/q"), Syntax::NQuads);
        // Each was read: the triples of the TriG graph, of the JSON-LD
        // object (an import chain proves it) and of the N-Quads line.
        let triples = |key: &str| {
            done.loaded
                .iter()
                .find(|l| l.key.ends_with(key))
                .unwrap()
                .analysis
                .parsed
                .triples
                .len()
        };
        assert_eq!((triples("/g"), triples("/j"), triples("/q")), (2, 2, 1));
        // An import IRI that ends in `.trig` does not make a Turtle file TriG.
        fs::write(root.join("t.ttl"), "<http://ex.com/t.trig> a <urn:x:C> .\n").unwrap();
        fs::write(
            root.join(CATALOG_FILE),
            r#"<catalog><uri name="http://ex.com/t.trig" uri="t.ttl"/></catalog>"#,
        )
        .unwrap();
        let j = job(&["http://ex.com/t.trig"], &[], vec![root.clone()], None);
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert_eq!(done.loaded[0].analysis.parsed.syntax, Syntax::Turtle);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_host_in_a_file_import_or_a_catalog_target_is_refused_on_every_system() {
        let root = tmp("hosts");
        fs::write(
            root.join(CATALOG_FILE),
            r#"<catalog>
<uri name="http://ex.com/a" uri="file://example.invalid/s/a.ttl"/>
<uri name="http://ex.com/b" uri="//example.invalid/s/b.ttl"/>
<uri name="http://ex.com/c" uri="file://localhost{NOWHERE}"/>
</catalog>"#
                .replace(
                    "{NOWHERE}",
                    if cfg!(windows) {
                        "/C:/nowhere/c.ttl"
                    } else {
                        "/nowhere/c.ttl"
                    },
                ),
        )
        .unwrap();
        let direct = "file://example.invalid/s/x.ttl";
        let j = job(
            &[
                "http://ex.com/a",
                "http://ex.com/b",
                "http://ex.com/c",
                direct,
            ],
            &[],
            vec![root.clone()],
            None,
        );
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        let st = statuses(&done);
        for iri in ["http://ex.com/a", "http://ex.com/b", direct] {
            unresolved_with(&st, iri, "naming a host");
        }
        // `localhost` is this machine: a file there is outside the workspace.
        unresolved_with(&st, "http://ex.com/c", "outside the workspace");
        assert!(done.loaded.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_mapping_to_a_host_is_kept_as_the_url_for_loading_to_refuse() {
        let dir = Path::new(if cfg!(windows) { "C:\\p\\q" } else { "/p/q" });
        let got = parse_catalog(
            r#"<catalog>
<uri name="http://ex.com/a" uri="file://example.invalid/s/a.ttl"/>
<uri name="http://ex.com/b" uri="//example.invalid/s/b.ttl"/>
<uri name="http://ex.com/c" uri="file:///p/q/c.ttl"/>
</catalog>"#,
            dir,
        );
        assert!(
            matches!(&got[0].1, Target::Url(u) if u == "file://example.invalid/s/a.ttl"),
            "{got:?}"
        );
        assert!(
            matches!(&got[1].1, Target::Url(u) if u == "file://example.invalid/s/b.ttl"),
            "{got:?}"
        );
        if !cfg!(windows) {
            assert_eq!(got[2].1, Target::File(PathBuf::from("/p/q/c.ttl")));
        }
    }

    #[test]
    fn a_cache_file_must_name_the_url_it_is_read_for() {
        let root = tmp("header");
        let cache = root.join("cache");
        fs::create_dir_all(&cache).unwrap();
        let (base, log) = serve();
        let a = format!("{base}/ok-a");
        let path = cache.join(format!("{}.ttl", hash_name(&a)));
        // A planted file with the right name and no header, and one with
        // another URL's header (as a hash collision would give).
        for planted in [
            "<http://evil/x> a <urn:x:Planted> .\n".to_string(),
            crate::fetch::with_header("http://other/", "<http://evil/x> a <urn:x:Planted> .\n"),
        ] {
            fs::write(&path, planted).unwrap();
            let mut j = job(&[&a], &[], vec![root.clone()], Some(cache.clone()));
            j.req.fetch = vec![a.clone()];
            let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
            let l = &done.loaded[0].analysis;
            assert!(
                l.defs.contains_key(format!("{a}#T").as_str()),
                "fetched, not planted"
            );
            assert!(!l.defs.contains_key("http://evil/x"));
            // The fetched file replaced it, and names its URL.
            let text = fs::read_to_string(&path).unwrap();
            assert!(crate::fetch::header_matches(text.as_bytes(), &a), "{text}");
        }
        assert_eq!(log.lock().unwrap().len(), 2);
        // With the network off and nothing valid cached, it is not resolved.
        fs::write(&path, "<http://evil/x> a <urn:x:Planted> .\n").unwrap();
        let j = job(&[&a], &[], vec![root.clone()], Some(cache.clone()));
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert!(done.loaded.is_empty(), "{:?}", done.status);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fetched_trig_or_jsonld_document_is_cached_and_read_in_its_syntax() {
        use crate::syntax::Syntax;
        let root = tmp("fetch-syntax");
        let cache = root.join("cache");
        let (base, log) = serve_with(|path| match path {
            "/g" => document(
                "application/trig",
                "<http://ex.com/g> { <urn:x:a> a <urn:x:C> . }\n",
            ),
            "/j" => document(
                "application/ld+json",
                r#"{"@id": "urn:x:a", "@type": "urn:x:C"}"#,
            ),
            "/n.nq" => document("text/plain", "<urn:x:a> <urn:x:p> <urn:x:b> <urn:x:g> .\n"),
            _ => NOT_FOUND.into(),
        });
        let iris: Vec<String> = ["g", "j", "n.nq"]
            .iter()
            .map(|p| format!("{base}/{p}"))
            .collect();
        let refs: Vec<&str> = iris.iter().map(String::as_str).collect();
        let mut j = job(&refs, &[], vec![root.clone()], Some(cache.clone()));
        j.req.network = true;
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        let syntaxes: Vec<Syntax> = ["/g", "/j", "/n.nq"]
            .iter()
            .map(|k| {
                done.loaded
                    .iter()
                    .find(|l| l.key.ends_with(k))
                    .unwrap_or_else(|| panic!("{k}: {:?}", done.status))
                    .analysis
                    .parsed
                    .syntax
            })
            .collect();
        assert_eq!(syntaxes, [Syntax::TriG, Syntax::JsonLd, Syntax::NQuads]);
        // Cached under a name that says which syntax; the next session reads
        // them from there, in the same syntax, without a request.
        let mut names: Vec<String> = fs::read_dir(&cache)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), 4, "{names:?}");
        for ext in ["trig", "jsonld", "nq", "url"] {
            assert!(names.iter().any(|n| n.ends_with(ext)), "{ext}: {names:?}");
        }
        let asked = log.lock().unwrap().len();
        j.req.network = false;
        let again = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert_eq!(log.lock().unwrap().len(), asked);
        let syntaxes: Vec<Syntax> = again
            .loaded
            .iter()
            .map(|l| l.analysis.parsed.syntax)
            .collect();
        assert_eq!(syntaxes, [Syntax::TriG, Syntax::JsonLd, Syntax::NQuads]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn asking_to_fetch_again_works_for_an_import_a_catalog_maps_to_a_url() {
        let root = tmp("refetch-catalog");
        let (base, log) = serve();
        let iri = "http://ex.com/mapped";
        let target = format!("{base}/missing");
        fs::write(
            root.join(CATALOG_FILE),
            format!(r#"<catalog><uri name="{iri}" uri="{target}"/></catalog>"#),
        )
        .unwrap();
        let mut j = job(&[iri], &[], vec![root.clone()], None);
        j.req.fetch = vec![iri.to_string()];
        let mut memo = Memo::default();
        resolve(&j, &mut memo, &|| false).unwrap();
        assert_eq!(*log.lock().unwrap(), ["/missing"]);
        // Remembered: not asked again...
        resolve(&j, &mut memo, &|| false).unwrap();
        assert_eq!(log.lock().unwrap().len(), 1);
        // ...until the user asks to fetch this import again. (The failure
        // used to be kept under the catalog's URL, which the IRI never named.)
        j.refetch = Some(iri.to_string());
        resolve(&j, &mut memo, &|| false).unwrap();
        assert_eq!(*log.lock().unwrap(), ["/missing", "/missing"]);
        // A different mapping is not held to the old failure.
        fs::write(
            root.join(CATALOG_FILE),
            format!(r#"<catalog><uri name="{iri}" uri="{base}/ok-new"/></catalog>"#),
        )
        .unwrap();
        j.refetch = None;
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        assert!(
            matches!(statuses(&done)[iri], Status::Loaded(_)),
            "{:?}",
            done.status
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pass_stops_at_its_budget_and_says_so_and_the_next_pass_goes_on() {
        let root = tmp("budget");
        let (base, log) = serve();
        let n = crate::fetch::PASS_REQUESTS as usize + 5;
        let iris: Vec<String> = (0..n).map(|i| format!("{base}/missing{i:03}")).collect();
        let refs: Vec<&str> = iris.iter().map(String::as_str).collect();
        let mut j = job(&refs, &[], vec![root.clone()], None);
        j.req.network = true;
        let mut memo = Memo::default();
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        let st = statuses(&done);
        assert_eq!(
            log.lock().unwrap().len(),
            crate::fetch::PASS_REQUESTS as usize
        );
        let spent = st
            .values()
            .filter(|s| matches!(s, Status::Unresolved(m) if m.contains("fetch budget")))
            .count();
        assert_eq!(spent, 5, "{st:?}");
        // The five that were not asked are not remembered as failures, so
        // the next pass asks them and no others.
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        assert_eq!(log.lock().unwrap().len(), n);
        assert!(
            statuses(&done)
                .values()
                .all(|s| matches!(s, Status::Unresolved(m) if !m.contains("fetch budget")))
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn loopback_and_private_addresses_are_not_fetched_unless_allowed() {
        let root = tmp("internal");
        let (base, log) = serve();
        let mut j = job(
            &[
                &format!("{base}/ok-a"),
                "http://169.254.169.254/latest/meta-data/",
                "http://10.0.0.1/o",
                "http://localhost:9/o",
            ],
            &[],
            vec![root.clone()],
            None,
        );
        j.req.network = true;
        j.req.allow_private = false;
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        let st = statuses(&done);
        assert!(done.loaded.is_empty(), "{st:?}");
        for (iri, why) in [
            (j.req.imports[0].as_str(), "loopback"),
            ("http://169.254.169.254/latest/meta-data/", "cloud metadata"),
            ("http://10.0.0.1/o", "private"),
            ("http://localhost:9/o", "loopback"),
        ] {
            unresolved_with(&st, iri, why);
        }
        assert!(log.lock().unwrap().is_empty(), "nothing was requested");
        let mut j = job(&[&format!("{base}/ok-a")], &[], vec![root.clone()], None);
        j.req.network = true;
        j.req.allow_private = true;
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert!(matches!(
            statuses(&done)[&format!("{base}/ok-a")],
            Status::Loaded(_)
        ));
        let _ = fs::remove_dir_all(&root);
    }
}
