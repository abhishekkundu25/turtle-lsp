//! The background thread: it takes jobs, resolves their imports (workspace,
//! catalogs, cache, network) and sends the answers back.

use super::catalog::Catalog;
use super::load::load;
use super::*;

// ---- background thread ----

#[derive(Default)]
pub(super) struct Memo {
    /// Path -> (modified, length, base IRI, parsed ontology).
    pub(super) files: FxHashMap<PathBuf, (SystemTime, u64, String, Ontology)>,
    /// URL -> when fetching it failed, and why.
    pub(super) failed: FxHashMap<String, (Instant, String)>,
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
        memo.failed.remove(without_fragment(iri));
    }
    let catalog = Catalog::scan(&req.roots);
    let local: FxHashSet<&str> = req.local.iter().map(String::as_str).collect();
    let mut queue: VecDeque<(String, u32)> = req.imports.iter().map(|i| (i.clone(), 0)).collect();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let mut status = Vec::new();
    let mut loaded = Vec::new();
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
            match load(&iri, req, &catalog, memo) {
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
            "<http://ex.com/b> <http://www.w3.org/2002/07/owl#imports> <http://ex.com/a#>, <http://ex.com/c>, <http://ex.com/local> .\n",
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
        memo.failed
            .insert(c.clone(), (Instant::now(), "earlier".into()));
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
}
