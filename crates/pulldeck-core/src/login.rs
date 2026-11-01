//! Signing in through the server's GitHub device flow.
//!
//! The client never talks to GitHub and never sees a GitHub token: the server brokers the flow
//! and returns an opaque session token (see the server's `docs/PROTOCOL.md`). The UI shows the
//! user a short code, they approve it on GitHub, and polling then yields the session token.

use crate::session::SessionToken;
use crate::settings::ServerUrl;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use reqwest::StatusCode;
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{sleep, Instant};

/// Why signing in (or out) failed.
#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    /// The server could not be reached.
    #[error("could not reach the server: {0}")]
    Http(#[from] reqwest::Error),
    /// The user refused the request on GitHub.
    #[error("sign-in was denied")]
    Denied,
    /// The code was not approved in time.
    #[error("the sign-in code expired; start again")]
    Expired,
    /// The server answered in a way this client does not understand.
    #[error("unexpected response from the server (HTTP {0})")]
    Unexpected(u16),
}

/// What the user has to do to finish signing in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    /// Opaque handle the client polls with.
    pub handle: String,
    /// The short code the user types in, e.g. `ABCD-1234`.
    pub user_code: String,
    /// Where to type it, e.g. `https://github.com/login/device`.
    pub verification_uri: String,
    /// Seconds until the code expires.
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

/// The answer to one poll.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    /// Not approved yet; poll again after `interval`.
    Pending {
        /// The server's current polling interval (it grows if GitHub asks us to slow down).
        interval: Duration,
    },
    /// Approved.
    SignedIn(SignIn),
}

/// A finished sign-in.
#[derive(Debug, PartialEq, Eq)]
pub struct SignIn {
    /// The session credential. Store it in the keyring.
    pub token: SessionToken,
    /// The GitHub login it belongs to.
    pub login: String,
}

#[derive(Deserialize)]
struct PollBody {
    status: String,
    #[serde(default)]
    interval: Option<u64>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    login: Option<String>,
}

/// Talks to the server's `/auth` endpoints.
pub struct LoginClient {
    http: reqwest::Client,
    server: ServerUrl,
    min_interval: Duration,
}

impl LoginClient {
    /// A client for `server`.
    pub fn new(server: ServerUrl) -> Result<Self, LoginError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("pulldeck/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self {
            http,
            server,
            min_interval: Duration::from_secs(1),
        })
    }

    /// Never poll faster than this, whatever the server suggests. Mostly for tests.
    pub fn with_min_interval(mut self, min_interval: Duration) -> Self {
        self.min_interval = min_interval;
        self
    }

    /// Begins the flow and returns the code to show the user.
    pub async fn start(&self) -> Result<DeviceCode, LoginError> {
        let response = self
            .http
            .post(self.server.url("/auth/device"))
            .send()
            .await?;
        if response.status() != StatusCode::OK {
            return Err(LoginError::Unexpected(response.status().as_u16()));
        }
        Ok(response.json().await?)
    }

    /// Asks whether the user has approved the code yet.
    pub async fn poll(&self, handle: &str) -> Result<Poll, LoginError> {
        let response = self
            .http
            .post(self.server.url("/auth/device/poll"))
            .json(&serde_json::json!({ "handle": handle }))
            .send()
            .await?;
        match response.status() {
            StatusCode::OK => {
                let body: PollBody = response.json().await?;
                match (body.status.as_str(), body.token, body.login) {
                    ("ok", Some(token), Some(login)) => Ok(Poll::SignedIn(SignIn {
                        token: SessionToken::new(token),
                        login,
                    })),
                    _ => Err(LoginError::Unexpected(200)),
                }
            }
            StatusCode::ACCEPTED => {
                let body: PollBody = response.json().await?;
                Ok(Poll::Pending {
                    interval: Duration::from_secs(body.interval.unwrap_or(5)),
                })
            }
            StatusCode::FORBIDDEN => Err(LoginError::Denied),
            StatusCode::GONE => Err(LoginError::Expired),
            other => Err(LoginError::Unexpected(other.as_u16())),
        }
    }

    /// Runs the whole flow: starts it, hands the code to `show`, then polls (never faster than
    /// the server's interval) until the user approves, denies, or the code expires. Dropping the
    /// returned future cancels it.
    pub async fn sign_in(&self, show: impl FnOnce(&DeviceCode)) -> Result<SignIn, LoginError> {
        let code = self.start().await?;
        show(&code);

        let deadline = Instant::now() + Duration::from_secs(code.expires_in);
        let mut interval = Duration::from_secs(code.interval).max(self.min_interval);
        let mut failures = 0u32;
        loop {
            sleep(interval).await;
            if Instant::now() >= deadline {
                return Err(LoginError::Expired);
            }
            match self.poll(&code.handle).await {
                Ok(Poll::SignedIn(signed_in)) => return Ok(signed_in),
                Ok(Poll::Pending { interval: next }) => {
                    failures = 0;
                    interval = next.max(self.min_interval);
                }
                // A dropped connection mid-flow should not throw away the user's approval:
                // tolerate a few in a row before giving up.
                Err(LoginError::Http(e)) if failures < 3 => {
                    failures += 1;
                    tracing::debug!(error = %e, "poll failed, retrying");
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Revokes the session on the server. Succeeds if the session is already gone.
    pub async fn logout(&self, token: &SessionToken) -> Result<(), LoginError> {
        let mut bearer = HeaderValue::from_str(&format!("Bearer {}", token.expose()))
            .map_err(|_| LoginError::Unexpected(0))?;
        bearer.set_sensitive(true);
        let response = self
            .http
            .post(self.server.url("/auth/logout"))
            .header(AUTHORIZATION, bearer)
            .send()
            .await?;
        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::UNAUTHORIZED => Ok(()),
            other => Err(LoginError::Unexpected(other.as_u16())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> LoginClient {
        LoginClient::new(ServerUrl::parse(&server.uri()).unwrap())
            .unwrap()
            .with_min_interval(Duration::ZERO)
    }

    fn code_body(expires_in: u64) -> serde_json::Value {
        json!({
            "handle": "h-1",
            "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "expires_in": expires_in,
            "interval": 0
        })
    }

    async fn mount_start(server: &MockServer, expires_in: u64) {
        Mock::given(method("POST"))
            .and(path("/auth/device"))
            .respond_with(ResponseTemplate::new(200).set_body_json(code_body(expires_in)))
            .mount(server)
            .await;
    }

    fn poll_mock() -> wiremock::MockBuilder {
        Mock::given(method("POST"))
            .and(path("/auth/device/poll"))
            .and(body_json(json!({ "handle": "h-1" })))
    }

    #[tokio::test]
    async fn start_returns_the_code_to_show() {
        let server = MockServer::start().await;
        mount_start(&server, 900).await;
        let code = client(&server).start().await.unwrap();
        assert_eq!(code.user_code, "ABCD-1234");
        assert_eq!(code.verification_uri, "https://github.com/login/device");
        assert_eq!(code.expires_in, 900);
    }

    #[tokio::test]
    async fn poll_reports_each_outcome() {
        let server = MockServer::start().await;
        poll_mock()
            .respond_with(
                ResponseTemplate::new(202)
                    .set_body_json(json!({"status": "pending", "interval": 7})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        poll_mock()
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"status": "ok", "token": "s3cret-token", "login": "octocat"}),
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        poll_mock()
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({"status": "denied"})))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        poll_mock()
            .respond_with(ResponseTemplate::new(410).set_body_json(json!({"status": "expired"})))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        poll_mock()
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = client(&server);
        assert_eq!(
            client.poll("h-1").await.unwrap(),
            Poll::Pending {
                interval: Duration::from_secs(7)
            }
        );
        let Poll::SignedIn(signed_in) = client.poll("h-1").await.unwrap() else {
            panic!("expected a sign-in");
        };
        assert_eq!(signed_in.login, "octocat");
        assert_eq!(signed_in.token.expose(), "s3cret-token");
        assert!(matches!(client.poll("h-1").await, Err(LoginError::Denied)));
        assert!(matches!(client.poll("h-1").await, Err(LoginError::Expired)));
        assert!(matches!(
            client.poll("h-1").await,
            Err(LoginError::Unexpected(500))
        ));
    }

    #[tokio::test]
    async fn a_malformed_success_is_not_treated_as_signed_in() {
        let server = MockServer::start().await;
        // 200 but no token: must not produce an empty session.
        poll_mock()
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ok"})))
            .mount(&server)
            .await;
        assert!(matches!(
            client(&server).poll("h-1").await,
            Err(LoginError::Unexpected(200))
        ));
    }

    #[tokio::test]
    async fn sign_in_shows_the_code_then_polls_until_approved() {
        let server = MockServer::start().await;
        mount_start(&server, 900).await;
        poll_mock()
            .respond_with(
                ResponseTemplate::new(202)
                    .set_body_json(json!({"status": "pending", "interval": 0})),
            )
            .up_to_n_times(2)
            .mount(&server)
            .await;
        poll_mock()
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"status": "ok", "token": "tok", "login": "octocat"})),
            )
            .mount(&server)
            .await;

        let mut shown = None;
        let signed_in = client(&server)
            .sign_in(|code| shown = Some(code.user_code.clone()))
            .await
            .unwrap();
        assert_eq!(shown.as_deref(), Some("ABCD-1234"));
        assert_eq!(signed_in.login, "octocat");
        assert_eq!(signed_in.token.expose(), "tok");
        // Two pending answers, then the approval.
        let polls = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/auth/device/poll")
            .count();
        assert_eq!(polls, 3);
    }

    #[tokio::test]
    async fn sign_in_stops_when_denied() {
        let server = MockServer::start().await;
        mount_start(&server, 900).await;
        poll_mock()
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({"status": "denied"})))
            .mount(&server)
            .await;
        let result = client(&server).sign_in(|_| {}).await;
        assert!(matches!(result, Err(LoginError::Denied)));
    }

    #[tokio::test]
    async fn sign_in_gives_up_when_the_code_expires() {
        let server = MockServer::start().await;
        mount_start(&server, 0).await; // already expired
        let result = client(&server).sign_in(|_| {}).await;
        assert!(matches!(result, Err(LoginError::Expired)));
    }

    #[tokio::test]
    async fn logout_sends_the_token_as_a_bearer_credential() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/logout"))
            .and(header("authorization", "Bearer tok-123"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        client(&server)
            .logout(&SessionToken::new("tok-123"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn logging_out_of_an_already_dead_session_is_fine() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth/logout"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        client(&server)
            .logout(&SessionToken::new("old"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_token_never_shows_up_in_debug_output_or_errors() {
        let server = MockServer::start().await;
        poll_mock()
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"status": "ok", "token": "s3cret-token", "login": "octocat"}),
            ))
            .mount(&server)
            .await;
        let Poll::SignedIn(signed_in) = client(&server).poll("h-1").await.unwrap() else {
            panic!("expected a sign-in");
        };
        let debug = format!(
            "{signed_in:?} {:?}",
            Poll::SignedIn(SignIn {
                token: SessionToken::new("s3cret-token"),
                login: "octocat".into(),
            })
        );
        assert!(!debug.contains("s3cret"), "{debug}");

        let failure = LoginError::Unexpected(500);
        assert!(!format!("{failure} {failure:?}").contains("s3cret"));
    }
}
