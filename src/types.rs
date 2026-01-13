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

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // FileStatus tests
    // ========================================================================

    #[test]
    fn test_file_status_badge() {
        assert_eq!(FileStatus::Added.badge(), "[A]");
        assert_eq!(FileStatus::Deleted.badge(), "[D]");
        assert_eq!(FileStatus::Modified.badge(), "[M]");
        assert_eq!(FileStatus::Renamed.badge(), "[R]");
    }

    #[test]
    fn test_file_status_color() {
        use ratatui::style::Color;
        assert_eq!(FileStatus::Added.color(), Color::Green);
        assert_eq!(FileStatus::Deleted.color(), Color::Red);
        assert_eq!(FileStatus::Modified.color(), Color::Yellow);
        assert_eq!(FileStatus::Renamed.color(), Color::Cyan);
    }

    #[test]
    fn test_file_status_clone() {
        let status = FileStatus::Added;
        let cloned = status.clone();
        assert_eq!(status, cloned);
    }

    #[test]
    fn test_file_status_copy() {
        let status = FileStatus::Modified;
        let copied = status;
        assert_eq!(status, copied);
    }

    // ========================================================================
    // DiffFile tests
    // ========================================================================

    fn create_test_diff_file(path: &str, hunks: Vec<Hunk>) -> DiffFile {
        DiffFile {
            path: path.to_string(),
            old_path: None,
            status: FileStatus::Modified,
            hunks,
        }
    }

    #[test]
    fn test_diff_file_filename_simple() {
        let file = create_test_diff_file("src/main.rs", vec![]);
        assert_eq!(file.filename(), "main.rs");
    }

    #[test]
    fn test_diff_file_filename_nested() {
        let file = create_test_diff_file("src/components/ui/Button.tsx", vec![]);
        assert_eq!(file.filename(), "Button.tsx");
    }

    #[test]
    fn test_diff_file_filename_no_directory() {
        let file = create_test_diff_file("README.md", vec![]);
        assert_eq!(file.filename(), "README.md");
    }

    #[test]
    fn test_diff_file_filename_deeply_nested() {
        let file = create_test_diff_file("a/b/c/d/e/f/file.txt", vec![]);
        assert_eq!(file.filename(), "file.txt");
    }

    #[test]
    fn test_diff_file_directory_with_path() {
        let file = create_test_diff_file("src/components/Button.tsx", vec![]);
        assert_eq!(file.directory(), Some("src/components"));
    }

    #[test]
    fn test_diff_file_directory_simple_path() {
        let file = create_test_diff_file("src/main.rs", vec![]);
        assert_eq!(file.directory(), Some("src"));
    }

    #[test]
    fn test_diff_file_directory_no_directory() {
        let file = create_test_diff_file("file.txt", vec![]);
        assert_eq!(file.directory(), None);
    }

    #[test]
    fn test_diff_file_directory_deeply_nested() {
        let file = create_test_diff_file("a/b/c/d/file.txt", vec![]);
        assert_eq!(file.directory(), Some("a/b/c/d"));
    }

    fn create_test_hunk(line_count: usize) -> Hunk {
        let lines: Vec<DiffLine> = (0..line_count)
            .map(|i| DiffLine {
                kind: LineKind::Context,
                content: format!("line {}", i),
                old_ln: Some(i as u32 + 1),
                new_ln: Some(i as u32 + 1),
            })
            .collect();

        Hunk {
            header: "@@ -1,3 +1,3 @@".to_string(),
            old_start: 1,
            old_count: line_count as u32,
            new_start: 1,
            new_count: line_count as u32,
            lines,
        }
    }

    #[test]
    fn test_diff_file_line_count_empty() {
        let file = create_test_diff_file("empty.txt", vec![]);
        assert_eq!(file.line_count(), 0);
    }

    #[test]
    fn test_diff_file_line_count_single_hunk() {
        let file = create_test_diff_file("single.txt", vec![create_test_hunk(5)]);
        assert_eq!(file.line_count(), 5);
    }

    #[test]
    fn test_diff_file_line_count_multiple_hunks() {
        let file = create_test_diff_file(
            "multi.txt",
            vec![create_test_hunk(3), create_test_hunk(4), create_test_hunk(2)],
        );
        assert_eq!(file.line_count(), 9);
    }

    // ========================================================================
    // PendingComment tests
    // ========================================================================

    #[test]
    fn test_pending_comment_new_general() {
        let comment = PendingComment::new_general("Test body".to_string());
        assert_eq!(comment.body, "Test body");
        assert!(comment.file_path.is_none());
        assert!(comment.line_number.is_none());
        assert!(comment.start_line.is_none());
    }

    #[test]
    fn test_pending_comment_new_inline() {
        let comment =
            PendingComment::new_inline("Inline comment".to_string(), "src/main.rs".to_string(), 42);
        assert_eq!(comment.body, "Inline comment");
        assert_eq!(comment.file_path, Some("src/main.rs".to_string()));
        assert_eq!(comment.line_number, Some(42));
        assert!(comment.start_line.is_none());
    }

    #[test]
    fn test_pending_comment_new_multiline() {
        let comment = PendingComment::new_multiline(
            "Multiline comment".to_string(),
            "src/lib.rs".to_string(),
            10,
            20,
        );
        assert_eq!(comment.body, "Multiline comment");
        assert_eq!(comment.file_path, Some("src/lib.rs".to_string()));
        assert_eq!(comment.line_number, Some(20)); // end line
        assert_eq!(comment.start_line, Some(10));
    }

    #[test]
    fn test_pending_comment_is_inline_true() {
        let comment =
            PendingComment::new_inline("test".to_string(), "file.rs".to_string(), 10);
        assert!(comment.is_inline());
    }

    #[test]
    fn test_pending_comment_is_inline_false() {
        let comment = PendingComment::new_general("test".to_string());
        assert!(!comment.is_inline());
    }

    #[test]
    fn test_pending_comment_is_inline_partial() {
        // Has file_path but no line_number
        let comment = PendingComment {
            body: "test".to_string(),
            file_path: Some("file.rs".to_string()),
            line_number: None,
            start_line: None,
        };
        assert!(!comment.is_inline());
    }

    #[test]
    fn test_pending_comment_is_multiline_true() {
        let comment = PendingComment::new_multiline(
            "test".to_string(),
            "file.rs".to_string(),
            5,
            10,
        );
        assert!(comment.is_multiline());
    }

    #[test]
    fn test_pending_comment_is_multiline_false() {
        let comment =
            PendingComment::new_inline("test".to_string(), "file.rs".to_string(), 10);
        assert!(!comment.is_multiline());
    }

    #[test]
    fn test_pending_comment_serialization() {
        let comment = PendingComment::new_inline(
            "Test body".to_string(),
            "src/test.rs".to_string(),
            42,
        );
        let json = serde_json::to_string(&comment).unwrap();
        let deserialized: PendingComment = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.body, comment.body);
        assert_eq!(deserialized.file_path, comment.file_path);
        assert_eq!(deserialized.line_number, comment.line_number);
        assert_eq!(deserialized.start_line, comment.start_line);
    }

    #[test]
    fn test_pending_comment_multiline_serialization() {
        let comment = PendingComment::new_multiline(
            "Multi".to_string(),
            "file.rs".to_string(),
            5,
            15,
        );
        let json = serde_json::to_string(&comment).unwrap();
        let deserialized: PendingComment = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.start_line, Some(5));
        assert_eq!(deserialized.line_number, Some(15));
    }

    // ========================================================================
    // ReviewPr tests
    // ========================================================================

    fn create_test_review_pr() -> ReviewPr {
        ReviewPr {
            number: 123,
            title: "Test PR".to_string(),
            repo_owner: "testowner".to_string(),
            repo_name: "testrepo".to_string(),
            author: "testauthor".to_string(),
            created_at: "2024-01-15T10:30:00Z".to_string(),
            url: "https://github.com/testowner/testrepo/pull/123".to_string(),
            head_sha: Some("abc123".to_string()),
        }
    }

    #[test]
    fn test_review_pr_repo_full_name() {
        let pr = create_test_review_pr();
        assert_eq!(pr.repo_full_name(), "testowner/testrepo");
    }

    #[test]
    fn test_review_pr_to_pr_info() {
        let pr = create_test_review_pr();
        let info = pr.to_pr_info();

        assert_eq!(info.owner, "testowner");
        assert_eq!(info.repo, "testrepo");
        assert_eq!(info.number, 123);
    }

    #[test]
    fn test_review_pr_age_format() {
        // This tests the function runs without panicking
        // The actual time difference depends on current time
        let pr = create_test_review_pr();
        let age = pr.age();

        // Should end with m, h, or d
        assert!(age.ends_with('m') || age.ends_with('h') || age.ends_with('d'));
    }

    #[test]
    fn test_review_pr_age_invalid_date() {
        let mut pr = create_test_review_pr();
        pr.created_at = "invalid date".to_string();
        // Should not panic, returns something reasonable
        let _age = pr.age();
    }

    #[test]
    fn test_review_pr_clone() {
        let pr = create_test_review_pr();
        let cloned = pr.clone();

        assert_eq!(pr.number, cloned.number);
        assert_eq!(pr.title, cloned.title);
        assert_eq!(pr.repo_owner, cloned.repo_owner);
        assert_eq!(pr.head_sha, cloned.head_sha);
    }

    // ========================================================================
    // PrInfo tests
    // ========================================================================

    #[test]
    fn test_pr_info_creation() {
        let info = PrInfo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
            number: 42,
        };

        assert_eq!(info.owner, "owner");
        assert_eq!(info.repo, "repo");
        assert_eq!(info.number, 42);
    }

    #[test]
    fn test_pr_info_clone() {
        let info = PrInfo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
            number: 42,
        };
        let cloned = info.clone();

        assert_eq!(info.owner, cloned.owner);
        assert_eq!(info.repo, cloned.repo);
        assert_eq!(info.number, cloned.number);
    }

    // ========================================================================
    // CommentThread tests
    // ========================================================================

    fn create_test_thread_comment(id: u64, body: &str, author: &str) -> ThreadComment {
        ThreadComment {
            id,
            body: body.to_string(),
            author: author.to_string(),
            created_at: "2024-01-15T10:30:00Z".to_string(),
            in_reply_to_id: None,
        }
    }

    fn create_test_comment_thread(is_inline: bool) -> CommentThread {
        CommentThread {
            id: 1,
            file_path: if is_inline {
                Some("src/main.rs".to_string())
            } else {
                None
            },
            line: if is_inline { Some(42) } else { None },
            start_line: None,
            comments: vec![
                create_test_thread_comment(1, "First comment", "user1"),
                create_test_thread_comment(2, "Second comment", "user2"),
            ],
            is_resolved: false,
        }
    }

    #[test]
    fn test_comment_thread_is_inline_true() {
        let thread = create_test_comment_thread(true);
        assert!(thread.is_inline());
    }

    #[test]
    fn test_comment_thread_is_inline_false() {
        let thread = create_test_comment_thread(false);
        assert!(!thread.is_inline());
    }

    #[test]
    fn test_comment_thread_comment_count() {
        let thread = create_test_comment_thread(true);
        assert_eq!(thread.comment_count(), 2);
    }

    #[test]
    fn test_comment_thread_comment_count_empty() {
        let thread = CommentThread {
            id: 1,
            file_path: None,
            line: None,
            start_line: None,
            comments: vec![],
            is_resolved: false,
        };
        assert_eq!(thread.comment_count(), 0);
    }

    #[test]
    fn test_comment_thread_preview_short() {
        let thread = create_test_comment_thread(true);
        let preview = thread.preview(100);
        assert_eq!(preview, "First comment");
    }

    #[test]
    fn test_comment_thread_preview_truncated() {
        let mut thread = create_test_comment_thread(true);
        thread.comments[0].body = "This is a very long comment that should be truncated".to_string();

        let preview = thread.preview(20);
        assert!(preview.len() <= 20);
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn test_comment_thread_preview_empty() {
        let thread = CommentThread {
            id: 1,
            file_path: None,
            line: None,
            start_line: None,
            comments: vec![],
            is_resolved: false,
        };
        assert_eq!(thread.preview(100), "");
    }

    #[test]
    fn test_comment_thread_preview_multiline_body() {
        let mut thread = create_test_comment_thread(true);
        thread.comments[0].body = "First line\nSecond line\nThird line".to_string();

        let preview = thread.preview(100);
        assert_eq!(preview, "First line"); // Only first line
    }

    #[test]
    fn test_comment_thread_author() {
        let thread = create_test_comment_thread(true);
        assert_eq!(thread.author(), "user1");
    }

    #[test]
    fn test_comment_thread_author_empty() {
        let thread = CommentThread {
            id: 1,
            file_path: None,
            line: None,
            start_line: None,
            comments: vec![],
            is_resolved: false,
        };
        assert_eq!(thread.author(), "unknown");
    }

    // ========================================================================
    // LineKind tests
    // ========================================================================

    #[test]
    fn test_line_kind_equality() {
        assert_eq!(LineKind::Context, LineKind::Context);
        assert_eq!(LineKind::Add, LineKind::Add);
        assert_eq!(LineKind::Del, LineKind::Del);
        assert_eq!(LineKind::Hunk, LineKind::Hunk);
    }

    #[test]
    fn test_line_kind_inequality() {
        assert_ne!(LineKind::Context, LineKind::Add);
        assert_ne!(LineKind::Add, LineKind::Del);
        assert_ne!(LineKind::Del, LineKind::Hunk);
    }

    #[test]
    fn test_line_kind_clone() {
        let kind = LineKind::Add;
        let cloned = kind.clone();
        assert_eq!(kind, cloned);
    }

    // ========================================================================
    // DiffLine tests
    // ========================================================================

    #[test]
    fn test_diff_line_context() {
        let line = DiffLine {
            kind: LineKind::Context,
            content: "unchanged line".to_string(),
            old_ln: Some(5),
            new_ln: Some(5),
        };

        assert_eq!(line.kind, LineKind::Context);
        assert_eq!(line.old_ln, Some(5));
        assert_eq!(line.new_ln, Some(5));
    }

    #[test]
    fn test_diff_line_add() {
        let line = DiffLine {
            kind: LineKind::Add,
            content: "new line".to_string(),
            old_ln: None,
            new_ln: Some(10),
        };

        assert_eq!(line.kind, LineKind::Add);
        assert_eq!(line.old_ln, None);
        assert_eq!(line.new_ln, Some(10));
    }

    #[test]
    fn test_diff_line_del() {
        let line = DiffLine {
            kind: LineKind::Del,
            content: "removed line".to_string(),
            old_ln: Some(8),
            new_ln: None,
        };

        assert_eq!(line.kind, LineKind::Del);
        assert_eq!(line.old_ln, Some(8));
        assert_eq!(line.new_ln, None);
    }

    #[test]
    fn test_diff_line_clone() {
        let line = DiffLine {
            kind: LineKind::Add,
            content: "test".to_string(),
            old_ln: None,
            new_ln: Some(1),
        };
        let cloned = line.clone();

        assert_eq!(line.kind, cloned.kind);
        assert_eq!(line.content, cloned.content);
        assert_eq!(line.old_ln, cloned.old_ln);
        assert_eq!(line.new_ln, cloned.new_ln);
    }

    // ========================================================================
    // Hunk tests
    // ========================================================================

    #[test]
    fn test_hunk_creation() {
        let hunk = Hunk {
            header: "@@ -1,5 +1,7 @@ fn main()".to_string(),
            old_start: 1,
            old_count: 5,
            new_start: 1,
            new_count: 7,
            lines: vec![],
        };

        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 5);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_count, 7);
        assert!(hunk.header.contains("fn main()"));
    }

    #[test]
    fn test_hunk_clone() {
        let hunk = create_test_hunk(3);
        let cloned = hunk.clone();

        assert_eq!(hunk.header, cloned.header);
        assert_eq!(hunk.old_start, cloned.old_start);
        assert_eq!(hunk.lines.len(), cloned.lines.len());
    }

    // ========================================================================
    // CommentUser tests
    // ========================================================================

    #[test]
    fn test_comment_user_deserialize() {
        let json = r#"{"login": "testuser"}"#;
        let user: CommentUser = serde_json::from_str(json).unwrap();
        assert_eq!(user.login, "testuser");
    }

    // ========================================================================
    // ThreadComment tests
    // ========================================================================

    #[test]
    fn test_thread_comment_creation() {
        let comment = ThreadComment {
            id: 12345,
            body: "Test comment body".to_string(),
            author: "testuser".to_string(),
            created_at: "2024-01-15T10:30:00Z".to_string(),
            in_reply_to_id: Some(12344),
        };

        assert_eq!(comment.id, 12345);
        assert_eq!(comment.body, "Test comment body");
        assert_eq!(comment.author, "testuser");
        assert_eq!(comment.in_reply_to_id, Some(12344));
    }

    #[test]
    fn test_thread_comment_clone() {
        let comment = create_test_thread_comment(1, "test", "author");
        let cloned = comment.clone();

        assert_eq!(comment.id, cloned.id);
        assert_eq!(comment.body, cloned.body);
        assert_eq!(comment.author, cloned.author);
    }

    // ========================================================================
    // ReviewComment tests (deserialization)
    // ========================================================================

    #[test]
    fn test_review_comment_deserialize() {
        let json = r#"{
            "id": 12345,
            "body": "Test review comment",
            "user": {"login": "reviewer"},
            "path": "src/main.rs",
            "line": 42,
            "start_line": null,
            "created_at": "2024-01-15T10:30:00Z"
        }"#;

        let comment: ReviewComment = serde_json::from_str(json).unwrap();
        assert_eq!(comment.id, 12345);
        assert_eq!(comment.body, "Test review comment");
        assert_eq!(comment.user.login, "reviewer");
        assert_eq!(comment.path, "src/main.rs");
        assert_eq!(comment.line, Some(42));
        assert!(comment.start_line.is_none());
    }

    #[test]
    fn test_review_comment_deserialize_multiline() {
        let json = r#"{
            "id": 12345,
            "body": "Multiline comment",
            "user": {"login": "reviewer"},
            "path": "src/lib.rs",
            "line": 50,
            "start_line": 40,
            "created_at": "2024-01-15T10:30:00Z"
        }"#;

        let comment: ReviewComment = serde_json::from_str(json).unwrap();
        assert_eq!(comment.line, Some(50));
        assert_eq!(comment.start_line, Some(40));
    }

    #[test]
    fn test_review_comment_deserialize_with_reply() {
        let json = r#"{
            "id": 12346,
            "body": "Reply",
            "user": {"login": "replier"},
            "path": "src/main.rs",
            "line": 42,
            "created_at": "2024-01-15T10:35:00Z",
            "in_reply_to_id": 12345
        }"#;

        let comment: ReviewComment = serde_json::from_str(json).unwrap();
        assert_eq!(comment.in_reply_to_id, Some(12345));
    }

    // ========================================================================
    // IssueComment tests (deserialization)
    // ========================================================================

    #[test]
    fn test_issue_comment_deserialize() {
        let json = r#"{
            "id": 99999,
            "body": "General PR comment",
            "user": {"login": "commenter"},
            "created_at": "2024-01-15T11:00:00Z"
        }"#;

        let comment: IssueComment = serde_json::from_str(json).unwrap();
        assert_eq!(comment.id, 99999);
        assert_eq!(comment.body, "General PR comment");
        assert_eq!(comment.user.login, "commenter");
    }
}
