//! Work-done progress (`window/workDoneProgress`): one progress per kind of
//! work, shown only when the client declared it can show them.
//!
//! The server asks the client for a token and goes on: `begin` goes out once
//! the client accepts it, and an `end` that comes first waits for that. A
//! refused token shows nothing.

use super::client::Client;
use lsp_server::RequestId;
use lsp_types::notification::Notification as _;
use lsp_types::request::Request as _;
use lsp_types::{
    NumberOrString, ProgressParams, ProgressParamsValue, WorkDoneProgress, WorkDoneProgressBegin,
    WorkDoneProgressCreateParams, WorkDoneProgressEnd,
};
use rustc_hash::{FxHashMap, FxHashSet};

/// The kinds of work that show progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Work {
    Index,
    Imports,
    Shacl,
}

pub(super) struct Progress {
    /// The client declared `window.workDoneProgress`.
    supported: bool,
    /// The token of the progress shown, per kind of work ([`Work`] order).
    shown: [Option<String>; 3],
    /// Progress whose create request is unanswered, by that request's id.
    pending: FxHashMap<RequestId, Pending>,
    /// Tokens whose `begin` went out and whose `end` has not.
    live: FxHashSet<String>,
}

/// A progress waiting for the client to accept its token.
struct Pending {
    token: String,
    title: String,
    /// The end message, when the work ended before the client answered.
    ended: Option<String>,
}

impl Progress {
    pub(super) fn new(supported: bool) -> Progress {
        Progress {
            supported,
            shown: Default::default(),
            pending: FxHashMap::default(),
            live: FxHashSet::default(),
        }
    }

    /// The client can show progress.
    pub(super) fn supported(&self) -> bool {
        self.supported
    }

    /// A progress of `work` is shown (begun and not ended).
    pub(super) fn is_shown(&self, work: Work) -> bool {
        self.shown[work as usize].is_some()
    }

    /// Show progress of `work` titled `title`, when the client supports
    /// progress: ask for a token now, `begin` once the client accepts it (see
    /// [`Progress::accepted`]). The main loop never waits for that.
    pub(super) fn begin(&mut self, client: &mut Client, work: Work, title: &str) {
        self.shown[work as usize] = self.create(client, title);
    }

    fn create(&mut self, client: &mut Client, title: &str) -> Option<String> {
        if !self.supported {
            return None;
        }
        let token = format!("turtle-progress-{}", client.next_number());
        let id = client.request(
            lsp_types::request::WorkDoneProgressCreate::METHOD,
            WorkDoneProgressCreateParams {
                token: NumberOrString::String(token.clone()),
            },
        );
        self.pending.insert(
            id,
            Pending {
                token: token.clone(),
                title: title.into(),
                ended: None,
            },
        );
        Some(token)
    }

    /// The client answered request `id`: when it asked for a token, begin
    /// (and end, if the work is already over). A refused token is dropped.
    pub(super) fn accepted(&mut self, client: &Client, id: &RequestId, ok: bool) {
        let Some(p) = self.pending.remove(id) else {
            return;
        };
        if !ok {
            return;
        }
        send(
            client,
            &p.token,
            WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: p.title,
                cancellable: Some(false),
                ..Default::default()
            }),
        );
        match p.ended {
            Some(message) => send_end(client, &p.token, message),
            None => {
                self.live.insert(p.token);
            }
        }
    }

    /// The progress of `work` ends with `message` (once begun, if it has not).
    pub(super) fn end(&mut self, client: &Client, work: Work, message: String) {
        let Some(token) = self.shown[work as usize].take() else {
            return;
        };
        if self.live.remove(&token) {
            send_end(client, &token, message);
        } else if let Some(p) = self.pending.values_mut().find(|p| p.token == token) {
            p.ended = Some(message);
        }
    }
}

fn send_end(client: &Client, token: &str, message: String) {
    send(
        client,
        token,
        WorkDoneProgress::End(WorkDoneProgressEnd {
            message: Some(message),
        }),
    );
}

fn send(client: &Client, token: &str, value: WorkDoneProgress) {
    let params = ProgressParams {
        token: NumberOrString::String(token.into()),
        value: ProgressParamsValue::WorkDone(value),
    };
    client.notify(lsp_types::notification::Progress::METHOD, params);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{Receiver, unbounded};
    use lsp_server::Message;

    /// What the client was sent: "request <method>", or "<kind> <message>"
    /// for a progress notification.
    fn sent(rx: &Receiver<Message>) -> Vec<String> {
        rx.try_iter()
            .map(|m| match m {
                Message::Request(r) => format!("request {}", r.method),
                Message::Notification(n) if n.method == "$/progress" => {
                    let v = &n.params["value"];
                    let text = v["title"].as_str().or(v["message"].as_str()).unwrap_or("");
                    format!("{} {text}", v["kind"].as_str().unwrap())
                }
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn an_end_before_the_token_is_accepted_goes_out_after_the_begin() {
        let (tx, rx) = unbounded();
        let mut client = Client::new(tx);
        let mut p = Progress::new(true);
        p.begin(&mut client, Work::Index, "Indexing workspace");
        assert!(p.is_shown(Work::Index) && !p.is_shown(Work::Shacl));
        assert_eq!(sent(&rx), ["request window/workDoneProgress/create"]);
        p.end(&client, Work::Index, "3 files".into());
        assert!(!p.is_shown(Work::Index));
        assert!(sent(&rx).is_empty(), "nothing before the client accepts");
        p.accepted(&client, &RequestId::from("turtle-1".to_string()), true);
        assert_eq!(sent(&rx), ["begin Indexing workspace", "end 3 files"]);
        // An accepted token that is still going ends when the work does.
        p.begin(&mut client, Work::Shacl, "Validating SHACL shapes");
        p.accepted(&client, &RequestId::from("turtle-2".to_string()), true);
        p.end(&client, Work::Shacl, "No problems".into());
        assert_eq!(
            sent(&rx),
            [
                "request window/workDoneProgress/create",
                "begin Validating SHACL shapes",
                "end No problems"
            ]
        );
    }

    #[test]
    fn a_refused_token_sends_nothing() {
        let (tx, rx) = unbounded();
        let mut client = Client::new(tx);
        let mut p = Progress::new(true);
        p.begin(&mut client, Work::Imports, "Resolving owl:imports");
        sent(&rx);
        p.accepted(&client, &RequestId::from("turtle-1".to_string()), false);
        p.end(&client, Work::Imports, "All imports resolved".into());
        assert!(sent(&rx).is_empty());
        // Nor does a client that cannot show progress get asked.
        let mut p = Progress::new(false);
        p.begin(&mut client, Work::Index, "Indexing workspace");
        assert!(!p.is_shown(Work::Index));
        assert!(sent(&rx).is_empty());
    }
}
