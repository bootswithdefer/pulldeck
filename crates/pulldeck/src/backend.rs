//! The QObject that bridges the core crate to QML.

use chrono::{DateTime, Utc};
use cxx_qt::Threading;
use cxx_qt_lib::QString;
use pulldeck_core::{GitHubClient, PullRequest};
use serde_json::{json, Value};
use std::pin::Pin;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

#[cxx_qt::bridge(namespace = "pulldeck")]
pub mod qobject {
    #[namespace = ""]
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    impl cxx_qt::Threading for PrBackend {}

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, prs_json, cxx_name = "prsJson")]
        #[qproperty(QString, status)]
        #[qproperty(QString, user)]
        #[qproperty(bool, loading)]
        type PrBackend = super::PrBackendRust;

        /// Fetch the user's open PRs in the background.
        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);
    }
}

/// Backing state for [`qobject::PrBackend`].
pub struct PrBackendRust {
    prs_json: QString,
    status: QString,
    user: QString,
    loading: bool,
}

impl Default for PrBackendRust {
    fn default() -> Self {
        Self {
            prs_json: QString::from("[]"),
            status: QString::from("Not refreshed yet"),
            user: QString::default(),
            loading: false,
        }
    }
}

fn runtime() -> &'static Runtime {
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
        if *self.loading() {
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
                        let status = format!("{} open PR{}", prs.len(), plural(prs.len()));
                        backend.as_mut().set_user(QString::from(login.as_str()));
                        backend
                            .as_mut()
                            .set_prs_json(QString::from(to_json(&prs).as_str()));
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
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Serializes PRs for QML, adding a human-friendly age.
fn to_json(prs: &[PullRequest]) -> String {
    let now = Utc::now();
    let rows: Vec<Value> = prs
        .iter()
        .map(|pr| {
            let mut row = serde_json::to_value(pr).unwrap_or(Value::Null);
            row["updated_ago"] = json!(ago(now, pr.updated_at));
            row
        })
        .collect();
    Value::Array(rows).to_string()
}

fn ago(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds().max(0);
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
    fn ago_buckets() {
        let now: DateTime<Utc> = "2026-10-31T12:00:00Z".parse().unwrap();
        let at = |s: &str| ago(now, s.parse().unwrap());
        assert_eq!(at("2026-10-31T11:59:59Z"), "just now");
        assert_eq!(at("2026-10-31T11:15:00Z"), "45m ago");
        assert_eq!(at("2026-10-31T07:00:00Z"), "5h ago");
        assert_eq!(at("2026-10-28T12:00:00Z"), "3d ago");
    }
}
