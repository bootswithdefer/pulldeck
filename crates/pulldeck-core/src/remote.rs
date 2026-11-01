//! Keeps a connection to the server alive and feeds what it says into [`Synced`].
//!
//! The wire is behind the [`Transport`] / [`Connector`] traits, so everything here (handshake,
//! resume, reconnect with backoff, rejecting actions while offline) is tested with an in-memory
//! fake and a paused clock: no sockets, no Qt. A real websocket transport only has to convert
//! between its protocol messages and [`ServerEvent`] / [`ClientCommand`].

use crate::session::{
    ClientCommand, Notification, PrRef, ServerEvent, SessionToken, SyncStatus, Synced,
};
use crate::store::Change;
use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};

/// Where the connection stands, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    /// Not connected; a retry is scheduled.
    Disconnected,
    /// Connecting or signing in.
    Connecting,
    /// Signed in and receiving updates.
    Live,
    /// Gave up: retrying cannot help (for example the session was revoked). The text says why.
    Failed(String),
}

/// Why a transport operation failed.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The peer closed the connection.
    #[error("connection closed")]
    Closed,
    /// A network or protocol failure.
    #[error("network error: {0}")]
    Io(String),
    /// Only from `send`: this one command is invalid (for example over a size limit). The
    /// connection is fine.
    #[error("invalid command: {0}")]
    Invalid(String),
}

/// One live connection.
pub trait Transport: Send {
    /// Sends a command. Validate it first and return [`TransportError::Invalid`] if it is bad.
    fn send(
        &mut self,
        command: ClientCommand,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// Waits for the next server event. Returns an error when the connection is lost or has been
    /// silent too long.
    fn recv(&mut self) -> impl Future<Output = Result<ServerEvent, TransportError>> + Send;
}

/// Opens connections.
pub trait Connector: Send {
    /// The kind of connection this opens.
    type Transport: Transport;

    /// Opens a new connection.
    fn connect(&mut self) -> impl Future<Output = Result<Self::Transport, TransportError>> + Send;
}

/// What the session reports to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionUpdate {
    /// The connection state changed.
    State(ConnState),
    /// The server accepted the session for this GitHub login.
    Login(String),
    /// Row changes for the PR list, in the order to apply them.
    Changes(Vec<Change>),
    /// A notification that was not seen before.
    Notification(Notification),
    /// The outcome of an action the user triggered.
    ActionResult {
        /// The id the action was sent with.
        request_id: String,
        /// `Ok` if accepted, otherwise the reason it was refused.
        result: Result<(), String>,
    },
    /// Whether the server can read the user's pull requests. The server only reports a problem
    /// (and later its end), never "all fine" up front, so whoever shows this should treat each
    /// `State(Connecting)` as a reset to `Ok`.
    Sync {
        /// The state.
        status: SyncStatus,
        /// Where to install the GitHub App, if that is the problem.
        install_url: Option<String>,
    },
    /// Another user's presence changed.
    Presence {
        /// Their login.
        login: String,
        /// Whether they are connected.
        online: bool,
        /// The PR they are looking at, if any.
        viewing: Option<PrRef>,
    },
}

/// Exponential delay between reconnect attempts.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    jitter: bool,
    attempt: u32,
}

impl Backoff {
    /// Delays of `base`, `2*base`, `4*base`, ... capped at `max`, with no jitter.
    pub const fn new(base: Duration, max: Duration) -> Self {
        Self {
            base,
            max,
            jitter: false,
            attempt: 0,
        }
    }

    /// Shaves up to 20% off each delay so many clients don't reconnect in lockstep.
    pub const fn with_jitter(mut self) -> Self {
        self.jitter = true;
        self
    }

    /// The delay before the next attempt; each call doubles it until the cap.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self
            .base
            .saturating_mul(1u32 << self.attempt.min(16))
            .min(self.max);
        self.attempt = self.attempt.saturating_add(1);
        if self.jitter {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            delay - delay / 5 * (nanos % 1000) / 1000
        } else {
            delay
        }
    }

    /// Forget past failures, after a connection succeeded.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

impl Default for Backoff {
    /// 1 s doubling up to 60 s, with jitter.
    fn default() -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(60)).with_jitter()
    }
}

enum Outcome {
    /// The UI went away or the command channel closed; stop.
    Shutdown,
    /// Retrying cannot help.
    Fatal(String),
    /// The connection is gone; wait and try again.
    Retry(String),
}

/// A session with the server: connect, sign in, resume, apply events, reconnect.
pub struct RemoteSession<C: Connector> {
    connector: C,
    token: SessionToken,
    backoff: Backoff,
    handshake_timeout: Duration,
    synced: Synced,
}

impl<C: Connector> RemoteSession<C> {
    /// A session that signs in with `token`.
    pub fn new(connector: C, token: SessionToken) -> Self {
        Self {
            connector,
            token,
            backoff: Backoff::default(),
            handshake_timeout: Duration::from_secs(10),
            synced: Synced::default(),
        }
    }

    /// Replaces the reconnect backoff.
    pub fn with_backoff(mut self, backoff: Backoff) -> Self {
        self.backoff = backoff;
        self
    }

    /// Runs until `commands` is closed, `updates` is dropped, or a fatal error occurs.
    ///
    /// Actions sent while not connected are refused immediately (the UI should disable them, but
    /// there is always a race), so a stale action is never delivered after a reconnect.
    pub async fn run(
        mut self,
        mut commands: mpsc::Receiver<ClientCommand>,
        updates: mpsc::Sender<SessionUpdate>,
    ) {
        loop {
            if !emit(&updates, SessionUpdate::State(ConnState::Connecting)).await {
                return;
            }
            match self.serve(&mut commands, &updates).await {
                Outcome::Shutdown => return,
                Outcome::Fatal(reason) => {
                    tracing::warn!(%reason, "session ended for good");
                    emit(&updates, SessionUpdate::State(ConnState::Failed(reason))).await;
                    return;
                }
                Outcome::Retry(reason) => {
                    tracing::debug!(%reason, "disconnected");
                    if !emit(&updates, SessionUpdate::State(ConnState::Disconnected)).await {
                        return;
                    }
                    let delay = self.backoff.next_delay();
                    if !pause(delay, &mut commands, &updates).await {
                        return;
                    }
                }
            }
        }
    }

    async fn serve(
        &mut self,
        commands: &mut mpsc::Receiver<ClientCommand>,
        updates: &mpsc::Sender<SessionUpdate>,
    ) -> Outcome {
        let mut transport = match self.connector.connect().await {
            Ok(transport) => transport,
            Err(e) => return Outcome::Retry(e.to_string()),
        };

        let hello = ClientCommand::Hello {
            token: self.token.clone(),
            last_seq: self.synced.last_seq(),
        };
        if let Err(e) = transport.send(hello).await {
            return Outcome::Retry(e.to_string());
        }
        match timeout(self.handshake_timeout, transport.recv()).await {
            Err(_) => return Outcome::Retry("sign-in timed out".into()),
            Ok(Err(e)) => return Outcome::Retry(e.to_string()),
            Ok(Ok(ServerEvent::Hello { login })) => {
                if !emit(updates, SessionUpdate::Login(login)).await {
                    return Outcome::Shutdown;
                }
            }
            // The server refused the session (revoked or unknown token): retrying with the same
            // token can never work, so stop and let the UI ask the user to sign in again.
            Ok(Ok(ServerEvent::Error { message })) => return Outcome::Fatal(message),
            Ok(Ok(_)) => return Outcome::Retry("unexpected first message".into()),
        }

        self.backoff.reset();
        // Anything queued while we were offline is stale by now.
        while let Ok(stale) = commands.try_recv() {
            if !reject(&stale, updates).await {
                return Outcome::Shutdown;
            }
        }
        if !emit(updates, SessionUpdate::State(ConnState::Live)).await {
            return Outcome::Shutdown;
        }

        loop {
            tokio::select! {
                event = transport.recv() => match event {
                    Err(e) => return Outcome::Retry(e.to_string()),
                    Ok(event) => {
                        if let Some(outcome) = self.handle(event, updates).await {
                            return outcome;
                        }
                    }
                },
                command = commands.recv() => match command {
                    None => return Outcome::Shutdown,
                    Some(command) => {
                        let request_id = command.request_id().map(str::to_owned);
                        match transport.send(command).await {
                            Ok(()) => {}
                            Err(TransportError::Invalid(reason)) => {
                                if let Some(request_id) = request_id {
                                    let update = SessionUpdate::ActionResult {
                                        request_id,
                                        result: Err(reason),
                                    };
                                    if !emit(updates, update).await {
                                        return Outcome::Shutdown;
                                    }
                                }
                            }
                            Err(e) => return Outcome::Retry(e.to_string()),
                        }
                    }
                },
            }
        }
    }

    /// Applies one event. Returns an outcome only if the connection should end.
    async fn handle(
        &mut self,
        event: ServerEvent,
        updates: &mpsc::Sender<SessionUpdate>,
    ) -> Option<Outcome> {
        let forwarded = match &event {
            ServerEvent::Error { message } => return Some(Outcome::Retry(message.clone())),
            ServerEvent::Accepted { request_id } => Some(SessionUpdate::ActionResult {
                request_id: request_id.clone(),
                result: Ok(()),
            }),
            ServerEvent::Rejected { request_id, reason } => Some(SessionUpdate::ActionResult {
                request_id: request_id.clone(),
                result: Err(reason.clone()),
            }),
            ServerEvent::Status { sync, install_url } => Some(SessionUpdate::Sync {
                status: *sync,
                install_url: install_url.clone(),
            }),
            ServerEvent::Presence {
                login,
                online,
                viewing,
            } => Some(SessionUpdate::Presence {
                login: login.clone(),
                online: *online,
                viewing: viewing.clone(),
            }),
            _ => None,
        };
        if let Some(update) = forwarded {
            return (!emit(updates, update).await).then_some(Outcome::Shutdown);
        }

        let applied = self.synced.apply(event);
        if !applied.changes.is_empty()
            && !emit(updates, SessionUpdate::Changes(applied.changes)).await
        {
            return Some(Outcome::Shutdown);
        }
        if let Some(notification) = applied.notification {
            if !emit(updates, SessionUpdate::Notification(notification)).await {
                return Some(Outcome::Shutdown);
            }
        }
        None
    }
}

/// Sends an update; `false` means the UI is gone and the session should stop.
async fn emit(updates: &mpsc::Sender<SessionUpdate>, update: SessionUpdate) -> bool {
    updates.send(update).await.is_ok()
}

/// Refuses an action that cannot be sent right now. Commands without an answer (acks, presence)
/// are simply dropped.
async fn reject(command: &ClientCommand, updates: &mpsc::Sender<SessionUpdate>) -> bool {
    match command.request_id() {
        Some(request_id) => {
            let update = SessionUpdate::ActionResult {
                request_id: request_id.to_owned(),
                result: Err("not connected".into()),
            };
            emit(updates, update).await
        }
        None => true,
    }
}

/// Waits out the backoff, refusing any actions that arrive meanwhile. `false` means stop.
async fn pause(
    delay: Duration,
    commands: &mut mpsc::Receiver<ClientCommand>,
    updates: &mpsc::Sender<SessionUpdate>,
) -> bool {
    let sleeper = sleep(delay);
    tokio::pin!(sleeper);
    loop {
        tokio::select! {
            () = &mut sleeper => return true,
            command = commands.recv() => match command {
                None => return false,
                Some(command) => {
                    if !reject(&command, updates).await {
                        return false;
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{notification, pr, pr_ref};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    type Incoming = Result<ServerEvent, TransportError>;

    struct FakeTransport {
        incoming: mpsc::UnboundedReceiver<Incoming>,
        sent: mpsc::UnboundedSender<ClientCommand>,
    }

    /// The test's end of a [`FakeTransport`].
    struct Server {
        events: mpsc::UnboundedSender<Incoming>,
        sent: mpsc::UnboundedReceiver<ClientCommand>,
    }

    impl Server {
        fn say(&self, event: ServerEvent) {
            self.events.send(Ok(event)).unwrap();
        }
        fn drop_connection(&self) {
            self.events.send(Err(TransportError::Closed)).unwrap();
        }
        async fn hears(&mut self) -> ClientCommand {
            self.sent.recv().await.expect("client sent something")
        }
    }

    impl Transport for FakeTransport {
        async fn send(&mut self, command: ClientCommand) -> Result<(), TransportError> {
            // Stands in for the real transport's validation of sizes and so on.
            if matches!(&command, ClientCommand::Message { body, .. } if body.is_empty()) {
                return Err(TransportError::Invalid("body is empty".into()));
            }
            self.sent.send(command).map_err(|_| TransportError::Closed)
        }
        async fn recv(&mut self) -> Result<ServerEvent, TransportError> {
            self.incoming
                .recv()
                .await
                .unwrap_or(Err(TransportError::Closed))
        }
    }

    fn connection() -> (FakeTransport, Server) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (sent_tx, sent_rx) = mpsc::unbounded_channel();
        (
            FakeTransport {
                incoming: event_rx,
                sent: sent_tx,
            },
            Server {
                events: event_tx,
                sent: sent_rx,
            },
        )
    }

    struct FakeConnector {
        queue: VecDeque<Result<FakeTransport, TransportError>>,
        connects: Arc<AtomicUsize>,
    }

    impl Connector for FakeConnector {
        type Transport = FakeTransport;
        async fn connect(&mut self) -> Result<FakeTransport, TransportError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            self.queue
                .pop_front()
                .unwrap_or(Err(TransportError::Io("no more connections".into())))
        }
    }

    struct Harness {
        updates: mpsc::Receiver<SessionUpdate>,
        commands: mpsc::Sender<ClientCommand>,
        connects: Arc<AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Harness {
        fn start(connections: Vec<Result<FakeTransport, TransportError>>) -> Self {
            let connects = Arc::new(AtomicUsize::new(0));
            let connector = FakeConnector {
                queue: connections.into(),
                connects: connects.clone(),
            };
            let session = RemoteSession::new(connector, SessionToken::new("tok"))
                .with_backoff(Backoff::new(Duration::from_secs(1), Duration::from_secs(8)));
            let (command_tx, command_rx) = mpsc::channel(16);
            let (update_tx, update_rx) = mpsc::channel(64);
            let task = tokio::spawn(session.run(command_rx, update_tx));
            Self {
                updates: update_rx,
                commands: command_tx,
                connects,
                task,
            }
        }

        async fn next(&mut self) -> SessionUpdate {
            self.updates.recv().await.expect("session ended early")
        }

        /// Skips to the next update that is not a state change.
        async fn next_content(&mut self) -> SessionUpdate {
            loop {
                match self.next().await {
                    SessionUpdate::State(_) => {}
                    other => return other,
                }
            }
        }

        async fn expect_state(&mut self, want: ConnState) {
            assert_eq!(self.next().await, SessionUpdate::State(want));
        }

        /// Walks a fresh connection through hello and the first snapshot.
        async fn sign_in(&mut self, server: &mut Server, login: &str) {
            self.expect_state(ConnState::Connecting).await;
            server.say(ServerEvent::Hello {
                login: login.into(),
            });
            assert_eq!(self.next().await, SessionUpdate::Login(login.into()));
            self.expect_state(ConnState::Live).await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn signs_in_with_the_token_and_applies_snapshot_then_deltas() {
        let (transport, mut server) = connection();
        let mut h = Harness::start(vec![Ok(transport)]);

        assert_eq!(
            server.hears().await,
            ClientCommand::Hello {
                token: SessionToken::new("tok"),
                last_seq: None
            }
        );
        h.sign_in(&mut server, "octocat").await;

        server.say(ServerEvent::Snapshot {
            seq: 1,
            prs: vec![pr(1, 5), pr(2, 4)],
        });
        assert!(matches!(h.next().await, SessionUpdate::Changes(c) if c.len() == 2));

        server.say(ServerEvent::PrUpsert {
            seq: 2,
            pr: pr(3, 9),
        });
        assert!(
            matches!(h.next().await, SessionUpdate::Changes(c) if c == vec![Change::Insert(0, pr(3, 9))])
        );

        server.say(ServerEvent::PrRemove {
            seq: 3,
            pr: pr_ref(3),
        });
        assert!(
            matches!(h.next().await, SessionUpdate::Changes(c) if c == vec![Change::Remove(0)])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stale_events_and_redelivered_notifications_are_not_forwarded() {
        let (transport, server) = connection();
        let mut h = Harness::start(vec![Ok(transport)]);
        let mut server = server;
        h.sign_in(&mut server, "me").await;

        server.say(ServerEvent::Snapshot {
            seq: 5,
            prs: vec![pr(1, 5)],
        });
        h.next_content().await;
        server.say(ServerEvent::PrUpsert {
            seq: 5,
            pr: pr(9, 9),
        }); // stale: ignored
        server.say(ServerEvent::Notification(notification(6, 100)));
        server.say(ServerEvent::Notification(notification(6, 100))); // redelivery: ignored
        server.say(ServerEvent::Notification(notification(7, 101)));

        let first = h.next_content().await;
        let second = h.next_content().await;
        assert!(matches!(first, SessionUpdate::Notification(n) if n.id == 100));
        assert!(matches!(second, SessionUpdate::Notification(n) if n.id == 101));
    }

    #[tokio::test(start_paused = true)]
    async fn reconnects_after_backoff_and_resumes_from_the_last_seq() {
        let (first, mut server1) = connection();
        let (second, mut server2) = connection();
        let mut h = Harness::start(vec![Ok(first), Ok(second)]);

        server1.hears().await;
        h.sign_in(&mut server1, "me").await;
        server1.say(ServerEvent::Snapshot {
            seq: 4,
            prs: vec![pr(1, 5), pr(2, 4)],
        });
        h.next_content().await;

        server1.drop_connection();
        h.expect_state(ConnState::Disconnected).await;
        let before = tokio::time::Instant::now();
        h.expect_state(ConnState::Connecting).await;
        assert!(
            before.elapsed() >= Duration::from_secs(1),
            "waited out the backoff"
        );

        // The resume point survived the reconnect.
        assert_eq!(
            server2.hears().await,
            ClientCommand::Hello {
                token: SessionToken::new("tok"),
                last_seq: Some(4)
            }
        );
        server2.say(ServerEvent::Hello { login: "me".into() });
        h.next().await; // Login
        h.expect_state(ConnState::Live).await;

        // A fresh snapshot after reconnecting reports only what actually changed.
        let mut renamed = pr(2, 4);
        renamed.title = "renamed".into();
        server2.say(ServerEvent::Snapshot {
            seq: 9,
            prs: vec![pr(1, 5), renamed.clone()],
        });
        assert!(
            matches!(h.next().await, SessionUpdate::Changes(c) if c == vec![Change::Update(1, renamed)])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_grows_between_failed_attempts_and_resets_after_success() {
        let mut b = Backoff::new(Duration::from_secs(1), Duration::from_secs(8));
        let delays: Vec<_> = (0..5).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 8], "doubles, then holds at the cap");
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_secs(1));

        // Jitter only ever shortens a delay, never past 80% of it.
        let mut j = Backoff::new(Duration::from_secs(10), Duration::from_secs(10)).with_jitter();
        for _ in 0..50 {
            let d = j.next_delay();
            assert!(
                d <= Duration::from_secs(10) && d >= Duration::from_secs(8),
                "{d:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn connection_failures_retry_with_growing_delays() {
        let mut h = Harness::start(vec![
            Err(TransportError::Io("down".into())),
            Err(TransportError::Io("down".into())),
        ]);
        let start = tokio::time::Instant::now();
        let mut connecting_at = Vec::new();
        for _ in 0..3 {
            h.expect_state(ConnState::Connecting).await;
            connecting_at.push(start.elapsed().as_secs());
            h.expect_state(ConnState::Disconnected).await;
        }
        // Attempts at 0 s, then after 1 s, then after a further 2 s.
        assert_eq!(connecting_at, [0, 1, 3]);
        assert_eq!(h.connects.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_sign_in_is_final() {
        let (transport, server) = connection();
        let mut h = Harness::start(vec![Ok(transport)]);
        h.expect_state(ConnState::Connecting).await;
        server.say(ServerEvent::Error {
            message: "session revoked".into(),
        });
        h.expect_state(ConnState::Failed("session revoked".into()))
            .await;
        h.task.await.unwrap();
        assert_eq!(
            h.connects.load(Ordering::SeqCst),
            1,
            "must not retry a revoked session"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_server_times_out_the_sign_in_and_retries() {
        let (transport, _server) = connection(); // never answers
        let mut h = Harness::start(vec![Ok(transport)]);
        h.expect_state(ConnState::Connecting).await;
        h.expect_state(ConnState::Disconnected).await; // after the 10 s handshake timeout
    }

    #[tokio::test(start_paused = true)]
    async fn actions_are_sent_while_live_and_answers_are_forwarded() {
        let (transport, mut server) = connection();
        let mut h = Harness::start(vec![Ok(transport)]);
        server.hears().await;
        h.sign_in(&mut server, "me").await;

        let action = ClientCommand::RequestReview {
            request_id: "r1".into(),
            pr: pr_ref(7),
            to: "hubot".into(),
        };
        h.commands.send(action.clone()).await.unwrap();
        assert_eq!(server.hears().await, action);

        server.say(ServerEvent::Accepted {
            request_id: "r1".into(),
        });
        assert_eq!(
            h.next().await,
            SessionUpdate::ActionResult {
                request_id: "r1".into(),
                result: Ok(())
            }
        );
        server.say(ServerEvent::Rejected {
            request_id: "r2".into(),
            reason: "rate limited".into(),
        });
        assert_eq!(
            h.next().await,
            SessionUpdate::ActionResult {
                request_id: "r2".into(),
                result: Err("rate limited".into())
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_invalid_action_is_reported_without_dropping_the_connection() {
        let (transport, mut server) = connection();
        let mut h = Harness::start(vec![Ok(transport)]);
        server.hears().await;
        h.sign_in(&mut server, "me").await;

        h.commands
            .send(ClientCommand::Message {
                request_id: "m1".into(),
                pr: pr_ref(7),
                to: "hubot".into(),
                body: String::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            h.next().await,
            SessionUpdate::ActionResult {
                request_id: "m1".into(),
                result: Err("body is empty".into())
            }
        );
        // Still connected: a later action goes through.
        h.commands
            .send(ClientCommand::AckNotification { id: 3 })
            .await
            .unwrap();
        assert_eq!(
            server.hears().await,
            ClientCommand::AckNotification { id: 3 }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn actions_sent_while_offline_are_refused_not_delivered_later() {
        let (first, server1) = connection();
        let (second, mut server2) = connection();
        let mut h = Harness::start(vec![Ok(first), Ok(second)]);
        let mut server1 = server1;
        server1.hears().await;
        h.sign_in(&mut server1, "me").await;

        server1.drop_connection();
        h.expect_state(ConnState::Disconnected).await;

        h.commands
            .send(ClientCommand::Nudge {
                request_id: "n1".into(),
                pr: pr_ref(1),
                to: "x".into(),
                note: None,
            })
            .await
            .unwrap();
        assert_eq!(
            h.next_content().await,
            SessionUpdate::ActionResult {
                request_id: "n1".into(),
                result: Err("not connected".into())
            }
        );

        // After the reconnect, the only thing the server hears is the hello, never the nudge.
        h.expect_state(ConnState::Connecting).await;
        assert!(matches!(server2.hears().await, ClientCommand::Hello { .. }));
        server2.say(ServerEvent::Hello { login: "me".into() });
        h.next().await;
        h.expect_state(ConnState::Live).await;
        assert!(
            server2.sent.try_recv().is_err(),
            "stale nudge must not be delivered"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn presence_and_notifications_reach_the_ui() {
        let (transport, mut server) = connection();
        let mut h = Harness::start(vec![Ok(transport)]);
        server.hears().await;
        h.sign_in(&mut server, "me").await;

        server.say(ServerEvent::Presence {
            login: "hubot".into(),
            online: true,
            viewing: Some(pr_ref(7)),
        });
        assert_eq!(
            h.next().await,
            SessionUpdate::Presence {
                login: "hubot".into(),
                online: true,
                viewing: Some(pr_ref(7))
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sync_status_reaches_the_ui_and_leaves_the_list_alone() {
        let (transport, mut server) = connection();
        let mut h = Harness::start(vec![Ok(transport)]);
        server.hears().await;
        h.sign_in(&mut server, "me").await;

        server.say(ServerEvent::Status {
            sync: SyncStatus::NotInstalled,
            install_url: Some("https://github.com/apps/x/installations/new".into()),
        });
        assert_eq!(
            h.next().await,
            SessionUpdate::Sync {
                status: SyncStatus::NotInstalled,
                install_url: Some("https://github.com/apps/x/installations/new".into())
            }
        );
        server.say(ServerEvent::Status {
            sync: SyncStatus::Ok,
            install_url: None,
        });
        assert_eq!(
            h.next().await,
            SessionUpdate::Sync {
                status: SyncStatus::Ok,
                install_url: None
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stops_cleanly_when_the_command_channel_closes() {
        let (transport, mut server) = connection();
        let mut h = Harness::start(vec![Ok(transport)]);
        server.hears().await;
        h.sign_in(&mut server, "me").await;
        drop(h.commands);
        h.task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn stops_cleanly_when_the_ui_goes_away() {
        let (transport, mut server) = connection();
        let h = Harness::start(vec![Ok(transport)]);
        server.hears().await;
        drop(h.updates);
        h.task.await.unwrap();
    }
}
