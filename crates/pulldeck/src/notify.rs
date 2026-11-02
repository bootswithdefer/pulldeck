//! Desktop notifications for things other users send.
//!
//! The inbox tab shows everything; this interrupts only for what is new. A server replays
//! unacknowledged items when a session starts, and announcing those again at every launch
//! would be noise, so only recent ones are shown.

use crate::backend::runtime;
use chrono::{DateTime, Duration, Utc};
use pulldeck_core::session::{Notification, NotificationKind};

/// Older than this when it arrives, a notification is backlog rather than news.
const FRESH_FOR: Duration = Duration::minutes(5);

/// Whether `n` is recent enough to interrupt the user.
fn is_fresh(n: &Notification, now: DateTime<Utc>) -> bool {
    now - n.created_at <= FRESH_FOR
}

/// The title and body of the desktop notification for `n`.
fn text(n: &Notification) -> (String, String) {
    let what = match n.kind {
        NotificationKind::ReviewRequested => "asked you to review",
        NotificationKind::Nudge => "nudged you about",
        NotificationKind::Message => "wrote to you about",
        NotificationKind::Other => "sent you something about",
    };
    let title = format!("{} {what} {}#{}", n.from, n.pr.repo, n.pr.number);
    (title, n.note.clone().unwrap_or_default())
}

/// Shows a desktop notification for `n` if it is fresh and the user has not turned them off.
/// The settings read and the D-Bus call both block, so they run off the Qt thread.
pub(crate) fn announce(n: &Notification) {
    if !is_fresh(n, Utc::now()) {
        return;
    }
    let (title, body) = text(n);
    runtime().spawn_blocking(move || {
        if !crate::auth::load_settings().notifications {
            return;
        }
        let shown = notify_rust::Notification::new()
            .appname("pulldeck")
            .summary(&title)
            .body(&body)
            .show();
        // No notification daemon (a bare window manager, a container) is not worth more than a log.
        if let Err(error) = shown {
            tracing::warn!(%error, "could not show a desktop notification");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulldeck_core::session::PrRef;

    fn notification(kind: NotificationKind, age: Duration) -> Notification {
        Notification {
            seq: 1,
            id: 1,
            from: "hubot".into(),
            kind,
            pr: PrRef {
                repo: "o/r".into(),
                number: 7,
            },
            note: Some("please look".into()),
            created_at: Utc::now() - age,
        }
    }

    #[test]
    fn only_recent_notifications_interrupt() {
        let now = Utc::now();
        let new = notification(NotificationKind::Nudge, Duration::seconds(10));
        let old = notification(NotificationKind::Nudge, Duration::hours(3));
        assert!(is_fresh(&new, now));
        assert!(!is_fresh(&old, now));
    }

    #[test]
    fn the_text_names_who_what_and_where() {
        let n = notification(NotificationKind::ReviewRequested, Duration::zero());
        assert_eq!(
            text(&n),
            (
                "hubot asked you to review o/r#7".into(),
                "please look".into()
            )
        );
    }

    #[test]
    fn a_missing_note_gives_an_empty_body() {
        let mut n = notification(NotificationKind::Message, Duration::zero());
        n.note = None;
        assert_eq!(text(&n).1, "");
    }
}
