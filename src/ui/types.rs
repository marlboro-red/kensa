//! UI type definitions for the application state machine.

/// A node in the file tree (either a folder or a file)
#[derive(Debug, Clone)]
pub enum TreeNode {
    Folder {
        name: String,
        path: String,
        children: Vec<TreeNode>,
    },
    File {
        name: String,
        index: usize, // Index into App.files
    },
}

/// A flattened tree item for rendering
#[derive(Debug, Clone)]
pub enum TreeItem {
    Folder {
        path: String,
        name: String,
        depth: usize,
        is_last: bool,
        ancestors_last: Vec<bool>,
    },
    File {
        index: usize,
        name: String,
        depth: usize,
        is_last: bool,
        ancestors_last: Vec<bool>,
    },
}

/// Which screen is currently active
#[derive(Clone, PartialEq, Eq)]
pub enum Screen {
    PrList,
    DiffView,
}

/// Focus state for the diff view UI
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Diff,
}

/// View mode for diff display
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Unified,
    Split,
}

/// Loading state for async operations
#[derive(Clone, PartialEq, Eq)]
pub enum LoadingState {
    Idle,
    Loading(String), // Message to display
    Success(String),
    Error(String),
}

/// Which PR list tab is active
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrListTab {
    ForReview,
    MyPrs,
}

/// Comment input mode
#[derive(Clone, PartialEq, Eq)]
pub enum CommentMode {
    None,
    /// Editing a comment: (text, optional (file_path, start_line, end_line) for inline)
    Editing {
        text: String,
        inline_context: Option<(String, u32, Option<u32>)>, // (path, end_line, optional start_line)
    },
    ViewingPending, // Viewing list of pending comments
    /// Viewing list of existing comment threads
    ViewingThreads {
        selected: usize,
        scroll: usize,
    },
    /// Viewing a single thread's details
    ViewingThread {
        index: usize,
        selected: usize,
        scroll: usize,
    },
    /// Composing a reply to a thread
    ReplyingToThread {
        index: usize,
        text: String,
    },
    /// Submitting a PR review (approve/request changes/comment)
    SubmittingReview {
        selected_action: usize, // 0=Approve, 1=Request Changes, 2=Comment Only
        body: String,           // Optional review comment
        editing_body: bool,     // True when typing in comment area
        reviewing_drafts: bool, // True when reviewing draft comments before submission
        selected_draft: usize,  // Which draft is selected (when reviewing_drafts)
        editing_draft: bool,    // True when editing selected draft text
    },
}

/// Help display state
#[derive(Clone, PartialEq, Eq)]
pub enum HelpMode {
    None,
    PrList,
    DiffView,
}
