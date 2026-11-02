//! Core logic for pulldeck: GitHub authentication and pull request queries.
//!
//! This crate is UI-agnostic so it can be tested without Qt.

pub mod auth;
pub mod client;
pub mod error;
pub mod login;
pub mod models;
pub mod remote;
pub mod session;
pub mod settings;
pub mod stack;
pub mod store;
pub mod tokens;
pub mod wire;
pub mod ws;

pub use client::GitHubClient;
pub use error::{Error, Result};
pub use models::{ChecksStatus, PullRequest, ReviewStatus};
