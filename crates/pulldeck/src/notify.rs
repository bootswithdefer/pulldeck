//! Desktop notifications for things other users send.
//!
//! The inbox tab shows everything; this interrupts only for what is new. A server replays
//! unacknowledged items when a session starts, and announcing those again at every launch
//! would be noise, so only recent ones are shown.

use crate::backend::runtime;
use chrono::{DateTime, Duration, Utc};
use pulldeck_core::session::{Notification, NotificationKind};
use pulldeck_core::PullRequest;

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
    show(title, body);
}

/// Pull requests in `new` that were not in `old`. Nothing is new before a first poll has
/// succeeded (`old` is `None`), so launching the app does not announce every open request.
fn newly_requested<'a>(
    old: Option<&[PullRequest]>,
    new: &'a [PullRequest],
) -> Vec<&'a PullRequest> {
    let Some(old) = old else { return Vec::new() };
    new.iter()
        .filter(|n| !old.iter().any(|o| o.repo == n.repo && o.number == n.number))
        .collect()
}

/// The most review requests announced one by one; more than this arrive as a single summary.
const MAX_SEPARATE: usize = 3;

/// Titles and bodies of the notifications for newly requested reviews.
fn review_text(requests: &[&PullRequest]) -> Vec<(String, String)> {
    if requests.len() > MAX_SEPARATE {
        return vec![(
            format!("{} new review requests", requests.len()),
            String::new(),
        )];
    }
    requests
        .iter()
        .map(|pr| {
            (
                format!(
                    "{} asked you to review {}#{}",
                    pr.author, pr.repo, pr.number
                ),
                pr.title.clone(),
            )
        })
        .collect()
}

/// Announces review requests that appeared since `old` (see [`newly_requested`]).
pub(crate) fn announce_review_requests(old: Option<&[PullRequest]>, new: &[PullRequest]) {
    for (title, body) in review_text(&newly_requested(old, new)) {
        show(title, body);
    }
}

/// Shows one desktop notification unless the user turned them off.
fn show(title: String, body: String) {
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

    fn request(repo: &str, number: u64) -> PullRequest {
        PullRequest {
            number,
            title: "Fix it".into(),
            url: String::new(),
            repo: repo.into(),
            author: "hubot".into(),
            is_draft: false,
            review: pulldeck_core::ReviewStatus::None,
            checks: pulldeck_core::ChecksStatus::None,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn the_first_poll_announces_nothing() {
        let all = [request("o/r", 1), request("o/r", 2)];
        assert!(newly_requested(None, &all).is_empty());
    }

    #[test]
    fn only_requests_absent_from_the_last_poll_are_new() {
        let old = [request("o/r", 1), request("o/r", 2)];
        let new = [request("o/r", 3), request("o/r", 1), request("o/other", 2)];
        let found = newly_requested(Some(&old), &new);
        assert_eq!(found, vec![&new[0], &new[2]]);
    }

    #[test]
    fn a_few_requests_are_announced_one_by_one_and_many_as_one_summary() {
        let prs: Vec<_> = (1..=4).map(|n| request("o/r", n)).collect();
        let few: Vec<_> = prs[..2].iter().collect();
        let texts = review_text(&few);
        assert_eq!(texts.len(), 2);
        assert_eq!(
            texts[0],
            ("hubot asked you to review o/r#1".into(), "Fix it".into())
        );
        let many: Vec<_> = prs.iter().collect();
        assert_eq!(
            review_text(&many),
            vec![("4 new review requests".into(), String::new())]
        );
    }

    #[test]
    fn a_missing_note_gives_an_empty_body() {
        let mut n = notification(NotificationKind::Message, Duration::zero());
        n.note = None;
        assert_eq!(text(&n).1, "");
    }
}
