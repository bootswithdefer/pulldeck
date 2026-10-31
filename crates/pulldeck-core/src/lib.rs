//! Core logic for pulldeck: GitHub authentication and pull request queries.
//!
//! This crate is UI-agnostic so it can be tested without Qt.

pub mod auth;
pub mod client;
pub mod error;
pub mod models;

pub use client::GitHubClient;
pub use error::{Error, Result};
pub use models::{ChecksStatus, PullRequest, ReviewStatus};
