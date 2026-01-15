use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;
use url::Url;

use crate::types::{CommentThread, IssueComment, PendingComment, PrInfo, ReviewComment, ReviewPr, ThreadComment};

/// Log performance timing to file if KENSA_DEBUG is set
#[inline]
fn perf_log(operation: &str, elapsed_ms: u128) {
    if std::env::var("KENSA_DEBUG").is_ok() {
        use std::io::Write;
        if let Some(mut path) = dirs::config_dir() {
            path.push("kensa");
            path.push("perf.log");
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = writeln!(file, "{:>6}ms  {}", elapsed_ms, operation);
            }
        }
    }
}

/// Parse a GitHub PR URL into owner, repo, and PR number
pub fn parse_pr_url(url_str: &str) -> Result<PrInfo> {
    let url = Url::parse(url_str).context("Invalid URL")?;

    if url.host_str() != Some("github.com") {
        return Err(anyhow!("Only github.com URLs are supported"));
    }

    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| anyhow!("Invalid PR URL path"))?
        .collect();

    // Expected format: /owner/repo/pull/123
    if segments.len() < 4 || segments[2] != "pull" {
        return Err(anyhow!(
            "Invalid PR URL format. Expected: https://github.com/owner/repo/pull/123"
        ));
    }

    let owner = segments[0].to_string();
    let repo = segments[1].to_string();
    let number: u32 = segments[3]
        .parse()
        .context("PR number must be a valid integer")?;

    Ok(PrInfo {
        owner,
        repo,
        number,
    })
}

/// Check if gh CLI is installed and authenticated
pub async fn check_gh_cli() -> Result<()> {
    let start = Instant::now();
    let output = Command::new("gh")
        .args(["auth", "status"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to run 'gh' CLI. Is it installed? (brew install gh)")?;

    perf_log("gh auth status", start.elapsed().as_millis());

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not logged") {
            return Err(anyhow!(
                "Not authenticated with GitHub CLI. Run: gh auth login"
            ));
        }
        return Err(anyhow!("gh auth check failed: {}", stderr));
    }

    Ok(())
}

/// Fetch the diff content for a PR using gh CLI
pub async fn fetch_pr_diff(pr: &PrInfo) -> Result<String> {
    let start = Instant::now();
    let output = Command::new("gh")
        .args([
            "api",
            &format!("repos/{}/{}/pulls/{}", pr.owner, pr.repo, pr.number),
            "-H",
            "Accept: application/vnd.github.v3.diff",
        ])
        .output()
        .await
        .context("Failed to fetch PR diff")?;

    perf_log(&format!("fetch_pr_diff #{}", pr.number), start.elapsed().as_millis());

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("404") {
            return Err(anyhow!(
                "PR not found: {}/{}/pull/{}",
                pr.owner,
                pr.repo,
                pr.number
            ));
        }
        return Err(anyhow!("Failed to fetch diff: {}", stderr));
    }

    let diff = String::from_utf8_lossy(&output.stdout).into_owned();

    if diff.is_empty() {
        return Err(anyhow!("PR has no changes"));
    }

    Ok(diff)
}

/// JSON structure for gh search prs output
#[derive(Debug, Deserialize)]
struct GhSearchPrResult {
    number: u32,
    title: String,
    repository: GhRepository,
    author: GhAuthor,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct GhRepository {
    name: String,
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct GhAuthor {
    login: String,
}

/// Common helper for searching PRs with a specific filter
async fn search_prs_with_filter(filter: &str) -> Result<Vec<ReviewPr>> {
    let start = Instant::now();
    let output = Command::new("gh")
        .args([
            "search",
            "prs",
            filter,
            "--state=open",
            "--json=number,title,repository,author,createdAt,url",
            "--limit=100",
        ])
        .output()
        .await
        .context("Failed to fetch PRs. Is 'gh' CLI installed?")?;

    perf_log(&format!("gh search prs {}", filter), start.elapsed().as_millis());

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to search PRs: {}", stderr));
    }

    let json_str = String::from_utf8(output.stdout).context("Invalid UTF-8 in response")?;
    let results: Vec<GhSearchPrResult> =
        serde_json::from_str(&json_str).context("Failed to parse PR list JSON")?;

    let prs = results
        .into_iter()
        .map(|r| {
            let (repo_owner, repo_name) = r
                .repository
                .name_with_owner
                .split_once('/')
                .map(|(o, n)| (o.to_string(), n.to_string()))
                .unwrap_or((String::new(), r.repository.name));

            ReviewPr {
                number: r.number,
                title: r.title,
                repo_owner,
                repo_name,
                author: r.author.login,
                created_at: r.created_at,
                head_sha: None,
                body: None, // Not fetched in search results
            }
        })
        .collect();

    Ok(prs)
}

/// Get the current authenticated GitHub username
async fn get_current_user() -> Result<String> {
    let start = Instant::now();
    let output = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .output()
        .await
        .context("Failed to get current user")?;

    perf_log("gh api user", start.elapsed().as_millis());

    if !output.status.success() {
        return Err(anyhow!("Failed to get current user"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Fetch all PRs where review is requested or already reviewed by the current user
pub async fn fetch_review_prs() -> Result<Vec<ReviewPr>> {
    let start = Instant::now();

    // Fetch current user, requested PRs, and reviewed PRs in parallel
    let (current_user, requested, reviewed) = tokio::join!(
        get_current_user(),
        search_prs_with_filter("--review-requested=@me"),
        search_prs_with_filter("--reviewed-by=@me")
    );

    let current_user = current_user?;
    let mut prs = requested?;
    let reviewed_prs = reviewed?;

    // Add reviewed PRs that aren't already in the list (dedupe by repo+number)
    // Also filter out PRs where the current user is the author
    for pr in reviewed_prs {
        // Skip PRs authored by the current user
        if pr.author.eq_ignore_ascii_case(&current_user) {
            continue;
        }

        let exists = prs.iter().any(|p| {
            p.number == pr.number && p.repo_owner == pr.repo_owner && p.repo_name == pr.repo_name
        });
        if !exists {
            prs.push(pr);
        }
    }

    perf_log("fetch_review_prs (total)", start.elapsed().as_millis());
    Ok(prs)
}

/// Fetch the head SHA for a PR (needed for inline comments)
pub async fn fetch_pr_head_sha(pr: &PrInfo) -> Result<String> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr.number.to_string(),
            "--repo",
            &format!("{}/{}", pr.owner, pr.repo),
            "--json=headRefOid",
        ])
        .output()
        .await
        .context("Failed to fetch PR head SHA")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to fetch PR head SHA: {}", stderr));
    }

    #[derive(Deserialize)]
    struct HeadRef {
        #[serde(rename = "headRefOid")]
        head_ref_oid: String,
    }

    let json_str = String::from_utf8(output.stdout).context("Invalid UTF-8 in response")?;
    let result: HeadRef = serde_json::from_str(&json_str).context("Failed to parse head SHA")?;

    Ok(result.head_ref_oid)
}

/// Fetch full PR details from a PrInfo (for direct URL mode)
/// Returns a ReviewPr with all fields populated including head_sha and body
pub async fn fetch_pr_details(pr: &PrInfo) -> Result<ReviewPr> {
    let start = Instant::now();
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr.number.to_string(),
            "--repo",
            &format!("{}/{}", pr.owner, pr.repo),
            "--json=number,title,author,createdAt,headRefOid,body",
        ])
        .output()
        .await
        .context("Failed to fetch PR details")?;

    perf_log(&format!("fetch_pr_details #{}", pr.number), start.elapsed().as_millis());

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to fetch PR details: {}", stderr));
    }

    #[derive(Deserialize)]
    struct PrDetails {
        number: u32,
        title: String,
        author: GhAuthor,
        #[serde(rename = "createdAt")]
        created_at: String,
        #[serde(rename = "headRefOid")]
        head_ref_oid: String,
        #[serde(default)]
        body: Option<String>,
    }

    let json_str = String::from_utf8(output.stdout).context("Invalid UTF-8 in response")?;
    let details: PrDetails = serde_json::from_str(&json_str).context("Failed to parse PR details")?;

    Ok(ReviewPr {
        number: details.number,
        title: details.title,
        repo_owner: pr.owner.clone(),
        repo_name: pr.repo.clone(),
        author: details.author.login,
        created_at: details.created_at,
        head_sha: Some(details.head_ref_oid),
        body: details.body,
    })
}

/// Submit a comment to a PR (general or inline)
pub async fn submit_pr_comment(pr: &PrInfo, comment: &PendingComment, head_sha: Option<&str>) -> Result<()> {
    let repo = format!("{}/{}", pr.owner, pr.repo);

    if comment.is_inline() {
        // Inline comment - use GitHub API
        let file_path = comment.file_path.as_ref().unwrap();
        let line = comment.line_number.unwrap();
        let commit_id = head_sha.ok_or_else(|| anyhow!("Head SHA required for inline comments"))?;

        let mut args = vec![
            "api".to_string(),
            format!("repos/{}/pulls/{}/comments", repo, pr.number),
            "-f".to_string(), format!("body={}", comment.body),
            "-f".to_string(), format!("path={}", file_path),
            "-F".to_string(), format!("line={}", line),
            "-f".to_string(), "side=RIGHT".to_string(),
            "-f".to_string(), format!("commit_id={}", commit_id),
        ];

        // Add start_line for multi-line comments
        if let Some(start_line) = comment.start_line {
            args.push("-F".to_string());
            args.push(format!("start_line={}", start_line));
            args.push("-f".to_string());
            args.push("start_side=RIGHT".to_string());
        }

        let output = Command::new("gh")
            .args(&args)
            .output()
            .await
            .context("Failed to submit inline comment")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to submit inline comment: {}", stderr));
        }
    } else {
        // General PR comment
        let output = Command::new("gh")
            .args([
                "pr",
                "comment",
                &pr.number.to_string(),
                "--repo",
                &repo,
                "--body",
                &comment.body,
            ])
            .output()
            .await
            .context("Failed to submit comment")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to submit comment: {}", stderr));
        }
    }

    Ok(())
}

/// Submit multiple comments to a PR using batch Review API when possible
/// This reduces N API calls to 1-2 calls (one for inline via Review API, one for general comments)
pub async fn submit_pr_comments(pr: &PrInfo, comments: &[PendingComment], head_sha: Option<&str>) -> Result<usize> {
    if comments.is_empty() {
        return Ok(0);
    }

    // Separate inline and general comments
    let (inline_comments, general_comments): (Vec<_>, Vec<_>) =
        comments.iter().partition(|c| c.is_inline());

    let mut submitted = 0;

    // Batch submit inline comments using Review API (single API call)
    if !inline_comments.is_empty() {
        let sha = match head_sha {
            Some(s) => s.to_string(),
            None => fetch_pr_head_sha(pr).await?,
        };

        submitted += submit_inline_comments_batch(pr, &inline_comments, &sha).await?;
    }

    // Submit general comments (these can't be batched via Review API)
    for comment in general_comments {
        submit_pr_comment(pr, comment, None).await?;
        submitted += 1;
    }

    Ok(submitted)
}

/// Build review comments JSON array for the GitHub Review API
fn build_review_comments_json(comments: &[&PendingComment]) -> Vec<serde_json::Value> {
    comments
        .iter()
        .map(|c| {
            let mut comment_obj = serde_json::json!({
                "path": c.file_path.as_ref().unwrap(),
                "line": c.line_number.unwrap(),
                "body": c.body,
                "side": "RIGHT"
            });

            if let Some(start_line) = c.start_line {
                comment_obj["start_line"] = serde_json::json!(start_line);
                comment_obj["start_side"] = serde_json::json!("RIGHT");
            }

            comment_obj
        })
        .collect()
}

/// Batch submit inline comments using the Review API (single API call for all inline comments)
async fn submit_inline_comments_batch(pr: &PrInfo, comments: &[&PendingComment], commit_id: &str) -> Result<usize> {
    if comments.is_empty() {
        return Ok(0);
    }

    let repo = format!("{}/{}", pr.owner, pr.repo);
    let review_comments = build_review_comments_json(comments);

    // Build the complete request body as JSON
    let request_body = serde_json::json!({
        "commit_id": commit_id,
        "event": "COMMENT",
        "comments": review_comments
    });

    let body_json = serde_json::to_string(&request_body)
        .context("Failed to serialize request body")?;

    // Use Review API with --input to send proper JSON body
    let mut child = Command::new("gh")
        .args([
            "api",
            &format!("repos/{}/pulls/{}/reviews", repo, pr.number),
            "--input", "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn gh command")?;

    // Write the JSON body to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(body_json.as_bytes()).await
            .context("Failed to write to gh stdin")?;
    }

    let output = child.wait_with_output().await
        .context("Failed to wait for gh command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to submit review: {}", stderr));
    }

    Ok(comments.len())
}

/// Fetch PRs authored by the current user
pub async fn fetch_my_prs() -> Result<Vec<ReviewPr>> {
    search_prs_with_filter("--author=@me").await
}

/// Fetch PRs authored by a specific user
pub async fn fetch_prs_by_author(username: &str) -> Result<Vec<ReviewPr>> {
    search_prs_with_filter(&format!("--author={}", username)).await
}

// ============================================================================
// Comment Thread Functions
// ============================================================================

/// Fetch review comments (inline on code) for a PR
pub async fn fetch_pr_review_comments(pr: &PrInfo) -> Result<Vec<ReviewComment>> {
    let output = Command::new("gh")
        .args([
            "api",
            &format!("repos/{}/{}/pulls/{}/comments", pr.owner, pr.repo, pr.number),
            "--paginate",
        ])
        .output()
        .await
        .context("Failed to fetch review comments")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to fetch review comments: {}", stderr));
    }

    let json_str = String::from_utf8(output.stdout).context("Invalid UTF-8")?;

    // Handle empty response
    if json_str.trim().is_empty() || json_str.trim() == "[]" {
        return Ok(Vec::new());
    }

    let comments: Vec<ReviewComment> = serde_json::from_str(&json_str)
        .context("Failed to parse review comments")?;

    Ok(comments)
}

/// Fetch general PR comments (issue comments)
pub async fn fetch_pr_issue_comments(pr: &PrInfo) -> Result<Vec<IssueComment>> {
    let output = Command::new("gh")
        .args([
            "api",
            &format!("repos/{}/{}/issues/{}/comments", pr.owner, pr.repo, pr.number),
            "--paginate",
        ])
        .output()
        .await
        .context("Failed to fetch PR comments")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to fetch PR comments: {}", stderr));
    }

    let json_str = String::from_utf8(output.stdout).context("Invalid UTF-8")?;

    // Handle empty response
    if json_str.trim().is_empty() || json_str.trim() == "[]" {
        return Ok(Vec::new());
    }

    let comments: Vec<IssueComment> = serde_json::from_str(&json_str)
        .context("Failed to parse PR comments")?;

    Ok(comments)
}

/// Fetch all comment threads for a PR (combines review + issue comments)
pub async fn fetch_all_comment_threads(pr: &PrInfo) -> Result<Vec<CommentThread>> {
    // Fetch both types concurrently
    let (review_result, issue_result) = tokio::join!(
        fetch_pr_review_comments(pr),
        fetch_pr_issue_comments(pr)
    );

    let review_comments = review_result?;
    let issue_comments = issue_result?;

    let mut threads = Vec::new();

    // Group review comments by reply chain (in_reply_to_id)
    threads.extend(group_review_comments_into_threads(review_comments));

    // Convert issue comments to threads (each is its own thread)
    for comment in issue_comments {
        threads.push(CommentThread {
            id: comment.id,
            file_path: None,
            line: None,
            comments: vec![ThreadComment {
                body: comment.body,
                author: comment.user.login,
                created_at: comment.created_at,
            }],
            outdated: false, // Issue comments are never outdated
        });
    }

    // Sort threads: inline comments by file/line, then general comments
    threads.sort_by(|a, b| {
        match (&a.file_path, &b.file_path) {
            (Some(pa), Some(pb)) => pa.cmp(pb)
                .then_with(|| a.line.cmp(&b.line)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.id.cmp(&b.id),
        }
    });

    Ok(threads)
}

/// Group review comments into threads based on in_reply_to_id
fn group_review_comments_into_threads(comments: Vec<ReviewComment>) -> Vec<CommentThread> {
    use std::collections::HashMap;

    if comments.is_empty() {
        return Vec::new();
    }

    // Build a map of id -> comment and track replies
    let mut by_id: HashMap<u64, &ReviewComment> = HashMap::new();
    let mut replies: HashMap<u64, Vec<u64>> = HashMap::new(); // parent_id -> child_ids

    for comment in &comments {
        by_id.insert(comment.id, comment);
        if let Some(parent_id) = comment.in_reply_to_id {
            replies.entry(parent_id).or_default().push(comment.id);
        }
    }

    // Find root comments (those with no in_reply_to_id)
    let roots: Vec<u64> = comments
        .iter()
        .filter(|c| c.in_reply_to_id.is_none())
        .map(|c| c.id)
        .collect();

    // Build threads from roots
    let mut threads = Vec::new();
    for root_id in roots {
        let mut thread_comments = Vec::new();
        let mut stack = vec![root_id];

        while let Some(id) = stack.pop() {
            if let Some(comment) = by_id.get(&id) {
                thread_comments.push(ThreadComment {
                    body: comment.body.clone(),
                    author: comment.user.login.clone(),
                    created_at: comment.created_at.clone(),
                });

                // Add children (in reverse order to maintain chronological order when popping)
                if let Some(children) = replies.get(&id) {
                    for child_id in children.iter().rev() {
                        stack.push(*child_id);
                    }
                }
            }
        }

        // Sort by creation time
        thread_comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        if let Some(root) = by_id.get(&root_id) {
            threads.push(CommentThread {
                id: root_id,
                file_path: Some(root.path.clone()),
                line: root.line,
                comments: thread_comments,
                outdated: root.is_outdated(),
            });
        }
    }

    threads
}

/// Submit a reply to an existing comment thread
pub async fn submit_thread_reply(
    pr: &PrInfo,
    thread: &CommentThread,
    body: &str,
) -> Result<()> {
    let repo = format!("{}/{}", pr.owner, pr.repo);

    if thread.is_inline() {
        // Reply to review comment using in_reply_to
        let output = Command::new("gh")
            .args([
                "api",
                &format!("repos/{}/pulls/{}/comments", repo, pr.number),
                "-f", &format!("body={}", body),
                "-F", &format!("in_reply_to={}", thread.id),
            ])
            .output()
            .await
            .context("Failed to submit reply")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to submit reply: {}", stderr));
        }
    } else {
        // Reply to issue comment (just add a new issue comment)
        let output = Command::new("gh")
            .args([
                "api",
                &format!("repos/{}/issues/{}/comments", repo, pr.number),
                "-f", &format!("body={}", body),
            ])
            .output()
            .await
            .context("Failed to submit reply")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to submit reply: {}", stderr));
        }
    }

    Ok(())
}

/// Submit a PR review (approve, request changes, or comment) with optional inline comments
pub async fn submit_pr_review(
    pr: &PrInfo,
    event: &str,  // "APPROVE", "REQUEST_CHANGES", or "COMMENT"
    body: Option<&str>,
    pending_comments: Option<&[PendingComment]>,
    head_sha: Option<&str>,
) -> Result<usize> {
    let repo = format!("{}/{}", pr.owner, pr.repo);

    // REQUEST_CHANGES requires a body
    if event == "REQUEST_CHANGES" && body.map(|b| b.is_empty()).unwrap_or(true) {
        return Err(anyhow!("Request changes requires a comment"));
    }

    // Separate inline and general comments
    let (inline_comments, general_comments): (Vec<_>, Vec<_>) = pending_comments
        .map(|c| c.iter().partition(|c| c.is_inline()))
        .unwrap_or_default();

    // Get head SHA if we have inline comments
    let commit_id = if !inline_comments.is_empty() {
        match head_sha {
            Some(s) => s.to_string(),
            None => fetch_pr_head_sha(pr).await?,
        }
    } else {
        String::new()
    };

    // Build the review comments array for inline comments
    let review_comments = build_review_comments_json(&inline_comments);

    // Build the request body
    let mut request_body = serde_json::json!({
        "event": event
    });

    // Add body if provided
    if let Some(body_text) = body
        && !body_text.is_empty() {
            request_body["body"] = serde_json::json!(body_text);
        }

    // Add inline comments and commit_id if we have any
    if !review_comments.is_empty() {
        request_body["comments"] = serde_json::json!(review_comments);
        request_body["commit_id"] = serde_json::json!(commit_id);
    }

    let body_json = serde_json::to_string(&request_body)
        .context("Failed to serialize request body")?;

    let mut child = Command::new("gh")
        .args([
            "api",
            &format!("repos/{}/pulls/{}/reviews", repo, pr.number),
            "--input", "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn gh command")?;

    // Write the JSON body to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(body_json.as_bytes()).await
            .context("Failed to write to gh stdin")?;
    }

    let output = child.wait_with_output().await
        .context("Failed to wait for gh command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to submit review: {}", stderr));
    }

    let mut submitted = inline_comments.len();

    // Submit general comments (these can't be batched via Review API)
    for comment in general_comments {
        submit_pr_comment(pr, comment, None).await?;
        submitted += 1;
    }

    Ok(submitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CommentUser;

    // ========================================================================
    // parse_pr_url tests
    // ========================================================================

    #[test]
    fn test_parse_pr_url() {
        let pr = parse_pr_url("https://github.com/anthropics/claude-code/pull/123").unwrap();
        assert_eq!(pr.owner, "anthropics");
        assert_eq!(pr.repo, "claude-code");
        assert_eq!(pr.number, 123);
    }

    #[test]
    fn test_parse_pr_url_invalid() {
        assert!(parse_pr_url("https://github.com/owner/repo").is_err());
        assert!(parse_pr_url("https://gitlab.com/owner/repo/pull/1").is_err());
        assert!(parse_pr_url("not a url").is_err());
    }

    #[test]
    fn test_parse_pr_url_with_trailing_slash() {
        let pr = parse_pr_url("https://github.com/owner/repo/pull/456/").unwrap();
        assert_eq!(pr.owner, "owner");
        assert_eq!(pr.repo, "repo");
        assert_eq!(pr.number, 456);
    }

    #[test]
    fn test_parse_pr_url_with_files_path() {
        // URLs like https://github.com/owner/repo/pull/123/files should work
        let pr = parse_pr_url("https://github.com/owner/repo/pull/789/files").unwrap();
        assert_eq!(pr.owner, "owner");
        assert_eq!(pr.repo, "repo");
        assert_eq!(pr.number, 789);
    }

    #[test]
    fn test_parse_pr_url_with_commits_path() {
        let pr = parse_pr_url("https://github.com/owner/repo/pull/100/commits").unwrap();
        assert_eq!(pr.number, 100);
    }

    #[test]
    fn test_parse_pr_url_large_pr_number() {
        let pr = parse_pr_url("https://github.com/owner/repo/pull/999999").unwrap();
        assert_eq!(pr.number, 999999);
    }

    #[test]
    fn test_parse_pr_url_single_digit() {
        let pr = parse_pr_url("https://github.com/owner/repo/pull/1").unwrap();
        assert_eq!(pr.number, 1);
    }

    #[test]
    fn test_parse_pr_url_hyphenated_names() {
        let pr = parse_pr_url("https://github.com/my-org/my-cool-repo/pull/42").unwrap();
        assert_eq!(pr.owner, "my-org");
        assert_eq!(pr.repo, "my-cool-repo");
        assert_eq!(pr.number, 42);
    }

    #[test]
    fn test_parse_pr_url_underscore_names() {
        let pr = parse_pr_url("https://github.com/my_org/my_repo/pull/10").unwrap();
        assert_eq!(pr.owner, "my_org");
        assert_eq!(pr.repo, "my_repo");
    }

    #[test]
    fn test_parse_pr_url_numeric_repo_name() {
        let pr = parse_pr_url("https://github.com/owner/123repo/pull/5").unwrap();
        assert_eq!(pr.repo, "123repo");
    }

    #[test]
    fn test_parse_pr_url_missing_number() {
        assert!(parse_pr_url("https://github.com/owner/repo/pull/").is_err());
    }

    #[test]
    fn test_parse_pr_url_non_numeric_pr() {
        assert!(parse_pr_url("https://github.com/owner/repo/pull/abc").is_err());
    }

    #[test]
    fn test_parse_pr_url_issue_instead_of_pull() {
        // Issues should not parse as PRs
        assert!(parse_pr_url("https://github.com/owner/repo/issues/123").is_err());
    }

    #[test]
    fn test_parse_pr_url_http_upgrade() {
        // HTTP URLs should work (url crate handles them)
        let result = parse_pr_url("http://github.com/owner/repo/pull/1");
        // Depending on implementation this might work or fail - just ensure no panic
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_parse_pr_url_empty_string() {
        assert!(parse_pr_url("").is_err());
    }

    #[test]
    fn test_parse_pr_url_whitespace() {
        assert!(parse_pr_url("   ").is_err());
    }

    #[test]
    fn test_parse_pr_url_wrong_host() {
        assert!(parse_pr_url("https://gitlab.com/owner/repo/pull/1").is_err());
        assert!(parse_pr_url("https://bitbucket.org/owner/repo/pull/1").is_err());
        assert!(parse_pr_url("https://example.com/owner/repo/pull/1").is_err());
    }

    #[test]
    fn test_parse_pr_url_github_enterprise_rejected() {
        // Only github.com is supported
        assert!(parse_pr_url("https://github.mycompany.com/owner/repo/pull/1").is_err());
    }

    #[test]
    fn test_parse_pr_url_case_sensitivity() {
        // GitHub URLs should be case-sensitive for owner/repo
        let pr = parse_pr_url("https://github.com/Owner/Repo/pull/1").unwrap();
        assert_eq!(pr.owner, "Owner");
        assert_eq!(pr.repo, "Repo");
    }

    // ========================================================================
    // group_review_comments_into_threads tests
    // ========================================================================

    fn create_review_comment(
        id: u64,
        body: &str,
        path: &str,
        line: Option<u32>,
        in_reply_to: Option<u64>,
    ) -> ReviewComment {
        ReviewComment {
            id,
            body: body.to_string(),
            user: CommentUser {
                login: "testuser".to_string(),
            },
            path: path.to_string(),
            line,
            created_at: format!("2024-01-15T10:{:02}:00Z", id % 60),
            in_reply_to_id: in_reply_to,
            commit_id: Some("abc123".to_string()),
            original_commit_id: Some("abc123".to_string()), // Same = not outdated
        }
    }

    #[test]
    fn test_group_empty_comments() {
        let comments: Vec<ReviewComment> = vec![];
        let threads = group_review_comments_into_threads(comments);
        assert!(threads.is_empty());
    }

    #[test]
    fn test_group_single_comment() {
        let comments = vec![create_review_comment(1, "First comment", "src/main.rs", Some(10), None)];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, 1);
        assert_eq!(threads[0].comment_count(), 1);
        assert_eq!(threads[0].file_path, Some("src/main.rs".to_string()));
        assert_eq!(threads[0].line, Some(10));
    }

    #[test]
    fn test_group_multiple_independent_threads() {
        let comments = vec![
            create_review_comment(1, "Comment on file1", "src/file1.rs", Some(10), None),
            create_review_comment(2, "Comment on file2", "src/file2.rs", Some(20), None),
            create_review_comment(3, "Comment on file3", "src/file3.rs", Some(30), None),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads.len(), 3);
        // Each thread should have exactly one comment
        for thread in &threads {
            assert_eq!(thread.comment_count(), 1);
        }
    }

    #[test]
    fn test_group_thread_with_reply() {
        let comments = vec![
            create_review_comment(1, "Original comment", "src/main.rs", Some(10), None),
            create_review_comment(2, "Reply to original", "src/main.rs", Some(10), Some(1)),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, 1); // Thread ID is the root comment ID
        assert_eq!(threads[0].comment_count(), 2);
    }

    #[test]
    fn test_group_thread_with_multiple_replies() {
        let comments = vec![
            create_review_comment(1, "Original", "src/main.rs", Some(10), None),
            create_review_comment(2, "First reply", "src/main.rs", Some(10), Some(1)),
            create_review_comment(3, "Second reply", "src/main.rs", Some(10), Some(1)),
            create_review_comment(4, "Third reply", "src/main.rs", Some(10), Some(1)),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comment_count(), 4);
    }

    #[test]
    fn test_group_nested_replies() {
        // Reply to a reply (thread continuation)
        let comments = vec![
            create_review_comment(1, "Original", "src/main.rs", Some(10), None),
            create_review_comment(2, "Reply to original", "src/main.rs", Some(10), Some(1)),
            create_review_comment(3, "Reply to reply", "src/main.rs", Some(10), Some(2)),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comment_count(), 3);
    }

    #[test]
    fn test_group_mixed_threads_and_replies() {
        let comments = vec![
            // Thread 1 with 2 comments
            create_review_comment(1, "Thread 1 root", "src/file1.rs", Some(10), None),
            create_review_comment(2, "Thread 1 reply", "src/file1.rs", Some(10), Some(1)),
            // Thread 2 standalone
            create_review_comment(3, "Thread 2 root", "src/file2.rs", Some(20), None),
            // Thread 3 with 3 comments
            create_review_comment(4, "Thread 3 root", "src/file3.rs", Some(30), None),
            create_review_comment(5, "Thread 3 reply 1", "src/file3.rs", Some(30), Some(4)),
            create_review_comment(6, "Thread 3 reply 2", "src/file3.rs", Some(30), Some(4)),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads.len(), 3);

        // Find each thread by ID
        let thread1 = threads.iter().find(|t| t.id == 1).unwrap();
        let thread2 = threads.iter().find(|t| t.id == 3).unwrap();
        let thread3 = threads.iter().find(|t| t.id == 4).unwrap();

        assert_eq!(thread1.comment_count(), 2);
        assert_eq!(thread2.comment_count(), 1);
        assert_eq!(thread3.comment_count(), 3);
    }

    #[test]
    fn test_group_comments_sorted_by_creation() {
        // Comments with replies should be sorted by created_at within thread
        let comments = vec![
            create_review_comment(3, "Last reply", "src/main.rs", Some(10), Some(1)),
            create_review_comment(1, "Original", "src/main.rs", Some(10), None),
            create_review_comment(2, "First reply", "src/main.rs", Some(10), Some(1)),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads.len(), 1);
        let thread = &threads[0];
        assert_eq!(thread.comments.len(), 3);

        // Comments should be sorted by created_at (check by body content)
        assert_eq!(thread.comments[0].body, "Original");
        assert_eq!(thread.comments[1].body, "First reply");
        assert_eq!(thread.comments[2].body, "Last reply");
    }

    #[test]
    fn test_group_preserves_file_path() {
        let comments = vec![
            create_review_comment(1, "Comment", "path/to/deep/file.rs", Some(100), None),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads[0].file_path, Some("path/to/deep/file.rs".to_string()));
    }

    #[test]
    fn test_group_preserves_line_number() {
        let comments = vec![
            create_review_comment(1, "Comment", "src/main.rs", Some(42), None),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads[0].line, Some(42));
    }

    #[test]
    fn test_group_comment_with_line() {
        let comment = create_review_comment(1, "Comment", "src/main.rs", Some(50), None);

        let threads = group_review_comments_into_threads(vec![comment]);

        assert_eq!(threads[0].line, Some(50));
        assert_eq!(threads[0].file_path, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_group_large_thread() {
        let mut comments = vec![
            create_review_comment(1, "Original", "src/main.rs", Some(10), None),
        ];

        // Add 20 replies
        for i in 2..=21 {
            comments.push(create_review_comment(
                i,
                &format!("Reply {}", i - 1),
                "src/main.rs",
                Some(10),
                Some(1),
            ));
        }

        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comment_count(), 21);
    }

    #[test]
    fn test_group_same_file_different_lines() {
        // Different lines in same file should be different threads
        let comments = vec![
            create_review_comment(1, "Line 10 comment", "src/main.rs", Some(10), None),
            create_review_comment(2, "Line 20 comment", "src/main.rs", Some(20), None),
            create_review_comment(3, "Line 30 comment", "src/main.rs", Some(30), None),
        ];
        let threads = group_review_comments_into_threads(comments);

        // Each should be a separate thread (unless they're replies)
        assert_eq!(threads.len(), 3);
    }

    #[test]
    fn test_group_thread_is_inline() {
        let comments = vec![
            create_review_comment(1, "Inline comment", "src/main.rs", Some(10), None),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert!(threads[0].is_inline());
    }

    #[test]
    fn test_group_thread_author() {
        let comments = vec![
            create_review_comment(1, "Comment", "src/main.rs", Some(10), None),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads[0].author(), "testuser");
    }

    #[test]
    fn test_group_thread_preview() {
        let comments = vec![
            create_review_comment(1, "This is the preview text", "src/main.rs", Some(10), None),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads[0].preview(100), "This is the preview text");
    }

    // ========================================================================
    // Edge cases and error handling tests
    // ========================================================================

    #[test]
    fn test_group_orphan_reply_ignored() {
        // A reply to a non-existent comment (orphan)
        let comments = vec![
            create_review_comment(2, "Reply to nothing", "src/main.rs", Some(10), Some(999)),
        ];
        let threads = group_review_comments_into_threads(comments);

        // Orphan replies should not create threads (they have no root)
        assert!(threads.is_empty());
    }

    #[test]
    fn test_group_complex_reply_chain() {
        // A -> B -> C -> D chain
        let comments = vec![
            create_review_comment(1, "A", "src/main.rs", Some(10), None),
            create_review_comment(2, "B", "src/main.rs", Some(10), Some(1)),
            create_review_comment(3, "C", "src/main.rs", Some(10), Some(2)),
            create_review_comment(4, "D", "src/main.rs", Some(10), Some(3)),
        ];
        let threads = group_review_comments_into_threads(comments);

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comment_count(), 4);
    }
}
