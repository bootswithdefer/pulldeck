//! The QObject that bridges the core crate to QML.
//!
//! `PrBackend` is a `QAbstractListModel`: QML binds a `ListView` straight to it and a refresh
//! only inserts, removes or updates the rows that changed (see `pulldeck_core::store::diff`).

use crate::inbox::InboxModelRust;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use pulldeck_core::session::ClientCommand;
use pulldeck_core::store::{self, Change};
use pulldeck_core::{GitHubClient, PullRequest};
use std::pin::Pin;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractListModel>);
        /// Base class of the model.
        #[qobject]
        type QAbstractListModel;
    }

    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;

        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;

        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;

        include!("cxx-qt-lib/qvector.h");
        type QVector_i32 = cxx_qt_lib::QVector<i32>;
    }

    impl cxx_qt::Threading for PrBackend {}

    extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        #[qproperty(QString, status)]
        #[qproperty(QString, user)]
        #[qproperty(bool, loading)]
        /// "offline" without a server, otherwise "connecting", "live", "disconnected" or "failed".
        #[qproperty(QString, connection)]
        #[qproperty(bool, server_configured, cxx_name = "serverConfigured")]
        /// "signedOut", "signingIn" or "signedIn" (a server session exists).
        #[qproperty(QString, auth)]
        /// While signing in: the code to enter on GitHub and where to enter it.
        #[qproperty(QString, user_code, cxx_name = "userCode")]
        #[qproperty(QString, verification_uri, cxx_name = "verificationUri")]
        /// Why the last sign-in failed or the saved session was rejected; empty otherwise.
        #[qproperty(QString, auth_error, cxx_name = "authError")]
        /// The server this app signs in to (from the settings file or the last sign-in).
        #[qproperty(QString, server_url, cxx_name = "serverUrl")]
        #[namespace = "pulldeck"]
        type PrBackend = super::PrBackendRust;

        /// Fetch the user's open PRs in the background. Does nothing when a server session owns
        /// the list.
        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        /// Start the sign-in flow with the server at `server`; progress shows in `auth`,
        /// `userCode` and `verificationUri`.
        #[qinvokable]
        #[cxx_name = "signIn"]
        fn sign_in(self: Pin<&mut Self>, server: &QString);

        /// Abandon a sign-in that is waiting for the user.
        #[qinvokable]
        #[cxx_name = "cancelSignIn"]
        fn cancel_sign_in(self: Pin<&mut Self>);

        /// End the server session, forget the saved token and go back to polling GitHub.
        #[qinvokable]
        #[cxx_name = "signOut"]
        fn sign_out(self: Pin<&mut Self>);

        /// Human-friendly age ("5m ago") of a Unix timestamp, relative to `now`.
        #[qinvokable]
        fn ago(self: &PrBackend, updated_at: i64, now: i64) -> QString;

        /// Ask the server to request a review from `to` on a PR.
        #[qinvokable]
        #[cxx_name = "requestReview"]
        fn request_review(self: Pin<&mut Self>, repo: &QString, number: i64, to: &QString);

        /// Send `to` a reminder about a PR (recorded by the server, not posted to GitHub).
        #[qinvokable]
        fn nudge(self: Pin<&mut Self>, repo: &QString, number: i64, to: &QString, note: &QString);

        /// Send `to` an in-app message about a PR (not a GitHub comment).
        #[qinvokable]
        #[cxx_name = "sendMessage"]
        fn send_message(
            self: Pin<&mut Self>,
            repo: &QString,
            number: i64,
            to: &QString,
            body: &QString,
        );

        /// Tell the server a notification has been read.
        #[qinvokable]
        #[cxx_name = "ackNotification"]
        fn ack_notification(self: Pin<&mut Self>, id: i64);

        /// The outcome of a request review, nudge or message.
        #[qsignal]
        #[cxx_name = "actionResult"]
        fn action_result(self: Pin<&mut Self>, request_id: QString, ok: bool, message: QString);
    }

    // Qt's meta-type system rejects a `Q_INVOKABLE` taking a non-const QObject reference, so the
    // inbox arrives as a pointer (which QML passes naturally), hence `unsafe`.
    unsafe extern "RustQt" {
        /// Give the backend the inbox so notifications from the server can reach it.
        ///
        /// # Safety
        ///
        /// `inbox` must be null or point to a live `InboxModel`.
        #[qinvokable]
        #[cxx_name = "attachInbox"]
        unsafe fn attach_inbox(self: Pin<&mut PrBackend>, inbox: *mut InboxModel);
    }

    // QAbstractListModel virtuals.
    extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        fn data(self: &PrBackend, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &PrBackend) -> QHash_i32_QByteArray;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &PrBackend, parent: &QModelIndex) -> i32;
    }

    // Notifications to attached views. Every begin_* must be paired with its end_*.
    extern "RustQt" {
        /// # Safety
        ///
        /// Must be followed by `end_insert_rows` once the row has been added.
        #[inherit]
        #[cxx_name = "beginInsertRows"]
        unsafe fn begin_insert_rows(
            self: Pin<&mut PrBackend>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        /// # Safety
        ///
        /// Pairs with `begin_insert_rows`.
        #[inherit]
        #[cxx_name = "endInsertRows"]
        unsafe fn end_insert_rows(self: Pin<&mut PrBackend>);

        /// # Safety
        ///
        /// Must be followed by `end_remove_rows` once the row has been removed.
        #[inherit]
        #[cxx_name = "beginRemoveRows"]
        unsafe fn begin_remove_rows(
            self: Pin<&mut PrBackend>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        /// # Safety
        ///
        /// Pairs with `begin_remove_rows`.
        #[inherit]
        #[cxx_name = "endRemoveRows"]
        unsafe fn end_remove_rows(self: Pin<&mut PrBackend>);
    }

    unsafe extern "RustQt" {
        /// Emitted when the data of existing rows changes.
        #[inherit]
        #[qsignal]
        #[cxx_name = "dataChanged"]
        fn data_changed(
            self: Pin<&mut PrBackend>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QVector_i32,
        );

        /// Model index for a row, needed to emit `dataChanged`.
        #[inherit]
        fn index(self: &PrBackend, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;
    }

    // The notification inbox model. It is declared here, not in inbox.rs, because every QObject
    // that derives from QAbstractListModel in a separate bridge generates the same cast symbols
    // and the link fails; its state and behaviour live in `inbox.rs`.
    impl cxx_qt::Threading for InboxModel {}

    extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        #[qproperty(i32, unread)]
        #[namespace = "pulldeck"]
        type InboxModel = super::InboxModelRust;

        /// The user read this notification: mark it locally and ask whoever owns the connection
        /// (via `ackRequested`) to tell the server.
        #[qinvokable]
        fn ack(self: Pin<&mut Self>, id: i64);

        /// Emitted when the user acknowledges a notification.
        #[qsignal]
        #[cxx_name = "ackRequested"]
        fn ack_requested(self: Pin<&mut Self>, id: i64);

        #[qinvokable]
        #[cxx_override]
        fn data(self: &InboxModel, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &InboxModel) -> QHash_i32_QByteArray;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &InboxModel, parent: &QModelIndex) -> i32;

        /// # Safety
        ///
        /// Must be followed by `end_insert_rows`.
        #[inherit]
        #[cxx_name = "beginInsertRows"]
        unsafe fn begin_insert_rows(
            self: Pin<&mut InboxModel>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        /// # Safety
        ///
        /// Pairs with `begin_insert_rows`.
        #[inherit]
        #[cxx_name = "endInsertRows"]
        unsafe fn end_insert_rows(self: Pin<&mut InboxModel>);
    }

    unsafe extern "RustQt" {
        /// Emitted when the data of existing inbox rows changes.
        #[inherit]
        #[qsignal]
        #[cxx_name = "dataChanged"]
        fn inbox_data_changed(
            self: Pin<&mut InboxModel>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QVector_i32,
        );

        /// Wraps replacing the whole inbox (sign-out).
        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn begin_reset_model(self: Pin<&mut InboxModel>);
        /// Pairs with `begin_reset_model`.
        #[inherit]
        #[cxx_name = "endResetModel"]
        fn end_reset_model(self: Pin<&mut InboxModel>);

        /// Model index for an inbox row, needed to emit `dataChanged`.
        #[inherit]
        #[cxx_name = "index"]
        fn inbox_index(
            self: &InboxModel,
            row: i32,
            column: i32,
            parent: &QModelIndex,
        ) -> QModelIndex;
    }
}

// Role ids start at Qt::UserRole (0x100) so they never collide with Qt's built-in roles.
const TITLE: i32 = 0x100;
const REPO: i32 = 0x101;
const NUMBER: i32 = 0x102;
const URL: i32 = 0x103;
const AUTHOR: i32 = 0x104;
const IS_DRAFT: i32 = 0x105;
const REVIEW: i32 = 0x106;
const CHECKS: i32 = 0x107;
const UPDATED_AT: i32 = 0x108;

/// Backing state for [`qobject::PrBackend`].
pub struct PrBackendRust {
    status: QString,
    user: QString,
    loading: bool,
    connection: QString,
    server_configured: bool,
    auth: QString,
    user_code: QString,
    verification_uri: QString,
    auth_error: QString,
    server_url: QString,
    /// The sign-in in progress, if any.
    pub(crate) login_task: Option<tokio::task::AbortHandle>,
    /// The running session's tasks, stopped on sign-out.
    pub(crate) session_tasks: Vec<tokio::task::AbortHandle>,
    prs: Vec<PullRequest>,
    /// Commands for the server session, once one is running.
    pub(crate) commands: Option<tokio::sync::mpsc::Sender<ClientCommand>>,
    /// Where server notifications go. In an `Arc` because cxx-qt needs this struct to be `Unpin`
    /// and a bare `CxxQtThread` is not.
    pub(crate) inbox: Option<std::sync::Arc<cxx_qt::CxxQtThread<qobject::InboxModel>>>,
}

impl Default for PrBackendRust {
    fn default() -> Self {
        Self {
            status: QString::from("Not refreshed yet"),
            user: QString::default(),
            loading: false,
            connection: QString::from("offline"),
            server_configured: false,
            auth: QString::from("signedOut"),
            user_code: QString::default(),
            verification_uri: QString::default(),
            auth_error: QString::default(),
            server_url: QString::default(),
            login_task: None,
            session_tasks: Vec::new(),
            prs: Vec::new(),
            commands: None,
            inbox: None,
        }
    }
}

pub(crate) fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to start tokio runtime")
    })
}

/// Everything the UI needs from one fetch, already stringly-typed.
type Fetched = Result<(String, Vec<PullRequest>), String>;

async fn fetch() -> Fetched {
    let client = GitHubClient::from_environment()
        .await
        .map_err(|e| e.to_string())?;
    client.authored_prs().await.map_err(|e| e.to_string())
}

impl qobject::PrBackend {
    /// Starts a background fetch; no-op if one is already running.
    pub fn refresh(mut self: Pin<&mut Self>) {
        // With a server session the server owns the list; polling GitHub as well would fight it.
        if *self.loading() || self.commands.is_some() {
            return;
        }
        self.as_mut().set_loading(true);
        self.as_mut().set_status(QString::from("Refreshing…"));

        let qt_thread = self.qt_thread();
        runtime().spawn(async move {
            let result = fetch().await;
            let queued = qt_thread.queue(move |mut backend| {
                match result {
                    Ok((login, prs)) => {
                        let status = open_prs_status(prs.len());
                        backend.as_mut().set_user(QString::from(login.as_str()));
                        backend.as_mut().replace_prs(prs);
                        backend.as_mut().set_status(QString::from(status.as_str()));
                    }
                    Err(message) => {
                        tracing::warn!(%message, "refresh failed");
                        backend.as_mut().set_status(QString::from(message.as_str()));
                    }
                }
                backend.as_mut().set_loading(false);
            });
            if queued.is_err() {
                tracing::debug!("UI went away before the refresh finished");
            }
        });
    }

    /// Human-friendly age of `updated_at` (Unix seconds) at `now`.
    pub fn ago(&self, updated_at: i64, now: i64) -> QString {
        QString::from(age(now, updated_at).as_str())
    }

    /// Swaps in a new list, telling attached views about each inserted, removed or updated row
    /// instead of resetting the whole model (so selection and scroll position survive).
    fn replace_prs(self: Pin<&mut Self>, new: Vec<PullRequest>) {
        let changes = store::diff(&self.prs, &new);
        tracing::debug!(
            rows = new.len(),
            changes = changes.len(),
            "applying refresh"
        );
        self.apply_changes(changes);
    }

    /// Applies row changes in order, notifying attached views of each one. Both the local refresh
    /// (which diffs a fresh fetch) and a server session (which sends changes already computed
    /// against its replica) end up here.
    /// How many PRs the list currently holds.
    pub(crate) fn pr_count(&self) -> usize {
        self.prs.len()
    }

    pub(crate) fn apply_changes(mut self: Pin<&mut Self>, changes: Vec<Change>) {
        let parent = QModelIndex::default();
        for change in changes {
            match &change {
                Change::Remove(row) => {
                    let row = *row as i32;
                    // SAFETY: paired with `end_remove_rows` below, with no early exit in between.
                    unsafe { self.as_mut().begin_remove_rows(&parent, row, row) };
                    change.apply(&mut self.as_mut().rust_mut().prs);
                    unsafe { self.as_mut().end_remove_rows() };
                }
                Change::Insert(row, _) => {
                    let row = *row as i32;
                    // SAFETY: paired with `end_insert_rows` below, with no early exit in between.
                    unsafe { self.as_mut().begin_insert_rows(&parent, row, row) };
                    change.apply(&mut self.as_mut().rust_mut().prs);
                    unsafe { self.as_mut().end_insert_rows() };
                }
                Change::Update(row, _) => {
                    change.apply(&mut self.as_mut().rust_mut().prs);
                    let index = self.index(*row as i32, 0, &parent);
                    // An empty role list means "every role changed".
                    self.as_mut()
                        .data_changed(&index, &index, &Default::default());
                }
            }
        }
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        self.prs.len() as i32
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        for (role, name) in [
            (TITLE, "title"),
            (REPO, "repo"),
            (NUMBER, "number"),
            (URL, "url"),
            (AUTHOR, "author"),
            (IS_DRAFT, "isDraft"),
            (REVIEW, "review"),
            (CHECKS, "checks"),
            (UPDATED_AT, "updatedAt"),
        ] {
            roles.insert(role, QByteArray::from(name));
        }
        roles
    }

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(pr) = usize::try_from(index.row())
            .ok()
            .and_then(|row| self.prs.get(row))
        else {
            return QVariant::default();
        };
        let text = |s: &str| QVariant::from(&QString::from(s));
        match role {
            TITLE => text(&pr.title),
            REPO => text(&pr.repo),
            NUMBER => QVariant::from(&(pr.number as i64)),
            URL => text(&pr.url),
            AUTHOR => text(&pr.author),
            IS_DRAFT => QVariant::from(&pr.is_draft),
            REVIEW => text(pr.review.as_str()),
            CHECKS => text(pr.checks.as_str()),
            UPDATED_AT => QVariant::from(&pr.updated_at.timestamp()),
            _ => QVariant::default(),
        }
    }
}

/// "3 open PRs" / "1 open PR", shown in the toolbar.
pub(crate) fn open_prs_status(count: usize) -> String {
    format!("{count} open PR{}", plural(count))
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// "5m ago"-style age of `then` at `now` (both Unix seconds); never negative.
fn age(now: i64, then: i64) -> String {
    let secs = (now - then).max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_buckets() {
        let now = 1_000_000;
        assert_eq!(age(now, now - 59), "just now");
        assert_eq!(age(now, now - 45 * 60), "45m ago");
        assert_eq!(age(now, now - 5 * 3600), "5h ago");
        assert_eq!(age(now, now - 3 * 86400), "3d ago");
    }

    #[test]
    fn age_of_a_future_timestamp_is_just_now() {
        assert_eq!(age(100, 500), "just now");
    }

    #[test]
    fn plural_forms() {
        assert_eq!(plural(1), "");
        assert_eq!(plural(0), "s");
        assert_eq!(plural(2), "s");
    }
}
