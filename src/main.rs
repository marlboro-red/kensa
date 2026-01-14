mod config;
mod drafts;
mod github;
mod parser;
mod syntax;
mod types;
mod ui;
mod update;

use anyhow::Result;
use clap::Parser;

use crate::config::Config;
use crate::github::{check_gh_cli, fetch_my_prs, fetch_pr_details, fetch_pr_diff, fetch_prs_by_author, fetch_review_prs, parse_pr_url};
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
    kensa --user <username>                       List PRs by a GitHub user
    kensa --upgrade                               Check for updates
    kensa --init-config                           Generate default config file
    kensa --edit-config                           Open config in editor

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
    /// GitHub PR URL (e.g., https://github.com/owner/repo/pull/123)
    pr_url: Option<String>,

    /// Show PRs by a specific GitHub user
    #[arg(long, short)]
    user: Option<String>,

    /// Disable automatic upgrade check on startup
    #[arg(long)]
    no_upgrade_check: bool,

    /// Check for updates and exit
    #[arg(long)]
    upgrade: bool,

    /// Generate a default config file at ~/.config/kensa/config.toml
    #[arg(long)]
    init_config: bool,

    /// Force overwrite existing config file (use with --init-config)
    #[arg(long)]
    force: bool,

    /// Open config file in your default editor ($EDITOR)
    #[arg(long, short = 'e')]
    edit_config: bool,
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

                    // Get current executable path and prepare backup path
                    // Renaming the running binary allows cargo to install the new one
                    let current_exe = std::env::current_exe().ok();
                    let backup_path = current_exe.as_ref().map(|p| p.with_extension("old"));

                    // Rename current executable to allow replacement
                    let renamed = if let (Some(exe), Some(backup)) =
                        (&current_exe, &backup_path)
                    {
                        // Remove any existing backup first
                        let _ = std::fs::remove_file(backup);
                        std::fs::rename(exe, backup).is_ok()
                    } else {
                        false
                    };

                    let status = std::process::Command::new("cargo")
                        .args([
                            "install",
                            "--git",
                            "https://github.com/marlboro-red/kensa",
                            "--force",
                        ])
                        .status();

                    match status {
                        Ok(s) if s.success() => {
                            // Clean up backup file on success
                            if let Some(ref backup) = backup_path {
                                let _ = std::fs::remove_file(backup);
                            }
                            eprintln!("\n\x1b[32mUpgrade complete!\x1b[0m");
                        }
                        Ok(_) => {
                            // Restore backup on failure
                            if renamed {
                                if let (Some(exe), Some(backup)) =
                                    (&current_exe, &backup_path)
                                {
                                    let _ = std::fs::rename(backup, exe);
                                }
                            }
                            eprintln!("\n\x1b[31mUpgrade failed.\x1b[0m");
                        }
                        Err(e) => {
                            // Restore backup on failure
                            if renamed {
                                if let (Some(exe), Some(backup)) =
                                    (&current_exe, &backup_path)
                                {
                                    let _ = std::fs::rename(backup, exe);
                                }
                            }
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

    // Handle --init-config: create config file and exit
    if args.init_config {
        match Config::init(args.force) {
            Ok(path) => {
                eprintln!("\x1b[32mConfig file created at:\x1b[0m {}", path.display());
                eprintln!("\nEdit this file to customize kensa settings.");
            }
            Err(e) => {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // Handle --edit-config: open config in editor and exit
    if args.edit_config {
        match Config::edit() {
            Ok(()) => {}
            Err(e) => {
                eprintln!("\x1b[31mError:\x1b[0m {}", e);
                std::process::exit(1);
            }
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

    if let Some(username) = args.user {
        // User mode - show PRs by that user
        eprintln!("Fetching PRs by @{}...", username);

        let prs = fetch_prs_by_author(&username).await?;

        if prs.is_empty() {
            eprintln!("No open PRs found for @{}", username);
            return Ok(());
        }

        eprintln!(
            "Found {} PRs by @{}. Starting viewer...",
            prs.len(),
            username
        );

        let mut app = App::new_with_author_prs(username, prs);
        app.run()?;
    } else if let Some(url) = args.pr_url {
        // Direct PR URL mode
        let pr_info = parse_pr_url(&url)?;
        eprintln!(
            "Fetching PR #{} from {}/{}...",
            pr_info.number, pr_info.owner, pr_info.repo
        );

        // Fetch diff and PR details concurrently
        let (diff_result, details_result) = tokio::join!(
            fetch_pr_diff(&pr_info),
            fetch_pr_details(&pr_info)
        );

        let diff_content = diff_result?;
        let pr = details_result?;
        let files = parse_diff(&diff_content);

        if files.is_empty() {
            eprintln!("No files found in diff");
            return Ok(());
        }

        eprintln!("Found {} files. Starting viewer...", files.len());

        let mut app = App::new_with_pr(files, pr);
        app.run()?;
    } else {
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

    Ok(())
}
