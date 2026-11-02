//! Glue between a server session (`pulldeck_core::remote`) and the Qt objects.
//!
//! A session runs on the tokio runtime and reports `SessionUpdate`s; everything that touches a
//! Qt object is queued back onto the Qt thread, so no network work happens there.

use crate::backend::{open_prs_status, qobject, runtime};
use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::QString;
use pulldeck_core::remote::{ConnState, Connector, RemoteSession, SessionUpdate};
use pulldeck_core::session::{ClientCommand, PrRef, SessionToken, SyncStatus};
use pulldeck_core::settings::ServerUrl;
use pulldeck_core::ws::WsConnector;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

fn state_name(state: &ConnState) -> &'static str {
    match state {
        ConnState::Connecting => "connecting",
        ConnState::Live => "live",
        ConnState::Disconnected => "disconnected",
        ConnState::Failed(_) => "failed",
    }
}

/// The names QML compares `syncStatus` against.
fn sync_name(status: SyncStatus) -> &'static str {
    match status {
        SyncStatus::Ok => "ok",
        SyncStatus::NeedsSignin => "needsSignin",
        SyncStatus::NotInstalled => "notInstalled",
    }
}

/// A fresh id for an action, unique for this process and well under the server's 64-byte limit.
pub(crate) fn next_request_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static START: OnceLock<u64> = OnceLock::new();
    let start = START.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
    });
    format!("{start:x}-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// A server and session token from the environment, for developing against a dev server before
/// the sign-in screen and keyring storage exist: `PULLDECK_SERVER=http://127.0.0.1:8080
/// PULLDECK_TOKEN=<token>`. Both must be set; the token is never logged.
fn development_session() -> Option<(ServerUrl, SessionToken)> {
    let server = std::env::var("PULLDECK_SERVER").ok()?;
    let token = std::env::var("PULLDECK_TOKEN").ok()?;
    match ServerUrl::parse(&server) {
        Ok(server) => Some((server, SessionToken::new(token))),
        Err(error) => {
            tracing::warn!(%error, "ignoring PULLDECK_SERVER");
            None
        }
    }
}

/// What to tell the server about the PR the user is looking at. Presence is opt-in: with it off
/// nothing is ever sent, whatever the caller asks.
fn presence_command(enabled: bool, viewing: Option<PrRef>) -> Option<ClientCommand> {
    enabled.then_some(ClientCommand::Presence { viewing })
}

fn pr_ref(repo: &QString, number: i64) -> PrRef {
    PrRef {
        repo: repo.to_string(),
        number: u64::try_from(number).unwrap_or_default(),
    }
}

/// A short description of an update for the debug log. Never includes message text or tokens.
fn describe(update: &SessionUpdate) -> String {
    match update {
        SessionUpdate::State(state) => format!("state {}", state_name(state)),
        SessionUpdate::Login(_) => "signed in".into(),
        SessionUpdate::Changes(changes) => format!("{} row change(s)", changes.len()),
        SessionUpdate::Notification(n) => format!("notification {} ({:?})", n.id, n.kind),
        SessionUpdate::ActionResult { result, .. } => {
            format!(
                "action {}",
                if result.is_ok() {
                    "accepted"
                } else {
                    "rejected"
                }
            )
        }
        SessionUpdate::Sync { status, .. } => format!("sync {}", sync_name(*status)),
        SessionUpdate::Presence { online, .. } => format!("presence online={online}"),
    }
}

impl qobject::PrBackend {
    /// Gives the backend the review-request list so polling can fill it.
    ///
    /// # Safety
    ///
    /// `reviews` must be null or point to a live `PrListModel` (QML guarantees this).
    pub unsafe fn attach_reviews(mut self: Pin<&mut Self>, reviews: *mut qobject::PrListModel) {
        // SAFETY: the caller guarantees the pointer is null or valid.
        if let Some(reviews) = unsafe { reviews.as_ref() } {
            self.as_mut().rust_mut().reviews = Some(Arc::new(reviews.qt_thread()));
        }
    }

    /// Gives the backend the closed-pull-request list so polling can fill it.
    ///
    /// # Safety
    ///
    /// `history` must be null or point to a live `PrListModel` (QML guarantees this).
    pub unsafe fn attach_history(mut self: Pin<&mut Self>, history: *mut qobject::PrListModel) {
        // SAFETY: the caller guarantees the pointer is null or valid.
        if let Some(history) = unsafe { history.as_ref() } {
            self.as_mut().rust_mut().history = Some(Arc::new(history.qt_thread()));
        }
    }

    /// Gives the backend the inbox so notifications from a server session can reach it. QML
    /// calls this once at startup, before any session starts.
    ///
    /// # Safety
    ///
    /// `inbox` must be null or point to a live `InboxModel` (QML guarantees this).
    pub unsafe fn attach_inbox(mut self: Pin<&mut Self>, inbox: *mut qobject::InboxModel) {
        // SAFETY: the caller guarantees the pointer is null or valid; a null pointer is ignored.
        let Some(inbox) = (unsafe { inbox.as_ref() }) else {
            return;
        };
        self.as_mut().rust_mut().inbox = Some(Arc::new(inbox.qt_thread()));
        #[cfg(feature = "demo")]
        if std::env::var_os("PULLDECK_DEMO").is_some() {
            crate::demo::start(self);
            return;
        }
        let presence = crate::auth::load_settings().presence;
        self.as_mut().set_presence(presence);
        if let Some((server, token)) = development_session() {
            // A development server has fake accounts; keep real pull requests out of its list.
            self.as_mut().rust_mut().poll = false;
            self.start_session(WsConnector::new(&server), token);
        } else {
            self.resume_saved_session();
        }
    }

    /// Starts a server session. Its pull requests are merged with the ones found by polling
    /// GitHub, and win where both have the same one.
    pub(crate) fn start_session<C>(mut self: Pin<&mut Self>, connector: C, token: SessionToken)
    where
        C: Connector + 'static,
    {
        let (command_tx, command_rx) = mpsc::channel(32);
        let (update_tx, update_rx) = mpsc::channel(256);
        self.as_mut().rust_mut().commands = Some(command_tx);
        self.as_mut().set_server_configured(true);
        self.as_mut().set_connection(QString::from("connecting"));
        self.as_mut()
            .set_status(QString::from("Connecting to server…"));

        self.as_mut().set_auth(QString::from("signedIn"));

        let backend = self.qt_thread();
        let inbox = self.inbox.clone();
        let session =
            runtime().spawn(RemoteSession::new(connector, token).run(command_rx, update_tx));
        let forwarder = runtime().spawn(forward(update_rx, backend, inbox));
        // A new session replaces any previous one.
        for task in std::mem::take(&mut self.as_mut().rust_mut().session_tasks) {
            task.abort();
        }
        self.rust_mut().session_tasks = vec![session.abort_handle(), forwarder.abort_handle()];
    }

    pub fn request_review(self: Pin<&mut Self>, repo: &QString, number: i64, to: &QString) {
        self.send_action(ClientCommand::RequestReview {
            request_id: next_request_id(),
            pr: pr_ref(repo, number),
            to: to.to_string(),
        });
    }

    pub fn nudge(self: Pin<&mut Self>, repo: &QString, number: i64, to: &QString, note: &QString) {
        let note = note.to_string();
        self.send_action(ClientCommand::Nudge {
            request_id: next_request_id(),
            pr: pr_ref(repo, number),
            to: to.to_string(),
            note: (!note.is_empty()).then_some(note),
        });
    }

    pub fn send_message(
        self: Pin<&mut Self>,
        repo: &QString,
        number: i64,
        to: &QString,
        body: &QString,
    ) {
        self.send_action(ClientCommand::Message {
            request_id: next_request_id(),
            pr: pr_ref(repo, number),
            to: to.to_string(),
            body: body.to_string(),
        });
    }

    pub fn set_presence_enabled(mut self: Pin<&mut Self>, enabled: bool) {
        if *self.presence() == enabled {
            return;
        }
        self.as_mut().set_presence(enabled);
        runtime().spawn_blocking(move || crate::auth::remember_presence(enabled));
        if !enabled {
            // Stop announcing the last PR. This is the one message sent with presence off, and
            // it only ever retracts.
            self.try_send(ClientCommand::Presence { viewing: None });
        }
    }

    pub fn viewing(self: Pin<&mut Self>, repo: &QString, number: i64) {
        let pr = pr_ref(repo, number);
        if let Some(command) = presence_command(*self.presence(), Some(pr)) {
            self.try_send(command);
        }
    }

    /// Tells the server a notification was read. If we are offline the server simply resends it
    /// next time, so a failed send is fine.
    pub fn ack_notification(self: Pin<&mut Self>, id: i64) {
        if let Ok(id) = u64::try_from(id) {
            self.try_send(ClientCommand::AckNotification { id });
        }
    }

    /// Sends an action, or reports right away that it could not be sent.
    fn send_action(self: Pin<&mut Self>, command: ClientCommand) {
        let request_id = command.request_id().unwrap_or_default().to_owned();
        if !self.try_send(command) {
            self.action_result(
                QString::from(request_id.as_str()),
                false,
                QString::from("not connected"),
            );
        }
    }

    fn try_send(&self, command: ClientCommand) -> bool {
        self.connection().to_string() == "live"
            && self
                .commands
                .as_ref()
                .is_some_and(|tx| tx.try_send(command).is_ok())
    }
}

/// Moves session updates onto the Qt thread. Ends when the session does or the UI is gone.
async fn forward(
    mut updates: mpsc::Receiver<SessionUpdate>,
    backend: CxxQtThread<qobject::PrBackend>,
    inbox: Option<Arc<CxxQtThread<qobject::InboxModel>>>,
) {
    while let Some(update) = updates.recv().await {
        tracing::debug!("session update: {}", describe(&update));
        let delivered = match update {
            SessionUpdate::State(state) => backend.queue(move |mut b| {
                if state == ConnState::Connecting {
                    // The server only reports problems, so each connection starts out healthy.
                    b.as_mut().set_sync_status(QString::from("ok"));
                    b.as_mut().set_install_url(QString::default());
                }
                if let ConnState::Failed(reason) = &state {
                    // The server turned the session down for good: the saved token is useless.
                    b.as_mut().set_status(QString::from(reason.as_str()));
                    b.session_failed(reason);
                } else {
                    b.set_connection(QString::from(state_name(&state)));
                }
            }),
            SessionUpdate::Login(login) => {
                backend.queue(move |mut b| b.as_mut().set_user(QString::from(login.as_str())))
            }
            SessionUpdate::Changes(changes) => backend.queue(move |mut b| {
                b.as_mut().apply_server_changes(changes);
                let status = open_prs_status(b.pr_count());
                tracing::debug!(rows = b.pr_count(), "list updated from server");
                b.as_mut().set_status(QString::from(status.as_str()));
            }),
            SessionUpdate::Notification(notification) => match &inbox {
                Some(inbox) => inbox.queue(move |mut i| i.as_mut().push(notification)),
                None => Ok(()),
            },
            SessionUpdate::ActionResult { request_id, result } => backend.queue(move |b| {
                let (ok, message) = match result {
                    Ok(()) => (true, String::new()),
                    Err(reason) => (false, reason),
                };
                b.action_result(
                    QString::from(request_id.as_str()),
                    ok,
                    QString::from(message.as_str()),
                );
            }),
            SessionUpdate::Sync {
                status,
                install_url,
            } => backend.queue(move |mut b| {
                b.as_mut().set_sync_status(QString::from(sync_name(status)));
                let url = install_url.unwrap_or_default();
                b.set_install_url(QString::from(url.as_str()));
            }),
            // Presence is shown once the UI has somewhere to put it.
            SessionUpdate::Presence { .. } => Ok(()),
        };
        if delivered.is_err() {
            tracing::debug!("UI went away; stopping session updates");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_unique_nonempty_and_within_the_server_limit() {
        let ids: Vec<_> = (0..100).map(|_| next_request_id()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());
        assert!(ids.iter().all(|id| !id.is_empty() && id.len() <= 64));
    }

    #[test]
    fn presence_is_never_sent_unless_the_user_turned_it_on() {
        let pr = PrRef {
            repo: "o/r".into(),
            number: 1,
        };
        assert!(presence_command(false, Some(pr.clone())).is_none());
        assert!(presence_command(false, None).is_none());
        assert!(matches!(
            presence_command(true, Some(pr)),
            Some(ClientCommand::Presence { viewing: Some(_) })
        ));
    }

    #[test]
    fn sync_states_have_stable_names_for_qml() {
        assert_eq!(sync_name(SyncStatus::Ok), "ok");
        assert_eq!(sync_name(SyncStatus::NeedsSignin), "needsSignin");
        assert_eq!(sync_name(SyncStatus::NotInstalled), "notInstalled");
    }

    #[test]
    fn connection_states_have_stable_names_for_qml() {
        assert_eq!(state_name(&ConnState::Connecting), "connecting");
        assert_eq!(state_name(&ConnState::Live), "live");
        assert_eq!(state_name(&ConnState::Disconnected), "disconnected");
        assert_eq!(state_name(&ConnState::Failed("x".into())), "failed");
    }
}
