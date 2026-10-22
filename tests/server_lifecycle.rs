//! The server's isolation and its disk index, driven through `Embedded`: the
//! server a browser hosts, which takes the same steps as the one behind stdio
//! (see `src/server.rs`) with the host calling it instead of its own loop.
//!
//! Embedded, the workspace folders are real directories and files, but nothing
//! is indexed at the start: files enter the index as the watcher reports them,
//! which is the path these tests cover. The initial index, its failure and
//! what changes before it lands are covered where the server is built
//! without a host (`src/server.rs`, `src/workspace.rs`).

use lsp_server::{Message, Notification, Request};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use turtle_lsp::server::Embedded;

/// A directory removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("turtle-lsp-life-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d.canonicalize().unwrap())
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

/// A client that answers nothing and notes what it was sent.
struct Client {
    server: Embedded,
    next: i32,
    sent: Vec<Message>,
}

impl Client {
    fn start(root: Option<&Path>, options: Value) -> Client {
        let folders = root.map_or(json!([]), |r| json!([{"uri": uri(r), "name": "w"}]));
        let params = json!({
            "processId": null,
            "rootUri": root.map(uri),
            "workspaceFolders": folders,
            "capabilities": {},
            "initializationOptions": options,
        });
        let (server, sent) = Embedded::start(params).unwrap();
        Client {
            server,
            next: 1,
            sent,
        }
    }

    fn handle(&mut self, messages: Vec<Message>) -> Vec<Message> {
        let sent = self.server.handle(messages);
        self.sent.extend(sent.iter().cloned());
        sent
    }

    fn notify(&mut self, method: &str, params: Value) -> Vec<Message> {
        self.handle(vec![Message::Notification(Notification::new(
            method.into(),
            params,
        ))])
    }

    /// The result of a request, or `{"error": {code, message}}`.
    fn ask(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let sent = self.handle(vec![Message::Request(Request::new(
            id.into(),
            method.into(),
            params,
        ))]);
        sent.into_iter()
            .find_map(|m| match m {
                Message::Response(r) if r.id == id.into() => Some(match r.response_result {
                    Ok(v) => v,
                    Err(e) => json!({"error": {"code": e.code, "message": e.message}}),
                }),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no response to {method}"))
    }

    fn open(&mut self, path: &Path, text: &str) -> Vec<Message> {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri(path), "languageId": "turtle", "version": 1, "text": text}}),
        )
    }

    fn close(&mut self, path: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri(path)}}),
        );
    }

    /// The watcher reports `changes` (path, type: 1 created, 2 changed, 3 deleted).
    fn watched(&mut self, changes: &[(&Path, u8)]) {
        let changes: Vec<Value> = changes
            .iter()
            .map(|(p, t)| json!({"uri": uri(p), "type": t}))
            .collect();
        self.notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes": changes}),
        );
    }

    /// The files whose subjects the workspace symbols name, sorted.
    fn indexed(&mut self) -> Vec<String> {
        let r = self.ask("workspace/symbol", json!({"query": ""}));
        let mut files: Vec<String> = r
            .as_array()
            .unwrap_or_else(|| panic!("{r}"))
            .iter()
            .map(|s| {
                let u = s["location"]["uri"].as_str().unwrap();
                u.rsplit('/').next().unwrap().to_string()
            })
            .collect();
        files.sort();
        files.dedup();
        files
    }
}

const CREATED: u8 = 1;
const CHANGED: u8 = 2;
const DELETED: u8 = 3;

fn ttl(subject: &str) -> String {
    format!("@prefix ex: <http://e.org/> .\nex:{subject} a ex:Thing .\n")
}

#[test]
fn a_watched_folder_delete_drops_the_files_indexed_below_it() {
    let dir = Dir::new("folder-delete");
    let a = dir.write("keep/a.ttl", &ttl("Alpha"));
    let b = dir.write("gone/b.ttl", &ttl("Beta"));
    let c = dir.write("gone/deep/c.ttl", &ttl("Gamma"));
    let d = dir.write("gone-not/d.ttl", &ttl("Delta"));
    let mut client = Client::start(Some(&dir.0), Value::Null);
    client.watched(&[(&a, CREATED), (&b, CREATED), (&c, CREATED), (&d, CREATED)]);
    assert_eq!(client.indexed(), ["a.ttl", "b.ttl", "c.ttl", "d.ttl"]);

    // The folder is deleted (or renamed away): the client reports the folder.
    std::fs::remove_dir_all(dir.0.join("gone")).unwrap();
    client.watched(&[(&dir.0.join("gone"), DELETED)]);
    assert_eq!(client.indexed(), ["a.ttl", "d.ttl"]);
    // A sibling whose name starts the same is not below it.
    assert!(client.indexed().contains(&"d.ttl".to_string()));

    // A deleted file is still a deleted file.
    std::fs::remove_file(&a).unwrap();
    client.watched(&[(&a, DELETED)]);
    assert_eq!(client.indexed(), ["d.ttl"]);
}

#[test]
fn a_change_in_node_modules_a_hidden_folder_or_outside_the_roots_is_ignored() {
    let dir = Dir::new("ignored");
    let outside = Dir::new("ignored-outside");
    let kept = dir.write("src/kept.ttl", &ttl("Kept"));
    let events = [
        (dir.write("node_modules/pkg/n.ttl", &ttl("Node")), CREATED),
        (dir.write(".hidden/h.ttl", &ttl("Hidden")), CHANGED),
        (dir.write("target/t.ttl", &ttl("Target")), CREATED),
        (outside.write("o.ttl", &ttl("Outside")), CREATED),
        (dir.write("src/notes.txt", "not rdf"), CREATED),
        (kept.clone(), CREATED),
    ];
    let mut client = Client::start(Some(&dir.0), Value::Null);
    let changes: Vec<(&Path, u8)> = events.iter().map(|(p, t)| (p.as_path(), *t)).collect();
    client.watched(&changes);
    assert_eq!(client.indexed(), ["kept.ttl"]);

    // The same for a deleted folder in a skipped place, which is no concern.
    client.watched(&[
        (&dir.0.join("node_modules"), DELETED),
        (&dir.0.join("src"), DELETED),
    ]);
    assert!(client.indexed().is_empty());
}

#[test]
fn closing_a_document_outside_the_roots_leaves_no_entry_but_one_inside_stays_indexed() {
    let dir = Dir::new("close-in");
    let outside = Dir::new("close-out");
    let inside = dir.write("in.ttl", &ttl("Inside"));
    let away = outside.write("out.ttl", &ttl("Away"));
    let mut client = Client::start(Some(&dir.0), Value::Null);
    client.open(&inside, &ttl("Inside"));
    client.open(&away, &ttl("Away"));
    assert_eq!(client.indexed(), ["in.ttl", "out.ttl"]);
    client.close(&inside);
    client.close(&away);
    assert_eq!(
        client.indexed(),
        ["in.ttl"],
        "the file outside the workspace came back from disk"
    );
}

#[test]
fn a_prefixes_file_change_reparses_the_files_it_serves_and_no_others() {
    let dir = Dir::new("prefixes");
    let pf = dir.write("a/prefixes.ttl", "@prefix ex: <http://one.org/> .\n");
    let served = dir.write("a/x.ttl", "ex:S a ex:T .\n");
    let nested_pf = dir.write("a/sub/prefixes.ttl", "@prefix ex: <http://two.org/> .\n");
    let nested = dir.write("a/sub/y.ttl", "ex:S a ex:T .\n");
    let other = dir.write("b/z.ttl", "@prefix ex: <http://z.org/> .\nex:S a ex:T .\n");
    let mut client = Client::start(Some(&dir.0), Value::Null);
    client.watched(&[
        (&pf, CREATED),
        (&served, CREATED),
        (&nested_pf, CREATED),
        (&nested, CREATED),
        (&other, CREATED),
    ]);
    // Who defines an IRI: ask where an open document's reference to it goes.
    let probe = dir.0.join("probe.ttl");
    let defines = |client: &mut Client, iri: &str| -> Vec<String> {
        client.open(
            &probe,
            &format!("<{iri}> <http://e.org/p> <http://e.org/o> .\n"),
        );
        let r = client.ask(
            "textDocument/definition",
            json!({"textDocument": {"uri": uri(&probe)}, "position": {"line": 0, "character": 3}}),
        );
        let mut files: Vec<String> = r
            .as_array()
            .unwrap_or_else(|| panic!("{r}"))
            .iter()
            .map(|l| {
                l["uri"]
                    .as_str()
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_string()
            })
            .filter(|f| f != "probe.ttl")
            .collect();
        files.sort();
        files
    };
    let before = defines(&mut client, "http://one.org/S");
    assert_eq!(before, ["x.ttl"], "{before:?}");
    assert_eq!(defines(&mut client, "http://two.org/S"), ["y.ttl"]);

    // The file changes on disk: what it serves follows; the nested folder's
    // own prefixes file keeps its files as they were.
    std::fs::write(&pf, "@prefix ex: <http://three.org/> .\n").unwrap();
    client.watched(&[(&pf, CHANGED)]);
    assert_eq!(defines(&mut client, "http://three.org/S"), ["x.ttl"]);
    assert!(defines(&mut client, "http://one.org/S").is_empty());
    assert_eq!(defines(&mut client, "http://two.org/S"), ["y.ttl"]);
    assert_eq!(defines(&mut client, "http://z.org/S"), ["z.ttl"]);

    // The nested one is deleted: its files fall back to the file above.
    std::fs::remove_file(&nested_pf).unwrap();
    client.watched(&[(&nested_pf, DELETED)]);
    assert_eq!(
        defines(&mut client, "http://three.org/S"),
        ["x.ttl", "y.ttl"]
    );
}

#[test]
fn settings_layer_in_order_and_the_client_section_has_the_last_word() {
    let dir = Dir::new("settings");
    let file = dir.write("a.ttl", "");
    let long = "@prefix ex: <http://e.org/> .\nex:s ex:p ex:o1, ex:o2, ex:o3, ex:o4, ex:o5, ex:o6, ex:o7, ex:o8 .\n";
    let mut client = Client::start(Some(&dir.0), json!({"format": {"lineWidth": 30}}));
    client.open(&file, long);
    let lines = |client: &mut Client| -> usize {
        let r = client.ask(
            "textDocument/formatting",
            json!({"textDocument": {"uri": uri(&file)}, "options": {"tabSize": 2, "insertSpaces": true}}),
        );
        r[0]["newText"]
            .as_str()
            .unwrap_or_else(|| panic!("{r}"))
            .lines()
            .count()
    };
    let narrow = lines(&mut client);
    // The configuration section overrides initializationOptions.
    client.notify(
        "workspace/didChangeConfiguration",
        json!({"settings": {"turtle": {"format": {"lineWidth": 400}}}}),
    );
    let wide = lines(&mut client);
    assert!(wide < narrow, "{wide} lines at 400 columns, {narrow} at 30");
    // Each section is the whole configuration: one without that key leaves
    // the layer below (initializationOptions) to answer.
    client.notify(
        "workspace/didChangeConfiguration",
        json!({"settings": {"turtle": {"lint": {"undescribedTerms": false}}}}),
    );
    assert_eq!(lines(&mut client), narrow);
    // A notification without the section leaves the settings as they are.
    client.notify("workspace/didChangeConfiguration", json!({"settings": {}}));
    assert_eq!(lines(&mut client), narrow);
}

#[test]
fn errors_in_a_request_do_not_stop_the_server_and_it_keeps_answering() {
    let dir = Dir::new("errors");
    let file = dir.write("a.ttl", "");
    let mut client = Client::start(Some(&dir.0), Value::Null);
    client.open(&file, &ttl("Alpha"));
    let unknown = client.ask("turtle/unknownMethod", json!({}));
    assert_eq!(unknown["error"]["code"], -32601, "{unknown}");
    let bad = client.ask("textDocument/hover", json!({"textDocument": 7}));
    assert_eq!(bad["error"]["code"], -32602, "{bad}");
    // A document that is not open answers null, not an error.
    let missing = client.ask(
        "textDocument/hover",
        json!({"textDocument": {"uri": "file:///nowhere.ttl"}, "position": {"line": 0, "character": 0}}),
    );
    assert!(missing.is_null(), "{missing}");
    // And the server is still there.
    let symbols = client.ask(
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri(&file)}}),
    );
    assert!(!symbols.as_array().unwrap().is_empty());
}

#[test]
fn the_timers_are_keyed_the_host_asks_when_the_next_is_due_and_firing_clears_it() {
    let dir = Dir::new("timers");
    let file = dir.write("a.ttl", "");
    let mut client = Client::start(Some(&dir.0), Value::Null);
    assert!(client.server.due_in_ms().is_none());
    client.open(&file, "");
    // A new subject changes the described set; the other documents follow
    // once typing pauses.
    client.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri(&file), "version": 2},
               "contentChanges": [{"text": ttl("Alpha")}]}),
    );
    // Two timers run: the refresh of the other documents (300 ms), and the
    // validation once typing pauses (1 s). The host is told the earlier.
    let due = client.server.due_in_ms().expect("a refresh is due");
    assert!(due <= 300.0, "{due}");
    // Not yet: ticking early fires nothing and keeps the timers.
    client.server.tick();
    assert!(client.server.due_in_ms().is_some_and(|d| d <= 300.0));
    std::thread::sleep(std::time::Duration::from_millis(350));
    client.server.tick();
    // The refresh fired and is gone; the validation is the one left.
    let left = client
        .server
        .due_in_ms()
        .expect("the validation is still due");
    assert!(left > 300.0, "{left}");
    std::thread::sleep(std::time::Duration::from_millis(left as u64 + 50));
    client.server.tick();
    assert!(
        client.server.due_in_ms().is_none(),
        "{:?}",
        client.server.due_in_ms()
    );
}
