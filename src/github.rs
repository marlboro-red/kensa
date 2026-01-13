use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::process::Stdio;
use tokio::process::Command;
use url::Url;

use crate::types::{CommentThread, IssueComment, PendingComment, PrInfo, ReviewComment, ReviewPr, ThreadComment};

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
    let output = Command::new("gh")
        .args(["auth", "status"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to run 'gh' CLI. Is it installed? (brew install gh)")?;

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

    let diff = String::from_utf8(output.stdout).context("Invalid UTF-8 in diff")?;

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
    url: String,
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

/// Fetch all PRs where review is requested from the current user
pub async fn fetch_review_prs() -> Result<Vec<ReviewPr>> {
    let output = Command::new("gh")
        .args([
            "search",
            "prs",
            "--review-requested=@me",
            "--state=open",
            "--json=number,title,repository,author,createdAt,url",
            "--limit=100",
        ])
        .output()
        .await
        .context("Failed to fetch PRs. Is 'gh' CLI installed?")?;

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
                url: r.url,
                head_sha: None,  // Fetched lazily when needed
            }
        })
        .collect();

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

/// Batch submit inline comments using the Review API (single API call for all inline comments)
async fn submit_inline_comments_batch(pr: &PrInfo, comments: &[&PendingComment], commit_id: &str) -> Result<usize> {
    if comments.is_empty() {
        return Ok(0);
    }

    let repo = format!("{}/{}", pr.owner, pr.repo);

    // Build the comments array for the Review API
    let review_comments: Vec<serde_json::Value> = comments
        .iter()
        .map(|c| {
            let mut comment_obj = serde_json::json!({
                "path": c.file_path.as_ref().unwrap(),
                "line": c.line_number.unwrap(),
                "body": c.body,
                "side": "RIGHT"
            });

            // Add start_line for multi-line comments
            if let Some(start_line) = c.start_line {
                comment_obj["start_line"] = serde_json::json!(start_line);
                comment_obj["start_side"] = serde_json::json!("RIGHT");
            }

            comment_obj
        })
        .collect();

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
    let output = Command::new("gh")
        .args([
            "search",
            "prs",
            "--author=@me",
            "--state=open",
            "--json=number,title,repository,author,createdAt,url",
            "--limit=100",
        ])
        .output()
        .await
        .context("Failed to fetch PRs. Is 'gh' CLI installed?")?;

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
                url: r.url,
                head_sha: None,  // Fetched lazily when needed
            }
        })
        .collect();

    Ok(prs)
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
            start_line: None,
            comments: vec![ThreadComment {
                id: comment.id,
                body: comment.body,
                author: comment.user.login,
                created_at: comment.created_at,
                in_reply_to_id: None,
            }],
            is_resolved: false,
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
                    id: comment.id,
                    body: comment.body.clone(),
                    author: comment.user.login.clone(),
                    created_at: comment.created_at.clone(),
                    in_reply_to_id: comment.in_reply_to_id,
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
                start_line: root.start_line,
                comments: thread_comments,
                is_resolved: false,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
