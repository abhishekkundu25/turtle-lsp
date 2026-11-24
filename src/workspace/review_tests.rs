use super::*;

const QUERY: &str = "SELECT * WHERE { ?s ?p ?o }\n";

/// A fresh folder below the temp dir (canonical), removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!(
            "turtle-lsp-review-ws-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(canonical(&d))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn uris(ws: &mut Workspace) -> Vec<String> {
    let mut v: Vec<String> = ws
        .sparql_documents()
        .iter()
        .map(|q| q.uri.clone())
        .collect();
    v.sort();
    v
}

fn query(ws: &mut Workspace, path: &Path) -> Arc<SparqlAnalysis> {
    let uri = path_to_uri(path).unwrap();
    ws.sparql_documents()
        .into_iter()
        .find(|q| q.uri == uri)
        .unwrap()
}

/// Without file watching, nothing reports a query file created later: the
/// folders are listed again on each call, and the files that did not
/// change are not parsed again.
#[test]
fn without_file_watching_new_query_files_are_found_on_the_next_call() {
    let root = Dir::new("rescan");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    ws.rescan_sparql = true;
    let a = root.0.join("a.rq");
    std::fs::write(&a, QUERY).unwrap();
    assert_eq!(uris(&mut ws), [path_to_uri(&a).unwrap()]);
    let kept = query(&mut ws, &a);

    let b = root.0.join("q").join("b.sparql");
    std::fs::create_dir_all(b.parent().unwrap()).unwrap();
    std::fs::write(&b, QUERY).unwrap();
    std::fs::create_dir_all(root.0.join("node_modules")).unwrap();
    std::fs::write(root.0.join("node_modules").join("x.rq"), QUERY).unwrap();
    let mut both = vec![path_to_uri(&a).unwrap(), path_to_uri(&b).unwrap()];
    both.sort();
    assert_eq!(uris(&mut ws), both);
    assert!(Arc::ptr_eq(&kept, &query(&mut ws, &a)), "a.rq parsed again");

    std::fs::remove_file(&b).unwrap();
    assert_eq!(uris(&mut ws), [path_to_uri(&a).unwrap()]);
}

/// With file watching, the index follows the watcher's events and reads
/// no folder again.
#[test]
fn with_file_watching_new_query_files_come_from_the_events() {
    let root = Dir::new("watched");
    let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
    assert!(!ws.rescan_sparql);
    let a = root.0.join("a.rq");
    std::fs::write(&a, QUERY).unwrap();
    assert_eq!(uris(&mut ws).len(), 1);
    let b = root.0.join("b.rq");
    std::fs::write(&b, QUERY).unwrap();
    assert_eq!(uris(&mut ws).len(), 1);
    ws.sparql_files_changed(std::slice::from_ref(&b));
    assert_eq!(uris(&mut ws).len(), 2);
}
