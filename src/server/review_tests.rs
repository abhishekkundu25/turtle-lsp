use super::*;
use crate::workspace::path_to_uri;
use serde_json::json;

/// A fresh folder below the temp dir (canonical), removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d =
            std::env::temp_dir().join(format!("turtle-lsp-review-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(crate::workspace::real(&d))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A server (not started) for the workspace folder `root`, with the
/// client's connection end, which must outlive it.
fn server_at(root: &std::path::Path, init: Value) -> (Server, wire::Outbox) {
    let (here, host) = wire::memory();
    let root = path_to_uri(root).unwrap();
    let (server, _, _) = Server::configure(
        here,
        json!({
            "capabilities": {},
            "workspaceFolders": [{"uri": root, "name": "w"}],
            "initializationOptions": init,
        }),
        Spawn::Thread,
    )
    .unwrap();
    (server, host)
}

fn configuration(settings: Value) -> Notification {
    Notification::new(
        "workspace/didChangeConfiguration".into(),
        json!({ "settings": settings }),
    )
}

fn event(p: &std::path::Path, typ: FileChangeType) -> FileEvent {
    FileEvent {
        uri: path_to_uri(p).unwrap().parse().unwrap(),
        typ,
    }
}

fn first_subject(server: &Server, path: &std::path::Path) -> String {
    let uri = path_to_uri(path).unwrap();
    match &server.ws.indexed_analysis(&uri).unwrap().parsed.triples[0].s {
        crate::syntax::Node::Iri(i) => i.as_str().to_string(),
        other => panic!("{other:?}"),
    }
}

/// The settings are the `turtle` section of the notification's
/// `settings`: the shape the VS Code extension sends.
#[test]
fn a_configuration_change_is_read_from_its_turtle_section() {
    let dir = Dir::new("shape");
    let (mut s, _client) = server_at(&dir.0, Value::Null);
    s.notification(configuration(json!({"format": {"lineWidth": 80}})));
    assert_eq!(
        s.layers.settings().format_line_width,
        100,
        "no turtle section"
    );
    s.notification(configuration(
        json!({"turtle": {"format": {"lineWidth": 80}}}),
    ));
    assert_eq!(s.layers.settings().format_line_width, 80);
}

/// A key the user removed is not kept from `initializationOptions`: the
/// client's latest section replaces its start-up snapshot, and the key
/// comes from turtle-lsp.json again (or the default).
#[test]
fn a_configuration_section_replaces_the_initialization_options() {
    let dir = Dir::new("layers");
    let (mut s, _client) = server_at(&dir.0, json!({"format": {"lineWidth": 80}}));
    assert_eq!(s.layers.settings().format_line_width, 80);
    s.notification(configuration(
        json!({"turtle": {"codeLens": {"references": false}}}),
    ));
    assert_eq!(s.layers.settings().format_line_width, 100);
    assert!(!s.layers.settings().code_lens_references);

    let dir = Dir::new("layers-file");
    std::fs::write(
        dir.0.join(SETTINGS_FILE),
        r#"{"format": {"lineWidth": 90}}"#,
    )
    .unwrap();
    let (mut s, _client) = server_at(&dir.0, json!({"format": {"lineWidth": 80}}));
    assert_eq!(s.layers.settings().format_line_width, 80);
    s.notification(configuration(json!({"turtle": {}})));
    assert_eq!(s.layers.settings().format_line_width, 90);
    // A client that only says "something changed" keeps what it sent.
    let (mut s, _client) = server_at(&dir.0, json!({"format": {"lineWidth": 80}}));
    s.notification(configuration(Value::Null));
    assert_eq!(s.layers.settings().format_line_width, 80);
}

/// A notification that changes a prefixes.ttl and a data file it
/// serves parses the data file once, with the new prefixes: not with the
/// old ones and then again in the background.
#[test]
fn prefixes_and_a_data_file_changed_together_parse_the_data_once_with_the_new_prefixes() {
    let dir = Dir::new("prefixes");
    let prefixes = dir.0.join("prefixes.ttl");
    let data = dir.0.join("data.ttl");
    std::fs::write(&prefixes, "@prefix ex: <http://old.example/> .\n").unwrap();
    std::fs::write(&data, "ex:a ex:p ex:o .\n").unwrap();
    let (mut s, _client) = server_at(&dir.0, Value::Null);
    s.watched_files_changed(vec![
        event(&data, FileChangeType::CREATED),
        event(&prefixes, FileChangeType::CREATED),
    ]);
    assert_eq!(first_subject(&s, &data), "http://old.example/a");

    std::fs::write(&prefixes, "@prefix ex: <http://newer.example/> .\n").unwrap();
    std::fs::write(&data, "ex:b ex:p ex:o .\n").unwrap();
    s.watched_files_changed(vec![
        event(&data, FileChangeType::CHANGED),
        event(&prefixes, FileChangeType::CHANGED),
    ]);
    assert_eq!(first_subject(&s, &data), "http://newer.example/b");
    // Nothing was left to parse again in the background.
    assert!(
        s.index
            .reparsed()
            .recv_timeout(Duration::from_millis(500))
            .is_err()
    );
}

/// A workspace-diagnostics thread that cannot start turns them off with
/// a warning, instead of panicking; a settings change turns them on again.
#[test]
fn a_workspace_diagnostics_thread_that_cannot_start_turns_them_off_with_a_warning() {
    let dir = Dir::new("wsdiag-start");
    let file = dir.0.join("a.ttl");
    std::fs::write(&file, "ex:a ex:p ex:o .\n").unwrap();
    let (mut s, host) = server_at(&dir.0, json!({"diagnostics": {"workspace": true}}));
    let roots = s.ws.roots().to_vec();
    let files = Workspace::new(roots.clone(), Default::default())
        .parse_paths(Workspace::scan_roots(&roots));
    s.merge_index(Ok(files));
    assert!(s.wsdiag.due().is_some(), "a pass is due");
    let _ = host.try_iter().count();

    worker::failing_starts(|| s.wsdiag_start());
    let sent: Vec<Message> = host.try_iter().collect();
    let warned = sent.iter().any(|m| {
            matches!(m, Message::Notification(n) if n.method == "window/showMessage"
                && n.params["message"].as_str().unwrap().contains("could not start its workspace diagnostics thread"))
        });
    assert!(warned, "{sent:?}");
    assert!(s.wsdiag.due().is_none(), "nothing is due any more");
    s.wsdiag.invalidate_all();
    assert!(s.wsdiag.due().is_none(), "they are off");

    // A settings change turns them on again, and the pass runs.
    s.notification(configuration(json!({"turtle": {
        "diagnostics": {"workspace": true},
        "format": {"lineWidth": 80},
    }})));
    assert!(s.wsdiag.due().is_some());
    s.wsdiag_start();
    let chunk = s
        .wsdiag
        .results()
        .recv_timeout(Duration::from_secs(10))
        .expect("a pass's results");
    let _ = host.try_iter().count();
    s.wsdiag_chunk(Some(chunk));
    let uri = path_to_uri(&file).unwrap();
    let published = host.try_iter().any(|m| {
        matches!(m, Message::Notification(n) if n.method == "textDocument/publishDiagnostics"
                && n.params["uri"] == uri.as_str())
    });
    assert!(published, "the closed file's diagnostics went out");
}

/// Catalogs, imported RDF/XML files and JSON-LD context files count
/// wherever the client reports them, also in folders the index skips.
#[test]
fn import_sources_and_context_files_in_skipped_folders_are_acted_on() {
    let dir = Dir::new("skipped");
    std::fs::create_dir_all(dir.0.join(".ctx")).unwrap();
    let ctx = dir.0.join(".ctx").join("ctx.jsonld");
    std::fs::write(&ctx, r#"{"@context": {"name": "http://ex.org/name"}}"#).unwrap();
    let (mut s, _client) = server_at(&dir.0, Value::Null);
    let doc = path_to_uri(&dir.0.join("a.jsonld")).unwrap();
    s.ws.open_doc(
        doc.clone(),
        r#"{"@context": ".ctx/ctx.jsonld", "@id": "http://ex.org/a", "name": "A"}"#.into(),
        1,
    );
    assert_eq!(s.ws.analysis(&doc).unwrap().parsed.triples.len(), 1);
    std::fs::write(&ctx, r#"{"@context": {"name": null}}"#).unwrap();
    s.watched_files_changed(vec![event(&ctx, FileChangeType::CHANGED)]);
    assert_eq!(s.ws.analysis(&doc).unwrap().parsed.triples.len(), 0);

    for source in [
        dir.0.join(".ontologies").join("catalog-v001.xml"),
        dir.0.join("venv").join("onto.owl"),
        dir.0.join("node_modules").join("x.rdf"),
    ] {
        let before = crate::imports::RETRIES.with(|c| c.get());
        s.watched_files_changed(vec![event(&source, FileChangeType::CHANGED)]);
        assert_eq!(
            crate::imports::RETRIES.with(|c| c.get()) - before,
            1,
            "{source:?}"
        );
    }
}
