/// Represents the status of a file in the diff
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

impl FileStatus {
    pub fn badge(&self) -> &'static str {
        match self {
            FileStatus::Added => "[A]",
            FileStatus::Deleted => "[D]",
            FileStatus::Modified => "[M]",
            FileStatus::Renamed => "[R]",
        }
    }

    pub fn color(&self) -> ratatui::style::Color {
        use ratatui::style::Color;
        match self {
            FileStatus::Added => Color::Green,
            FileStatus::Deleted => Color::Red,
            FileStatus::Modified => Color::Yellow,
            FileStatus::Renamed => Color::Cyan,
        }
    }
}

/// Type of a diff line
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

/// A single line in a diff
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    pub old_ln: Option<u32>,
    pub new_ln: Option<u32>,
}

/// A hunk in a diff (a contiguous block of changes)
#[derive(Debug, Clone)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// A file in the diff
#[derive(Debug, Clone)]
pub struct DiffFile {
    pub path: String,
    pub status: FileStatus,
    pub hunks: Vec<Hunk>,
}

impl DiffFile {
    pub fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

/// Parsed PR information from URL
#[derive(Debug, Clone)]
pub struct PrInfo {
    pub owner: String,
    pub repo: String,
    pub number: u32,
}

/// A PR awaiting review
#[derive(Debug, Clone)]
pub struct ReviewPr {
    pub number: u32,
    pub title: String,
    pub repo_owner: String,
    pub repo_name: String,
    pub author: String,
    pub created_at: String,
    pub head_sha: Option<String>,  // For inline comments
}

/// A pending comment to be submitted later
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingComment {
    pub body: String,
    pub file_path: Option<String>,  // For inline comments
    pub line_number: Option<u32>,   // For inline comments (end line for multi-line)
    pub start_line: Option<u32>,    // For multi-line comments
}

impl PendingComment {
    pub fn new_general(body: String) -> Self {
        Self {
            body,
            file_path: None,
            line_number: None,
            start_line: None,
        }
    }

    pub fn new_inline(body: String, file_path: String, line_number: u32) -> Self {
        Self {
            body,
            file_path: Some(file_path),
            line_number: Some(line_number),
            start_line: None,
        }
    }

    pub fn new_multiline(body: String, file_path: String, start_line: u32, end_line: u32) -> Self {
        Self {
            body,
            file_path: Some(file_path),
            line_number: Some(end_line),
            start_line: Some(start_line),
        }
    }

    pub fn is_inline(&self) -> bool {
        self.file_path.is_some() && self.line_number.is_some()
    }
}

impl ReviewPr {
    /// Full repository name (owner/repo)
    pub fn repo_full_name(&self) -> String {
        format!("{}/{}", self.repo_owner, self.repo_name)
    }

    /// Convert to PrInfo for fetching diff
    pub fn to_pr_info(&self) -> PrInfo {
        PrInfo {
            owner: self.repo_owner.clone(),
            repo: self.repo_name.clone(),
            number: self.number,
        }
    }

    /// Format the age of the PR (e.g., "2d", "3h", "5m")
    pub fn age(&self) -> String {
        use std::time::SystemTime;

        // Parse ISO 8601 date
        let created = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.timestamp())
            .unwrap_or(0);

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let diff_secs = now - created;

        if diff_secs < 3600 {
            format!("{}m", diff_secs / 60)
        } else if diff_secs < 86400 {
            format!("{}h", diff_secs / 3600)
        } else {
            format!("{}d", diff_secs / 86400)
        }
    }
}

// ============================================================================
// Comment Thread Types (for viewing existing PR comments)
// ============================================================================

/// A comment author from GitHub API
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CommentUser {
    pub login: String,
}

/// A single comment in a thread (used for both review and issue comments)
#[derive(Debug, Clone)]
pub struct ThreadComment {
    pub body: String,
    pub author: String,
    pub created_at: String,
}

/// A review comment (inline on code) from GitHub API
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub body: String,
    pub user: CommentUser,
    pub path: String,
    pub line: Option<u32>,
    #[serde(rename = "created_at")]
    pub created_at: String,
    #[serde(default)]
    pub in_reply_to_id: Option<u64>,
}

/// An issue comment (general PR comment) from GitHub API
#[derive(Debug, Clone, serde::Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub body: String,
    pub user: CommentUser,
    #[serde(rename = "created_at")]
    pub created_at: String,
}

/// A comment thread (grouped by position or reply chain)
#[derive(Debug, Clone)]
pub struct CommentThread {
    pub id: u64,                      // ID of root comment
    pub file_path: Option<String>,    // None for general PR comments
    pub line: Option<u32>,            // Line number for inline comments
    pub comments: Vec<ThreadComment>, // All comments in thread (root + replies)
}

impl CommentThread {
    /// Check if this is an inline (review) comment thread
    pub fn is_inline(&self) -> bool {
        self.file_path.is_some()
    }

    /// Get the total number of comments in thread
    pub fn comment_count(&self) -> usize {
        self.comments.len()
    }

    /// Get a preview of the thread (first comment body truncated)
    pub fn preview(&self, max_len: usize) -> String {
        self.comments
            .first()
            .map(|c| {
                let first_line = c.body.lines().next().unwrap_or("");
                if first_line.len() > max_len {
                    format!("{}...", &first_line[..max_len.saturating_sub(3)])
                } else {
                    first_line.to_string()
                }
            })
            .unwrap_or_default()
    }

    /// Get the author of the first comment
    pub fn author(&self) -> &str {
        self.comments
            .first()
            .map(|c| c.author.as_str())
            .unwrap_or("unknown")
    }
}
