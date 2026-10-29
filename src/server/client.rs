//! The server's way to the client: the connection's sender and the ids of
//! the requests it sends. The state groups send through it, never through
//! the server.

use super::wire::Out;
use crossbeam_channel::{SendError, Sender};
use lsp_server::{Message, Notification, Request, RequestId};
use lsp_types::notification::Notification as _;
use lsp_types::{MessageType, ShowMessageParams};

pub(super) struct Client {
    sender: Sender<Out>,
    /// The number of the last request sent.
    next_id: i32,
}

impl Client {
    pub(super) fn new(sender: Sender<Out>) -> Client {
        Client { sender, next_id: 0 }
    }

    /// Send `msg`; Err when the connection is gone.
    pub(super) fn send(&self, msg: impl Into<Message>) -> Result<(), SendError<Out>> {
        self.sender.send(Out::Message(msg.into()))
    }

    /// Answer request `id` with `json`, its result as JSON text
    /// (`wire::encode`); Err when the connection is gone.
    pub(super) fn respond(&self, id: RequestId, json: String) -> Result<(), SendError<Out>> {
        self.sender.send(Out::Result { id, json })
    }

    /// The connection's sender, for a thread that answers a request
    /// itself. The connection stays open while the thread holds it.
    pub(super) fn sender(&self) -> Sender<Out> {
        self.sender.clone()
    }

    /// Send a notification (a closed connection drops it).
    pub(super) fn notify(&self, method: &str, params: impl serde::Serialize) {
        let _ = self.send(Notification::new(method.into(), params));
    }

    /// Show `message` as a warning.
    pub(super) fn warn(&self, message: String) {
        self.show(MessageType::WARNING, message);
    }

    pub(super) fn show(&self, typ: MessageType, message: String) {
        let params = ShowMessageParams { typ, message };
        self.notify(lsp_types::notification::ShowMessage::METHOD, params);
    }

    /// The number the next request takes (`turtle-<n>`).
    pub(super) fn next_number(&self) -> i32 {
        self.next_id + 1
    }

    /// Send a request; the answer is matched by the returned id.
    pub(super) fn request(&mut self, method: &str, params: impl serde::Serialize) -> RequestId {
        self.next_id += 1;
        let id = RequestId::from(format!("turtle-{}", self.next_id));
        let _ = self.send(Request::new(id.clone(), method.into(), params));
        id
    }
}
