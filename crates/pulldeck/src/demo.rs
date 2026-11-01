//! A scripted fake server, for developing and demonstrating the collaboration UI without a real
//! one. Not part of normal builds: compile with `--features demo` and run with `PULLDECK_DEMO=1`
//! (or `just demo`). It drives the real session, glue and models end to end.

use crate::backend::{qobject, runtime};
use chrono::{Duration as ChronoDuration, Utc};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use pulldeck_core::remote::{Connector, Transport, TransportError};
use pulldeck_core::session::{
    ClientCommand, Notification, NotificationKind, PrRef, ServerEvent, SessionToken,
};
use pulldeck_core::{ChecksStatus, PullRequest, ReviewStatus};
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;

/// Starts a session against the scripted server.
pub(crate) fn start(mut backend: Pin<&mut qobject::PrBackend>) {
    let inbox = backend.inbox.clone();
    let ui = backend.qt_thread();
    // Only the scripted server's pull requests: not whatever `gh` can see.
    backend.as_mut().rust_mut().poll = false;
    backend
        .as_mut()
        .start_session(DemoConnector::default(), SessionToken::new("demo"));

    // Play the part of the user a few seconds in, through the same entry points QML uses: read
    // the first notification (which tells the server), then ask for a review.
    runtime().spawn(async move {
        sleep(Duration::from_secs(4)).await;
        if let Some(inbox) = inbox {
            let _ = inbox.queue(|mut inbox| inbox.as_mut().ack(1));
        }
        sleep(Duration::from_millis(500)).await;
        let _ = ui.queue(|backend| {
            backend.request_review(&QString::from("demo/app"), 3, &QString::from("hubot"));
        });
    });
}

fn pr(
    number: u64,
    title: &str,
    review: ReviewStatus,
    checks: ChecksStatus,
    minutes_ago: i64,
) -> PullRequest {
    PullRequest {
        number,
        title: title.into(),
        url: format!("https://github.com/demo/app/pull/{number}"),
        repo: "demo/app".into(),
        author: "demo-user".into(),
        is_draft: false,
        review,
        checks,
        updated_at: Utc::now() - ChronoDuration::minutes(minutes_ago),
    }
}

fn pr_ref(number: u64) -> PrRef {
    PrRef {
        repo: "demo/app".into(),
        number,
    }
}

fn notification(
    seq: u64,
    id: u64,
    from: &str,
    kind: NotificationKind,
    number: u64,
    note: Option<&str>,
) -> ServerEvent {
    ServerEvent::Notification(Notification {
        seq,
        id,
        from: from.into(),
        kind,
        pr: pr_ref(number),
        note: note.map(str::to_owned),
        created_at: Utc::now(),
    })
}

#[derive(Default)]
struct DemoConnector {
    used: bool,
}

impl Connector for DemoConnector {
    type Transport = DemoTransport;

    async fn connect(&mut self) -> Result<DemoTransport, TransportError> {
        if std::mem::replace(&mut self.used, true) {
            return Err(TransportError::Io(
                "the demo server accepts one connection".into(),
            ));
        }
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        tokio::spawn(script(events_tx, commands_rx));
        Ok(DemoTransport {
            incoming: events_rx,
            outgoing: commands_tx,
        })
    }
}

struct DemoTransport {
    incoming: mpsc::UnboundedReceiver<ServerEvent>,
    outgoing: mpsc::UnboundedSender<ClientCommand>,
}

impl Transport for DemoTransport {
    async fn send(&mut self, command: ClientCommand) -> Result<(), TransportError> {
        self.outgoing
            .send(command)
            .map_err(|_| TransportError::Closed)
    }

    async fn recv(&mut self) -> Result<ServerEvent, TransportError> {
        self.incoming.recv().await.ok_or(TransportError::Closed)
    }
}

/// The fake server's side: say hello, push a snapshot, trickle in changes and notifications, and
/// accept whatever the UI asks for.
async fn script(
    events: mpsc::UnboundedSender<ServerEvent>,
    mut commands: mpsc::UnboundedReceiver<ClientCommand>,
) {
    let say = |event| events.send(event).is_ok();

    // Wait for the client's hello before answering, like the real server.
    if !matches!(commands.recv().await, Some(ClientCommand::Hello { .. })) {
        return;
    }
    if !say(ServerEvent::Hello {
        login: "demo-user".into(),
    }) {
        return;
    }
    say(ServerEvent::Snapshot {
        seq: 1,
        prs: vec![
            pr(
                3,
                "Add retry to the sync job",
                ReviewStatus::ReviewRequired,
                ChecksStatus::Pending,
                5,
            ),
            pr(
                2,
                "Fix flaky login test",
                ReviewStatus::ChangesRequested,
                ChecksStatus::Failure,
                40,
            ),
            pr(
                1,
                "Bump dependencies",
                ReviewStatus::None,
                ChecksStatus::Success,
                300,
            ),
        ],
    });

    // Later events arrive on a timer while we also answer commands.
    let timeline = async {
        sleep(Duration::from_millis(1500)).await;
        say(notification(
            2,
            1,
            "hubot",
            NotificationKind::Nudge,
            3,
            Some("Could you look at this today?"),
        ));
        sleep(Duration::from_millis(1000)).await;
        say(ServerEvent::PrUpsert {
            seq: 3,
            pr: pr(
                2,
                "Fix flaky login test",
                ReviewStatus::Approved,
                ChecksStatus::Success,
                0,
            ),
        });
        sleep(Duration::from_millis(1000)).await;
        say(notification(
            4,
            2,
            "mona",
            NotificationKind::ReviewRequested,
            1,
            None,
        ));
        sleep(Duration::from_millis(1000)).await;
        say(ServerEvent::PrRemove {
            seq: 5,
            pr: pr_ref(1),
        });
        // Redelivery of an already-seen notification: the inbox must ignore it.
        sleep(Duration::from_millis(500)).await;
        say(notification(
            2,
            1,
            "hubot",
            NotificationKind::Nudge,
            3,
            Some("Could you look at this today?"),
        ));
        std::future::pending::<()>().await;
    };
    tokio::pin!(timeline);

    loop {
        tokio::select! {
            () = &mut timeline => {}
            command = commands.recv() => match command {
                None => return,
                Some(command) => {
                    // Debug output is safe: the session token redacts itself.
                    tracing::debug!("demo server heard: {command:?}");
                    if let Some(request_id) = command.request_id() {
                        sleep(Duration::from_millis(300)).await;
                        say(ServerEvent::Accepted { request_id: request_id.to_owned() });
                    }
                }
            },
        }
    }
}
