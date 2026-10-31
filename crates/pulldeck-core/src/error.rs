//! Error types.

/// Everything that can go wrong talking to GitHub.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No token in the environment and the `gh` CLI could not provide one.
    #[error("not authenticated: set GH_TOKEN or run `gh auth login` ({0})")]
    Auth(String),
    /// Network or HTTP failure.
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// GitHub returned GraphQL errors.
    #[error("GitHub API error: {0}")]
    Api(String),
    /// The response did not have the expected shape.
    #[error("unexpected response: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;
