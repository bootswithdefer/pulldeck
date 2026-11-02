//! Signing in and out of a pulldeck server, and picking up a saved session at startup.
//!
//! The network and keyring work runs on the tokio runtime; results come back through
//! `qt_thread().queue`, so the Qt thread never blocks on either.

use crate::backend::{qobject, runtime};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use pulldeck_core::login::{LoginClient, SignIn};
use pulldeck_core::settings::{ServerUrl, Settings};
use pulldeck_core::tokens::{KeyringTokenStore, TokenStore};
use pulldeck_core::ws::WsConnector;
use std::pin::Pin;

/// What a finished sign-in hands back to the Qt thread: the session, and a warning if it could
/// not be saved (the user is still signed in for this run).
type Finished = Result<(SignIn, Option<String>), String>;

pub(crate) fn load_settings() -> Settings {
    let Some(path) = Settings::default_path() else {
        return Settings::default();
    };
    Settings::load_from(&path).unwrap_or_else(|error| {
        tracing::warn!(%error, "ignoring unreadable settings");
        Settings::default()
    })
}

fn remember_server(server: &ServerUrl) -> Result<(), String> {
    let path = Settings::default_path().ok_or("no config directory")?;
    let mut settings = load_settings();
    settings.server_url = Some(server.url(""));
    settings.save_to(&path).map_err(|e| e.to_string())
}

/// Changes one setting and saves the file, keeping every other setting.
fn update_settings(what: &str, change: impl FnOnce(&mut Settings)) {
    let Some(path) = Settings::default_path() else {
        return;
    };
    let mut settings = load_settings();
    change(&mut settings);
    if let Err(error) = settings.save_to(&path) {
        tracing::warn!(%error, "could not save the {what} setting");
    }
}

/// Saves the presence choice.
pub(crate) fn remember_presence(enabled: bool) {
    update_settings("presence", |s| s.presence = enabled);
}

/// Saves the notifications choice.
pub(crate) fn remember_notifications(enabled: bool) {
    update_settings("notifications", |s| s.notifications = enabled);
}

/// Runs a blocking keyring call off the async workers.
async fn keyring<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok()
}

/// Saves the session and the server it belongs to. Returns a warning for the user on failure.
async fn save_session(server: &ServerUrl, signed_in: &SignIn) -> Option<String> {
    let store = KeyringTokenStore::new(server);
    let token = signed_in.token.clone();
    let mut problems = Vec::new();
    match keyring(move || store.save(&token)).await {
        Some(Ok(())) => {}
        Some(Err(error)) => problems.push(error.to_string()),
        None => problems.push("keyring task failed".into()),
    }
    if let Err(error) = remember_server(server) {
        problems.push(format!("settings: {error}"));
    }
    (!problems.is_empty()).then(|| {
        format!(
            "Signed in, but this session was not saved ({}). You will have to sign in again next time.",
            problems.join("; ")
        )
    })
}

/// Revokes the session on the server (best effort) and forgets the saved token.
async fn forget_session(server: ServerUrl, revoke: bool) {
    let store = KeyringTokenStore::new(&server);
    if revoke {
        if let Some(Ok(Some(token))) = keyring({
            let store = KeyringTokenStore::new(&server);
            move || store.load()
        })
        .await
        {
            if let Ok(client) = LoginClient::new(server) {
                if let Err(error) = client.logout(&token).await {
                    tracing::debug!(%error, "logout request failed");
                }
            }
        }
    }
    if let Some(Err(error)) = keyring(move || store.clear()).await {
        tracing::warn!(%error, "could not clear the saved session");
    }
}

impl qobject::PrBackend {
    /// At startup, looks for a saved server and session and resumes it. Does nothing when no
    /// server is configured, so the app keeps polling GitHub itself.
    pub(crate) fn resume_saved_session(mut self: Pin<&mut Self>) {
        let server = match load_settings().server() {
            Ok(Some(server)) => server,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, "ignoring the configured server");
                return;
            }
        };
        self.as_mut()
            .set_server_url(QString::from(server.url("").as_str()));
        let backend = self.qt_thread();
        runtime().spawn(async move {
            let store = KeyringTokenStore::new(&server);
            let saved = keyring(move || store.load()).await;
            let _ = backend.queue(move |mut b| match saved {
                Some(Ok(Some(token))) => b.start_session(WsConnector::new(&server), token),
                Some(Err(error)) => {
                    tracing::warn!(%error, "could not read the saved session");
                    let message = format!("Could not read the saved session: {error}");
                    b.as_mut().set_auth_error(QString::from(message.as_str()));
                }
                _ => {}
            });
        });
    }

    pub fn sign_in(mut self: Pin<&mut Self>, server: &QString) {
        // Signing in again is how an expired GitHub login (needsSignin) gets fixed, so it is
        // allowed while a session exists; the new session replaces the old one.
        let signed_in = self.auth().to_string() == "signedIn";
        if self.login_task.is_some()
            || (signed_in && self.sync_status().to_string() != "needsSignin")
        {
            return;
        }
        let parsed = ServerUrl::parse(&server.to_string()).map_err(|e| e.to_string());
        let (server, client) = match parsed.and_then(|server| {
            let client = LoginClient::new(server.clone()).map_err(|e| e.to_string())?;
            Ok((server, client))
        }) {
            Ok(both) => both,
            Err(message) => {
                self.set_auth_error(QString::from(message.as_str()));
                return;
            }
        };
        self.as_mut().set_auth_error(QString::default());
        self.as_mut().set_auth(QString::from("signingIn"));

        let backend = self.qt_thread();
        let task = runtime().spawn(async move {
            let shown = backend.clone();
            let signed_in = client
                .sign_in(|code| {
                    let (user_code, uri) = (code.user_code.clone(), code.verification_uri.clone());
                    let _ = shown.queue(move |mut b| {
                        b.as_mut().set_user_code(QString::from(user_code.as_str()));
                        b.as_mut().set_verification_uri(QString::from(uri.as_str()));
                    });
                })
                .await;
            let finished: Finished = match signed_in {
                Ok(signed_in) => {
                    let warning = save_session(&server, &signed_in).await;
                    Ok((signed_in, warning))
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = backend.queue(move |b| b.finish_sign_in(&server, finished));
        });
        self.rust_mut().login_task = Some(task.abort_handle());
    }

    fn finish_sign_in(mut self: Pin<&mut Self>, server: &ServerUrl, finished: Finished) {
        self.as_mut().rust_mut().login_task = None;
        self.as_mut().set_user_code(QString::default());
        self.as_mut().set_verification_uri(QString::default());
        match finished {
            Ok((signed_in, warning)) => {
                self.as_mut()
                    .set_server_url(QString::from(server.url("").as_str()));
                let warning = warning.unwrap_or_default();
                self.as_mut()
                    .set_auth_error(QString::from(warning.as_str()));
                self.as_mut()
                    .set_user(QString::from(signed_in.login.as_str()));
                self.start_session(WsConnector::new(server), signed_in.token);
            }
            Err(message) => {
                let idle = self.idle_auth();
                self.as_mut().set_auth(idle);
                self.set_auth_error(QString::from(message.as_str()));
            }
        }
    }

    /// What `auth` is when no sign-in is in progress: signed in if a session is still running
    /// (signing in again while the server says "needsSignin"), otherwise signed out.
    fn idle_auth(&self) -> QString {
        QString::from(if *self.server_configured() {
            "signedIn"
        } else {
            "signedOut"
        })
    }

    pub fn cancel_sign_in(mut self: Pin<&mut Self>) {
        if let Some(task) = self.as_mut().rust_mut().login_task.take() {
            task.abort();
        }
        if self.auth().to_string() == "signingIn" {
            let idle = self.idle_auth();
            self.as_mut().set_auth(idle);
            self.as_mut().set_user_code(QString::default());
            self.as_mut().set_verification_uri(QString::default());
        }
    }

    pub fn sign_out(mut self: Pin<&mut Self>) {
        let server = ServerUrl::parse(&self.server_url().to_string()).ok();
        self.as_mut().end_session();
        if let Some(server) = server {
            runtime().spawn(forget_session(server, true));
        }
        // Back to polling GitHub directly.
        self.refresh();
    }

    /// The server turned the session down for good (sign in again). Forget it everywhere.
    pub(crate) fn session_failed(mut self: Pin<&mut Self>, reason: &str) {
        let server = ServerUrl::parse(&self.server_url().to_string()).ok();
        self.as_mut().end_session();
        let message = format!("Signed out: {reason}");
        self.as_mut()
            .set_auth_error(QString::from(message.as_str()));
        if let Some(server) = server {
            runtime().spawn(forget_session(server, false));
        }
    }

    /// Stops the session and empties everything it filled in.
    fn end_session(mut self: Pin<&mut Self>) {
        if let Some(task) = self.as_mut().rust_mut().login_task.take() {
            task.abort();
        }
        for task in self.as_mut().rust_mut().session_tasks.drain(..) {
            task.abort();
        }
        self.as_mut().rust_mut().commands = None;
        self.as_mut().set_server_configured(false);
        self.as_mut().set_connection(QString::from("offline"));
        self.as_mut().set_auth(QString::from("signedOut"));
        self.as_mut().set_user(QString::default());
        // The server's pull requests go; anything found by polling stays.
        self.as_mut().clear_server_prs();
        if let Some(inbox) = self.inbox.clone() {
            let _ = inbox.queue(|i| i.reset());
        }
    }
}
