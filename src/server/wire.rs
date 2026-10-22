//! The server's side of the connection: what it sends ([`Out`]), the stdio
//! transport that writes it, and the `initialize` handshake.
//!
//! A request's result goes out as JSON text ([`encode`]), never as a
//! `serde_json::Value`: a large answer (25,000 document symbols are 5.5 MB
//! of JSON) would otherwise exist three times at once, as the Rust values,
//! the `Value` tree and the text, and the allocator keeps the pages of the
//! largest answer seen. The bytes are the ones lsp-server's
//! `Message::write` writes for the same `Response`: [`encode`] orders
//! object members as `serde_json::Map` does, and [`Out::write`] frames the
//! text as `Message::write` does.
//!
//! What the server sends goes through one channel to one writer thread, in
//! the order sent, so a response never passes a notification sent before
//! it. A host that reads in the same thread (`Embedded`, the tests) gets
//! [`Message`]s from an [`Outbox`]: a raw result is parsed back into a
//! `Value` there.

use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_server::{ErrorCode, Message, RequestId, Response};
use serde::Serialize;
use serde::ser::{self, Serializer};
use serde_json::{Result, Value};
use std::borrow::Cow;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{self, BufRead, Write};

/// Something the server sends.
#[derive(Debug)]
pub(super) enum Out {
    Message(Message),
    /// The answer to request `id`: its result, as JSON text ([`encode`]).
    Result {
        id: RequestId,
        json: String,
    },
}

impl Out {
    /// The message this is: a raw result is parsed back into a `Value`.
    pub(super) fn into_message(self) -> Message {
        match self {
            Out::Message(m) => m,
            Out::Result { id, json } => match serde_json::from_str(&json) {
                Ok(result) => Response {
                    id,
                    response_result: Ok(result),
                },
                Err(e) => Response::new_err(id, ErrorCode::InternalError as i32, e.to_string()),
            }
            .into(),
        }
    }

    /// Write this as one LSP frame, byte for byte as `Message::write` writes
    /// the same message: `{"jsonrpc":"2.0","id":…,"result":…}`.
    #[cfg(not(target_arch = "wasm32"))]
    fn write(&self, w: &mut impl Write) -> io::Result<()> {
        match self {
            Out::Message(m) => m.write(w),
            Out::Result { id, json } => {
                let head = format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":",
                    serde_json::to_string(id)?
                );
                write!(w, "Content-Length: {}\r\n\r\n", head.len() + json.len() + 1)?;
                w.write_all(head.as_bytes())?;
                w.write_all(json.as_bytes())?;
                w.write_all(b"}")?;
                w.flush()
            }
        }
    }
}

/// Write everything sent on `outbox` to `w`, in order, until every sender
/// is gone or a write fails.
#[cfg(not(target_arch = "wasm32"))]
fn write_all(outbox: Receiver<Out>, w: &mut impl Write) -> io::Result<()> {
    outbox.into_iter().try_for_each(|out| out.write(w))
}

/// Read messages from `input` onto `inbox` until the input ends or `exit`
/// is read (nothing after `exit` is read).
#[cfg(not(target_arch = "wasm32"))]
fn read(input: &mut impl BufRead, inbox: &Sender<Message>) -> io::Result<()> {
    while let Some(msg) = Message::read(input)? {
        let exit = matches!(&msg, Message::Notification(n) if n.method == EXIT);
        inbox.send(msg).map_err(io::Error::other)?;
        if exit {
            break;
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
const EXIT: &str = "exit";

/// The threads of the stdio transport.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct IoThreads {
    reader: std::thread::JoinHandle<io::Result<()>>,
    writer: std::thread::JoinHandle<io::Result<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl IoThreads {
    /// Wait for the reader, then the writer: the first error either had.
    pub(super) fn join(self) -> io::Result<()> {
        match self.reader.join() {
            Ok(r) => r?,
            Err(panic) => std::panic::resume_unwind(panic),
        }
        match self.writer.join() {
            Ok(r) => r,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

/// The connection over stdin and stdout: the sender of what the server
/// writes, the messages read (queued as they are read, so the server takes
/// them a batch at a time), and the two threads.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn stdio() -> io::Result<(Sender<Out>, Receiver<Message>, IoThreads)> {
    // A rendezvous channel, as lsp-server's: the server waits for the writer
    // to take each message, so a client that stops reading holds the server
    // back instead of filling its memory.
    let (out, outbox) = crossbeam_channel::bounded::<Out>(0);
    let writer = std::thread::Builder::new()
        .name("turtle-lsp-writer".into())
        .spawn(move || write_all(outbox, &mut io::stdout().lock()))?;
    let (inbox_tx, inbox) = unbounded();
    let reader = std::thread::Builder::new()
        .name("turtle-lsp-reader".into())
        .spawn(move || read(&mut io::stdin().lock(), &inbox_tx))?;
    Ok((out, inbox, IoThreads { reader, writer }))
}

/// What the server sent, for a host that reads it in the same thread.
pub(super) struct Outbox(Receiver<Out>);

impl Outbox {
    /// The messages sent since the last call.
    pub(super) fn try_iter(&self) -> impl Iterator<Item = Message> + '_ {
        self.0.try_iter().map(Out::into_message)
    }
}

/// A sender for the server and the outbox it fills.
pub(super) fn memory() -> (Sender<Out>, Outbox) {
    let (tx, rx) = unbounded();
    (tx, Outbox(rx))
}

#[cfg(not(target_arch = "wasm32"))]
type Error = Box<dyn std::error::Error + Send + Sync>;

#[cfg(not(target_arch = "wasm32"))]
fn disconnected() -> Error {
    "disconnected channel".into()
}

/// Wait for the `initialize` request: its id and params. As lsp-server's
/// `Connection::initialize_start`: a request before it is answered
/// `ServerNotInitialized`, a notification before it is dropped, and `exit`
/// (or a response) ends the session with an error.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn initialize_start(
    out: &Sender<Out>,
    inbox: &Receiver<Message>,
) -> std::result::Result<(RequestId, Value), Error> {
    loop {
        match inbox.recv().map_err(|_| disconnected())? {
            Message::Request(req) if req.method == "initialize" => return Ok((req.id, req.params)),
            Message::Request(req) => {
                let resp = Response::new_err(
                    req.id.clone(),
                    ErrorCode::ServerNotInitialized as i32,
                    format!("expected initialize request, got {req:?}"),
                );
                out.send(Out::Message(resp.into()))?;
            }
            Message::Notification(n) if n.method != EXIT => {}
            msg => return Err(format!("expected initialize request, got {msg:?}").into()),
        }
    }
}

/// Answer `initialize` (request `id`) with `result`, JSON text, and wait for
/// `initialized`. As lsp-server's `Connection::initialize_finish`: any other
/// message ends the session with an error.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn initialize_finish(
    out: &Sender<Out>,
    inbox: &Receiver<Message>,
    id: RequestId,
    result: String,
) -> std::result::Result<(), Error> {
    out.send(Out::Result { id, json: result })?;
    match inbox.recv() {
        Ok(Message::Notification(n)) if n.method == "initialized" => Ok(()),
        Ok(msg) => Err(format!("expected initialized notification, got: {msg:?}").into()),
        Err(_) => Err(disconnected()),
    }
}

/// `value` as JSON text: the text `serde_json::to_string` writes for
/// `serde_json::to_value(value)`, without building the `Value`. serde_json
/// is built without `preserve_order`, so a `Value` object is a `BTreeMap`:
/// its members come out sorted by key, and of a key given twice the last
/// value is kept. Here only an object's members wait, as text, to be
/// sorted; arrays are written as they come. Numbers whose `Value` writes
/// differently from the number itself (`f32`, `i128`, `u128`) go through
/// their `Value`.
pub(super) fn encode<T: ?Sized + Serialize>(value: &T) -> Result<String> {
    let mut out = Vec::new();
    value.serialize(Canonical(&mut out))?;
    // serde_json writes UTF-8 only.
    String::from_utf8(out).map_err(ser::Error::custom)
}

/// Writes one value as its `Value` would be written.
struct Canonical<'a>(&'a mut Vec<u8>);

impl<'a> Canonical<'a> {
    /// As serde_json writes `v`, which is how its `Value` writes too.
    fn plain<T: ?Sized + Serialize>(self, v: &T) -> Result<()> {
        serde_json::to_writer(&mut *self.0, v)
    }

    /// Through `v`'s `Value`.
    fn via_value<T: Serialize>(self, v: T) -> Result<()> {
        serde_json::to_writer(&mut *self.0, &serde_json::to_value(v)?)
    }

    /// `{"variant":`, the start of an externally tagged variant.
    fn variant(self, variant: &str) -> Result<&'a mut Vec<u8>> {
        self.0.push(b'{');
        serde_json::to_writer(&mut *self.0, variant)?;
        self.0.push(b':');
        Ok(self.0)
    }
}

impl<'a> Serializer for Canonical<'a> {
    type Ok = ();
    type Error = serde_json::Error;
    type SerializeSeq = Array<'a>;
    type SerializeTuple = Array<'a>;
    type SerializeTupleStruct = Array<'a>;
    type SerializeTupleVariant = Array<'a>;
    type SerializeMap = Object<'a>;
    type SerializeStruct = Object<'a>;
    type SerializeStructVariant = Object<'a>;

    fn serialize_bool(self, v: bool) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_i8(self, v: i8) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_i16(self, v: i16) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_i32(self, v: i32) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_i64(self, v: i64) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_i128(self, v: i128) -> Result<()> {
        self.via_value(v)
    }
    fn serialize_u8(self, v: u8) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_u16(self, v: u16) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_u32(self, v: u32) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_u64(self, v: u64) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_u128(self, v: u128) -> Result<()> {
        self.via_value(v)
    }
    // Its `Value` holds it as an f64.
    fn serialize_f32(self, v: f32) -> Result<()> {
        self.via_value(v)
    }
    fn serialize_f64(self, v: f64) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_char(self, v: char) -> Result<()> {
        self.plain(&v)
    }
    fn serialize_str(self, v: &str) -> Result<()> {
        self.plain(v)
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<()> {
        self.plain(v)
    }
    fn serialize_none(self) -> Result<()> {
        self.serialize_unit()
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<()> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<()> {
        self.0.extend_from_slice(b"null");
        Ok(())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<()> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<()> {
        self.plain(variant)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<()> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<()> {
        let out = self.variant(variant)?;
        value.serialize(Canonical(&mut *out))?;
        out.push(b'}');
        Ok(())
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Array<'a>> {
        Ok(Array::new(self.0, "]"))
    }
    fn serialize_tuple(self, len: usize) -> Result<Array<'a>> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<Array<'a>> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Array<'a>> {
        Ok(Array::new(self.variant(variant)?, "]}"))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Object<'a>> {
        Ok(Object::new(self.0, "}"))
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Object<'a>> {
        self.serialize_map(Some(len))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Object<'a>> {
        Ok(Object::new(self.variant(variant)?, "}}"))
    }
}

/// An array, written element by element.
struct Array<'a> {
    out: &'a mut Vec<u8>,
    empty: bool,
    close: &'static str,
}

impl<'a> Array<'a> {
    fn new(out: &'a mut Vec<u8>, close: &'static str) -> Array<'a> {
        out.push(b'[');
        Array {
            out,
            empty: true,
            close,
        }
    }

    fn element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        if !std::mem::take(&mut self.empty) {
            self.out.push(b',');
        }
        value.serialize(Canonical(&mut *self.out))
    }

    fn close(self) -> Result<()> {
        self.out.extend_from_slice(self.close.as_bytes());
        Ok(())
    }
}

impl ser::SerializeSeq for Array<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }
    fn end(self) -> Result<()> {
        self.close()
    }
}

impl ser::SerializeTuple for Array<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }
    fn end(self) -> Result<()> {
        self.close()
    }
}

impl ser::SerializeTupleStruct for Array<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }
    fn end(self) -> Result<()> {
        self.close()
    }
}

impl ser::SerializeTupleVariant for Array<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }
    fn end(self) -> Result<()> {
        self.close()
    }
}

/// An object: its members are kept as text until it ends, then written
/// sorted by key.
struct Object<'a> {
    out: &'a mut Vec<u8>,
    /// Each member's key, and where its value is in `values`.
    members: Vec<(Cow<'static, str>, std::ops::Range<usize>)>,
    values: Vec<u8>,
    /// The key given to `serialize_key`, for the next `serialize_value`.
    key: Option<String>,
    close: &'static str,
}

impl<'a> Object<'a> {
    fn new(out: &'a mut Vec<u8>, close: &'static str) -> Object<'a> {
        out.push(b'{');
        Object {
            out,
            members: Vec::new(),
            values: Vec::new(),
            key: None,
            close,
        }
    }

    fn member<T: ?Sized + Serialize>(&mut self, key: Cow<'static, str>, value: &T) -> Result<()> {
        let start = self.values.len();
        value.serialize(Canonical(&mut self.values))?;
        self.members.push((key, start..self.values.len()));
        Ok(())
    }

    fn close(mut self) -> Result<()> {
        // Stable: of a key given twice, the later value sorts after.
        self.members.sort_by(|a, b| a.0.cmp(&b.0));
        let mut empty = true;
        for (i, (key, value)) in self.members.iter().enumerate() {
            if self.members.get(i + 1).is_some_and(|next| next.0 == *key) {
                // A later value replaces it, as in a map.
                continue;
            }
            if !std::mem::take(&mut empty) {
                self.out.push(b',');
            }
            serde_json::to_writer(&mut *self.out, key.as_ref())?;
            self.out.push(b':');
            self.out.extend_from_slice(&self.values[value.clone()]);
        }
        self.out.extend_from_slice(self.close.as_bytes());
        Ok(())
    }
}

/// The text `key` takes as an object key: what serde_json's `Value` makes
/// of it (a string as itself, a number or a bool as its text, anything
/// else an error).
fn map_key<K: ?Sized + Serialize>(key: &K) -> Result<String> {
    struct OneKey<'k, K: ?Sized>(&'k K);
    impl<K: ?Sized + Serialize> Serialize for OneKey<'_, K> {
        fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
            use ser::SerializeMap as _;
            let mut map = s.serialize_map(Some(1))?;
            map.serialize_entry(self.0, &())?;
            map.end()
        }
    }
    match serde_json::to_value(OneKey(key))? {
        Value::Object(map) => map
            .into_iter()
            .next()
            .map(|(k, _)| k)
            .ok_or_else(|| ser::Error::custom("no key")),
        _ => Err(ser::Error::custom("no key")),
    }
}

impl ser::SerializeMap for Object<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<()> {
        self.key = Some(map_key(key)?);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        // A bug in the caller, as serde_json's own map says.
        let key = self
            .key
            .take()
            .expect("serialize_value called before serialize_key");
        self.member(Cow::Owned(key), value)
    }
    fn end(self) -> Result<()> {
        self.close()
    }
}

impl ser::SerializeStruct for Object<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.member(Cow::Borrowed(key), value)
    }
    fn end(self) -> Result<()> {
        self.close()
    }
}

impl ser::SerializeStructVariant for Object<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.member(Cow::Borrowed(key), value)
    }
    fn end(self) -> Result<()> {
        self.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::{Notification, Request};
    use lsp_types::*;
    use serde_json::json;
    use std::collections::{BTreeMap, HashMap};

    /// The frame lsp-server writes for the response to `id` with `result`.
    fn lsp_server_frame(id: &RequestId, result: &impl Serialize) -> Vec<u8> {
        let mut w = Vec::new();
        Message::from(Response::new_ok(id.clone(), result))
            .write(&mut w)
            .unwrap();
        w
    }

    fn frame(id: &RequestId, result: &impl Serialize) -> Vec<u8> {
        let mut w = Vec::new();
        Out::Result {
            id: id.clone(),
            json: encode(result).unwrap(),
        }
        .write(&mut w)
        .unwrap();
        w
    }

    #[track_caller]
    fn same_frame(result: &impl Serialize) {
        for id in [
            RequestId::from(7),
            RequestId::from(-3),
            RequestId::from("turtle-\"1\"\u{e9}".to_string()),
        ] {
            let ours = String::from_utf8(frame(&id, result)).unwrap();
            let theirs = String::from_utf8(lsp_server_frame(&id, result)).unwrap();
            assert_eq!(ours, theirs);
        }
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn range(a: u32, b: u32) -> Range {
        Range {
            start: pos(a, 1),
            end: pos(b, 2),
        }
    }

    #[allow(deprecated)]
    fn symbol(name: &str, children: Option<Vec<DocumentSymbol>>) -> DocumentSymbol {
        DocumentSymbol {
            name: name.into(),
            detail: Some("ex:Thing \u{2014} \"quoted\"\n\ttab \\ \u{1}".into()),
            kind: SymbolKind::CLASS,
            tags: None,
            deprecated: None,
            range: range(1, 4),
            selection_range: range(1, 1),
            children,
        }
    }

    #[test]
    fn a_result_frame_is_the_bytes_lsp_server_writes() {
        same_frame(&Value::Null);
        same_frame(&());
        same_frame(&Option::<Hover>::None);
        same_frame(&"cancelled: a newer run replaced it");
        same_frame(&json!({"b": [1, -2, 2.5, 1e300, true, null], "a": {"z": "", "y": {}}, "": []}));
        let nested = vec![
            symbol(
                "ex:A",
                Some(vec![symbol("ex:p", None), symbol("ex:q", Some(vec![]))]),
            ),
            symbol("ex:B", None),
        ];
        same_frame(&Some(DocumentSymbolResponse::Nested(nested)));
        let edits = vec![TextEdit {
            range: range(0, 9),
            new_text: "@prefix ex: <http://e.org/> .\n".into(),
        }];
        let mut changes = HashMap::new();
        for u in ["file:///w/b.ttl", "file:///w/a.ttl", "file:///w/%C3%A9.ttl"] {
            changes.insert(u.parse::<Uri>().unwrap(), edits.clone());
        }
        same_frame(&WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        });
        let report: DocumentDiagnosticReportResult =
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: Some("3:1:0".into()),
                    items: vec![Diagnostic {
                        range: range(2, 2),
                        severity: Some(DiagnosticSeverity::WARNING),
                        code: Some(NumberOrString::String("undeclared-prefix".into())),
                        source: Some("turtle".into()),
                        message: "Prefix `ex:` is not declared".into(),
                        data: Some(json!({"prefix": "ex", "iri": "http://e.org/"})),
                        ..Default::default()
                    }],
                },
            })
            .into();
        same_frame(&report);
        same_frame(&CompletionResponse::List(CompletionList {
            is_incomplete: true,
            items: vec![CompletionItem {
                label: "ex:Thing".into(),
                kind: Some(CompletionItemKind::CLASS),
                sort_text: Some("0001".into()),
                data: Some(json!({"uri": "file:///w/a.ttl", "n": 3})),
                ..Default::default()
            }],
        }));
        same_frame(&SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: vec![SemanticToken {
                delta_line: 1,
                delta_start: 2,
                length: 3,
                token_type: 4,
                token_modifiers_bitset: 5,
            }],
        }));
        // Numbers a `Value` holds otherwise, keys that are not strings, and
        // the shapes of enum variants.
        #[derive(Serialize)]
        enum E {
            Unit,
            New(u8),
            Tuple(i8, f32),
            Struct { z: u8, a: u8 },
        }
        let mut keys = BTreeMap::new();
        keys.insert(10, "ten");
        keys.insert(9, "nine");
        same_frame(&(
            0.1f32,
            f32::NAN,
            f64::INFINITY,
            -0.0f64,
            u64::MAX,
            i64::MIN,
            7i128,
            'é',
            keys,
            [
                E::Unit,
                E::New(1),
                E::Tuple(-1, 0.5),
                E::Struct { z: 1, a: 2 },
            ],
        ));
        // A key given twice keeps its last value, as a `Value` map does.
        #[derive(Serialize)]
        struct Twice {
            b: u8,
            #[serde(flatten)]
            more: BTreeMap<&'static str, u8>,
        }
        same_frame(&Twice {
            b: 1,
            more: [("b", 2), ("a", 3)].into_iter().collect(),
        });
    }

    #[test]
    fn a_raw_result_reads_back_as_the_value() {
        let id = RequestId::from(4);
        let result = json!({"b": 1, "a": [1.5, "x"]});
        let back = Out::Result {
            id: id.clone(),
            json: encode(&result).unwrap(),
        }
        .into_message();
        match back {
            Message::Response(r) => {
                assert_eq!(r.id, id);
                assert_eq!(r.response_result.unwrap(), result);
            }
            other => panic!("{other:?}"),
        }
    }

    /// The frames in `bytes`, as their bodies.
    fn bodies(bytes: &[u8]) -> Vec<Value> {
        let mut r = bytes;
        let mut out = Vec::new();
        while let Some(m) = Message::read(&mut r).unwrap() {
            out.push(serde_json::to_value(m).unwrap());
        }
        out
    }

    #[test]
    fn a_message_sent_before_a_result_is_written_before_it() {
        let (tx, rx) = unbounded();
        let note = Notification::new("textDocument/publishDiagnostics".into(), json!({"n": 1}));
        tx.send(Out::Message(note.into())).unwrap();
        tx.send(Out::Result {
            id: RequestId::from(1),
            json: "[1]".into(),
        })
        .unwrap();
        tx.send(Out::Message(
            Response::new_ok(RequestId::from(2), ()).into(),
        ))
        .unwrap();
        drop(tx);
        let mut w = Vec::new();
        write_all(rx, &mut w).unwrap();
        assert_eq!(
            bodies(&w),
            [
                json!({"method": "textDocument/publishDiagnostics", "params": {"n": 1}}),
                json!({"id": 1, "result": [1]}),
                json!({"id": 2, "result": null}),
            ]
        );
    }

    fn request(id: i32, method: &str) -> Message {
        Request::new(id.into(), method.into(), json!({"capabilities": {}})).into()
    }

    fn notification(method: &str) -> Message {
        Notification::new(method.into(), Value::Null).into()
    }

    #[test]
    fn the_handshake_refuses_requests_before_initialize_and_waits_for_initialized() {
        let (out, outbox) = memory();
        let (tx, inbox) = unbounded();
        for m in [
            request(1, "textDocument/hover"),
            notification("textDocument/didOpen"),
            request(2, "initialize"),
            notification("initialized"),
        ] {
            tx.send(m).unwrap();
        }
        let (id, params) = initialize_start(&out, &inbox).unwrap();
        assert_eq!(
            (id.clone(), params),
            (2.into(), json!({"capabilities": {}}))
        );
        let refused: Vec<Message> = outbox.try_iter().collect();
        match refused.as_slice() {
            [Message::Response(r)] => {
                assert_eq!(r.id, 1.into());
                let e = r.response_result.as_ref().unwrap_err();
                assert_eq!(e.code, ErrorCode::ServerNotInitialized as i32);
                assert!(
                    e.message
                        .starts_with("expected initialize request, got Request")
                );
            }
            other => panic!("{other:?}"),
        }
        initialize_finish(&out, &inbox, id, "{\"capabilities\":{}}".into()).unwrap();
        match outbox.try_iter().collect::<Vec<_>>().as_slice() {
            [Message::Response(r)] => {
                assert_eq!(r.id, 2.into());
                assert_eq!(
                    r.response_result.as_ref().unwrap(),
                    &json!({"capabilities": {}})
                );
            }
            other => panic!("{other:?}"),
        }
        // Anything but `initialized` after the answer ends the session.
        tx.send(request(3, "initialize")).unwrap();
        let e = initialize_finish(&out, &inbox, 3.into(), "{}".into()).unwrap_err();
        assert!(
            e.to_string()
                .starts_with("expected initialized notification, got: Request")
        );
        drop(tx);
        let e = initialize_finish(&out, &inbox, 4.into(), "{}".into()).unwrap_err();
        assert_eq!(e.to_string(), "disconnected channel");
    }

    #[test]
    fn exit_before_initialize_ends_the_session_and_the_reader() {
        let (out, _outbox) = memory();
        let (tx, inbox) = unbounded();
        tx.send(notification("exit")).unwrap();
        let e = initialize_start(&out, &inbox).unwrap_err();
        assert!(
            e.to_string()
                .starts_with("expected initialize request, got Notification")
        );
        drop(tx);
        assert_eq!(
            initialize_start(&out, &inbox).unwrap_err().to_string(),
            "disconnected channel"
        );

        // The reader stops at `exit`: what follows is not read.
        let mut input = Vec::new();
        for m in [
            request(1, "initialize"),
            notification("exit"),
            request(2, "shutdown"),
        ] {
            m.write(&mut input).unwrap();
        }
        let (tx, read_back) = unbounded();
        read(&mut input.as_slice(), &tx).unwrap();
        let methods: Vec<String> = read_back
            .try_iter()
            .map(|m| match m {
                Message::Request(r) => r.method,
                Message::Notification(n) => n.method,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(methods, ["initialize", "exit"]);
    }
}
