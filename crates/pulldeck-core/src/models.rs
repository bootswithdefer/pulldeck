//! Data types shared between the client and the UI.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Aggregate review state of a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// Approved by the required reviewers.
    Approved,
    /// A reviewer requested changes.
    ChangesRequested,
    /// Waiting on review.
    ReviewRequired,
    /// No review policy applies.
    None,
}

impl ReviewStatus {
    /// Stable snake_case name, as used by the UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::ReviewRequired => "review_required",
            Self::None => "none",
        }
    }

    pub(crate) fn from_graphql(value: Option<&str>) -> Self {
        match value {
            Some("APPROVED") => Self::Approved,
            Some("CHANGES_REQUESTED") => Self::ChangesRequested,
            Some("REVIEW_REQUIRED") => Self::ReviewRequired,
            _ => Self::None,
        }
    }
}

/// Aggregate CI state of the latest commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecksStatus {
    /// All checks passed.
    Success,
    /// At least one check failed or errored.
    Failure,
    /// Checks are queued or running.
    Pending,
    /// No checks reported.
    None,
}

impl ChecksStatus {
    /// Stable snake_case name, as used by the UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Pending => "pending",
            Self::None => "none",
        }
    }

    pub(crate) fn from_graphql(value: Option<&str>) -> Self {
        match value {
            Some("SUCCESS") => Self::Success,
            Some("FAILURE" | "ERROR") => Self::Failure,
            Some("PENDING" | "EXPECTED") => Self::Pending,
            _ => Self::None,
        }
    }
}

/// A pull request as shown in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullRequest {
    /// PR number within its repository.
    pub number: u64,
    /// PR title.
    pub title: String,
    /// Web URL.
    pub url: String,
    /// `owner/name` of the repository.
    pub repo: String,
    /// Author login.
    pub author: String,
    /// Whether the PR is a draft.
    pub is_draft: bool,
    /// Aggregate review state.
    pub review: ReviewStatus,
    /// Aggregate CI state.
    pub checks: ChecksStatus,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// The branch this PR merges into. Empty when unknown (the server does not send it).
    pub base_ref: String,
    /// The branch this PR comes from, for telling stacked PRs apart (see [`crate::stack`]). Empty
    /// when unknown, and when the branch lives in a fork: it then cannot be another PR's base.
    pub head_ref: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The UI matches on these names, and they must agree with the serialized form.
    #[test]
    fn status_names_match_their_serialized_form() {
        for status in [
            ReviewStatus::Approved,
            ReviewStatus::ChangesRequested,
            ReviewStatus::ReviewRequired,
            ReviewStatus::None,
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), status.as_str());
        }
        for status in [
            ChecksStatus::Success,
            ChecksStatus::Failure,
            ChecksStatus::Pending,
            ChecksStatus::None,
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), status.as_str());
        }
    }
}
