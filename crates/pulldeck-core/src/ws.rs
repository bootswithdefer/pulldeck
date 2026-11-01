//! The real connection to the server: a websocket carrying JSON text frames.
//!
//! Plugs into [`crate::remote::RemoteSession`] as a [`Connector`]. Commands are validated against
//! the server's size limits before anything is sent, frames of a type this client does not know
//! are skipped, and a connection that has been silent for too long counts as dead.
//!
//! Frames are never logged: a `hello` carries the session token.

use crate::remote::{Connector, Transport, TransportError};
use crate::session::{ClientCommand, ServerEvent};
use crate::settings::ServerUrl;
use crate::wire;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

/// How long opening the connection may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The server pings every 30 s; two missed pings mean the connection is dead.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Opens websocket connections to a server.
#[derive(Debug, Clone)]
pub struct WsConnector {
    url: String,
    idle_timeout: Duration,
}

impl WsConnector {
    /// Connects to `server`'s `/ws` endpoint (`wss://` unless it is a loopback `http` address).
    pub fn new(server: &ServerUrl) -> Self {
        Self {
            url: server.websocket(),
            idle_timeout: IDLE_TIMEOUT,
        }
    }

    /// How long the connection may be silent (no frame of any kind, pings included) before it
    /// is treated as dead. Mostly for tests.
    pub fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.idle_timeout = idle_timeout;
        self
    }
}

impl Connector for WsConnector {
    type Transport = WsTransport;

    async fn connect(&mut self) -> Result<WsTransport, TransportError> {
        let (stream, _response) = timeout(CONNECT_TIMEOUT, connect_async(self.url.as_str()))
            .await
            .map_err(|_| TransportError::Io("connecting timed out".into()))?
            .map_err(map_error)?;
        Ok(WsTransport {
            stream,
            idle_timeout: self.idle_timeout,
        })
    }
}

/// One open websocket connection.
pub struct WsTransport {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    idle_timeout: Duration,
}

fn map_error(error: WsError) -> TransportError {
    match error {
        WsError::ConnectionClosed | WsError::AlreadyClosed => TransportError::Closed,
        other => TransportError::Io(other.to_string()),
    }
}

impl Transport for WsTransport {
    async fn send(&mut self, command: ClientCommand) -> Result<(), TransportError> {
        let text = wire::encode(&command).map_err(TransportError::Invalid)?;
        self.stream
            .send(Message::text(text))
            .await
            .map_err(map_error)
    }

    async fn recv(&mut self) -> Result<ServerEvent, TransportError> {
        loop {
            // The timer restarts for every frame of any kind, so the server's pings keep a quiet
            // but healthy connection alive.
            let frame = timeout(self.idle_timeout, self.stream.next())
                .await
                .map_err(|_| {
                    TransportError::Io(format!(
                        "no data from the server for {} s",
                        self.idle_timeout.as_secs_f32().round()
                    ))
                })?;
            let message = match frame {
                None => return Err(TransportError::Closed),
                Some(Err(error)) => return Err(map_error(error)),
                Some(Ok(message)) => message,
            };
            match message {
                Message::Text(text) => {
                    let decoded = wire::decode(text.as_str()).map_err(|error| {
                        TransportError::Io(format!("malformed frame from the server: {error}"))
                    })?;
                    // Unknown message types are skipped so a newer server cannot break us.
                    if let Some(event) = decoded.into_event() {
                        return Ok(event);
                    }
                }
                // Reading a ping only *queues* the pong; flush so the server sees it now.
                Message::Ping(_) => self.stream.flush().await.map_err(map_error)?,
                Message::Close(_) => return Err(TransportError::Closed),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{Backoff, ConnState, RemoteSession, SessionUpdate};
    use crate::session::SessionToken;
    use crate::store::Change;
    use serde_json::{json, Value};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;
    use tokio_tungstenite::accept_async;

    const FIXTURES: &str = include_str!("../tests/fixtures/frames.json");

    /// The fixture frame with this name, as JSON text.
    fn frame(name: &str) -> String {
        let all: Vec<Value> = serde_json::from_str(FIXTURES).unwrap();
        let entry = all
            .into_iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("no fixture named {name}"));
        entry["frame"].to_string()
    }

    async fn listener() -> (TcpListener, ServerUrl) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        (listener, ServerUrl::parse(&url).unwrap())
    }

    async fn accept(listener: &TcpListener) -> WebSocketStream<TcpStream> {
        let (tcp, _) = listener.accept().await.unwrap();
        accept_async(tcp).await.unwrap()
    }

    /// The next text frame the client sent, parsed.
    async fn next_json(ws: &mut WebSocketStream<TcpStream>) -> Value {
        match ws.next().await.unwrap().unwrap() {
            Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
            other => panic!("expected a text frame, got {other:?}"),
        }
    }

    /// The next update from a running session, or a failed test if it takes too long.
    async fn next(updates: &mut mpsc::Receiver<SessionUpdate>) -> SessionUpdate {
        timeout(Duration::from_secs(5), updates.recv())
            .await
            .expect("timed out waiting for the session")
            .expect("session ended")
    }

    async fn connect(url: &ServerUrl) -> WsTransport {
        WsConnector::new(url).connect().await.unwrap()
    }

    #[tokio::test]
    async fn frames_flow_both_ways_and_unknown_types_are_skipped() {
        let (listener, url) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener).await;
            let hello = next_json(&mut ws).await;
            assert_eq!(hello["type"], "hello");
            assert_eq!(hello["token"], "tok");
            assert_eq!(hello["last_seq"], 7);
            ws.send(Message::text(frame("server hello"))).await.unwrap();
            ws.send(Message::text(r#"{"type":"from_the_future","x":1}"#))
                .await
                .unwrap();
            ws.send(Message::text(frame("server snapshot")))
                .await
                .unwrap();
        });

        let mut transport = connect(&url).await;
        transport
            .send(wire::hello(&SessionToken::new("tok"), Some(7)))
            .await
            .unwrap();
        assert!(
            matches!(transport.recv().await.unwrap(), ServerEvent::Hello { login } if login == "octocat")
        );
        // The unknown frame in between is skipped, not an error.
        assert!(
            matches!(transport.recv().await.unwrap(), ServerEvent::Snapshot { seq: 3, prs } if prs.len() == 1)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn pings_get_a_prompt_pong_even_when_nothing_else_is_sent() {
        let (listener, url) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener).await;
            ws.send(Message::Ping(vec![1, 2, 3].into())).await.unwrap();
            // The pong must arrive on its own, without the client writing anything else.
            let reply = timeout(Duration::from_secs(2), ws.next())
                .await
                .expect("pong was never flushed")
                .unwrap()
                .unwrap();
            assert!(matches!(reply, Message::Pong(_)), "{reply:?}");
            ws.send(Message::text(frame("server hello"))).await.unwrap();
        });

        let mut transport = connect(&url).await;
        assert!(matches!(
            transport.recv().await.unwrap(),
            ServerEvent::Hello { .. }
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_invalid_command_is_refused_before_anything_is_sent() {
        let (listener, url) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener).await;
            // Only the valid command may arrive.
            let first = next_json(&mut ws).await;
            assert_eq!(first, json!({"type": "ack_notification", "id": 9}));
        });

        let mut transport = connect(&url).await;
        let empty_body = ClientCommand::Message {
            request_id: "r".into(),
            pr: crate::session::PrRef {
                repo: "o/r".into(),
                number: 1,
            },
            to: "hubot".into(),
            body: String::new(),
        };
        assert!(matches!(
            transport.send(empty_body).await,
            Err(TransportError::Invalid(reason)) if reason.contains("body")
        ));
        transport
            .send(ClientCommand::AckNotification { id: 9 })
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_silent_connection_times_out() {
        let (listener, url) = listener().await;
        let server = tokio::spawn(async move {
            let _ws = accept(&listener).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let mut transport = WsConnector::new(&url)
            .with_idle_timeout(Duration::from_millis(100))
            .connect()
            .await
            .unwrap();
        assert!(matches!(
            transport.recv().await,
            Err(TransportError::Io(message)) if message.contains("no data")
        ));
        server.abort();
    }

    #[tokio::test]
    async fn a_close_frame_ends_the_connection() {
        let (listener, url) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener).await;
            ws.close(None).await.unwrap();
        });
        let mut transport = connect(&url).await;
        assert!(matches!(
            transport.recv().await,
            Err(TransportError::Closed)
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_malformed_frame_ends_the_connection_with_an_error() {
        let (listener, url) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener).await;
            ws.send(Message::text("this is not json")).await.unwrap();
        });
        let mut transport = connect(&url).await;
        assert!(matches!(
            transport.recv().await,
            Err(TransportError::Io(message)) if message.contains("malformed")
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connecting_to_nothing_fails_cleanly() {
        let (listener, url) = listener().await;
        drop(listener); // nothing is listening on that port any more
        assert!(matches!(
            WsConnector::new(&url).connect().await,
            Err(TransportError::Io(_))
        ));
    }

    /// The whole stack over real sockets: sign-in, snapshot, a dropped connection, then a
    /// reconnect that resumes from the last sequence number and reports only what changed.
    #[tokio::test]
    async fn a_full_session_survives_a_dropped_connection_and_resumes() {
        let (listener, url) = listener().await;
        let server = tokio::spawn(async move {
            // First connection: fresh hello, snapshot, one delta, then the socket drops.
            let mut ws = accept(&listener).await;
            let hello = next_json(&mut ws).await;
            assert_eq!(hello["last_seq"], Value::Null);
            ws.send(Message::text(frame("server hello"))).await.unwrap();
            ws.send(Message::text(frame("server snapshot")))
                .await
                .unwrap(); // seq 3
            ws.send(Message::text(frame("server pr_upsert")))
                .await
                .unwrap(); // seq 4
            drop(ws);

            // Second connection: the client must resume after seq 4.
            let mut ws = accept(&listener).await;
            let hello = next_json(&mut ws).await;
            assert_eq!(
                hello["last_seq"], 4,
                "must resume from the last applied event"
            );
            ws.send(Message::text(frame("server hello"))).await.unwrap();
            ws.send(Message::text(frame("server pr_remove")))
                .await
                .unwrap(); // seq 5
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let session =
            RemoteSession::new(WsConnector::new(&url), SessionToken::new("tok")).with_backoff(
                Backoff::new(Duration::from_millis(10), Duration::from_millis(10)),
            );
        let (_commands, command_rx) = mpsc::channel(8);
        let (update_tx, mut updates) = mpsc::channel(64);
        tokio::spawn(session.run(command_rx, update_tx));

        assert_eq!(
            next(&mut updates).await,
            SessionUpdate::State(ConnState::Connecting)
        );
        assert!(
            matches!(next(&mut updates).await, SessionUpdate::Login(login) if login == "octocat")
        );
        assert_eq!(
            next(&mut updates).await,
            SessionUpdate::State(ConnState::Live)
        );
        assert!(
            matches!(next(&mut updates).await, SessionUpdate::Changes(c) if matches!(c.as_slice(), [Change::Insert(0, _)]))
        );
        assert!(
            matches!(next(&mut updates).await, SessionUpdate::Changes(_)),
            "the upsert at seq 4"
        );
        assert_eq!(
            next(&mut updates).await,
            SessionUpdate::State(ConnState::Disconnected)
        );
        assert_eq!(
            next(&mut updates).await,
            SessionUpdate::State(ConnState::Connecting)
        );
        assert!(matches!(next(&mut updates).await, SessionUpdate::Login(_)));
        assert_eq!(
            next(&mut updates).await,
            SessionUpdate::State(ConnState::Live)
        );
        assert!(
            matches!(next(&mut updates).await, SessionUpdate::Changes(c) if matches!(c.as_slice(), [Change::Remove(0)])),
            "the removal at seq 5 after the resume"
        );
        server.abort();
    }
}
