mod drafts;
mod github;
mod parser;
mod syntax;
mod types;
mod ui;
mod update;

use anyhow::Result;
use clap::Parser;

use crate::github::{check_gh_cli, fetch_my_prs, fetch_pr_diff, fetch_review_prs, parse_pr_url};
use crate::parser::parse_diff;
use crate::ui::App;
use crate::update::check_for_update;

const LOGO: &str = r#"
  検査
  kensa
"#;

#[derive(Parser)]
#[command(name = "kensa")]
#[command(about = "A fast TUI for reviewing GitHub PRs")]
#[command(version)]
#[command(after_help = "\
EXAMPLES:
    kensa                                         List PRs awaiting your review
    kensa https://github.com/owner/repo/pull/123  Open a specific PR
    kensa --upgrade                               Check for updates

KEY BINDINGS:
    PR List:
        j/k         Navigate up/down
        Enter       Open PR diff
        Tab         Switch between 'For Review' / 'My PRs'
        r           Refresh list
        q           Quit

    Diff View:
        j/k         Scroll diff
        h/l         Previous/next file
        Tab         Toggle file tree
        /           Search files
        c           Comment on current line
        v           Visual mode (select lines for multi-line comment)
        t           View comment threads
        p           View pending comments
        S           Submit review
        o           Open PR in browser
        ?           Show help
        q           Back to PR list

    Comments:
        Ctrl+S      Save comment
        Esc         Cancel

REQUIREMENTS:
    GitHub CLI (gh) must be installed and authenticated.
    Install: https://cli.github.com/
")]
struct Args {
    /// GitHub PR URL (e.g., https://github.com/owner/repo/pull/123).
    /// If not provided, shows PRs awaiting your review.
    pr_url: Option<String>,

    /// Disable automatic upgrade check on startup
    #[arg(long)]
    no_upgrade_check: bool,

    /// Check for updates and exit
    #[arg(long)]
    upgrade: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Handle --upgrade: check for updates and exit
    if args.upgrade {
        eprintln!("Checking for updates...");
        if let Some(update_msg) = check_for_update(true).await {
            eprintln!("\x1b[33m{}\x1b[0m", update_msg);
            eprint!("\nUpgrade now? [Y/n] ");
            use std::io::Write;
            let _ = std::io::stderr().flush();

            // Read user input (default is yes)
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).is_ok() {
                let input = input.trim().to_lowercase();
                if input.is_empty() || input == "y" || input == "yes" {
                    eprintln!("\nUpgrading...\n");
                    let status = std::process::Command::new("cargo")
                        .args(["install", "--git", "https://github.com/marlboro-red/kensa", "--force"])
                        .status();

                    match status {
                        Ok(s) if s.success() => {
                            eprintln!("\n\x1b[32mUpgrade complete!\x1b[0m");
                        }
                        Ok(_) => {
                            eprintln!("\n\x1b[31mUpgrade failed.\x1b[0m");
                        }
                        Err(e) => {
                            eprintln!("\n\x1b[31mFailed to run cargo: {}\x1b[0m", e);
                        }
                    }
                } else {
                    eprintln!("Upgrade cancelled.");
                }
            }
        } else {
            eprintln!("Already up to date (v{})", update::VERSION);
        }
        return Ok(());
    }

    // Show logo
    eprintln!("{}", LOGO);

    // Check for updates (unless disabled)
    if !args.no_upgrade_check {
        if let Some(update_msg) = check_for_update(false).await {
            eprintln!("\x1b[33m{}\x1b[0m\n", update_msg); // Yellow color
        }
    }

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
