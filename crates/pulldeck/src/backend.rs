//! The QObject that bridges the core crate to QML.
//!
//! `PrBackend` is a `QAbstractListModel`: QML binds a `ListView` straight to it and a refresh
//! only inserts, removes or updates the rows that changed (see `pulldeck_core::store::diff`).

use crate::inbox::InboxModelRust;
use crate::reviews::ReviewModelRust;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use pulldeck_core::session::ClientCommand;
use pulldeck_core::store::{self, Change};
use pulldeck_core::{GitHubClient, PullRequest};
use std::collections::HashSet;
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
        /// Whether the user chose to tell others which PR they are viewing. Off by default; change
        /// it with `setPresenceEnabled`, which also saves it.
        #[qproperty(bool, presence)]
        /// Whether the server can read your pull requests: "ok", "needsSignin" (sign in again; the
        /// list is stale) or "notInstalled" (the GitHub App is not installed anywhere you can see).
        #[qproperty(QString, sync_status, cxx_name = "syncStatus")]
        /// Where to install the GitHub App when `syncStatus` is "notInstalled"; may be empty.
        #[qproperty(QString, install_url, cxx_name = "installUrl")]
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

        /// Fetch the user's open PRs from GitHub in the background. Works with or without a
        /// server session; the results are merged with what the server tracks.
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

        /// Turn presence on or off and remember the choice. Turning it off also tells the server
        /// we are no longer viewing anything.
        #[qinvokable]
        #[cxx_name = "setPresenceEnabled"]
        fn set_presence_enabled(self: Pin<&mut Self>, enabled: bool);

        /// The user opened this PR. Sent to the server only if presence is on.
        #[qinvokable]
        fn viewing(self: Pin<&mut Self>, repo: &QString, number: i64);

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

        /// Give the backend the model that lists pull requests waiting for the user's review.
        ///
        /// # Safety
        ///
        /// `reviews` must be null or point to a live `ReviewModel`.
        #[qinvokable]
        #[cxx_name = "attachReviews"]
        unsafe fn attach_reviews(self: Pin<&mut PrBackend>, reviews: *mut ReviewModel);
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

    // Pull requests waiting for the user's review; state and behaviour in `reviews.rs`.
    impl cxx_qt::Threading for ReviewModel {}

    unsafe extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        #[qproperty(i32, count)]
        #[namespace = "pulldeck"]
        type ReviewModel = super::ReviewModelRust;

        #[qinvokable]
        #[cxx_override]
        fn data(self: &ReviewModel, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &ReviewModel) -> QHash_i32_QByteArray;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &ReviewModel, parent: &QModelIndex) -> i32;

        /// Wraps replacing the whole list.
        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn review_begin_reset_model(self: Pin<&mut ReviewModel>);
        /// Pairs with `review_begin_reset_model`.
        #[inherit]
        #[cxx_name = "endResetModel"]
        fn review_end_reset_model(self: Pin<&mut ReviewModel>);
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
const SERVER_BACKED: i32 = 0x109;

/// Backing state for [`qobject::PrBackend`].
pub struct PrBackendRust {
    status: QString,
    user: QString,
    loading: bool,
    connection: QString,
    server_configured: bool,
    presence: bool,
    sync_status: QString,
    install_url: QString,
    auth: QString,
    user_code: QString,
    verification_uri: QString,
    auth_error: QString,
    server_url: QString,
    /// The sign-in in progress, if any.
    pub(crate) login_task: Option<tokio::task::AbortHandle>,
    /// The running session's tasks, stopped on sign-out.
    pub(crate) session_tasks: Vec<tokio::task::AbortHandle>,
    /// What is shown: `server` and `polled` merged (see `store::merge`).
    prs: Vec<PullRequest>,
    /// Pull requests the server tracks. Empty without a session.
    server: Vec<PullRequest>,
    /// Pull requests found by polling GitHub directly, including ones the server cannot see.
    polled: Vec<PullRequest>,
    /// Which shown rows the server tracks; only those can take server actions.
    backed: HashSet<(String, u64)>,
    /// Whether to poll GitHub. Off for the demo and for a development server, so that fake
    /// accounts are not mixed with real pull requests.
    pub(crate) poll: bool,
    /// Commands for the server session, once one is running.
    pub(crate) commands: Option<tokio::sync::mpsc::Sender<ClientCommand>>,
    /// Where server notifications go. In an `Arc` because cxx-qt needs this struct to be `Unpin`
    /// and a bare `CxxQtThread` is not.
    pub(crate) inbox: Option<std::sync::Arc<cxx_qt::CxxQtThread<qobject::InboxModel>>>,
    /// Where pull requests awaiting the user's review go.
    pub(crate) reviews: Option<std::sync::Arc<cxx_qt::CxxQtThread<qobject::ReviewModel>>>,
}

impl Default for PrBackendRust {
    fn default() -> Self {
        Self {
            status: QString::from("Not refreshed yet"),
            user: QString::default(),
            loading: false,
            connection: QString::from("offline"),
            server_configured: false,
            presence: false,
            sync_status: QString::from("ok"),
            install_url: QString::default(),
            auth: QString::from("signedOut"),
            user_code: QString::default(),
            verification_uri: QString::default(),
            auth_error: QString::default(),
            server_url: QString::default(),
            login_task: None,
            session_tasks: Vec::new(),
            prs: Vec::new(),
            server: Vec::new(),
            polled: Vec::new(),
            backed: HashSet::new(),
            poll: true,
            commands: None,
            inbox: None,
            reviews: None,
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

/// Everything the UI needs from one fetch, already stringly-typed. The two searches are
/// independent: GitHub failing one (its search times out at times) must not hide the other.
struct Fetched {
    authored: Result<(String, Vec<PullRequest>), String>,
    /// `None` when that search failed, so the last list stays.
    reviews: Option<Vec<PullRequest>>,
}

async fn fetch() -> Fetched {
    let client = match GitHubClient::from_environment().await {
        Ok(client) => client,
        Err(e) => {
            return Fetched {
                authored: Err(e.to_string()),
                reviews: None,
            }
        }
    };
    let (authored, reviews) = tokio::join!(client.authored_prs(), client.review_requests());
    Fetched {
        authored: authored.map_err(|e| e.to_string()),
        reviews: reviews
            .map_err(|error| tracing::warn!(%error, "could not fetch review requests"))
            .ok(),
    }
}

impl qobject::PrBackend {
    /// Starts a background fetch; no-op if one is already running.
    pub fn refresh(mut self: Pin<&mut Self>) {
        // Polling works with or without a server: the server only sees repositories its GitHub
        // App is installed on, and gives live updates for those.
        if *self.loading() || !self.poll {
            return;
        }
        self.as_mut().set_loading(true);
        self.as_mut().set_status(QString::from("Refreshing…"));

        let qt_thread = self.qt_thread();
        runtime().spawn(async move {
            let fetched = fetch().await;
            let queued = qt_thread.queue(move |mut backend| {
                if let (Some(model), Some(reviews)) = (&backend.reviews, fetched.reviews) {
                    model.queue(move |mut m| m.as_mut().replace(reviews)).ok();
                }
                match fetched.authored {
                    Ok((login, prs)) => {
                        // A server session knows who you are better than a token on this machine.
                        if !*backend.server_configured() {
                            backend.as_mut().set_user(QString::from(login.as_str()));
                        }
                        backend.as_mut().replace_polled(prs);
                        let status = open_prs_status(backend.pr_count());
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

    /// Stores a fresh poll of GitHub and updates what is shown.
    fn replace_polled(mut self: Pin<&mut Self>, polled: Vec<PullRequest>) {
        self.as_mut().rust_mut().polled = polled;
        self.rebuild();
    }

    /// Applies row changes from the server session to the server's list and updates what is
    /// shown. The changes are relative to the server's list, not to what is displayed.
    pub(crate) fn apply_server_changes(mut self: Pin<&mut Self>, changes: Vec<Change>) {
        for change in &changes {
            change.apply(&mut self.as_mut().rust_mut().server);
        }
        self.rebuild();
    }

    /// Forgets everything the server told us (sign-out); polled pull requests stay.
    pub(crate) fn clear_server_prs(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().server.clear();
        self.rebuild();
    }

    /// Recomputes the merged list and tells attached views about each inserted, removed or
    /// updated row instead of resetting the whole model (so selection and scroll survive).
    fn rebuild(mut self: Pin<&mut Self>) {
        let merged = store::merge(&self.server, &self.polled);
        let backed: HashSet<_> = self
            .server
            .iter()
            .map(|p| (p.repo.clone(), p.number))
            .collect();
        let previously_backed = std::mem::replace(&mut self.as_mut().rust_mut().backed, backed);
        let changes = store::diff(&self.prs, &merged);
        tracing::debug!(
            rows = merged.len(),
            changes = changes.len(),
            "applying list update"
        );
        self.as_mut().apply_changes(changes);

        // A row can move between "server tracks it" and "only polled" with no change to the
        // pull request itself, which the diff cannot see.
        let parent = QModelIndex::default();
        for row in 0..self.prs.len() {
            let key = (self.prs[row].repo.clone(), self.prs[row].number);
            if previously_backed.contains(&key) != self.backed.contains(&key) {
                let index = self.index(row as i32, 0, &parent);
                self.as_mut()
                    .data_changed(&index, &index, &Default::default());
            }
        }
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
            (SERVER_BACKED, "serverBacked"),
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
            SERVER_BACKED => QVariant::from(&self.backed.contains(&(pr.repo.clone(), pr.number))),
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
