mod drafts;
mod github;
mod parser;
mod syntax;
mod types;
mod ui;

use anyhow::Result;
use clap::Parser;

use crate::github::{check_gh_cli, fetch_my_prs, fetch_pr_diff, fetch_review_prs, parse_pr_url};
use crate::parser::parse_diff;
use crate::ui::App;

const LOGO: &str = r#"
  検査
  kensa
"#;

#[derive(Parser)]
#[command(name = "kensa")]
#[command(about = "A fast TUI for reviewing GitHub PRs")]
#[command(version)]
struct Args {
    /// GitHub PR URL (e.g., https://github.com/owner/repo/pull/123)
    /// If not provided, shows PRs awaiting your review
    pr_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Show logo
    eprintln!("{}", LOGO);

    // Check gh CLI is available
    check_gh_cli().await?;

    match args.pr_url {
        Some(url) => {
            // Direct PR URL mode
            let pr = parse_pr_url(&url)?;
            eprintln!(
                "Fetching PR #{} from {}/{}...",
                pr.number, pr.owner, pr.repo
            );

            let diff_content = fetch_pr_diff(&pr).await?;
            let files = parse_diff(&diff_content);

            if files.is_empty() {
                eprintln!("No files found in diff");
                return Ok(());
            }

            eprintln!("Found {} files. Starting viewer...", files.len());

            let mut app = App::new(files);
            app.run()?;
        }
        None => {
            // PR list mode - show PRs awaiting review and my PRs
            eprintln!("Fetching PRs...");

            // Fetch both lists concurrently
            let (review_prs, my_prs) = tokio::join!(fetch_review_prs(), fetch_my_prs());

            let review_prs = review_prs?;
            let my_prs = my_prs?;

            let total = review_prs.len() + my_prs.len();
            if total == 0 {
                eprintln!("No open PRs found.");
                return Ok(());
            }

            eprintln!(
                "Found {} PRs for review, {} of your PRs. Starting viewer...",
                review_prs.len(),
                my_prs.len()
            );

            let mut app = App::new_with_prs(review_prs, my_prs);
            app.run()?;
        }
    }

    Ok(())
}
