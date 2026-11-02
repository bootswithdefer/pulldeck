//! pulldeck: track your GitHub pull requests from a native Qt window.

mod auth;
mod backend;
#[cfg(feature = "demo")]
mod demo;
mod inbox;
mod notify;
mod pr_list;
mod server;
mod tray;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};
use tracing_subscriber::EnvFilter;

const HELP: &str = "\
Track your GitHub pull requests.

Usage: pulldeck [--version | --help]

Authenticates with GH_TOKEN, GITHUB_TOKEN or `gh auth token` and polls GitHub itself, so it works
with no server. Signing in to a pulldeck server adds live updates and collaboration for the
repositories the server can see; polling continues for the rest.
Set RUST_LOG=pulldeck=debug for verbose logs.

Development: PULLDECK_SERVER (e.g. http://127.0.0.1:8080) together with PULLDECK_TOKEN uses that
server's pull requests only (no polling). See CONTRIBUTING.md.";

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("pulldeck {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("--help" | "-h") => {
            println!("{HELP}");
            return;
        }
        _ => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "pulldeck=info".into()),
        )
        .init();

    // Fusion follows the platform palette (KDE colour schemes) without extra packages.
    if std::env::var_os("QT_QUICK_CONTROLS_STYLE").is_none() {
        std::env::set_var("QT_QUICK_CONTROLS_STYLE", "Fusion");
    }

    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    if let Some(mut engine) = engine.as_mut() {
        // A QML error would otherwise leave an event loop running with no window and no message.
        engine
            .as_mut()
            .on_object_creation_failed(|_, url| {
                eprintln!("pulldeck: could not load the UI from {url}");
                eprintln!("Run with QT_LOGGING_RULES='qt.qml*=true' for the QML errors.");
                std::process::exit(1);
            })
            .release();
        engine.load(&QUrl::from(
            "qrc:/qt/qml/io/github/bootswithdefer/pulldeck/qml/main.qml",
        ));
    }

    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
