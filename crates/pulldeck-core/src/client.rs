//! GitHub GraphQL client.

use crate::{auth, ChecksStatus, Error, PullRequest, Result, ReviewStatus};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::{json, Value};

const ENDPOINT: &str = "https://api.github.com/graphql";
const PAGE_SIZE: u32 = 50;

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
            .build()?;
        Ok(Self { http })
    }

    /// Discovers a token (env or `gh`) and builds a client.
    pub async fn from_environment() -> Result<Self> {
        Self::new(&auth::token().await?)
    }

    /// Fetches all open pull requests authored by the authenticated user.
    ///
    /// Returns the viewer's login together with the PRs, newest first.
    pub async fn authored_prs(&self) -> Result<(String, Vec<PullRequest>)> {
        let mut prs = Vec::new();
        let mut after: Option<String> = None;

        let login = loop {
            let body = json!({
                "query": AUTHORED_QUERY,
                "variables": { "q": "is:pr is:open archived:false author:@me sort:updated-desc", "after": after },
            });
            let response: Value = self
                .http
                .post(ENDPOINT)
                .json(&body)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;

            let (page_login, page, next) = parse_page(&response)?;
            prs.extend(page);
            match next {
                Some(cursor) if prs.len() < (PAGE_SIZE as usize) * 10 => after = Some(cursor),
                _ => break page_login,
            }
        };
        Ok((login, prs))
    }
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
