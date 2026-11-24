//! The browser entry: [`Embedded`], a [`Server`] the host drives one step
//! at a time instead of its own loop.

use super::{Server, wire};
use crate::clock::Instant;
use crate::worker::Spawn;
use crossbeam_channel::unbounded;
use lsp_server::{Message, RequestId};
use serde_json::Value;

/// A server driven by its host instead of its own loop, for a host with no
/// threads and no stdio (the browser build, web/engine). The host hands over
/// messages and the server takes one step over them, returning what it sent;
/// the host calls [`Embedded::tick`] when [`Embedded::due_in_ms`] says a timer
/// is due. The workspace has no folders: it is the documents the host opens,
/// indexed from the start. SHACL validation and imports run inline, and no
/// import is loaded.
pub struct Embedded {
    pub(super) server: Server,
    inbox: crossbeam_channel::Sender<Message>,
    outbox: wire::Outbox,
    pub(super) ended: bool,
}

impl Embedded {
    /// A server for `initialize` params `params`, with what it sent: the
    /// initialize response first (to request id 0).
    pub fn start(params: Value) -> Result<(Embedded, Vec<Message>), String> {
        let (sender, outbox) = wire::memory();
        let (mut server, result, _roots) =
            Server::configure(sender, params, Spawn::Inline).map_err(|e| e.to_string())?;
        let (inbox, inbox_rx) = unbounded();
        server.inbox = inbox_rx;
        server
            .client
            .respond(RequestId::from(0), result)
            .map_err(|e| e.to_string())?;
        server.start();
        server.merge_index(Ok(Vec::new()));
        let mut embedded = Embedded {
            server,
            inbox,
            outbox,
            ended: false,
        };
        let sent = embedded.sent();
        Ok((embedded, sent))
    }

    /// Hand over `messages` and take one step; what the server sent.
    pub fn handle(&mut self, messages: Vec<Message>) -> Vec<Message> {
        if self.ended {
            return Vec::new();
        }
        for m in messages {
            let _ = self.inbox.send(m);
        }
        let first = self.server.inbox.try_recv().ok();
        self.step(first)
    }

    /// Run the timers that are due, then a step; what the server sent.
    pub fn tick(&mut self) -> Vec<Message> {
        if self.ended {
            return Vec::new();
        }
        self.server.fire_due();
        self.step(None)
    }

    /// Milliseconds until the next timer, if one is pending. None once
    /// the session has ended: [`Embedded::tick`] would not run it.
    pub fn due_in_ms(&self) -> Option<f64> {
        if self.ended {
            return None;
        }
        let due = self.server.next_due()?;
        Some(due.saturating_duration_since(Instant::now()).as_secs_f64() * 1000.0)
    }

    fn step(&mut self, first: Option<Message>) -> Vec<Message> {
        match self.server.step(first) {
            Ok(Some(_)) | Err(_) => self.ended = true,
            Ok(None) => {}
        }
        self.sent()
    }

    fn sent(&mut self) -> Vec<Message> {
        self.outbox.try_iter().collect()
    }
}
