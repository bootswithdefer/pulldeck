//! Connects to a pulldeck server with the real transport and prints what it says, for trying the
//! client against a (dev) server without the UI.
//!
//! ```text
//! PULLDECK_TOKEN=<session token> cargo run -p pulldeck-core --example live -- \
//!     http://127.0.0.1:8080 [--seconds 5] [--nudge hubot dev/pulldeck 1]
//! ```
//!
//! The token comes from the environment, not an argument, so it does not show up in the process
//! list. It is never printed.

use pulldeck_core::remote::{ConnState, RemoteSession, SessionUpdate};
use pulldeck_core::session::{ClientCommand, PrRef, SessionToken};
use pulldeck_core::settings::ServerUrl;
use pulldeck_core::ws::WsConnector;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{timeout_at, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let server = ServerUrl::parse(&args.next().ok_or("usage: live <server url> [options]")?)?;
    let token =
        SessionToken::new(std::env::var("PULLDECK_TOKEN").map_err(|_| "set PULLDECK_TOKEN")?);

    let mut seconds = 5;
    let mut nudge: Option<ClientCommand> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" => seconds = args.next().ok_or("--seconds needs a value")?.parse()?,
            "--nudge" => {
                let to = args
                    .next()
                    .ok_or("--nudge needs: <login> <repo> <number>")?;
                let repo = args
                    .next()
                    .ok_or("--nudge needs: <login> <repo> <number>")?;
                let number = args
                    .next()
                    .ok_or("--nudge needs: <login> <repo> <number>")?;
                nudge = Some(ClientCommand::Nudge {
                    request_id: "live-example-1".into(),
                    pr: PrRef {
                        repo,
                        number: number.parse()?,
                    },
                    to,
                    note: Some("hello from the live example".into()),
                });
            }
            other => return Err(format!("unknown option {other}").into()),
        }
    }

    let (command_tx, command_rx) = mpsc::channel(8);
    let (update_tx, mut updates) = mpsc::channel(64);
    tokio::spawn(RemoteSession::new(WsConnector::new(&server), token).run(command_rx, update_tx));

    let deadline = Instant::now() + Duration::from_secs(seconds);
    while let Ok(Some(update)) = timeout_at(deadline, updates.recv()).await {
        match update {
            SessionUpdate::State(state) => {
                println!("state: {state:?}");
                if state == ConnState::Live {
                    if let Some(command) = nudge.take() {
                        command_tx.send(command).await?;
                        println!("sent a nudge");
                    }
                }
                if let ConnState::Failed(reason) = state {
                    return Err(format!("failed: {reason}").into());
                }
            }
            SessionUpdate::Login(login) => println!("signed in as {login}"),
            SessionUpdate::Changes(changes) => {
                println!("{} row change(s): {changes:?}", changes.len())
            }
            SessionUpdate::Notification(n) => println!(
                "notification #{} ({:?}) from {} about {}#{}: {}",
                n.id,
                n.kind,
                n.from,
                n.pr.repo,
                n.pr.number,
                n.note.as_deref().unwrap_or("(no note)")
            ),
            SessionUpdate::ActionResult { request_id, result } => {
                println!("action {request_id}: {result:?}");
            }
            SessionUpdate::Sync {
                status,
                install_url,
            } => println!("sync: {status:?} {}", install_url.unwrap_or_default()),
            SessionUpdate::Presence { login, online, .. } => {
                println!("presence: {login} online={online}");
            }
        }
    }
    Ok(())
}
