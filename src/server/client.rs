//! The server's way to the client: the connection's sender and the ids of
//! the requests it sends. The state groups send through it, never through
//! the server.

use crossbeam_channel::{SendError, Sender};
use lsp_server::{Message, Notification, Request, RequestId};
use lsp_types::notification::Notification as _;
use lsp_types::{MessageType, ShowMessageParams};

pub(super) struct Client {
    sender: Sender<Message>,
    /// The number of the last request sent.
    next_id: i32,
}

impl Client {
    pub(super) fn new(sender: Sender<Message>) -> Client {
        Client { sender, next_id: 0 }
    }

    /// Send `msg`; Err when the connection is gone.
    pub(super) fn send(&self, msg: impl Into<Message>) -> Result<(), SendError<Message>> {
        self.sender.send(msg.into())
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
