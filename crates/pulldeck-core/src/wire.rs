//! The client's copy of the server's wire format.
//!
//! Messages are JSON text frames over a websocket (see `docs/PROTOCOL.md` in the server repo).
//! The server defines the protocol; this module mirrors it so the client does not depend on the
//! server's crate (that repo is private and this crate is published). Drift is caught by
//! `tests/fixtures/frames.json`, which both sides test against, and by the boundary tests below.
//!
//! Frames are never logged: a `hello` carries the session token.

use crate::models::{ChecksStatus, PullRequest, ReviewStatus};
use crate::session::{
    ClientCommand, Notification, NotificationKind, PrRef, ServerEvent, SessionToken,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

// Size limits, mirroring the server's `ClientMsg::validate()`. The server enforces them too and
// answers an over-long action with a rejection; checking here gives the user the reason at once.
const MAX_REQUEST_ID: usize = 64;
const MAX_LOGIN: usize = 39;
const MAX_REPO: usize = 140;
const MAX_NOTE: usize = 500;
const MAX_BODY: usize = 2000;

/// The session token as it goes on the wire: a plain JSON string whose `Debug` is redacted.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireToken(String);

impl fmt::Debug for WireToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WireToken(<redacted>)")
    }
}

/// Identifies a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrKey {
    pub repo: String,
    pub number: u64,
}

impl From<&PrRef> for PrKey {
    fn from(pr: &PrRef) -> Self {
        Self {
            repo: pr.repo.clone(),
            number: pr.number,
        }
    }
}

impl From<PrKey> for PrRef {
    fn from(key: PrKey) -> Self {
        Self {
            repo: key.repo,
            number: key.number,
        }
    }
}

/// Review state. `Other` is a state a newer server added; it shows as "none".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireReview {
    Approved,
    ChangesRequested,
    ReviewRequired,
    None,
    #[serde(other)]
    Other,
}

/// CI state. `Other` is a state a newer server added; it shows as "none".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireChecks {
    Success,
    Failure,
    Pending,
    None,
    #[serde(other)]
    Other,
}

/// A pull request as the server describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WirePr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub repo: String,
    pub author: String,
    pub is_draft: bool,
    pub review: WireReview,
    pub checks: WireChecks,
    pub updated_at: DateTime<Utc>,
}

impl From<WirePr> for PullRequest {
    fn from(pr: WirePr) -> Self {
        Self {
            number: pr.number,
            title: pr.title,
            url: pr.url,
            repo: pr.repo,
            author: pr.author,
            is_draft: pr.is_draft,
            review: match pr.review {
                WireReview::Approved => ReviewStatus::Approved,
                WireReview::ChangesRequested => ReviewStatus::ChangesRequested,
                WireReview::ReviewRequired => ReviewStatus::ReviewRequired,
                WireReview::None | WireReview::Other => ReviewStatus::None,
            },
            checks: match pr.checks {
                WireChecks::Success => ChecksStatus::Success,
                WireChecks::Failure => ChecksStatus::Failure,
                WireChecks::Pending => ChecksStatus::Pending,
                WireChecks::None | WireChecks::Other => ChecksStatus::None,
            },
            updated_at: pr.updated_at,
        }
    }
}

/// Why a notification was sent. `Other` is a kind a newer server added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireKind {
    ReviewRequested,
    Nudge,
    Message,
    #[serde(other)]
    Other,
}

/// What the client sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello {
        token: WireToken,
        #[serde(default)]
        last_seq: Option<u64>,
    },
    RequestReview {
        request_id: String,
        pr: PrKey,
        to: String,
    },
    Nudge {
        request_id: String,
        pr: PrKey,
        to: String,
        #[serde(default)]
        note: Option<String>,
    },
    Message {
        request_id: String,
        pr: PrKey,
        to: String,
        body: String,
    },
    AckNotification {
        id: u64,
    },
    Presence {
        #[serde(default)]
        viewing: Option<PrKey>,
    },
    #[serde(other)]
    Unknown,
}

/// What the server sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Hello {
        version: u32,
        login: String,
    },
    Snapshot {
        seq: u64,
        prs: Vec<WirePr>,
    },
    PrUpsert {
        seq: u64,
        pr: WirePr,
    },
    PrRemove {
        seq: u64,
        key: PrKey,
    },
    Error {
        message: String,
    },
    Notification {
        seq: u64,
        id: u64,
        from: String,
        kind: WireKind,
        pr: PrKey,
        #[serde(default)]
        note: Option<String>,
        created_at: DateTime<Utc>,
    },
    Accepted {
        request_id: String,
    },
    Rejected {
        request_id: String,
        reason: String,
    },
    Presence {
        login: String,
        online: bool,
        #[serde(default)]
        viewing: Option<PrKey>,
    },
    #[serde(other)]
    Unknown,
}

impl ServerMsg {
    /// The domain event for this message, or `None` for a message type this client does not
    /// know (which is to be ignored, so a newer server does not break an older client).
    pub fn into_event(self) -> Option<ServerEvent> {
        Some(match self {
            Self::Hello { login, .. } => ServerEvent::Hello { login },
            Self::Snapshot { seq, prs } => ServerEvent::Snapshot {
                seq,
                prs: prs.into_iter().map(PullRequest::from).collect(),
            },
            Self::PrUpsert { seq, pr } => ServerEvent::PrUpsert { seq, pr: pr.into() },
            Self::PrRemove { seq, key } => ServerEvent::PrRemove {
                seq,
                pr: key.into(),
            },
            Self::Error { message } => ServerEvent::Error { message },
            Self::Notification {
                seq,
                id,
                from,
                kind,
                pr,
                note,
                created_at,
            } => ServerEvent::Notification(Notification {
                seq,
                id,
                from,
                kind: match kind {
                    WireKind::ReviewRequested => NotificationKind::ReviewRequested,
                    WireKind::Nudge => NotificationKind::Nudge,
                    WireKind::Message => NotificationKind::Message,
                    WireKind::Other => NotificationKind::Other,
                },
                pr: pr.into(),
                note,
                created_at,
            }),
            Self::Accepted { request_id } => ServerEvent::Accepted { request_id },
            Self::Rejected { request_id, reason } => ServerEvent::Rejected { request_id, reason },
            Self::Presence {
                login,
                online,
                viewing,
            } => ServerEvent::Presence {
                login,
                online,
                viewing: viewing.map(PrRef::from),
            },
            Self::Unknown => return None,
        })
    }
}

impl From<&ClientCommand> for ClientMsg {
    fn from(command: &ClientCommand) -> Self {
        match command {
            ClientCommand::Hello { token, last_seq } => Self::Hello {
                token: WireToken(token.expose().to_owned()),
                last_seq: *last_seq,
            },
            ClientCommand::RequestReview { request_id, pr, to } => Self::RequestReview {
                request_id: request_id.clone(),
                pr: pr.into(),
                to: to.clone(),
            },
            ClientCommand::Nudge {
                request_id,
                pr,
                to,
                note,
            } => Self::Nudge {
                request_id: request_id.clone(),
                pr: pr.into(),
                to: to.clone(),
                note: note.clone(),
            },
            ClientCommand::Message {
                request_id,
                pr,
                to,
                body,
            } => Self::Message {
                request_id: request_id.clone(),
                pr: pr.into(),
                to: to.clone(),
                body: body.clone(),
            },
            ClientCommand::AckNotification { id } => Self::AckNotification { id: *id },
            ClientCommand::Presence { viewing } => Self::Presence {
                viewing: viewing.as_ref().map(PrKey::from),
            },
        }
    }
}

impl ClientMsg {
    /// Checks the sizes the server enforces, so the user hears the reason immediately.
    pub fn validate(&self) -> Result<(), String> {
        fn bytes(what: &str, value: &str, max: usize) -> Result<(), String> {
            if value.is_empty() || value.len() > max {
                return Err(format!("{what} must be 1..={max} bytes"));
            }
            Ok(())
        }
        fn chars(what: &str, value: &str, max: usize) -> Result<(), String> {
            if value.chars().count() > max {
                return Err(format!("{what} must be at most {max} characters"));
            }
            Ok(())
        }
        let target = |request_id: &str, pr: &PrKey, to: &str| -> Result<(), String> {
            bytes("request_id", request_id, MAX_REQUEST_ID)?;
            bytes("repo", &pr.repo, MAX_REPO)?;
            bytes("to", to, MAX_LOGIN)
        };
        match self {
            Self::RequestReview { request_id, pr, to } => target(request_id, pr, to),
            Self::Nudge {
                request_id,
                pr,
                to,
                note,
            } => {
                target(request_id, pr, to)?;
                chars("note", note.as_deref().unwrap_or_default(), MAX_NOTE)
            }
            Self::Message {
                request_id,
                pr,
                to,
                body,
            } => {
                target(request_id, pr, to)?;
                if body.is_empty() {
                    return Err("body must not be empty".into());
                }
                chars("body", body, MAX_BODY)
            }
            Self::Hello { .. }
            | Self::AckNotification { .. }
            | Self::Presence { .. }
            | Self::Unknown => Ok(()),
        }
    }
}

/// Turns a command into the JSON text frame to send, after checking its sizes.
pub fn encode(command: &ClientCommand) -> Result<String, String> {
    let message = ClientMsg::from(command);
    message.validate()?;
    serde_json::to_string(&message).map_err(|e| e.to_string())
}

/// Parses a text frame from the server.
pub fn decode(text: &str) -> Result<ServerMsg, serde_json::Error> {
    serde_json::from_str(text)
}

/// Builds the `hello` command for a token, for callers that only have the raw token.
pub fn hello(token: &SessionToken, last_seq: Option<u64>) -> ClientCommand {
    ClientCommand::Hello {
        token: token.clone(),
        last_seq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const FIXTURES: &str = include_str!("../tests/fixtures/frames.json");

    fn fixtures() -> Vec<Value> {
        serde_json::from_str(FIXTURES).expect("fixtures are valid JSON")
    }

    #[test]
    fn every_fixture_frame_parses_and_round_trips_unless_lossy() {
        let frames = fixtures();
        assert!(frames.len() >= 17, "fixture file looks truncated");
        for entry in &frames {
            let name = entry["name"].as_str().unwrap();
            let frame = &entry["frame"];
            let lossy = entry["lossy"].as_bool().unwrap_or(false);
            let reencoded = match entry["direction"].as_str().unwrap() {
                "client" => serde_json::to_value(
                    serde_json::from_value::<ClientMsg>(frame.clone())
                        .unwrap_or_else(|e| panic!("{name}: {e}")),
                ),
                "server" => serde_json::to_value(
                    serde_json::from_value::<ServerMsg>(frame.clone())
                        .unwrap_or_else(|e| panic!("{name}: {e}")),
                ),
                other => panic!("{name}: unknown direction {other}"),
            }
            .unwrap();
            if !lossy {
                assert_eq!(&reencoded, frame, "{name} does not round-trip");
            }
        }
    }

    #[test]
    fn every_fixture_variant_is_understood_not_unknown() {
        for entry in fixtures() {
            let name = entry["name"].as_str().unwrap();
            match entry["direction"].as_str().unwrap() {
                "client" => {
                    let msg: ClientMsg = serde_json::from_value(entry["frame"].clone()).unwrap();
                    assert_ne!(msg, ClientMsg::Unknown, "{name}");
                }
                _ => {
                    let msg: ServerMsg = serde_json::from_value(entry["frame"].clone()).unwrap();
                    assert!(msg.into_event().is_some(), "{name} decoded as unknown");
                }
            }
        }
    }

    #[test]
    fn a_notification_from_a_newer_server_still_yields_its_id_and_seq() {
        let entry = fixtures()
            .into_iter()
            .find(|e| {
                e["name"]
                    .as_str()
                    .unwrap()
                    .contains("kind from a newer server")
            })
            .expect("fixture present");
        let msg: ServerMsg = serde_json::from_value(entry["frame"].clone()).unwrap();
        let Some(ServerEvent::Notification(n)) = msg.into_event() else {
            panic!("expected a notification");
        };
        assert_eq!(n.kind, NotificationKind::Other);
        // The id is what lets the client acknowledge it, so it must survive an unknown kind.
        assert!(n.id > 0 && n.seq > 0);
    }

    #[test]
    fn unknown_states_from_a_newer_server_show_as_none() {
        let entry = fixtures()
            .into_iter()
            .find(|e| {
                e["name"]
                    .as_str()
                    .unwrap()
                    .contains("review and checks states")
            })
            .expect("fixture present");
        let msg: ServerMsg = serde_json::from_value(entry["frame"].clone()).unwrap();
        let Some(ServerEvent::PrUpsert { pr, .. }) = msg.into_event() else {
            panic!("expected an upsert");
        };
        assert_eq!(pr.review, ReviewStatus::None);
        assert_eq!(pr.checks, ChecksStatus::None);
    }

    #[test]
    fn unknown_message_types_are_ignorable_but_garbage_is_an_error() {
        let unknown = decode(r#"{"type":"from_the_future","x":1}"#).unwrap();
        assert_eq!(unknown, ServerMsg::Unknown);
        assert!(unknown.into_event().is_none());
        assert!(decode("not json").is_err());
        assert!(decode(r#"{"no_type":true}"#).is_err());
    }

    #[test]
    fn hello_goes_out_with_the_token_but_never_prints_it() {
        let command = hello(&SessionToken::new("s3cret-token"), Some(12));
        let json = encode(&command).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&json).unwrap(),
            serde_json::json!({"type": "hello", "token": "s3cret-token", "last_seq": 12})
        );
        let message = ClientMsg::from(&command);
        assert!(!format!("{message:?} {message:#?}").contains("s3cret"));
    }

    fn review(request_id: &str, repo: &str, to: &str) -> ClientCommand {
        ClientCommand::RequestReview {
            request_id: request_id.into(),
            pr: PrRef {
                repo: repo.into(),
                number: 7,
            },
            to: to.into(),
        }
    }

    #[test]
    fn field_size_limits_hold_at_the_boundary() {
        let ok = |c: &ClientCommand| encode(c).is_ok();
        // request_id 1..=64 bytes
        assert!(ok(&review(&"a".repeat(64), "o/r", "hubot")));
        assert!(!ok(&review(&"a".repeat(65), "o/r", "hubot")));
        assert!(!ok(&review("", "o/r", "hubot")));
        // repo 1..=140 bytes
        assert!(ok(&review("r", &"a".repeat(140), "hubot")));
        assert!(!ok(&review("r", &"a".repeat(141), "hubot")));
        assert!(!ok(&review("r", "", "hubot")));
        // login 1..=39 bytes
        assert!(ok(&review("r", "o/r", &"a".repeat(39))));
        assert!(!ok(&review("r", "o/r", &"a".repeat(40))));
        assert!(!ok(&review("r", "o/r", "")));
    }

    #[test]
    fn notes_and_bodies_count_characters_not_bytes() {
        let nudge = |note: &str| ClientCommand::Nudge {
            request_id: "r".into(),
            pr: PrRef {
                repo: "o/r".into(),
                number: 1,
            },
            to: "x".into(),
            note: Some(note.into()),
        };
        let message = |body: &str| ClientCommand::Message {
            request_id: "r".into(),
            pr: PrRef {
                repo: "o/r".into(),
                number: 1,
            },
            to: "x".into(),
            body: body.into(),
        };
        // 500 two-byte characters is 1000 bytes but still within the limit.
        assert!(encode(&nudge(&"é".repeat(500))).is_ok());
        assert!(encode(&nudge(&"é".repeat(501))).is_err());
        assert!(encode(&message(&"é".repeat(2000))).is_ok());
        assert!(encode(&message(&"é".repeat(2001))).is_err());
        assert!(encode(&message("")).is_err(), "an empty body is refused");
        // A nudge without a note is fine.
        let bare = ClientCommand::Nudge {
            request_id: "r".into(),
            pr: PrRef {
                repo: "o/r".into(),
                number: 1,
            },
            to: "x".into(),
            note: None,
        };
        assert!(encode(&bare).is_ok());
    }

    #[test]
    fn rejection_reasons_say_which_field_is_wrong() {
        let err = encode(&review("r", "o/r", &"a".repeat(40))).unwrap_err();
        assert!(err.contains("to") && err.contains("39"), "{err}");
    }

    #[test]
    fn acks_and_presence_have_nothing_to_validate() {
        assert!(encode(&ClientCommand::AckNotification { id: 5 }).is_ok());
        assert!(encode(&ClientCommand::Presence { viewing: None }).is_ok());
    }
}
