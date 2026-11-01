//! The client's replica of what the server knows about the user: the PR list, the notification
//! inbox and the resume point.
//!
//! Everything here is pure and synchronous, so it is tested without sockets, timers or Qt. The
//! types are *domain* types, deliberately independent of the wire format: a transport converts
//! between its protocol messages and [`ServerEvent`] / [`ClientCommand`].

use crate::store::{self, Change};
use crate::PullRequest;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::fmt;

/// Opaque credential issued by the server after sign-in. It is not a GitHub token.
///
/// `Debug` is redacted so the token cannot leak through logs or panics, and it is deliberately
/// not `Display`. Use [`SessionToken::expose`] only to send it or store it in the keyring.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(String);

impl SessionToken {
    /// Wraps a raw token.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The raw token.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionToken(<redacted>)")
    }
}

/// Identifies a pull request independent of its current contents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrRef {
    /// `owner/name` of the repository.
    pub repo: String,
    /// PR number within the repository.
    pub number: u64,
}

/// Why a notification was sent. `Other` covers kinds a newer server adds, so an older client can
/// still show (and acknowledge) them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    /// Someone asked this user to review a PR.
    ReviewRequested,
    /// A reminder about a PR.
    Nudge,
    /// A free-form in-app message about a PR.
    Message,
    /// A kind this client does not know.
    Other,
}

/// Something another user sent to this user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// Per-user event sequence number it was delivered with.
    pub seq: u64,
    /// Globally unique id, used to acknowledge and to drop redeliveries.
    pub id: u64,
    /// Login of the sender.
    pub from: String,
    /// Why it was sent.
    pub kind: NotificationKind,
    /// The PR it is about.
    pub pr: PrRef,
    /// Optional text from the sender.
    pub note: Option<String>,
    /// When the server recorded it.
    pub created_at: DateTime<Utc>,
}

/// What the server tells the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    /// The server accepted the session.
    Hello {
        /// GitHub login the session belongs to.
        login: String,
    },
    /// The complete PR list; replaces whatever the client had.
    Snapshot {
        /// Sequence number of this event.
        seq: u64,
        /// The PRs, in the server's order.
        prs: Vec<PullRequest>,
    },
    /// A PR was added or changed.
    PrUpsert {
        /// Sequence number of this event.
        seq: u64,
        /// The PR's new contents.
        pr: PullRequest,
    },
    /// A PR is no longer relevant to this user.
    PrRemove {
        /// Sequence number of this event.
        seq: u64,
        /// The PR that went away.
        pr: PrRef,
    },
    /// A notification for the user (carries its own `seq`).
    Notification(Notification),
    /// The action with this request id was applied.
    Accepted {
        /// Id the client chose for the action.
        request_id: String,
    },
    /// The action with this request id was refused.
    Rejected {
        /// Id the client chose for the action.
        request_id: String,
        /// Why.
        reason: String,
    },
    /// Another user's presence changed. Ephemeral.
    Presence {
        /// Their login.
        login: String,
        /// Whether they are connected.
        online: bool,
        /// The PR they are looking at, if any.
        viewing: Option<PrRef>,
    },
    /// Whether the server can currently read this user's pull requests. Ephemeral.
    Status {
        /// The state.
        sync: SyncStatus,
        /// Where to install the GitHub App, for [`SyncStatus::NotInstalled`].
        install_url: Option<String>,
    },
    /// The server is closing the connection.
    Error {
        /// Why.
        message: String,
    },
}

/// Whether the server can read the user's pull requests from GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncStatus {
    /// Working. Assumed until the server says otherwise.
    #[default]
    Ok,
    /// GitHub no longer accepts the user's credentials; the list is stale until they sign in again.
    NeedsSignin,
    /// The GitHub App is installed nowhere the user can see, so there is nothing to read.
    NotInstalled,
}

/// What the client tells the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientCommand {
    /// Authenticate, optionally resuming after the last applied event.
    Hello {
        /// The session credential.
        token: SessionToken,
        /// Last sequence number applied, if resuming.
        last_seq: Option<u64>,
    },
    /// Ask the server to request a review from `to` on GitHub.
    RequestReview {
        /// Client-chosen id that makes retries safe.
        request_id: String,
        /// The PR.
        pr: PrRef,
        /// GitHub login of the reviewer.
        to: String,
    },
    /// Send a record-only reminder.
    Nudge {
        /// Client-chosen id that makes retries safe.
        request_id: String,
        /// The PR.
        pr: PrRef,
        /// Recipient login.
        to: String,
        /// Optional text.
        note: Option<String>,
    },
    /// Send a record-only in-app message.
    Message {
        /// Client-chosen id that makes retries safe.
        request_id: String,
        /// The PR.
        pr: PrRef,
        /// Recipient login.
        to: String,
        /// The text.
        body: String,
    },
    /// The user has seen this notification.
    AckNotification {
        /// Notification id.
        id: u64,
    },
    /// Which PR the user is looking at. Only ever sent if the user turned presence on.
    Presence {
        /// The PR, if any.
        viewing: Option<PrRef>,
    },
}

impl ClientCommand {
    /// The retry id of an action that gets an accepted/rejected answer.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::RequestReview { request_id, .. }
            | Self::Nudge { request_id, .. }
            | Self::Message { request_id, .. } => Some(request_id),
            _ => None,
        }
    }
}

/// Notifications received so far, with which ones the user has acknowledged.
#[derive(Debug, Default)]
pub struct Inbox {
    items: Vec<Notification>,
    acked: HashSet<u64>,
}

impl Inbox {
    /// Adds a notification. Returns `false` (and changes nothing) if its id was already seen,
    /// which is how redeliveries are dropped.
    pub fn insert(&mut self, notification: Notification) -> bool {
        if self.items.iter().any(|n| n.id == notification.id) {
            return false;
        }
        self.items.push(notification);
        true
    }

    /// Marks a notification acknowledged. Returns `true` if it exists and was not already.
    pub fn ack(&mut self, id: u64) -> bool {
        self.items.iter().any(|n| n.id == id) && self.acked.insert(id)
    }

    /// All notifications, oldest delivery first.
    pub fn items(&self) -> &[Notification] {
        &self.items
    }

    /// Row of the notification with this id when the inbox is shown newest first.
    pub fn row_of(&self, id: u64) -> Option<usize> {
        self.items
            .iter()
            .position(|n| n.id == id)
            .map(|position| self.items.len() - 1 - position)
    }

    /// The notification at `row` when the inbox is shown newest first.
    pub fn at_row(&self, row: usize) -> Option<&Notification> {
        self.items
            .len()
            .checked_sub(1 + row)
            .and_then(|position| self.items.get(position))
    }

    /// Whether the user has acknowledged this notification.
    pub fn is_acked(&self, id: u64) -> bool {
        self.acked.contains(&id)
    }

    /// How many notifications are still unacknowledged.
    pub fn unacked(&self) -> usize {
        self.items
            .iter()
            .filter(|n| !self.acked.contains(&n.id))
            .count()
    }
}

/// What applying one event changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Update {
    /// Row changes for the PR list, in the order to apply them.
    pub changes: Vec<Change>,
    /// A notification that was not seen before.
    pub notification: Option<Notification>,
}

/// The PR list, inbox and resume point, kept in step with the server.
#[derive(Debug, Default)]
pub struct Synced {
    prs: Vec<PullRequest>,
    inbox: Inbox,
    last_seq: Option<u64>,
}

impl Synced {
    /// The current PR list, in display order.
    pub fn prs(&self) -> &[PullRequest] {
        &self.prs
    }

    /// The notification inbox.
    pub fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    /// Mutable access to the inbox, for local acknowledgements.
    pub fn inbox_mut(&mut self) -> &mut Inbox {
        &mut self.inbox
    }

    /// The highest sequence number applied; sent in `hello` to resume after a reconnect.
    pub fn last_seq(&self) -> Option<u64> {
        self.last_seq
    }

    /// Applies one server event and reports what changed. Events that are not part of the
    /// replicated state (accepted, rejected, presence, ...) change nothing.
    ///
    /// - A snapshot always replaces the list and sets the resume point.
    /// - PR deltas at or below the resume point are stale or duplicated and are ignored.
    /// - Notifications are deduplicated by id, not by sequence: after a fresh snapshot the server
    ///   re-sends older unacknowledged ones, whose sequence numbers are below the snapshot's.
    pub fn apply(&mut self, event: ServerEvent) -> Update {
        match event {
            ServerEvent::Snapshot { seq, prs } => {
                self.last_seq = Some(seq);
                self.replace(prs)
            }
            ServerEvent::PrUpsert { seq, pr } if self.is_newer(seq) => {
                self.last_seq = Some(seq);
                let mut next = self.prs.clone();
                apply_upsert(&mut next, pr);
                self.replace(next)
            }
            ServerEvent::PrRemove { seq, pr } if self.is_newer(seq) => {
                self.last_seq = Some(seq);
                let mut next = self.prs.clone();
                apply_remove(&mut next, &pr.repo, pr.number);
                self.replace(next)
            }
            ServerEvent::Notification(notification) => {
                self.last_seq = Some(
                    self.last_seq
                        .map_or(notification.seq, |s| s.max(notification.seq)),
                );
                let fresh = self.inbox.insert(notification.clone());
                Update {
                    changes: Vec::new(),
                    notification: fresh.then_some(notification),
                }
            }
            _ => Update::default(),
        }
    }

    fn is_newer(&self, seq: u64) -> bool {
        self.last_seq.is_none_or(|last| seq > last)
    }

    fn replace(&mut self, next: Vec<PullRequest>) -> Update {
        let changes = store::diff(&self.prs, &next);
        self.prs = next;
        Update {
            changes,
            notification: None,
        }
    }
}

/// Adds `pr` to `list`, or replaces the entry for the same repo and number, then restores the
/// server's order: most recently updated first. The sort is stable, so ties keep their order.
pub fn apply_upsert(list: &mut Vec<PullRequest>, pr: PullRequest) {
    match list
        .iter_mut()
        .find(|have| have.repo == pr.repo && have.number == pr.number)
    {
        Some(slot) => *slot = pr,
        None => list.push(pr),
    }
    list.sort_by_key(|pr| std::cmp::Reverse(pr.updated_at));
}

/// Removes the PR with this repo and number, if present.
pub fn apply_remove(list: &mut Vec<PullRequest>, repo: &str, number: u64) {
    list.retain(|pr| !(pr.repo == repo && pr.number == number));
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::{ChecksStatus, ReviewStatus};

    /// A PR in `o/r`; `minute` is its update time, so larger means more recent.
    pub fn pr(number: u64, minute: u32) -> PullRequest {
        PullRequest {
            number,
            title: format!("PR {number}"),
            url: format!("https://github.com/o/r/pull/{number}"),
            repo: "o/r".into(),
            author: "octocat".into(),
            is_draft: false,
            review: ReviewStatus::None,
            checks: ChecksStatus::None,
            updated_at: format!("2026-10-01T12:{minute:02}:00Z").parse().unwrap(),
        }
    }

    pub fn pr_ref(number: u64) -> PrRef {
        PrRef {
            repo: "o/r".into(),
            number,
        }
    }

    pub fn notification(seq: u64, id: u64) -> Notification {
        Notification {
            seq,
            id,
            from: "hubot".into(),
            kind: NotificationKind::Nudge,
            pr: pr_ref(7),
            note: None,
            created_at: "2026-10-01T12:00:00Z".parse().unwrap(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{notification, pr, pr_ref};
    use super::*;

    fn numbers(synced: &Synced) -> Vec<u64> {
        synced.prs().iter().map(|p| p.number).collect()
    }

    #[test]
    fn token_debug_never_shows_the_secret() {
        let token = SessionToken::new("s3cret-value");
        assert_eq!(format!("{token:?}"), "SessionToken(<redacted>)");
        assert!(!format!("{token:#?}").contains("s3cret"));
        assert!(!format!(
            "{:?}",
            ClientCommand::Hello {
                token: token.clone(),
                last_seq: None
            }
        )
        .contains("s3cret"));
        assert_eq!(token.expose(), "s3cret-value");
    }

    #[test]
    fn snapshot_replaces_the_list_and_sets_the_resume_point() {
        let mut s = Synced::default();
        let update = s.apply(ServerEvent::Snapshot {
            seq: 3,
            prs: vec![pr(1, 5), pr(2, 4)],
        });
        assert_eq!(update.changes.len(), 2);
        assert_eq!(numbers(&s), [1, 2]);
        assert_eq!(s.last_seq(), Some(3));
    }

    #[test]
    fn upserts_keep_most_recently_updated_first() {
        let mut s = Synced::default();
        s.apply(ServerEvent::Snapshot {
            seq: 1,
            prs: vec![pr(1, 5), pr(2, 4)],
        });
        // PR 2 gets newer activity and jumps to the top; PR 3 is brand new and newest of all.
        s.apply(ServerEvent::PrUpsert {
            seq: 2,
            pr: pr(2, 9),
        });
        assert_eq!(numbers(&s), [2, 1]);
        s.apply(ServerEvent::PrUpsert {
            seq: 3,
            pr: pr(3, 10),
        });
        assert_eq!(numbers(&s), [3, 2, 1]);
    }

    #[test]
    fn removal_drops_just_that_pr() {
        let mut s = Synced::default();
        s.apply(ServerEvent::Snapshot {
            seq: 1,
            prs: vec![pr(1, 5), pr(2, 4)],
        });
        let update = s.apply(ServerEvent::PrRemove {
            seq: 2,
            pr: pr_ref(1),
        });
        assert_eq!(update.changes, vec![Change::Remove(0)]);
        assert_eq!(numbers(&s), [2]);
    }

    #[test]
    fn stale_and_duplicate_deltas_are_ignored() {
        let mut s = Synced::default();
        s.apply(ServerEvent::Snapshot {
            seq: 5,
            prs: vec![pr(1, 5)],
        });
        for seq in [5, 4, 1] {
            let update = s.apply(ServerEvent::PrUpsert { seq, pr: pr(9, 9) });
            assert_eq!(update, Update::default(), "seq {seq} must be ignored");
        }
        assert_eq!(numbers(&s), [1]);
        assert_eq!(s.last_seq(), Some(5));
    }

    #[test]
    fn an_unchanged_upsert_produces_no_row_changes() {
        let mut s = Synced::default();
        s.apply(ServerEvent::Snapshot {
            seq: 1,
            prs: vec![pr(1, 5), pr(2, 4)],
        });
        let update = s.apply(ServerEvent::PrUpsert {
            seq: 2,
            pr: pr(2, 4),
        });
        assert!(update.changes.is_empty());
    }

    #[test]
    fn a_new_snapshot_reports_only_what_differs() {
        let mut s = Synced::default();
        s.apply(ServerEvent::Snapshot {
            seq: 1,
            prs: vec![pr(1, 5), pr(2, 4)],
        });
        // After a reconnect the server resends everything; only PR 2 changed.
        let mut changed = pr(2, 4);
        changed.title = "renamed".into();
        let update = s.apply(ServerEvent::Snapshot {
            seq: 9,
            prs: vec![pr(1, 5), changed.clone()],
        });
        assert_eq!(update.changes, vec![Change::Update(1, changed)]);
        assert_eq!(s.last_seq(), Some(9));
    }

    #[test]
    fn snapshot_plus_deltas_equals_the_final_list_for_every_small_case() {
        // Start from every ordered list of up to 3 ids, apply each delta, and compare with
        // building the expected list directly. Covers insert, replace and remove together.
        fn lists() -> Vec<Vec<u64>> {
            let mut out = vec![vec![]];
            for a in 1..=3 {
                out.push(vec![a]);
                for b in 1..=3 {
                    if b != a {
                        out.push(vec![a, b]);
                    }
                }
            }
            out
        }
        for start in lists() {
            let prs: Vec<_> = start
                .iter()
                .enumerate()
                .map(|(i, &n)| pr(n, 30 - i as u32))
                .collect();
            for id in 1..=3 {
                let mut s = Synced::default();
                s.apply(ServerEvent::Snapshot {
                    seq: 1,
                    prs: prs.clone(),
                });
                let mut expected = prs.clone();
                let newest = pr(id, 40);

                s.apply(ServerEvent::PrUpsert {
                    seq: 2,
                    pr: newest.clone(),
                });
                apply_upsert(&mut expected, newest);
                assert_eq!(s.prs(), expected, "upsert {id} into {start:?}");

                s.apply(ServerEvent::PrRemove {
                    seq: 3,
                    pr: pr_ref(id),
                });
                apply_remove(&mut expected, "o/r", id);
                assert_eq!(s.prs(), expected, "remove {id} from {start:?}");
            }
        }
    }

    #[test]
    fn notifications_are_deduplicated_by_id() {
        let mut s = Synced::default();
        let first = s.apply(ServerEvent::Notification(notification(4, 100)));
        assert_eq!(first.notification.as_ref().map(|n| n.id), Some(100));
        let again = s.apply(ServerEvent::Notification(notification(4, 100)));
        assert_eq!(again, Update::default());
        assert_eq!(s.inbox().items().len(), 1);
    }

    #[test]
    fn older_unacked_notifications_arrive_after_a_fresh_snapshot() {
        // A fresh hello gets the snapshot (seq 50), then unacked notifications with the
        // sequence numbers they were created with, which are lower. They must still be kept.
        let mut s = Synced::default();
        s.apply(ServerEvent::Snapshot {
            seq: 50,
            prs: vec![],
        });
        let update = s.apply(ServerEvent::Notification(notification(12, 7)));
        assert!(update.notification.is_some());
        assert_eq!(
            s.last_seq(),
            Some(50),
            "an older notification must not lower the resume point"
        );
    }

    #[test]
    fn notifications_advance_the_resume_point() {
        let mut s = Synced::default();
        s.apply(ServerEvent::Snapshot {
            seq: 3,
            prs: vec![],
        });
        s.apply(ServerEvent::Notification(notification(8, 1)));
        assert_eq!(s.last_seq(), Some(8));
        // The shared counter means a PR delta below it is stale.
        assert_eq!(
            s.apply(ServerEvent::PrUpsert {
                seq: 7,
                pr: pr(1, 1)
            }),
            Update::default()
        );
    }

    #[test]
    fn acking_tracks_unread_counts() {
        let mut inbox = Inbox::default();
        inbox.insert(notification(1, 10).clone());
        inbox.insert(notification(2, 11));
        assert_eq!(inbox.unacked(), 2);
        assert!(inbox.ack(10));
        assert!(!inbox.ack(10), "second ack is a no-op");
        assert!(!inbox.ack(999), "unknown id is a no-op");
        assert_eq!(
            (inbox.unacked(), inbox.is_acked(10), inbox.is_acked(11)),
            (1, true, false)
        );
    }

    #[test]
    fn rows_are_numbered_newest_first() {
        let mut inbox = Inbox::default();
        assert_eq!(inbox.at_row(0), None);
        for id in [10, 11, 12] {
            inbox.insert(notification(id, id));
        }
        // Arrival order was 10, 11, 12, so the newest (12) is row 0.
        let by_row: Vec<_> = (0..3).map(|r| inbox.at_row(r).map(|n| n.id)).collect();
        assert_eq!(by_row, [Some(12), Some(11), Some(10)]);
        assert_eq!(inbox.at_row(3), None);
        // row_of and at_row are inverses.
        for id in [10, 11, 12] {
            let row = inbox.row_of(id).unwrap();
            assert_eq!(inbox.at_row(row).map(|n| n.id), Some(id));
        }
        assert_eq!(inbox.row_of(99), None);
    }

    #[test]
    fn a_new_notification_always_lands_on_row_zero() {
        let mut inbox = Inbox::default();
        for id in 1..=4 {
            inbox.insert(notification(id, id));
            assert_eq!(inbox.row_of(id), Some(0));
        }
    }

    #[test]
    fn events_outside_the_replicated_state_change_nothing() {
        let mut s = Synced::default();
        for event in [
            ServerEvent::Hello { login: "me".into() },
            ServerEvent::Accepted {
                request_id: "r".into(),
            },
            ServerEvent::Rejected {
                request_id: "r".into(),
                reason: "no".into(),
            },
            ServerEvent::Presence {
                login: "x".into(),
                online: true,
                viewing: None,
            },
            ServerEvent::Error {
                message: "bye".into(),
            },
        ] {
            assert_eq!(s.apply(event), Update::default());
        }
        assert_eq!(s.last_seq(), None);
    }

    #[test]
    fn only_actions_have_request_ids() {
        let nudge = ClientCommand::Nudge {
            request_id: "abc".into(),
            pr: pr_ref(1),
            to: "x".into(),
            note: None,
        };
        assert_eq!(nudge.request_id(), Some("abc"));
        assert_eq!(ClientCommand::AckNotification { id: 1 }.request_id(), None);
        assert_eq!(ClientCommand::Presence { viewing: None }.request_id(), None);
    }
}
