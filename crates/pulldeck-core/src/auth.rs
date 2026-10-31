//! Token discovery. No secrets are stored by pulldeck itself.

use crate::{Error, Result};
use tokio::process::Command;

/// Returns a GitHub token.
///
/// Order: `GH_TOKEN`, `GITHUB_TOKEN`, then `gh auth token`.
pub async fn token() -> Result<String> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = std::env::var(var) {
            let value = value.trim();
            if !value.is_empty() {
                return Ok(value.to_owned());
            }
        }
    }

    let output = Command::new("gh")
        .args(["auth", "token"])
        .output()
        .await
        .map_err(|e| Error::Auth(format!("could not run gh: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Auth(stderr.trim().to_owned()));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if token.is_empty() {
        return Err(Error::Auth("gh returned an empty token".into()));
    }
    Ok(token)
}
