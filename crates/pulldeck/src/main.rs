//! pulldeck: track your GitHub pull requests from a native Qt window.

mod backend;
mod inbox;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};
use tracing_subscriber::EnvFilter;

const HELP: &str = "\
Track your GitHub pull requests.

Usage: pulldeck [--version | --help]

Authenticates with GH_TOKEN, GITHUB_TOKEN or `gh auth token`.
Set RUST_LOG=pulldeck=debug for verbose logs.";

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

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from(
            "qrc:/qt/qml/io/github/bootswithdefer/pulldeck/qml/main.qml",
        ));
    }

    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
