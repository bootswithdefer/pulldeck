//! Prints your open PRs: `cargo run -p pulldeck-core --example list`.

use pulldeck_core::GitHubClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = GitHubClient::from_environment().await?;
    let (login, prs) = client.authored_prs().await?;
    println!("{login}: {} open PRs", prs.len());
    for pr in &prs {
        println!(
            "{:<40} #{:<5} {:?}/{:?}  {}",
            pr.repo, pr.number, pr.review, pr.checks, pr.title
        );
    }
    Ok(())
}
