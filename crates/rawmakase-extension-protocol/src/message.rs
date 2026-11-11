//! The envelope of every line, in both directions.
//!
//! ```json
//! {"t":"req","id":1,"m":"command.run","p":{...}}
//! {"t":"ok","id":1,"v":...}
//! {"t":"err","id":1,"code":"cancelled","message":"..."}
//! {"t":"progress","id":1,"done":0.5,"text":"..."}
//! {"t":"cancel","id":1}
//! {"t":"event","e":"selection-changed","p":{...}}
//! ```
//!
//! Ids are per direction: the app numbers its requests and the program its
//! own, so the same number may be in flight both ways. Every request gets
//! exactly one reply, `ok` or `err`; `progress` may come before it. Fields a
//! message doesn't know are ignored, so a later protocol version can add some
//! without breaking this one's readers.
//!
//! In API 2 an app request may carry handles to shared memory, declared beside
//! the envelope's fields ([`Handles`]): `"handles": n`, and on Windows
//! `"hv": [...]`, the values the app duplicated into the program.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One line of the protocol.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Message {
    /// A request, which the other side answers with exactly one `ok` or `err`.
    Req {
        /// Unique among this side's requests in flight.
        id: u64,
        /// The method, one of [`crate::method`]'s names.
        m: String,
        /// The method's parameters, an object (or absent for none).
        #[serde(default, skip_serializing_if = "Value::is_null")]
        p: Value,
    },
    /// A request succeeded, with its value.
    Ok {
        /// The request answered.
        id: u64,
        /// The method's result; `null` for none.
        #[serde(default)]
        v: Value,
    },
    /// A request failed.
    Err {
        /// The request answered.
        id: u64,
        /// Why, for code to act on.
        code: ErrorCode,
        /// Why, for a person to read.
        #[serde(default)]
        message: String,
    },
    /// How far a request in flight is, before its reply.
    Progress {
        /// The request it reports on.
        id: u64,
        /// The fraction done, 0 to 1, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        done: Option<f64>,
        /// What it is doing, for a status line.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Asks the other side to stop a request it is running; its reply is then
    /// `err` with [`ErrorCode::Cancelled`] (or whatever it had already sent).
    Cancel {
        /// The request to stop.
        id: u64,
    },
    /// A notification that expects no reply.
    Event {
        /// The event, one of [`crate::method`]'s event names.
        e: String,
        /// Its parameters, an object (or absent for none).
        #[serde(default, skip_serializing_if = "Value::is_null")]
        p: Value,
    },
}

impl Message {
    /// A request.
    pub fn request(id: u64, method: &str, params: Value) -> Self {
        Self::Req {
            id,
            m: method.to_string(),
            p: params,
        }
    }
    /// The reply to request `id`.
    pub fn reply(id: u64, result: Result<Value, RemoteError>) -> Self {
        match result {
            Ok(v) => Self::Ok { id, v },
            Err(e) => Self::Err {
                id,
                code: e.code,
                message: e.message,
            },
        }
    }
    /// An event.
    pub fn event(name: &str, params: Value) -> Self {
        Self::Event {
            e: name.to_string(),
            p: params,
        }
    }

    /// The line this message is sent as, without its newline. JSON escapes
    /// every newline inside strings, so the line holds none.
    pub fn encode(&self) -> Result<Vec<u8>, crate::framing::FrameError> {
        let line = serde_json::to_vec(self).map_err(|e| crate::framing::FrameError::Malformed {
            reason: e.to_string(),
        })?;
        if line.len() > crate::MAX_LINE {
            return Err(crate::framing::FrameError::TooLong);
        }
        Ok(line)
    }

    /// Reads one line. A line that isn't a message says, when it can, which
    /// request it was meant to be, so the reader can answer that request with
    /// [`ErrorCode::InvalidRequest`] instead of leaving it to its deadline.
    pub fn decode(line: &[u8]) -> Result<Self, DecodeError> {
        serde_json::from_slice(line).map_err(|e| {
            let request = serde_json::from_slice::<Value>(line)
                .ok()
                .filter(|v| v.get("t").and_then(Value::as_str) == Some("req"))
                .and_then(|v| v.get("id").and_then(Value::as_u64));
            DecodeError {
                request,
                reason: e.to_string(),
            }
        })
    }
}

/// The handles a line declares beside its message: `"handles": n`, and on
/// Windows `"hv"`, their values in the program's process. Only the app sends
/// handles, at most [`crate::MAX_HANDLES`] per message.
///
/// On Linux and macOS the handles travel as `SCM_RIGHTS` with the line's first
/// byte, and the reader takes the next `n` it received, in order. On Windows
/// the app duplicates them into the program first, and `hv` lists them in the
/// order buffer descriptors index them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handles {
    /// How many handles the message carries.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub handles: u32,
    /// Windows: the handles' values in the program, `handles` of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hv: Option<Vec<u64>>,
}
fn is_zero(n: &u32) -> bool {
    *n == 0
}
impl Handles {
    /// Reads the handles a line declares; none when it declares none. A line
    /// whose `handles` or `hv` isn't of its type doesn't decode.
    pub fn read(line: &[u8]) -> Result<Self, DecodeError> {
        // Most lines name neither field; they skip the second parse.
        let named = |field: &[u8]| line.windows(field.len()).any(|w| w == field);
        if !named(b"\"handles\"") && !named(b"\"hv\"") {
            return Ok(Self::default());
        }
        serde_json::from_slice(line).map_err(|e| DecodeError {
            request: None,
            reason: e.to_string(),
        })
    }
}

impl Message {
    /// The line this message is sent as with its handles declared, without
    /// its newline. Refused, like [`Message::encode`], when it is longer than
    /// a line.
    pub fn encode_with(&self, handles: &Handles) -> Result<Vec<u8>, crate::framing::FrameError> {
        let malformed = |e: serde_json::Error| crate::framing::FrameError::Malformed {
            reason: e.to_string(),
        };
        let mut value = serde_json::to_value(self).map_err(malformed)?;
        let fields = serde_json::to_value(handles).map_err(malformed)?;
        if let (Some(object), Value::Object(fields)) = (value.as_object_mut(), fields) {
            object.extend(fields);
        }
        let line = serde_json::to_vec(&value).map_err(malformed)?;
        if line.len() > crate::MAX_LINE {
            return Err(crate::framing::FrameError::TooLong);
        }
        Ok(line)
    }
}

/// A line that isn't a [`Message`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    /// The id, when the line was a request with a readable id.
    pub request: Option<u64>,
    /// What was wrong, for the log.
    pub reason: String,
}
impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not a protocol message: {}", self.reason)
    }
}
impl std::error::Error for DecodeError {}

/// Why a request failed. The codes the protocol defines are variants; the
/// control layer's own codes (for example `no_document`, `not_editable`) pass through as
/// [`ErrorCode::Other`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The request was cancelled before it finished.
    Cancelled,
    /// The request outlived its deadline.
    Deadline,
    /// The request's parameters don't fit its method, or the line wasn't a
    /// message.
    InvalidRequest,
    /// The method isn't one this side knows.
    UnknownMethod,
    /// A proposal's target token no longer matches what it names.
    StaleTarget,
    /// The extension, or the contribution, is disabled.
    Disabled,
    /// The side is doing something that excludes this request now.
    Busy,
    /// The channel's queue is full: the other side isn't reading.
    ChannelFull,
    /// The reply, or a message, would exceed [`crate::MAX_LINE`].
    TooLarge,
    /// The other side speaks another [`crate::PROTOCOL`].
    UnsupportedProtocol,
    /// The program exited, or the channel closed, before the reply.
    Exited,
    /// A proposal arrived for an app request no longer in flight (answered,
    /// cancelled or past its deadline); nothing was applied.
    NotAdmitted,
    /// The contribution needs something installed first, such as a model to
    /// download. The message is a [`NeedsSetup`] as JSON, which
    /// [`NeedsSetup::read`] reads.
    NeedsSetup,
    /// What the contribution runs on isn't available here, such as a runtime
    /// library or a remote worker; the message says why.
    Unavailable,
    /// A code this crate doesn't define, kept as sent.
    Other(String),
}
impl ErrorCode {
    /// Every code [`ErrorCode::Other`] doesn't stand for, as sent.
    pub const KNOWN: [(&'static str, ErrorCode); 14] = [
        ("cancelled", ErrorCode::Cancelled),
        ("deadline", ErrorCode::Deadline),
        ("invalid_request", ErrorCode::InvalidRequest),
        ("unknown_method", ErrorCode::UnknownMethod),
        ("stale_target", ErrorCode::StaleTarget),
        ("disabled", ErrorCode::Disabled),
        ("busy", ErrorCode::Busy),
        ("channel_full", ErrorCode::ChannelFull),
        ("too_large", ErrorCode::TooLarge),
        ("unsupported_protocol", ErrorCode::UnsupportedProtocol),
        ("exited", ErrorCode::Exited),
        ("not_admitted", ErrorCode::NotAdmitted),
        ("needs_setup", ErrorCode::NeedsSetup),
        ("unavailable", ErrorCode::Unavailable),
    ];
    /// The code as sent.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Other(code) => code,
            known => Self::KNOWN
                .iter()
                .find(|(_, c)| c == known)
                .map(|(name, _)| *name)
                .unwrap_or_default(),
        }
    }
    /// The code a sent string stands for.
    pub fn parse(code: &str) -> Self {
        Self::KNOWN
            .iter()
            .find(|(name, _)| *name == code)
            .map_or_else(|| Self::Other(code.to_string()), |(_, c)| c.clone())
    }
}
impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(d)?))
    }
}

/// A failed request, as the other side answered it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteError {
    /// Why, for code to act on.
    pub code: ErrorCode,
    /// Why, for a person to read.
    pub message: String,
}
impl RemoteError {
    /// An error with this code and message.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.message.is_empty() {
            write!(f, "{}", self.code)
        } else {
            write!(f, "{} ({})", self.message, self.code)
        }
    }
}
impl std::error::Error for RemoteError {}

/// What a [`ErrorCode::NeedsSetup`] error asks for, sent as its message in
/// JSON (`{"what":"…","bytes":n}`), since the envelope has no data field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedsSetup {
    /// What setting up installs, for a person: "the Subject model".
    pub what: String,
    /// How much it downloads, in bytes.
    pub bytes: u64,
}
impl NeedsSetup {
    /// The error to answer with.
    pub fn error(&self) -> RemoteError {
        let message = serde_json::to_string(self).expect("a setup is JSON");
        RemoteError::new(ErrorCode::NeedsSetup, message)
    }
    /// Reads a `needs_setup` error's message. `None` when it isn't one this
    /// version reads: show the message as plain text then.
    pub fn read(message: &str) -> Option<Self> {
        serde_json::from_str(message).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_kind_of_message_round_trips_in_its_documented_shape() {
        let cases = [
            (
                Message::request(1, "command.run", json!({"command": "auto"})),
                r#"{"t":"req","id":1,"m":"command.run","p":{"command":"auto"}}"#,
            ),
            (
                Message::reply(1, Ok(json!({"x": 1}))),
                r#"{"t":"ok","id":1,"v":{"x":1}}"#,
            ),
            (
                Message::reply(2, Err(RemoteError::new(ErrorCode::Cancelled, "stopped"))),
                r#"{"t":"err","id":2,"code":"cancelled","message":"stopped"}"#,
            ),
            (
                Message::Progress {
                    id: 3,
                    done: Some(0.5),
                    text: None,
                },
                r#"{"t":"progress","id":3,"done":0.5}"#,
            ),
            (Message::Cancel { id: 4 }, r#"{"t":"cancel","id":4}"#),
            (
                Message::event("quitting", Value::Null),
                r#"{"t":"event","e":"quitting"}"#,
            ),
        ];
        for (message, line) in cases {
            assert_eq!(String::from_utf8(message.encode().unwrap()).unwrap(), line);
            assert_eq!(Message::decode(line.as_bytes()).unwrap(), message);
        }
    }

    #[test]
    fn a_code_this_crate_does_not_know_passes_through() {
        let line = br#"{"t":"err","id":5,"code":"no_document","message":"Open a photo"}"#;
        let Message::Err { code, .. } = Message::decode(line).unwrap() else {
            panic!("an error");
        };
        assert_eq!(code, ErrorCode::Other("no_document".into()));
        for (name, code) in ErrorCode::KNOWN {
            assert_eq!(ErrorCode::parse(name), code);
            assert_eq!(code.as_str(), name);
        }
    }

    #[test]
    fn a_setup_travels_as_the_message_and_falls_back_to_text() {
        let setup = NeedsSetup {
            what: "the Subject model".into(),
            bytes: 184_000_000,
        };
        let error = setup.error();
        assert_eq!(error.code, ErrorCode::NeedsSetup);
        assert_eq!(
            error.message,
            r#"{"what":"the Subject model","bytes":184000000}"#
        );
        let line = Message::reply(4, Err(error)).encode().unwrap();
        let Message::Err { code, message, .. } = Message::decode(&line).unwrap() else {
            panic!("an error");
        };
        assert_eq!(code, ErrorCode::NeedsSetup);
        assert_eq!(NeedsSetup::read(&message), Some(setup));
        assert_eq!(NeedsSetup::read("Download the Subject model first"), None);
        assert_eq!(NeedsSetup::read(r#"{"what":"x"}"#), None);
        assert_eq!(ErrorCode::parse("unavailable"), ErrorCode::Unavailable);
    }

    #[test]
    fn fields_a_message_does_not_know_are_ignored() {
        let line = br#"{"t":"ok","id":1,"v":null,"handles":0}"#;
        assert_eq!(
            Message::decode(line).unwrap(),
            Message::Ok {
                id: 1,
                v: Value::Null
            }
        );
    }

    #[test]
    fn handles_are_declared_beside_the_message() {
        let message = Message::request(3, "mask.generate", json!({"generator": "subject"}));
        let unix = Handles {
            handles: 2,
            hv: None,
        };
        let line = message.encode_with(&unix).unwrap();
        assert_eq!(
            String::from_utf8(line.clone()).unwrap(),
            r#"{"handles":2,"id":3,"m":"mask.generate","p":{"generator":"subject"},"t":"req"}"#
        );
        assert_eq!(Message::decode(&line).unwrap(), message);
        assert_eq!(Handles::read(&line).unwrap(), unix);
        let windows = Handles {
            handles: 2,
            hv: Some(vec![0x1f4, 0x1f8]),
        };
        let line = message.encode_with(&windows).unwrap();
        assert_eq!(Handles::read(&line).unwrap(), windows);
        // None declared: the line is the plain one.
        assert_eq!(
            message.encode_with(&Handles::default()).unwrap(),
            serde_json::to_vec(&serde_json::to_value(&message).unwrap()).unwrap()
        );
        assert_eq!(
            Handles::read(&message.encode().unwrap()).unwrap(),
            Handles::default()
        );
        assert!(Handles::read(br#"{"t":"req","id":1,"m":"x","handles":"two"}"#).is_err());
        assert!(Handles::read(br#"{"t":"req","id":1,"m":"x","handles":-1}"#).is_err());
        let big = Message::reply(1, Ok(json!("x".repeat(crate::MAX_LINE - 20))));
        assert_eq!(
            big.encode_with(&unix).unwrap_err(),
            crate::framing::FrameError::TooLong
        );
    }

    #[test]
    fn a_broken_request_names_its_id_so_it_can_be_answered() {
        let error = Message::decode(br#"{"t":"req","id":9,"p":{}}"#).unwrap_err();
        assert_eq!(error.request, Some(9));
        assert_eq!(
            Message::decode(b"{\"t\":\"ok\"}").unwrap_err().request,
            None
        );
        assert_eq!(Message::decode(b"not json").unwrap_err().request, None);
        assert_eq!(
            Message::decode(br#"{"t":"nope","id":1}"#)
                .unwrap_err()
                .request,
            None
        );
    }

    #[test]
    fn a_message_too_long_for_a_line_is_refused_when_encoded() {
        let big = "x".repeat(crate::MAX_LINE);
        assert_eq!(
            Message::reply(1, Ok(json!(big))).encode().unwrap_err(),
            crate::framing::FrameError::TooLong
        );
    }
}
