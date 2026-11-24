use super::*;
use crate::workspace::{PANIC_IN_ANALYSIS, PANIC_IN_FACTS, path_to_uri};
use serde_json::json;

const TTL: &str = "@prefix ex: <http://e.org/> .\nex:A a ex:B .\n";

/// Silence panic messages while `f` runs: the panics are the point. The
/// guard is shared with the other tests that silence panics, and
/// restores the hook even when `f` panics.
fn quietly<T>(f: impl FnOnce() -> T) -> T {
    let _quiet = crate::workspace::tests::QuietPanics::new();
    f()
}

fn embedded() -> Embedded {
    Embedded::start(json!({"processId": null, "rootUri": null, "capabilities": {}}))
        .unwrap()
        .0
}

fn notify(method: &str, params: Value) -> Message {
    Message::Notification(Notification::new(method.into(), params))
}

fn open(e: &mut Embedded, uri: &str, text: &str) -> Vec<Message> {
    e.handle(vec![notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "turtle", "version": 1, "text": text}}),
    )])
}

fn ask(e: &mut Embedded, id: i32, method: &str, params: Value) -> Value {
    let sent = e.handle(vec![Message::Request(Request::new(
        id.into(),
        method.into(),
        params,
    ))]);
    sent.into_iter()
        .find_map(|m| match m {
            Message::Response(r) if r.id == id.into() => Some(match r.response_result {
                Err(e) => json!({"error": e.message}),
                Ok(v) => v,
            }),
            _ => None,
        })
        .expect("a response")
}

/// The messages of the diagnostics published for `uri`.
fn published(sent: &[Message], uri: &str) -> Option<Vec<String>> {
    sent.iter().rev().find_map(|m| match m {
        Message::Notification(n) if n.method == "textDocument/publishDiagnostics" => {
            (n.params["uri"] == uri).then(|| {
                n.params["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| d["message"].as_str().unwrap().to_string())
                    .collect()
            })
        }
        _ => None,
    })
}

/// Embedded, a client that opts in (`registrations::OPT_IN`) and
/// registers formatting, rename, completion and code actions itself gets
/// them registered once it is initialized (the host sends `initialized`
/// after the result), not in the result. One that registers them itself
/// but does not opt in gets the result of a client that cannot, and no
/// registration.
#[test]
fn embedded_dynamic_features_are_registered_after_initialized() {
    let caps = json!({
        "formatting": {"dynamicRegistration": true},
        "rename": {"dynamicRegistration": true},
        "completion": {"dynamicRegistration": true},
        "codeAction": {"dynamicRegistration": true},
    });
    let params = |options: Value| {
        json!({
            "processId": null,
            "rootUri": null,
            "capabilities": {"textDocument": caps},
            "initializationOptions": options,
        })
    };
    let result_of = |sent: &[Message]| {
        sent.iter()
            .find_map(|m| match m {
                Message::Response(r) => r.response_result.clone().ok(),
                _ => None,
            })
            .unwrap()
    };
    let (mut e, sent) = Embedded::start(params(json!({registrations::OPT_IN: true}))).unwrap();
    let result = result_of(&sent);
    for provider in [
        "documentFormattingProvider",
        "renameProvider",
        "completionProvider",
        "codeActionProvider",
    ] {
        assert!(result["capabilities"].get(provider).is_none(), "{provider}");
    }
    assert!(result["capabilities"]["hoverProvider"] == true);
    let registrations = |sent: &[Message]| -> Vec<Value> {
        sent.iter()
            .filter_map(|m| match m {
                Message::Request(r) if r.method == "client/registerCapability" => {
                    Some(r.params["registrations"].as_array().unwrap().clone())
                }
                _ => None,
            })
            .flatten()
            .collect()
    };
    assert!(registrations(&sent).is_empty());
    let sent = e.handle(vec![notify("initialized", json!({}))]);
    let regs = registrations(&sent);
    let methods: Vec<_> = regs.iter().map(|r| r["method"].as_str().unwrap()).collect();
    assert_eq!(
        methods,
        [
            "textDocument/formatting",
            "textDocument/rename",
            "textDocument/completion",
            "textDocument/codeAction"
        ]
    );
    for r in &regs {
        let selector = r["registerOptions"]["documentSelector"].to_string();
        assert!(
            selector.contains("turtle") && !selector.contains("owl"),
            "{selector}"
        );
    }

    let mut plain = embedded();
    assert!(registrations(&plain.handle(vec![notify("initialized", json!({}))])).is_empty());

    // Dynamic registration without the opt-in: the static result.
    let (_, plain_sent) =
        Embedded::start(json!({"processId": null, "rootUri": null, "capabilities": {}})).unwrap();
    let (mut quiet, sent) = Embedded::start(params(json!({}))).unwrap();
    assert_eq!(result_of(&sent), result_of(&plain_sent));
    assert_eq!(
        result_of(&sent)["capabilities"]["renameProvider"],
        json!({"prepareProvider": true})
    );
    let sent = [sent, quiet.handle(vec![notify("initialized", json!({}))])].concat();
    assert!(registrations(&sent).is_empty());
}

fn symbols(e: &mut Embedded, id: i32, uri: &str) -> usize {
    let r = ask(
        e,
        id,
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    );
    r.as_array().unwrap_or_else(|| panic!("{r}")).len()
}

/// Embedded, reasoning runs inline; turning it on creates its state,
/// turning it off drops the state and the findings.
#[test]
fn reasoning_turned_on_and_off_by_the_settings() {
    const DISJOINT: &str = "@prefix ex: <http://example.org/> .\n\
            @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
            ex:Cat owl:disjointWith ex:Dog .\n\
            ex:tom a ex:Cat , ex:Dog .\n";
    let uri = "file:///w/a.ttl";
    let found = |sent: &[Message]| {
        published(sent, uri).map(|m| m.iter().any(|m| m.contains("which are disjoint")))
    };
    let rules = |r: &str| {
        notify(
            "workspace/didChangeConfiguration",
            json!({"settings": {"turtle": {"reasoning": {"rules": r, "diagnostics": true}}}}),
        )
    };
    let mut e = embedded();
    // On by default: the state is there, and findings stay off.
    assert!(e.server.reasoning.is_some());
    e.handle(vec![rules("off")]);
    let sent = open(&mut e, uri, DISJOINT);
    assert_eq!(found(&sent), Some(false));
    assert!(e.server.reasoning.is_none());
    assert!(!e.server.timers.is_set(Timer::ReasoningTyping));

    let sent = e.handle(vec![rules("default")]);
    assert_eq!(found(&sent), Some(true));
    // An edit reasons again once typing pauses.
    e.handle(vec![notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": DISJOINT}]}),
    )]);
    assert!(e.server.timers.is_set(Timer::ReasoningTyping));
    // Another rule set reruns over the same state.
    let shared = e.server.reasoning.clone().unwrap();
    let sent = e.handle(vec![rules("complete")]);
    assert!(Arc::ptr_eq(&shared, e.server.reasoning.as_ref().unwrap()));
    assert_eq!(found(&sent), Some(true));

    let sent = e.handle(vec![rules("off")]);
    assert_eq!(found(&sent), Some(false));
    assert!(e.server.reasoning.is_none());
    assert!(!e.server.timers.is_set(Timer::ReasoningTyping));
    assert!(e.server.reasoning_runs.diagnostics().is_empty());
}

/// The `query.*` settings are read only when a query runs: changing only
/// them stores them and reasons, validates and checks nothing again.
/// Any other change still does.
#[test]
fn a_change_to_query_settings_alone_reruns_nothing() {
    const DATA: &str = "@prefix ex: <http://example.org/> .\n\
            @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
            @prefix sh: <http://www.w3.org/ns/shacl#> .\n\
            ex:Cat owl:disjointWith ex:Dog .\n\
            ex:tom a ex:Cat , ex:Dog .\n\
            ex:S a sh:NodeShape ; sh:targetNode ex:tom ; sh:property [ sh:path ex:name ; sh:minCount 1 ] .\n";
    let uri = "file:///w/a.ttl";
    let configure = |e: &mut Embedded, rules: &str, query: Value| {
        e.handle(vec![notify(
            "workspace/didChangeConfiguration",
            json!({"settings": {"turtle": {
                "reasoning": {"rules": rules, "diagnostics": true},
                "query": query,
            }}}),
        )])
    };
    let generations = |e: &Embedded| {
        (
            e.server.reasoning_runs.generation(),
            e.server.shacl.generation(),
        )
    };
    let mut e = embedded();
    open(&mut e, uri, DATA);
    let sent = configure(&mut e, "default", json!({"reasoning": "off"}));
    assert!(published(&sent, uri).is_some());
    let before = generations(&e);

    let sent = configure(
        &mut e,
        "default",
        json!({"reasoning": "default", "maxRows": 5, "timeoutSeconds": 3}),
    );
    assert_eq!(published(&sent, uri), None, "nothing is checked again");
    assert_eq!(generations(&e), before, "no reasoning run, no validation");
    let s = e.server.layers.settings();
    assert_eq!(s.query_reasoning, crate::settings::Reasoning::Default);
    assert_eq!(s.query_max_rows, Some(5));
    assert_eq!(s.query_timeout, Some(Duration::from_secs(3)));
    assert!(e.server.dirty.is_empty());

    let sent = configure(
        &mut e,
        "complete",
        json!({"reasoning": "default", "maxRows": 5, "timeoutSeconds": 3}),
    );
    assert!(published(&sent, uri).is_some());
    let after = generations(&e);
    assert!(
        after.0 > before.0 && after.1 > before.1,
        "{before:?} {after:?}"
    );
}

/// A run over text since edited (on the thread, it finished just as the
/// edit came, before the edit's cancel reached it) lands after the edit:
/// neither its findings nor the state it wrote are shown. Had the
/// cancel reached it first, it would have stopped.
#[test]
fn a_run_that_lands_after_an_edit_shows_nothing() {
    const AXIOMS: &str = "@prefix ex: <http://example.org/> .\n\
            @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
            ex:Cat owl:disjointWith ex:Dog .\n\
            ex:Kitten rdfs:subClassOf ex:Cat .\n";
    let clean = format!("{AXIOMS}ex:tom a ex:Dog .\n");
    let clash = format!("{AXIOMS}ex:tom a ex:Kitten , ex:Dog .\n");
    let uri = "file:///w/a.ttl";
    let change = |version: i32, text: &str| {
        notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
        )
    };
    let hover = |e: &mut Embedded, id: i32| {
        let r = ask(
            e,
            id,
            "textDocument/hover",
            json!({"textDocument": {"uri": uri}, "position": {"line": 5, "character": 4}}),
        );
        r["contents"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let mut e = embedded();
    open(&mut e, uri, &clean);
    e.handle(vec![notify(
        "workspace/didChangeConfiguration",
        json!({"settings": {"turtle": {"reasoning": {"rules": "default", "diagnostics": true}}}}),
    )]);
    assert!(e.server.reasoning_runs.diagnostics().is_empty());
    // The run that starts once typing pauses after the first edit.
    e.handle(vec![change(2, &clash)]);
    let docs: Vec<_> = e
        .server
        .ws
        .knowledge()
        .docs()
        .map(|(_, s, a)| (s, a.clone()))
        .collect();
    let old = e
        .server
        .reasoning_runs
        .job(docs, e.server.layers.settings(), e.server.enc);
    let shared = e.server.reasoning.clone().unwrap();
    // It finishes as the text changes back; then it lands.
    let landed = crate::reasoning::run_inline(&shared, &old);
    assert!(!landed.diagnostics.is_empty(), "the old text has a finding");
    e.handle(vec![change(3, &clean)]);
    assert!(old.cancel.is_cancelled(), "the edit stops the run");
    let again = crate::reasoning::run_inline(&shared, &old);
    assert_eq!(again.stopped, Some(crate::reasoning::Stopped::Superseded));
    e.server.reasoning_outcome(landed);
    assert!(
        e.server.reasoning_runs.diagnostics().is_empty(),
        "{:?}",
        e.server.reasoning_runs.diagnostics()
    );
    let md = hover(&mut e, 10);
    assert!(!md.contains("ex:Cat"), "{md}");
    // The run once typing pauses shows what the text says.
    e.server.reason();
    assert!(e.server.reasoning_runs.diagnostics().is_empty());
    let md = hover(&mut e, 11);
    assert!(!md.contains("Inferred"), "{md}");
}

/// A SPARQL document adds no triples: opening, editing and closing one
/// supersedes no run and schedules none. An RDF document does.
#[test]
fn a_sparql_document_does_not_rerun_reasoning() {
    let mut e = embedded();
    open(&mut e, "file:///w/a.ttl", TTL);
    assert!(e.server.timers.is_set(Timer::ReasoningTyping));
    e.server.timers.clear(Timer::ReasoningTyping);
    let before = e.server.reasoning_runs.generation();
    let q = "file:///w/q.rq";
    e.handle(vec![
            notify(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": q, "languageId": "sparql", "version": 1, "text": "SELECT * { ?s ?p ?o }"}}),
            ),
            notify(
                "textDocument/didChange",
                json!({"textDocument": {"uri": q, "version": 2}, "contentChanges": [{"text": "ASK { ?s ?p ?o }"}]}),
            ),
            notify(
                "textDocument/didClose",
                json!({"textDocument": {"uri": q}}),
            ),
        ]);
    assert_eq!(e.server.reasoning_runs.generation(), before);
    assert!(!e.server.timers.is_set(Timer::ReasoningTyping));
    e.handle(vec![notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": "file:///w/a.ttl", "version": 2}, "contentChanges": [{"text": TTL}]}),
        )]);
    assert!(e.server.reasoning_runs.generation() > before);
    assert!(e.server.timers.is_set(Timer::ReasoningTyping));
}

/// A server on threads, with progress, and its client's end.
fn threaded() -> (Server, wire::Outbox) {
    let (here, host) = wire::memory();
    let (server, _, _) = Server::configure(
        here,
        json!({"capabilities": {"window": {"workDoneProgress": true}}}),
        Spawn::Thread,
    )
    .unwrap();
    (server, host)
}

/// A reasoning job over nothing, submitted to the thread with its
/// progress begun, as `reason` does.
fn submitted(server: &mut Server) -> u64 {
    let job = server
        .reasoning_runs
        .job(Vec::new(), server.layers.settings(), server.enc);
    let generation = job.generation;
    let shared = server.reasoning.clone().unwrap();
    server.reasoning_runs.submit(job, &shared).unwrap();
    if !server.progress.is_shown(Work::Reasoning) {
        server
            .progress
            .begin(&mut server.client, Work::Reasoning, REASONING_TITLE);
    }
    generation
}

fn outcome(
    generation: u64,
    stopped: Option<crate::reasoning::Stopped>,
) -> crate::reasoning::Outcome {
    crate::reasoning::Outcome {
        generation,
        diagnostics: Default::default(),
        stopped,
    }
}

#[test]
fn a_dropped_outcome_ends_the_progress_unless_a_newer_run_is_going_on() {
    use crate::reasoning::Stopped;
    let (mut server, _host) = threaded();
    // An edit stops the run; the next starts once typing pauses.
    let first = submitted(&mut server);
    server.reasoning_runs.invalidate();
    server.reasoning_outcome(outcome(first, Some(Stopped::Superseded)));
    assert!(!server.progress.is_shown(Work::Reasoning));
    assert!(!server.timers.is_set(Timer::ReasoningProgress));
    // A run that finished before the edit's cancel reached it.
    let second = submitted(&mut server);
    server.reasoning_runs.invalidate();
    server.reasoning_outcome(outcome(second, None));
    assert!(!server.progress.is_shown(Work::Reasoning));
    // A newer run is going on: its progress goes on.
    let third = submitted(&mut server);
    let fourth = submitted(&mut server);
    server.reasoning_outcome(outcome(third, Some(Stopped::Superseded)));
    assert!(server.progress.is_shown(Work::Reasoning));
    server.reasoning_outcome(outcome(fourth, None));
    assert!(!server.progress.is_shown(Work::Reasoning));
}

#[test]
fn a_run_stopped_at_its_time_limit_keeps_the_findings_and_warns_once_until_one_finishes() {
    use crate::reasoning::Stopped;
    let (mut server, host) = threaded();
    let uri = "file:///w/a.ttl".to_string();
    let finding = Diagnostic {
        message: "ex:tom is both a ex:Cat and a ex:Dog, which are disjoint.".into(),
        ..Default::default()
    };
    let findings: crate::reasoning::ByUri = [(uri.clone(), vec![finding])].into_iter().collect();
    let first = submitted(&mut server);
    server.reasoning_outcome(crate::reasoning::Outcome {
        diagnostics: findings.clone(),
        ..outcome(first, None)
    });
    let messages = |host: &wire::Outbox| {
        let mut shown = Vec::new();
        let mut logged = Vec::new();
        for m in host.try_iter() {
            if let Message::Notification(n) = m {
                let text = n.params["message"].as_str().unwrap_or_default().to_string();
                match n.method.as_str() {
                    "window/showMessage" => shown.push(text),
                    "window/logMessage" => logged.push(text),
                    _ => {}
                }
            }
        }
        (shown, logged)
    };
    messages(&host);
    let stop = |server: &mut Server| {
        let g = submitted(server);
        let limit = Duration::from_secs(10);
        server.reasoning_outcome(outcome(
            g,
            Some(Stopped::TimeLimit {
                limit,
                kept: Some(first),
            }),
        ));
        assert!(!server.progress.is_shown(Work::Reasoning));
        assert_eq!(server.reasoning_runs.diagnostics(), &findings);
    };
    stop(&mut server);
    let (shown, logged) = messages(&host);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0].contains("time limit of 10 s"), "{shown:?}");
    assert!(shown[0].contains("turtle.reasoning.rules"), "{shown:?}");
    assert_eq!(logged, shown);
    // Each stop after it is logged only.
    stop(&mut server);
    let (shown, logged) = messages(&host);
    assert!(shown.is_empty(), "{shown:?}");
    assert_eq!(logged.len(), 1);
    // Once a run finishes, the next stop is shown again.
    let g = submitted(&mut server);
    server.reasoning_outcome(crate::reasoning::Outcome {
        diagnostics: findings.clone(),
        ..outcome(g, None)
    });
    stop(&mut server);
    assert_eq!(messages(&host).0.len(), 1);
}

#[test]
fn an_embedded_server_that_has_ended_has_no_timer_due() {
    let mut e = embedded();
    open(&mut e, "file:///w/a.ttl", "ex:s ex:p ex:o .\n");
    assert!(
        e.due_in_ms().is_some(),
        "an open document has a timer pending"
    );
    ask(&mut e, 1, "shutdown", Value::Null);
    assert!(e.ended);
    assert_eq!(e.due_in_ms(), None);
    assert!(e.tick().is_empty());
}

#[test]
fn a_panic_after_a_batch_does_not_end_the_session_or_skip_the_phases_after_it() {
    let mut e = embedded();
    e.server.fault = true;
    let sent = quietly(|| open(&mut e, "file:///w/a.ttl", "ex:s ex:p ex:o .\n"));
    assert!(!e.ended);
    let shown = published(&sent, "file:///w/a.ttl").expect("diagnostics still went out");
    assert!(shown.iter().any(|m| m.contains("ex")), "{shown:?}");
    assert!(!e.server.fault, "the fault fired");
    symbols(&mut e, 2, "file:///w/a.ttl");
}

#[test]
fn a_document_whose_analysis_panics_shows_an_internal_error_and_the_server_goes_on() {
    for marker in [PANIC_IN_ANALYSIS, PANIC_IN_FACTS] {
        let mut e = embedded();
        let (bad, good) = ("file:///w/bad.ttl", "file:///w/good.ttl");
        let sent = quietly(|| {
            open(&mut e, good, TTL);
            open(&mut e, bad, &format!("# {marker}\n{TTL}"))
        });
        assert!(!e.ended, "{marker}");
        let shown = published(&sent, bad).expect("the bad document gets its diagnostics");
        assert!(
            shown
                .iter()
                .any(|m| m.contains("internal error") && m.contains("report")),
            "{marker}: {shown:?}"
        );
        // Every other document and request still works.
        assert!(symbols(&mut e, 2, good) > 0, "{marker}");
        let sent = quietly(|| {
            e.handle(vec![notify(
                "textDocument/didChange",
                json!({"textDocument": {"uri": good, "version": 2},
                           "contentChanges": [{"text": format!("# {marker}\n")}]}),
            )])
        });
        let shown = published(&sent, good).unwrap();
        assert!(
            shown.iter().any(|m| m.contains("internal error")),
            "{marker}: {shown:?}"
        );
        // The fix clears it.
        let sent = e.handle(vec![notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": good, "version": 3}, "contentChanges": [{"text": TTL}]}),
        )]);
        assert!(
            published(&sent, good)
                .unwrap()
                .iter()
                .all(|m| !m.contains("internal error"))
        );
    }
}

#[test]
fn a_pulled_report_for_a_document_whose_analysis_panics_has_the_error_too() {
    let mut e = Embedded::start(json!({
            "capabilities": {"textDocument": {"diagnostic": {}}, "workspace": {"diagnostics": {"refreshSupport": true}}},
        }))
        .unwrap()
        .0;
    let uri = "file:///w/bad.ttl";
    quietly(|| {
        open(&mut e, uri, &format!("# {PANIC_IN_ANALYSIS}\n{TTL}"));
        let r = ask(
            &mut e,
            2,
            "textDocument/diagnostic",
            json!({"textDocument": {"uri": uri}}),
        );
        assert!(r.to_string().contains("internal error"), "{r}");
    });
}

#[test]
fn a_failed_initial_index_is_reported_and_the_server_works_without_it() {
    let (here, host) = wire::memory();
    let (mut server, _, _) = Server::configure(
        here,
        json!({"capabilities": {"window": {"workDoneProgress": true}}}),
        Spawn::Thread,
    )
    .unwrap();
    assert!(!server.index.merged());
    server.index_arrived(Fed::Got(Err(
        "turtle-lsp internal error in index: boom".into()
    )));
    assert!(server.index.merged());
    let sent: Vec<Message> = host.try_iter().collect();
    let warned = sent.iter().any(|m| {
        matches!(m,
            Message::Notification(n) if n.method == "window/showMessage"
                && n.params["message"].as_str().unwrap().contains("boom")
                && n.params["message"].as_str().unwrap().contains("not indexed"))
    });
    assert!(warned, "{sent:?}");
    // A thread that ended without a word is a failure too, not 0 files.
    let (here, host) = wire::memory();
    let (mut server, _, _) =
        Server::configure(here, json!({"capabilities": {}}), Spawn::Thread).unwrap();
    server.index_arrived(Fed::Gone);
    assert!(server.index.merged());
    let sent: Vec<Message> = host.try_iter().collect();
    assert!(
        sent.iter()
            .any(|m| matches!(m, Message::Notification(n) if n.method == "window/showMessage"))
    );
}

#[test]
fn a_change_on_disk_before_the_initial_index_lands_is_not_undone_by_it() {
    let dir = std::env::temp_dir().join(format!("turtle-lsp-premerge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("folder")).unwrap();
    let dir = crate::workspace::real(&dir);
    let (kept, gone, in_folder) = (
        dir.join("kept.ttl"),
        dir.join("gone.ttl"),
        dir.join("folder/f.ttl"),
    );
    for f in [&kept, &gone, &in_folder] {
        std::fs::write(f, TTL).unwrap();
    }
    let (here, _host) = wire::memory();
    let root = path_to_uri(&dir).unwrap();
    let (mut server, _, roots) = Server::configure(
        here,
        json!({"capabilities": {}, "workspaceFolders": [{"uri": root, "name": "w"}]}),
        Spawn::Thread,
    )
    .unwrap();
    server.ws.expect_initial_index();
    let mut reader = Workspace::new(roots.clone(), Default::default());
    let files = reader.parse_paths(Workspace::scan_roots(&roots));
    assert_eq!(files.len(), 3);
    // The disk changes while the index is on its way.
    std::fs::remove_file(&gone).unwrap();
    std::fs::remove_dir_all(dir.join("folder")).unwrap();
    let event = |p: &std::path::Path| FileEvent {
        uri: path_to_uri(p).unwrap().parse().unwrap(),
        typ: FileChangeType::DELETED,
    };
    server.watched_files_changed(vec![event(&gone), event(&dir.join("folder"))]);
    server.index_arrived(Fed::Got(Ok(files)));
    let indexed = |p: &std::path::Path| server.ws.is_indexed(&path_to_uri(p).unwrap());
    assert!(indexed(&kept));
    assert!(!indexed(&gone), "a deleted file came back");
    assert!(!indexed(&in_folder), "a file of a deleted folder came back");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A notification of many changed files indexes them in one parallel pass
/// (it parsed them one at a time, a pass each), and tells the SPARQL
/// index of the queries among them.
#[test]
fn a_notification_of_many_changes_is_indexed_in_one_pass() {
    let dir = std::env::temp_dir().join(format!("turtle-lsp-batch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = crate::workspace::real(&dir);
    let (here, _host) = wire::memory();
    let root = path_to_uri(&dir).unwrap();
    let (mut server, _, _) = Server::configure(
        here,
        json!({"capabilities": {}, "workspaceFolders": [{"uri": root, "name": "w"}]}),
        Spawn::Thread,
    )
    .unwrap();
    let event = |p: &std::path::Path, typ| FileEvent {
        uri: path_to_uri(p).unwrap().parse().unwrap(),
        typ,
    };
    let first = dir.join("first.rq");
    std::fs::write(&first, "SELECT * WHERE { ?s ?p ?o }\n").unwrap();
    assert_eq!(server.ws.sparql_documents().len(), 1);
    let files: Vec<std::path::PathBuf> = (0..120).map(|i| dir.join(format!("f{i}.ttl"))).collect();
    for f in &files {
        std::fs::write(f, TTL).unwrap();
    }
    let second = dir.join("second.rq");
    std::fs::write(&second, "SELECT * WHERE { ?s ?p ?o }\n").unwrap();
    std::fs::remove_file(&first).unwrap();
    let mut changes: Vec<FileEvent> = files
        .iter()
        .map(|f| event(f, FileChangeType::CHANGED))
        .collect();
    changes.push(event(&files[0], FileChangeType::CHANGED));
    changes.push(event(&second, FileChangeType::CREATED));
    changes.push(event(&first, FileChangeType::DELETED));
    let before = crate::workspace::INDEX_PASSES.with(|c| c.get());
    server.watched_files_changed(changes);
    assert_eq!(crate::workspace::INDEX_PASSES.with(|c| c.get()) - before, 1);
    assert_eq!(server.ws.indexed_count(), 120);
    let queries = server.ws.sparql_documents();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].uri, path_to_uri(&second).unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn shutdown_waits_for_exit_and_says_how_it_ended() {
    let ended = |messages: Vec<Message>, wait_ms: u64, drop_inbox: bool| {
        let (here, _host) = wire::memory();
        let (mut server, _, _) =
            Server::configure(here, json!({"capabilities": {}}), Spawn::Thread).unwrap();
        let (tx, rx) = unbounded();
        server.inbox = rx;
        for m in messages {
            tx.send(m).unwrap();
        }
        if drop_inbox {
            drop(tx);
        } else {
            // Keep the sender alive: only the clock ends the wait.
            std::mem::forget(tx);
        }
        let req = Request::new(1.into(), "shutdown".into(), Value::Null);
        server
            .shutdown(req, std::iter::empty(), Duration::from_millis(wait_ms))
            .unwrap()
    };
    let exit = || notify("exit", Value::Null);
    assert_eq!(ended(vec![exit()], 5000, false), Ended::Exit);
    assert_eq!(ended(vec![], 50, false), Ended::ShutdownTimedOut);
    assert_eq!(ended(vec![], 5000, true), Ended::ClientGone);
    // Requests meanwhile are refused, and exit still ends it.
    let hover = Message::Request(Request::new(
        2.into(),
        "textDocument/hover".into(),
        Value::Null,
    ));
    assert_eq!(ended(vec![hover, exit()], 5000, false), Ended::Exit);
    assert!(Ended::Exit.clean());
    for not in [
        Ended::ExitWithoutShutdown,
        Ended::ClientGone,
        Ended::ShutdownTimedOut,
    ] {
        assert!(!not.clean());
    }
}

#[test]
fn after_a_shutdown_timeout_the_reader_is_not_joined() {
    // The reader of a client that sent `shutdown` and went quiet blocks
    // on stdin for good: joining it would keep the process alive.
    let join = |ended| {
        let mut joined = false;
        let clean = end_session(ended, || {
            joined = true;
            Ok::<(), std::io::Error>(())
        })
        .unwrap();
        (joined, clean)
    };
    assert_eq!(join(Ended::ShutdownTimedOut), (false, false));
    assert_eq!(join(Ended::Exit), (true, true));
    assert_eq!(join(Ended::ExitWithoutShutdown), (true, false));
    assert_eq!(join(Ended::ClientGone), (true, false));
}

/// A SHACL thread that cannot start ends the validation going on with
/// "Validation stopped" and says why, instead of panicking; the next
/// validation starts one.
#[test]
fn a_shacl_thread_that_cannot_start_stops_the_validation_and_says_why() {
    let (here, host) = wire::memory();
    let (mut server, _, _) = Server::configure(
        here,
        json!({"capabilities": {"window": {"workDoneProgress": true}}}),
        Spawn::Thread,
    )
    .unwrap();
    server.merge_index(Ok(Vec::new()));
    let shapes = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://e.org/> .\nex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .\n";
    let _ = server
        .ws
        .open_doc("file:///w/shapes.ttl".into(), shapes.into(), 1);
    // A validation going on, its progress shown.
    server
        .progress
        .begin(&mut server.client, Work::Shacl, "Validating");
    let token = host
        .try_iter()
        .find_map(|m| match m {
            Message::Request(r) if r.method == "window/workDoneProgress/create" => Some(r.id),
            _ => None,
        })
        .expect("a progress token asked for");
    server.progress.accepted(&server.client, &token, true);
    let _ = host.try_iter().count();

    worker::failing_starts(|| server.validate_shacl());
    let sent: Vec<Message> = host.try_iter().collect();
    let warned = sent.iter().any(|m| {
            matches!(m, Message::Notification(n) if n.method == "window/showMessage"
                && n.params["message"].as_str().unwrap().contains("could not start its SHACL validation thread"))
        });
    assert!(warned, "{sent:?}");
    let stopped = sent.iter().any(|m| {
        matches!(m, Message::Notification(n) if n.method == "$/progress"
                && n.params["value"]["kind"] == "end"
                && n.params["value"]["message"] == "Validation stopped")
    });
    assert!(stopped, "{sent:?}");
    assert!(!server.progress.is_shown(Work::Shacl));
    assert!(!server.timers.is_set(Timer::ShaclProgress));

    // The next validation starts the thread and gets its answer.
    server.validate_shacl();
    let outcome = server
        .shacl
        .rx()
        .recv_timeout(Duration::from_secs(10))
        .expect("an outcome from the thread");
    assert!(server.shacl.is_current(&outcome));
}

/// Each answer is the text of the `Value` the same result makes (what
/// lsp-server wrote): the result itself, not wrapped or reordered.
#[test]
fn an_answer_is_the_text_of_its_result_as_a_value() {
    let mut e = embedded();
    let uri = "file:///w/a.ttl";
    let text =
        "@prefix ex: <http://e.org/> .\nex:A a ex:B ;\n  ex:p \"x\u{e9}\" .\nex:B ex:q ex:A .\n";
    open(&mut e, uri, text);
    let doc = json!({"textDocument": {"uri": uri}});
    let pos = Position::new(1, 1);
    let at = json!({"textDocument": {"uri": uri}, "position": pos});
    let options = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        ..Default::default()
    };
    let range = Range::new(Position::new(0, 0), Position::new(9, 0));
    let completion: CompletionParams = serde_json::from_value(
        json!({"textDocument": {"uri": uri}, "position": {"line": 3, "character": 4}}),
    )
    .unwrap();
    let value = |v: serde_json::Result<Value>| v.unwrap().to_string();
    let mut s = e.server.session();
    let cases = [
        (
            "textDocument/documentSymbol",
            doc.clone(),
            value(serde_json::to_value(
                s.document_symbols(uri).map(DocumentSymbolResponse::Nested),
            )),
        ),
        (
            "textDocument/hover",
            at.clone(),
            value(serde_json::to_value(s.hover(uri, pos))),
        ),
        (
            "textDocument/definition",
            at.clone(),
            value(serde_json::to_value(
                s.definition(uri, pos).map(GotoDefinitionResponse::Array),
            )),
        ),
        (
            "textDocument/references",
            json!({"textDocument": {"uri": uri}, "position": pos, "context": {"includeDeclaration": true}}),
            value(serde_json::to_value(s.references(uri, pos, true))),
        ),
        (
            "textDocument/foldingRange",
            doc.clone(),
            value(serde_json::to_value(s.folding_ranges(uri))),
        ),
        (
            "textDocument/semanticTokens/full",
            doc.clone(),
            value(serde_json::to_value(s.semantic_tokens_full(uri))),
        ),
        (
            "textDocument/formatting",
            json!({"textDocument": {"uri": uri}, "options": options}),
            value(serde_json::to_value(s.formatting(uri, &options))),
        ),
        (
            "textDocument/inlayHint",
            json!({"textDocument": {"uri": uri}, "range": range}),
            value(serde_json::to_value(s.inlay_hints(uri, range))),
        ),
        (
            "textDocument/completion",
            serde_json::to_value(&completion).unwrap(),
            value(serde_json::to_value(s.completion(&completion))),
        ),
    ];
    for (method, params, expected) in cases {
        let req = Request::new(1.into(), method.into(), params);
        let got = e.server.request(req).unwrap().unwrap();
        assert_eq!(got, expected, "{method}");
    }
}

/// The Run query commands take one document URI, and are advertised.
#[test]
fn run_query_commands_take_a_uri_and_options() {
    let mut e = embedded();
    let run = |e: &mut Embedded, command: &str, arguments: Value| {
        let params = json!({ "command": command, "arguments": arguments });
        let req = Request::new(1.into(), "workspace/executeCommand".into(), params);
        e.server.request(req)
    };
    let Err((code, msg)) = run(&mut e, run_query::RUN_QUERY, json!([])) else {
        panic!("accepted")
    };
    assert_eq!(code as i32, ErrorCode::InvalidParams as i32);
    assert!(
        msg.starts_with("turtle.runQuery takes a document URI, then"),
        "{msg}"
    );
    let q = "file:///w/q.rq";
    for bad in [
        json!([q, { "reasoning": "on" }]),
        json!([q, {}, {}]),
        json!([q, "off"]),
    ] {
        let Err((code, _)) = run(&mut e, run_query::RUN_QUERY, bad.clone()) else {
            panic!("accepted {bad}")
        };
        assert_eq!(code as i32, ErrorCode::InvalidParams as i32, "{bad}");
    }
    assert!(run(&mut e, run_query::CANCEL_QUERY, json!([1])).is_err());
    assert!(run(&mut e, run_query::CANCEL_QUERY, json!([q, {}])).is_err());
    let cancelled = run(&mut e, run_query::CANCEL_QUERY, json!(["file:///w/q.rq"]));
    assert!(matches!(cancelled, Ok(Some(n)) if n == "null"));
    let commands =
        capabilities(Encoding::Utf16, false, Default::default()).execute_command_provider;
    assert_eq!(
        commands.unwrap().commands,
        [
            "turtle.fetchImport",
            "turtle.runQuery",
            "turtle.cancelQuery"
        ]
    );
}

/// The `initialize` result is the text the `Value` it was built as
/// gave: lsp-types' result with `typeHierarchyProvider` set in it.
#[test]
fn the_initialize_result_is_the_text_its_value_gave() {
    let all = registrations::Dynamic {
        formatting: true,
        rename: true,
        completion: true,
        code_action: true,
    };
    for enc in [Encoding::Utf8, Encoding::Utf16] {
        for (pull, dynamic) in [
            (false, Default::default()),
            (true, Default::default()),
            (false, all),
        ] {
            let result = InitializeResult {
                capabilities: capabilities(enc, pull, dynamic),
                server_info: Some(ServerInfo {
                    name: "turtle-lsp".into(),
                    version: Some(crate::VERSION.into()),
                }),
            };
            let mut value = serde_json::to_value(result).unwrap();
            value["capabilities"]["typeHierarchyProvider"] = Value::Bool(true);
            let ours = wire::encode(&Initialized::new(capabilities(enc, pull, dynamic))).unwrap();
            assert_eq!(ours, value.to_string());
        }
    }
}

#[test]
fn guard_turns_a_panic_into_an_error() {
    let r: Result<(), String> = quietly(|| guard("hover", || panic!("boom")));
    let msg = r.unwrap_err();
    assert!(msg.contains("hover") && msg.contains("boom"), "{msg}");
    assert_eq!(guard("x", || 7), Ok(7));
}
