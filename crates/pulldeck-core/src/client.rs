//! GitHub GraphQL client.

use crate::{auth, ChecksStatus, Error, PullRequest, Result, ReviewStatus};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::{json, Value};
use std::time::Duration;

const ENDPOINT: &str = "https://api.github.com/graphql";
/// Pages of an open-pull-request search to read: a ceiling of 500, far more than anyone has open.
const MAX_PAGES: usize = 10;
/// GitHub's search sometimes answers with a gateway timeout after ~30s and succeeds on the next
/// try, so allow a slow answer and try a few times.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const ATTEMPTS: u32 = 4;
const RETRY_PAUSE: Duration = Duration::from_secs(1);

const AUTHORED_QUERY: &str = r"
query($q: String!, $after: String) {
  viewer { login }
  search(query: $q, type: ISSUE, first: 50, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on PullRequest {
        number
        title
        url
        isDraft
        isCrossRepository
        baseRefName
        headRefName
        updatedAt
        reviewDecision
        repository { nameWithOwner }
        author { login }
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
      }
    }
  }
}";

/// Authenticated GitHub client.
#[derive(Debug, Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    endpoint: String,
    /// Pause before the second attempt; each further attempt waits one more of these.
    retry_pause: Duration,
}

impl GitHubClient {
    /// Builds a client from an explicit token.
    pub fn new(token: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| Error::Auth(e.to_string()))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("pulldeck/", env!("CARGO_PKG_VERSION"))),
        );
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            endpoint: ENDPOINT.into(),
            retry_pause: RETRY_PAUSE,
        })
    }

    /// Discovers a token (env or `gh`) and builds a client.
    pub async fn from_environment() -> Result<Self> {
        Self::new(&auth::token().await?)
    }

    /// Fetches all open pull requests authored by the authenticated user.
    ///
    /// Returns the viewer's login together with the PRs, newest first.
    pub async fn authored_prs(&self) -> Result<(String, Vec<PullRequest>)> {
        self.search("is:open author:@me", MAX_PAGES).await
    }

    /// Fetches all open pull requests where a review is requested from the authenticated user,
    /// directly (not through a team: an organisation's teams can ask for hundreds), newest first.
    pub async fn review_requests(&self) -> Result<Vec<PullRequest>> {
        Ok(self
            .search("is:open user-review-requested:@me", MAX_PAGES)
            .await?
            .1)
    }

    /// The most recent pull requests the authenticated user wrote that are no longer open (merged
    /// or closed), newest first. One page: this is a look back, not an archive.
    pub async fn recently_closed(&self) -> Result<Vec<PullRequest>> {
        Ok(self.search("is:closed author:@me", 1).await?.1)
    }

    /// Runs a pull request search for `qualifier` over unarchived pull requests, reading at most
    /// `max_pages` pages.
    async fn search(
        &self,
        qualifier: &str,
        max_pages: usize,
    ) -> Result<(String, Vec<PullRequest>)> {
        let mut prs = Vec::new();
        let mut after: Option<String> = None;
        let mut pages = 0;

        let login = loop {
            let body = json!({
                "query": AUTHORED_QUERY,
                "variables": { "q": format!("is:pr archived:false {qualifier} sort:updated-desc"), "after": after },
            });
            let response = self.post(&body).await?;

            let (page_login, page, next) = parse_page(&response)?;
            prs.extend(page);
            pages += 1;
            match next {
                Some(cursor) if pages < max_pages => after = Some(cursor),
                _ => break page_login,
            }
        };
        Ok((login, prs))
    }
}

impl GitHubClient {
    /// Posts one GraphQL request, trying again after a pause when GitHub answers 502, 503 or 504
    /// or the network fails. Anything else is not retried: a 401 means the token is dead, and
    /// 403 or 429 mean a rate limit, which is better waited out until the next poll.
    async fn post(&self, body: &Value) -> Result<Value> {
        let mut attempt = 1;
        loop {
            let failure = match self.http.post(&self.endpoint).json(body).send().await {
                Ok(response) if response.status().is_success() => match response.json().await {
                    Ok(body) => return Ok(body),
                    // The connection broke or timed out while the body was arriving.
                    Err(e) if e.is_timeout() || e.is_body() || e.is_decode() => Error::Http(e),
                    Err(e) => return Err(e.into()),
                },
                Ok(response) if is_gateway_error(response.status()) => Error::Api(format!(
                    "GitHub answered HTTP {}",
                    response.status().as_u16()
                )),
                Ok(response) => return Err(refused(response).await),
                Err(e) if e.is_timeout() || e.is_connect() || e.is_request() => Error::Http(e),
                Err(e) => return Err(e.into()),
            };
            if attempt >= ATTEMPTS {
                return Err(failure);
            }
            tracing::debug!(attempt, error = %failure, "GitHub request failed; retrying");
            tokio::time::sleep(self.retry_pause * attempt).await;
            attempt += 1;
        }
    }
}

fn is_gateway_error(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 502..=504)
}

/// The error for a response GitHub refused, including what it said and the rate limit headers,
/// which are what tell a rate limit from a permissions problem. Never includes the token.
async fn refused(response: reqwest::Response) -> Error {
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let remaining = header("x-ratelimit-remaining");
    let retry_after = header("retry-after");
    let message = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut text = format!("GitHub refused the request (HTTP {status})");
    if !message.is_empty() {
        text.push_str(&format!(": {message}"));
    }
    if let Some(remaining) = remaining {
        text.push_str(&format!(" [rate limit remaining {remaining}]"));
    }
    if let Some(retry_after) = retry_after {
        text.push_str(&format!(" [retry after {retry_after}s]"));
    }
    Error::Api(text)
}

/// Parses one GraphQL search page into (viewer login, PRs, next cursor).
pub(crate) fn parse_page(response: &Value) -> Result<(String, Vec<PullRequest>, Option<String>)> {
    if let Some(errors) = response.get("errors").and_then(Value::as_array) {
        let message = errors
            .iter()
            .filter_map(|e| e.get("message").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::Api(message));
    }

    let data = &response["data"];
    let login = data["viewer"]["login"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let search = &data["search"];

    let prs = search["nodes"]
        .as_array()
        .map(|nodes| nodes.iter().filter_map(parse_pr).collect())
        .unwrap_or_default();

    let page_info = &search["pageInfo"];
    let next = page_info["hasNextPage"]
        .as_bool()
        .unwrap_or(false)
        .then(|| page_info["endCursor"].as_str().map(str::to_owned))
        .flatten();

    Ok((login, prs, next))
}

fn parse_pr(node: &Value) -> Option<PullRequest> {
    // Non-PR search hits come back as empty objects.
    let number = node.get("number")?.as_u64()?;
    let rollup = node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str();

    Some(PullRequest {
        number,
        title: node["title"].as_str()?.to_owned(),
        url: node["url"].as_str()?.to_owned(),
        repo: node["repository"]["nameWithOwner"].as_str()?.to_owned(),
        author: node["author"]["login"]
            .as_str()
            .unwrap_or("ghost")
            .to_owned(),
        is_draft: node["isDraft"].as_bool().unwrap_or(false),
        review: ReviewStatus::from_graphql(node["reviewDecision"].as_str()),
        checks: ChecksStatus::from_graphql(rollup),
        updated_at: node["updatedAt"].as_str()?.parse().ok()?,
        base_ref: node["baseRefName"].as_str().unwrap_or_default().to_owned(),
        // A branch in a fork is not a branch of this repository that another PR could target.
        head_ref: match node["isCrossRepository"].as_bool() {
            Some(false) => node["headRefName"].as_str().unwrap_or_default().to_owned(),
            _ => String::new(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client_for(server: &MockServer) -> GitHubClient {
        let mut client = GitHubClient::new("test-token").unwrap();
        client.endpoint = server.uri();
        client.retry_pause = Duration::from_millis(1);
        client
    }

    fn one_page() -> Value {
        json!({"data": {
            "viewer": {"login": "octocat"},
            "search": {"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": []}
        }})
    }

    #[tokio::test]
    async fn review_requests_search_for_requests_to_the_viewer_not_their_own_prs() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("user-review-requested:@me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(one_page()))
            .expect(1)
            .mount(&server)
            .await;

        let prs = client_for(&server).review_requests().await.unwrap();
        assert!(prs.is_empty());
    }

    #[tokio::test]
    async fn a_gateway_error_is_retried_until_it_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(502))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(one_page()))
            .mount(&server)
            .await;

        let (login, prs) = client_for(&server).authored_prs().await.unwrap();
        assert_eq!(login, "octocat");
        assert!(prs.is_empty());
    }

    #[tokio::test]
    async fn recently_closed_reads_one_page_even_when_there_are_more() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("is:closed author:@me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
                "viewer": {"login": "octocat"},
                "search": {"pageInfo": {"hasNextPage": true, "endCursor": "c1"}, "nodes": []}
            }})))
            .expect(1)
            .mount(&server)
            .await;

        let prs = client_for(&server).recently_closed().await.unwrap();
        assert!(prs.is_empty());
    }

    #[tokio::test]
    async fn a_response_cut_off_mid_body_is_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"data\": {"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(one_page()))
            .mount(&server)
            .await;

        let (login, _) = client_for(&server).authored_prs().await.unwrap();
        assert_eq!(login, "octocat");
    }

    #[tokio::test]
    async fn gives_up_after_four_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(4)
            .mount(&server)
            .await;

        let err = client_for(&server).authored_prs().await.unwrap_err();
        assert!(err.to_string().contains("503"), "{err}");
    }

    #[tokio::test]
    async fn a_rate_limit_is_not_retried_and_says_so() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("retry-after", "60")
                    .set_body_json(json!({"message": "API rate limit exceeded"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let err = client_for(&server)
            .authored_prs()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("403"), "{err}");
        assert!(err.contains("API rate limit exceeded"), "{err}");
        assert!(err.contains("remaining 0"), "{err}");
        assert!(err.contains("retry after 60"), "{err}");
        assert!(!err.contains("test-token"), "{err}");
    }

    #[test]
    fn parses_a_page() {
        let response = json!({"data": {
            "viewer": {"login": "octocat"},
            "search": {
                "pageInfo": {"hasNextPage": true, "endCursor": "abc"},
                "nodes": [
                    {
                        "number": 7, "title": "Add thing", "url": "https://github.com/o/r/pull/7",
                        "isDraft": true, "updatedAt": "2026-10-01T12:00:00Z",
                        "reviewDecision": "CHANGES_REQUESTED",
                        "repository": {"nameWithOwner": "o/r"}, "author": {"login": "octocat"},
                        "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "FAILURE"}}}]}
                    },
                    {}
                ]
            }
        }});

        let (login, prs, next) = parse_page(&response).unwrap();
        assert_eq!(login, "octocat");
        assert_eq!(next.as_deref(), Some("abc"));
        assert_eq!(prs.len(), 1, "empty non-PR node is skipped");
        assert_eq!(prs[0].repo, "o/r");
        assert!(prs[0].is_draft);
        assert_eq!(prs[0].review, ReviewStatus::ChangesRequested);
        assert_eq!(prs[0].checks, ChecksStatus::Failure);
    }

    #[test]
    fn surfaces_graphql_errors() {
        let response = json!({"errors": [{"message": "Bad credentials"}]});
        let err = parse_page(&response).unwrap_err();
        assert!(matches!(err, Error::Api(m) if m == "Bad credentials"));
    }

    #[test]
    fn missing_rollup_means_no_checks() {
        let node = json!({
            "number": 1, "title": "t", "url": "u", "updatedAt": "2026-01-01T00:00:00Z",
            "repository": {"nameWithOwner": "o/r"}, "author": null,
            "commits": {"nodes": []}
        });
        let pr = parse_pr(&node).unwrap();
        assert_eq!(pr.checks, ChecksStatus::None);
        assert_eq!(pr.review, ReviewStatus::None);
        assert_eq!(pr.author, "ghost");
    }
}
