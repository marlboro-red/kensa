use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Terminal;

use crate::syntax::Highlighter;
use crate::types::{CommentThread, DiffFile, LineKind, PendingComment, ReviewPr};

/// A node in the file tree (either a folder or a file)
#[derive(Debug, Clone)]
enum TreeNode {
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
enum TreeItem {
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

const BG_COLOR: Color = Color::Rgb(22, 22, 22);
const DEL_BG: Color = Color::Rgb(60, 30, 30);
const ADD_BG: Color = Color::Rgb(30, 60, 30);
const CURSOR_BG: Color = Color::Rgb(45, 45, 65); // Highlight for cursor line
const CURSOR_GUTTER: Color = Color::Rgb(100, 100, 180); // Brighter gutter for cursor

/// Which screen is currently active
#[derive(Clone, PartialEq, Eq)]
pub enum Screen {
    PrList,
    DiffView,
}

/// Focus state for the diff view UI
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
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

/// Application state
pub struct App {
    // Current screen
    screen: Screen,
    loading: LoadingState,

    // PR list tab
    pr_tab: PrListTab,

    // PRs for review
    pub review_prs: Vec<ReviewPr>,
    pub selected_review_pr: usize,
    review_pr_scroll: usize,
    filtered_review_pr_indices: Vec<usize>,

    // My PRs (authored by me)
    pub my_prs: Vec<ReviewPr>,
    pub selected_my_pr: usize,
    my_pr_scroll: usize,
    filtered_my_pr_indices: Vec<usize>,

    // Shared PR list state
    pr_search_mode: bool,
    pr_search_query: String,
    repo_filter: Option<String>, // None = all repos
    available_repos: Vec<String>,
    repo_filter_index: usize, // 0 = all, 1+ = specific repo

    // Diff view state
    pub files: Vec<DiffFile>,
    pub selected_file: usize,
    pub scroll_offset: usize,
    pub view_mode: ViewMode,
    pub collapsed: HashSet<usize>,
    pub highlighter: Highlighter,
    focus: Focus,
    should_quit: bool,
    tree_scroll: usize,
    search_mode: bool,
    search_query: String,
    filtered_indices: Vec<usize>,
    collapsed_folders: HashSet<String>,
    selected_tree_item: Option<String>,

    // For async diff loading
    diff_receiver: Option<mpsc::Receiver<Result<(Vec<DiffFile>, Option<String>), String>>>, // (files, head_sha)
    current_pr: Option<ReviewPr>,

    // Comment drafting
    pending_comments: Vec<PendingComment>,
    comment_mode: CommentMode,
    selected_pending_comment: usize,
    editing_comment_index: Option<usize>, // Index of comment being edited (None = new comment)

    // Cursor position for inline comments (line index in current file's flattened diff)
    diff_cursor: usize,

    // Visual selection for block comments
    visual_mode: bool,
    selection_anchor: usize, // Start of selection (where 'v' was pressed)

    // Help display
    help_mode: HelpMode,

    // Comment threads (existing comments from GitHub)
    comment_threads: Vec<CommentThread>,
    line_to_threads: HashMap<(String, u32), Vec<usize>>, // Quick lookup: (file_path, line) -> thread indices

    // Async receivers for non-blocking operations
    comment_threads_receiver: Option<mpsc::Receiver<Result<Vec<CommentThread>, String>>>,
    pr_list_receiver: Option<mpsc::Receiver<Result<(Vec<ReviewPr>, Vec<ReviewPr>), String>>>,
    comment_submit_receiver: Option<mpsc::Receiver<Result<usize, String>>>,
    reply_submit_receiver: Option<mpsc::Receiver<Result<usize, String>>>, // thread_index on success
    review_submit_receiver: Option<mpsc::Receiver<Result<(String, usize), String>>>, // (review action, comments count) on success
}

impl App {
    /// Create app in diff view mode (for direct PR URL)
    pub fn new(files: Vec<DiffFile>) -> Self {
        let file_count = files.len();
        Self {
            screen: Screen::DiffView,
            loading: LoadingState::Idle,

            // PR list state (empty for direct mode)
            pr_tab: PrListTab::ForReview,
            review_prs: Vec::new(),
            selected_review_pr: 0,
            review_pr_scroll: 0,
            filtered_review_pr_indices: Vec::new(),
            my_prs: Vec::new(),
            selected_my_pr: 0,
            my_pr_scroll: 0,
            filtered_my_pr_indices: Vec::new(),
            pr_search_mode: false,
            pr_search_query: String::new(),
            repo_filter: None,
            available_repos: Vec::new(),
            repo_filter_index: 0,

            // Diff view state
            files,
            selected_file: 0,
            scroll_offset: 0,
            view_mode: ViewMode::Unified,
            collapsed: HashSet::new(),
            highlighter: Highlighter::new(),
            focus: Focus::Tree,
            should_quit: false,
            tree_scroll: 0,
            search_mode: false,
            search_query: String::new(),
            filtered_indices: (0..file_count).collect(),
            collapsed_folders: HashSet::new(),
            selected_tree_item: None,

            diff_receiver: None,
            current_pr: None,

            pending_comments: Vec::new(),
            comment_mode: CommentMode::None,
            selected_pending_comment: 0,
            editing_comment_index: None,

            diff_cursor: 0,
            visual_mode: false,
            selection_anchor: 0,

            help_mode: HelpMode::None,

            comment_threads: Vec::new(),
            line_to_threads: HashMap::new(),

            comment_threads_receiver: None,
            pr_list_receiver: None,
            comment_submit_receiver: None,
            reply_submit_receiver: None,
            review_submit_receiver: None,
        }
    }

    /// Create app in PR list mode
    pub fn new_with_prs(mut review_prs: Vec<ReviewPr>, mut my_prs: Vec<ReviewPr>) -> Self {
        // Sort both lists by repo for proper grouping
        review_prs.sort_by(|a, b| {
            a.repo_full_name()
                .cmp(&b.repo_full_name())
                .then_with(|| b.number.cmp(&a.number)) // Newer PRs first within repo
        });
        my_prs.sort_by(|a, b| {
            a.repo_full_name()
                .cmp(&b.repo_full_name())
                .then_with(|| b.number.cmp(&a.number))
        });

        let review_count = review_prs.len();
        let my_count = my_prs.len();

        // Extract unique repos from both lists and sort them
        let mut repos: Vec<String> = review_prs
            .iter()
            .chain(my_prs.iter())
            .map(|pr| pr.repo_full_name())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        repos.sort();

        Self {
            screen: Screen::PrList,
            loading: LoadingState::Idle,

            // PR list state
            pr_tab: PrListTab::ForReview,
            review_prs,
            selected_review_pr: 0,
            review_pr_scroll: 0,
            filtered_review_pr_indices: (0..review_count).collect(),
            my_prs,
            selected_my_pr: 0,
            my_pr_scroll: 0,
            filtered_my_pr_indices: (0..my_count).collect(),
            pr_search_mode: false,
            pr_search_query: String::new(),
            repo_filter: None,
            available_repos: repos,
            repo_filter_index: 0,

            // Diff view state (empty until a PR is selected)
            files: Vec::new(),
            selected_file: 0,
            scroll_offset: 0,
            view_mode: ViewMode::Unified,
            collapsed: HashSet::new(),
            highlighter: Highlighter::new(),
            focus: Focus::Tree,
            should_quit: false,
            tree_scroll: 0,
            search_mode: false,
            search_query: String::new(),
            filtered_indices: Vec::new(),
            collapsed_folders: HashSet::new(),
            selected_tree_item: None,

            diff_receiver: None,
            current_pr: None,

            pending_comments: Vec::new(),
            comment_mode: CommentMode::None,
            selected_pending_comment: 0,
            editing_comment_index: None,

            diff_cursor: 0,
            visual_mode: false,
            selection_anchor: 0,

            help_mode: HelpMode::None,

            comment_threads: Vec::new(),
            line_to_threads: HashMap::new(),

            comment_threads_receiver: None,
            pr_list_receiver: None,
            comment_submit_receiver: None,
            reply_submit_receiver: None,
            review_submit_receiver: None,
        }
    }

    /// Build a tree structure from the flat file list
    fn build_tree(&self) -> Vec<TreeNode> {
        let mut root: HashMap<String, TreeNode> = HashMap::new();

        // Only include filtered files
        for &file_idx in &self.filtered_indices {
            let file = &self.files[file_idx];
            let parts: Vec<&str> = file.path.split('/').collect();

            if parts.len() == 1 {
                // File at root level
                root.insert(
                    file.path.clone(),
                    TreeNode::File {
                        name: file.path.clone(),
                        index: file_idx,
                    },
                );
            } else {
                // File in a subdirectory - build path
                self.insert_into_tree(&mut root, &parts, file_idx);
            }
        }

        // Convert HashMap to sorted Vec
        let mut nodes: Vec<TreeNode> = root.into_values().collect();
        self.sort_tree_nodes(&mut nodes);
        nodes
    }

    fn insert_into_tree(
        &self,
        root: &mut HashMap<String, TreeNode>,
        parts: &[&str],
        file_idx: usize,
    ) {
        if parts.is_empty() {
            return;
        }

        let first = parts[0];

        if parts.len() == 1 {
            // This is a file
            root.insert(
                first.to_string(),
                TreeNode::File {
                    name: first.to_string(),
                    index: file_idx,
                },
            );
        } else {
            // This is a folder
            let first_folder_path = first.to_string();

            let folder = root
                .entry(first.to_string())
                .or_insert_with(|| TreeNode::Folder {
                    name: first.to_string(),
                    path: first_folder_path,
                    children: Vec::new(),
                });

            if let TreeNode::Folder { children, path, .. } = folder {
                // Update path to be the full path up to this folder
                if parts.len() > 1 {
                    *path = parts[0].to_string();
                }
                let mut child_map: HashMap<String, TreeNode> = children
                    .drain(..)
                    .map(|n| {
                        let key = match &n {
                            TreeNode::Folder { name, .. } => name.clone(),
                            TreeNode::File { name, .. } => name.clone(),
                        };
                        (key, n)
                    })
                    .collect();

                self.insert_into_tree_nested(&mut child_map, &parts[1..], file_idx, &parts[..1]);

                *children = child_map.into_values().collect();
            }
        }
    }

    fn insert_into_tree_nested(
        &self,
        parent: &mut HashMap<String, TreeNode>,
        parts: &[&str],
        file_idx: usize,
        prefix: &[&str],
    ) {
        if parts.is_empty() {
            return;
        }

        let first = parts[0];

        if parts.len() == 1 {
            // This is a file
            parent.insert(
                first.to_string(),
                TreeNode::File {
                    name: first.to_string(),
                    index: file_idx,
                },
            );
        } else {
            // This is a folder
            let mut full_path_parts: Vec<&str> = prefix.to_vec();
            full_path_parts.push(first);
            let folder_path = full_path_parts.join("/");

            let folder = parent
                .entry(first.to_string())
                .or_insert_with(|| TreeNode::Folder {
                    name: first.to_string(),
                    path: folder_path.clone(),
                    children: Vec::new(),
                });

            if let TreeNode::Folder { children, .. } = folder {
                let mut child_map: HashMap<String, TreeNode> = children
                    .drain(..)
                    .map(|n| {
                        let key = match &n {
                            TreeNode::Folder { name, .. } => name.clone(),
                            TreeNode::File { name, .. } => name.clone(),
                        };
                        (key, n)
                    })
                    .collect();

                self.insert_into_tree_nested(
                    &mut child_map,
                    &parts[1..],
                    file_idx,
                    &full_path_parts,
                );

                *children = child_map.into_values().collect();
            }
        }
    }

    fn sort_tree_nodes(&self, nodes: &mut [TreeNode]) {
        nodes.sort_by(|a, b| {
            match (a, b) {
                // Folders come before files
                (TreeNode::Folder { name: a, .. }, TreeNode::Folder { name: b, .. }) => a.cmp(b),
                (TreeNode::File { name: a, .. }, TreeNode::File { name: b, .. }) => a.cmp(b),
                (TreeNode::Folder { .. }, TreeNode::File { .. }) => std::cmp::Ordering::Less,
                (TreeNode::File { .. }, TreeNode::Folder { .. }) => std::cmp::Ordering::Greater,
            }
        });

        for node in nodes.iter_mut() {
            if let TreeNode::Folder { children, .. } = node {
                self.sort_tree_nodes(children);
            }
        }
    }

    /// Flatten the tree into a list of items for rendering
    fn flatten_tree(&self, nodes: &[TreeNode]) -> Vec<TreeItem> {
        let mut items = Vec::new();
        self.flatten_tree_recursive(nodes, 0, &mut items, &[]);
        items
    }

    fn flatten_tree_recursive(
        &self,
        nodes: &[TreeNode],
        depth: usize,
        items: &mut Vec<TreeItem>,
        ancestors_last: &[bool],
    ) {
        let len = nodes.len();
        for (i, node) in nodes.iter().enumerate() {
            let is_last = i == len - 1;
            let mut current_ancestors: Vec<bool> = ancestors_last.to_vec();

            match node {
                TreeNode::Folder {
                    name,
                    path,
                    children,
                } => {
                    items.push(TreeItem::Folder {
                        path: path.clone(),
                        name: name.clone(),
                        depth,
                        is_last,
                        ancestors_last: current_ancestors.clone(),
                    });

                    // Only show children if folder is expanded
                    if !self.collapsed_folders.contains(path) {
                        current_ancestors.push(is_last);
                        self.flatten_tree_recursive(children, depth + 1, items, &current_ancestors);
                    }
                }
                TreeNode::File { name, index } => {
                    items.push(TreeItem::File {
                        index: *index,
                        name: name.clone(),
                        depth,
                        is_last,
                        ancestors_last: current_ancestors,
                    });
                }
            }
        }
    }

    /// Get the tree prefix characters for a given depth and position
    fn get_tree_prefix(&self, depth: usize, is_last: bool, ancestors_last: &[bool]) -> String {
        let mut prefix = String::new();

        for &ancestor_is_last in ancestors_last {
            if ancestor_is_last {
                prefix.push_str("  ");
            } else {
                prefix.push_str("│ ");
            }
        }

        if depth > 0 || !ancestors_last.is_empty() || depth == 0 {
            if is_last {
                prefix.push_str("└─");
            } else {
                prefix.push_str("├─");
            }
        }

        prefix
    }

    pub fn run(&mut self) -> Result<()> {
        let mut terminal = setup_terminal()?;
        let result = self.event_loop(&mut terminal);
        restore_terminal(&mut terminal)?;
        result
    }

    fn event_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        loop {
            // Check for async diff loading completion
            if let Some(ref receiver) = self.diff_receiver {
                if let Ok(result) = receiver.try_recv() {
                    match result {
                        Ok((files, head_sha)) => {
                            let file_count = files.len();
                            self.files = files;
                            self.filtered_indices = (0..file_count).collect();
                            self.selected_file = 0;
                            self.scroll_offset = 0;
                            self.diff_cursor = 0;
                            self.collapsed.clear();
                            self.collapsed_folders.clear();
                            self.selected_tree_item = None;
                            self.screen = Screen::DiffView;
                            self.loading = LoadingState::Idle;
                            // Store head SHA for optimized comment submission
                            if let Some(ref mut pr) = self.current_pr {
                                pr.head_sha = head_sha;
                            }
                            // Load existing comment threads from GitHub (async)
                            self.load_comment_threads();
                        }
                        Err(e) => {
                            self.loading = LoadingState::Error(e);
                        }
                    }
                    self.diff_receiver = None;
                }
            }

            // Check for async comment threads loading completion
            if let Some(ref receiver) = self.comment_threads_receiver {
                if let Ok(result) = receiver.try_recv() {
                    match result {
                        Ok(threads) => {
                            self.build_line_to_threads_map(&threads);
                            self.comment_threads = threads;
                        }
                        Err(e) => {
                            eprintln!("Warning: Failed to load comment threads: {}", e);
                            self.comment_threads = Vec::new();
                            self.line_to_threads.clear();
                            self.line_to_threads.shrink_to_fit();
                        }
                    }
                    self.comment_threads_receiver = None;
                }
            }

            // Check for async PR list refresh completion
            if let Some(ref receiver) = self.pr_list_receiver {
                if let Ok(result) = receiver.try_recv() {
                    match result {
                        Ok((mut review_prs, mut my_prs)) => {
                            // Sort by repo for proper grouping
                            review_prs.sort_by(|a, b| {
                                a.repo_full_name()
                                    .cmp(&b.repo_full_name())
                                    .then_with(|| b.number.cmp(&a.number))
                            });
                            my_prs.sort_by(|a, b| {
                                a.repo_full_name()
                                    .cmp(&b.repo_full_name())
                                    .then_with(|| b.number.cmp(&a.number))
                            });

                            let review_count = review_prs.len();
                            let my_count = my_prs.len();

                            let mut repos: Vec<String> = review_prs
                                .iter()
                                .chain(my_prs.iter())
                                .map(|pr| pr.repo_full_name())
                                .collect::<HashSet<_>>()
                                .into_iter()
                                .collect();
                            repos.sort();

                            self.review_prs = review_prs;
                            self.my_prs = my_prs;
                            self.available_repos = repos;
                            self.filtered_review_pr_indices = (0..review_count).collect();
                            self.filtered_my_pr_indices = (0..my_count).collect();
                            self.selected_review_pr = 0;
                            self.selected_my_pr = 0;
                            self.review_pr_scroll = 0;
                            self.my_pr_scroll = 0;
                            self.repo_filter = None;
                            self.repo_filter_index = 0;
                            self.loading = LoadingState::Idle;
                        }
                        Err(e) => {
                            self.loading = LoadingState::Error(e);
                        }
                    }
                    self.pr_list_receiver = None;
                }
            }

            // Check for async comment submission completion
            if let Some(ref receiver) = self.comment_submit_receiver {
                if let Ok(result) = receiver.try_recv() {
                    match result {
                        Ok(submitted) => {
                            self.pending_comments.clear();
                            self.selected_pending_comment = 0;
                            self.save_current_drafts();
                            self.loading = LoadingState::Success(format!(
                                "Successfully submitted {} comment(s)!",
                                submitted
                            ));
                        }
                        Err(e) => {
                            self.loading = LoadingState::Error(format!("Failed to submit: {}", e));
                        }
                    }
                    self.comment_submit_receiver = None;
                }
            }

            // Check for async reply submission completion
            if let Some(ref receiver) = self.reply_submit_receiver {
                if let Ok(result) = receiver.try_recv() {
                    match result {
                        Ok(thread_index) => {
                            self.loading = LoadingState::Success("Reply submitted!".to_string());
                            // Refresh threads to show new reply
                            self.load_comment_threads();
                            // Go back to threads list
                            self.comment_mode = CommentMode::ViewingThreads {
                                selected: thread_index,
                                scroll: 0,
                            };
                        }
                        Err(e) => {
                            self.loading = LoadingState::Error(format!("Failed: {}", e));
                        }
                    }
                    self.reply_submit_receiver = None;
                }
            }

            // Check for async review submission completion
            if let Some(ref receiver) = self.review_submit_receiver {
                if let Ok(result) = receiver.try_recv() {
                    match result {
                        Ok((event, comments_count)) => {
                            let base_msg = match event.as_str() {
                                "APPROVE" => "PR approved",
                                "REQUEST_CHANGES" => "Changes requested on PR",
                                _ => "Review comment submitted",
                            };
                            let msg = if comments_count > 0 {
                                format!("{} with {} comment(s)!", base_msg, comments_count)
                            } else {
                                format!("{}!", base_msg)
                            };
                            self.loading = LoadingState::Success(msg);
                            // Clear pending comments after successful submission
                            self.pending_comments.clear();
                            self.save_current_drafts();
                        }
                        Err(e) => {
                            self.loading = LoadingState::Error(format!("Failed: {}", e));
                        }
                    }
                    self.review_submit_receiver = None;
                }
            }

            terminal.draw(|f| self.render(f))?;

            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key);
                    }
                }
            }

            if self.should_quit {
                break;
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // If loading, only allow quit
        if matches!(self.loading, LoadingState::Loading(_)) {
            if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                self.should_quit = true;
            }
            return;
        }

        // Clear error or success on any key
        if matches!(
            self.loading,
            LoadingState::Error(_) | LoadingState::Success(_)
        ) {
            self.loading = LoadingState::Idle;
            return;
        }

        match self.screen {
            Screen::PrList => self.handle_key_pr_list(key),
            Screen::DiffView => self.handle_key_diff_view(key),
        }
    }

    fn handle_key_pr_list(&mut self, key: KeyEvent) {
        // Handle help mode
        if self.help_mode != HelpMode::None {
            self.help_mode = HelpMode::None;
            return;
        }

        // Handle search mode input
        if self.pr_search_mode {
            match key.code {
                KeyCode::Esc => {
                    self.pr_search_mode = false;
                    self.pr_search_query.clear();
                    self.update_filtered_pr_indices();
                }
                KeyCode::Enter => {
                    self.pr_search_mode = false;
                }
                KeyCode::Backspace => {
                    self.pr_search_query.pop();
                    self.update_filtered_pr_indices();
                }
                KeyCode::Char(c) => {
                    self.pr_search_query.push(c);
                    self.update_filtered_pr_indices();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => {
                if self.repo_filter.is_some() {
                    // Clear repo filter
                    self.repo_filter = None;
                    self.repo_filter_index = 0;
                    self.update_filtered_pr_indices();
                } else if !self.pr_search_query.is_empty() {
                    // Clear search
                    self.pr_search_query.clear();
                    self.update_filtered_pr_indices();
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('/') => {
                self.pr_search_mode = true;
                self.pr_search_query.clear();
            }
            KeyCode::Char('1') => {
                self.pr_tab = PrListTab::ForReview;
                self.update_filtered_pr_indices();
            }
            KeyCode::Char('2') => {
                self.pr_tab = PrListTab::MyPrs;
                self.update_filtered_pr_indices();
            }
            KeyCode::Tab => self.toggle_pr_tab(),
            KeyCode::Char('j') | KeyCode::Down => self.move_pr_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_pr_up(),
            KeyCode::Char('f') => self.cycle_repo_filter(),
            KeyCode::Enter => self.select_pr(),
            KeyCode::Char('o') => self.open_selected_pr_in_browser(),
            KeyCode::Char('R') => self.refresh_pr_list(),
            KeyCode::Char('?') => self.help_mode = HelpMode::PrList,
            _ => {}
        }
    }

    fn toggle_pr_tab(&mut self) {
        self.pr_tab = match self.pr_tab {
            PrListTab::ForReview => PrListTab::MyPrs,
            PrListTab::MyPrs => PrListTab::ForReview,
        };
        self.pr_search_query.clear();
        self.update_filtered_pr_indices();
    }

    fn handle_key_diff_view(&mut self, key: KeyEvent) {
        // Handle help mode
        if self.help_mode != HelpMode::None {
            self.help_mode = HelpMode::None;
            return;
        }

        // Handle comment editing mode input
        if let CommentMode::Editing {
            ref mut text,
            ref inline_context,
        } = self.comment_mode
        {
            // Check for save shortcuts: Ctrl+Enter, Ctrl+S, or Alt+Enter
            let is_save = match key.code {
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => true,
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => true,
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
                _ => false,
            };

            if is_save {
                // Submit the comment
                if !text.is_empty() {
                    let comment = match inline_context.clone() {
                        Some((path, end_line, Some(start_line))) => {
                            PendingComment::new_multiline(text.clone(), path, start_line, end_line)
                        }
                        Some((path, line, None)) => {
                            PendingComment::new_inline(text.clone(), path, line)
                        }
                        None => PendingComment::new_general(text.clone()),
                    };
                    // If editing an existing comment, replace it; otherwise add new
                    if let Some(idx) = self.editing_comment_index {
                        if idx < self.pending_comments.len() {
                            self.pending_comments[idx] = comment;
                        }
                    } else {
                        self.pending_comments.push(comment);
                    }
                    self.save_current_drafts();
                }
                self.comment_mode = CommentMode::None;
                self.editing_comment_index = None;
                return;
            }

            match key.code {
                KeyCode::Esc => {
                    self.comment_mode = CommentMode::None;
                    self.editing_comment_index = None;
                }
                KeyCode::Enter => {
                    // Add newline
                    text.push('\n');
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) => {
                    text.push(c);
                }
                _ => {}
            }
            return;
        }

        // Handle viewing pending comments mode
        if self.comment_mode == CommentMode::ViewingPending {
            match key.code {
                KeyCode::Esc | KeyCode::Char('C') => {
                    self.comment_mode = CommentMode::None;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if !self.pending_comments.is_empty() {
                        self.selected_pending_comment =
                            (self.selected_pending_comment + 1) % self.pending_comments.len();
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if !self.pending_comments.is_empty() {
                        self.selected_pending_comment = self
                            .selected_pending_comment
                            .checked_sub(1)
                            .unwrap_or(self.pending_comments.len().saturating_sub(1));
                    }
                }
                KeyCode::Char('d') | KeyCode::Delete => {
                    // Delete selected comment
                    if !self.pending_comments.is_empty() {
                        self.pending_comments.remove(self.selected_pending_comment);
                        if self.selected_pending_comment >= self.pending_comments.len() {
                            self.selected_pending_comment =
                                self.pending_comments.len().saturating_sub(1);
                        }
                        self.save_current_drafts();
                        if self.pending_comments.is_empty() {
                            self.comment_mode = CommentMode::None;
                        }
                    }
                }
                KeyCode::Char('S') => {
                    // Submit all pending comments
                    self.submit_pending_comments();
                }
                KeyCode::Char('e') | KeyCode::Enter => {
                    // Edit selected comment
                    if !self.pending_comments.is_empty() {
                        let comment = &self.pending_comments[self.selected_pending_comment];
                        let inline_context = if let (Some(path), Some(line)) =
                            (comment.file_path.clone(), comment.line_number)
                        {
                            Some((path, line, comment.start_line))
                        } else {
                            None
                        };
                        self.editing_comment_index = Some(self.selected_pending_comment);
                        self.comment_mode = CommentMode::Editing {
                            text: comment.body.clone(),
                            inline_context,
                        };
                    }
                }
                _ => {}
            }
            return;
        }

        // Handle viewing threads list mode
        if let CommentMode::ViewingThreads {
            ref mut selected,
            scroll: _,
        } = self.comment_mode
        {
            match key.code {
                KeyCode::Esc => {
                    self.comment_mode = CommentMode::None;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if !self.comment_threads.is_empty()
                        && *selected < self.comment_threads.len() - 1
                    {
                        *selected += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Enter => {
                    // Open thread detail view
                    let idx = *selected;
                    self.comment_mode = CommentMode::ViewingThread {
                        index: idx,
                        selected: 0,
                        scroll: 0,
                    };
                }
                KeyCode::Char('r') => {
                    // Start reply
                    let idx = *selected;
                    self.comment_mode = CommentMode::ReplyingToThread {
                        index: idx,
                        text: String::new(),
                    };
                }
                KeyCode::Char('g') => {
                    // Jump to thread location in diff
                    let idx = *selected;
                    // Extract data before mutable operations
                    let thread_info = self
                        .comment_threads
                        .get(idx)
                        .and_then(|t| t.file_path.as_ref().map(|p| (p.clone(), t.line)));

                    if let Some((path, line)) = thread_info {
                        // Find file index and switch to it
                        if let Some(file_idx) = self.files.iter().position(|f| f.path == path) {
                            self.select_file(file_idx);
                            self.focus = Focus::Diff;
                            // Try to scroll to the line
                            if let Some(ln) = line {
                                self.scroll_offset = (ln as usize).saturating_sub(5);
                            }
                        }
                        self.comment_mode = CommentMode::None;
                    }
                }
                _ => {}
            }
            return;
        }

        // Handle viewing single thread detail mode
        if let CommentMode::ViewingThread {
            index,
            selected: _,
            ref mut scroll,
        } = self.comment_mode
        {
            // Calculate max scroll (total lines - visible height)
            // Use approximate visible height of 20 lines for popup
            let max_scroll = self.comment_threads.get(index).map(|thread| {
                let wrap_width = 80; // Approximate wrap width
                let mut total_lines = 0;
                for comment in &thread.comments {
                    total_lines += 1; // Header
                    total_lines += Self::wrap_text(&comment.body, wrap_width).len();
                    total_lines += 1; // Separator
                }
                total_lines.saturating_sub(20) // Approximate visible height
            }).unwrap_or(0);

            match key.code {
                KeyCode::Esc => {
                    // Go back to thread list
                    let idx = index;
                    self.comment_mode = CommentMode::ViewingThreads {
                        selected: idx,
                        scroll: 0,
                    };
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    *scroll = scroll.saturating_add(1).min(max_scroll);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    *scroll = scroll.saturating_sub(1);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    *scroll = scroll.saturating_add(10).min(max_scroll);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    *scroll = scroll.saturating_sub(10);
                }
                KeyCode::Char('r') => {
                    let idx = index;
                    self.comment_mode = CommentMode::ReplyingToThread {
                        index: idx,
                        text: String::new(),
                    };
                }
                _ => {}
            }
            return;
        }

        // Handle reply to thread mode
        if let CommentMode::ReplyingToThread {
            index,
            ref mut text,
        } = self.comment_mode
        {
            // Check for save shortcuts: Ctrl+Enter, Ctrl+S, or Alt+Enter
            let is_save = match key.code {
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => true,
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => true,
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
                _ => false,
            };

            if is_save && !text.is_empty() {
                let idx = index;
                let reply_text = text.clone();
                self.submit_thread_reply(idx, &reply_text);
                return;
            }

            match key.code {
                KeyCode::Esc => {
                    let idx = index;
                    self.comment_mode = CommentMode::ViewingThread {
                        index: idx,
                        selected: 0,
                        scroll: 0,
                    };
                }
                KeyCode::Enter => {
                    text.push('\n');
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) => {
                    text.push(c);
                }
                _ => {}
            }
            return;
        }

        // Handle submitting review mode
        if let CommentMode::SubmittingReview {
            ref mut selected_action,
            ref mut body,
            ref mut editing_body,
            ref mut reviewing_drafts,
            ref mut selected_draft,
            ref mut editing_draft,
        } = self.comment_mode
        {
            // Check for submit shortcut: Ctrl+Enter or Alt+Enter
            let is_submit = match key.code {
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => true,
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => true,
                _ => false,
            };

            if is_submit {
                // If we have drafts and haven't reviewed them yet, switch to draft review mode
                if !*reviewing_drafts && !self.pending_comments.is_empty() {
                    *reviewing_drafts = true;
                    *selected_draft = 0;
                    return;
                }
                // Otherwise submit the review with all drafts
                let action = *selected_action;
                let review_body = if body.is_empty() {
                    None
                } else {
                    Some(body.clone())
                };
                self.submit_review(action, review_body);
                return;
            }

            if *editing_draft {
                // Editing a draft comment's text
                let draft_idx = *selected_draft;
                if draft_idx < self.pending_comments.len() {
                    match key.code {
                        KeyCode::Esc => {
                            *editing_draft = false;
                            self.save_current_drafts();
                        }
                        KeyCode::Enter => {
                            self.pending_comments[draft_idx].body.push('\n');
                        }
                        KeyCode::Backspace => {
                            self.pending_comments[draft_idx].body.pop();
                        }
                        KeyCode::Char(c) => {
                            self.pending_comments[draft_idx].body.push(c);
                        }
                        _ => {}
                    }
                }
                return;
            }

            if *reviewing_drafts {
                // In draft review mode
                let draft_count = self.pending_comments.len();
                match key.code {
                    KeyCode::Esc => {
                        // Go back to action selection
                        *reviewing_drafts = false;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if draft_count > 0 {
                            *selected_draft = (*selected_draft + 1) % draft_count;
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if draft_count > 0 {
                            *selected_draft = selected_draft
                                .checked_sub(1)
                                .unwrap_or(draft_count.saturating_sub(1));
                        }
                    }
                    KeyCode::Char('e') | KeyCode::Enter => {
                        // Edit selected draft
                        if draft_count > 0 {
                            *editing_draft = true;
                        }
                    }
                    KeyCode::Char('d') | KeyCode::Char('x') => {
                        // Delete selected draft
                        if draft_count > 0 && *selected_draft < draft_count {
                            self.pending_comments.remove(*selected_draft);
                            if *selected_draft >= self.pending_comments.len()
                                && !self.pending_comments.is_empty()
                            {
                                *selected_draft = self.pending_comments.len() - 1;
                            }
                            self.save_current_drafts();
                        }
                    }
                    _ => {}
                }
                return;
            }

            if *editing_body {
                // In comment editing mode - all keys go to text input
                match key.code {
                    KeyCode::Esc => {
                        // Exit comment editing, go back to action selection
                        *editing_body = false;
                    }
                    KeyCode::Enter => {
                        body.push('\n');
                    }
                    KeyCode::Backspace => {
                        body.pop();
                    }
                    KeyCode::Char(c) => {
                        body.push(c);
                    }
                    _ => {}
                }
            } else {
                // In action selection mode
                match key.code {
                    KeyCode::Esc => {
                        self.comment_mode = CommentMode::None;
                    }
                    KeyCode::Char('1') => {
                        *selected_action = 0; // Approve
                    }
                    KeyCode::Char('2') => {
                        *selected_action = 1; // Request Changes
                    }
                    KeyCode::Char('3') => {
                        *selected_action = 2; // Comment Only
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        *selected_action = (*selected_action + 1) % 3;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        *selected_action = selected_action.checked_sub(1).unwrap_or(2);
                    }
                    KeyCode::Char('c') | KeyCode::Enter => {
                        // Enter comment editing mode
                        *editing_body = true;
                    }
                    KeyCode::Char('d') => {
                        // Go directly to draft review if there are drafts
                        if !self.pending_comments.is_empty() {
                            *reviewing_drafts = true;
                            *selected_draft = 0;
                        }
                    }
                    _ => {}
                }
            }
            return;
        }

        // Handle search mode input
        if self.search_mode {
            match key.code {
                KeyCode::Esc => {
                    self.search_mode = false;
                    self.search_query.clear();
                    self.update_filtered_indices();
                }
                KeyCode::Enter => {
                    self.search_mode = false;
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.update_filtered_indices();
                }
                KeyCode::Char(c) => {
                    self.search_query.push(c);
                    self.update_filtered_indices();
                }
                _ => {}
            }
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') => {
                    self.scroll_half_page_up();
                    return;
                }
                KeyCode::Char('d') => {
                    self.scroll_half_page_down();
                    return;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Char('q') => {
                // If we came from PR list, go back; otherwise quit
                if !self.review_prs.is_empty() {
                    self.screen = Screen::PrList;
                    self.current_pr = None;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Esc => {
                if self.visual_mode {
                    // Exit visual mode first
                    self.visual_mode = false;
                } else if !self.filtered_indices.is_empty()
                    && self.filtered_indices.len() != self.files.len()
                {
                    self.search_query.clear();
                    self.update_filtered_indices();
                } else if !self.review_prs.is_empty() {
                    self.screen = Screen::PrList;
                    self.current_pr = None;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('/') => {
                self.search_mode = true;
                self.search_query.clear();
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Char('h') | KeyCode::Left => self.prev_file(),
            KeyCode::Char('l') | KeyCode::Right => self.next_file(),
            KeyCode::Enter | KeyCode::Tab => self.toggle_focus(),
            KeyCode::Char('d') => self.toggle_view_mode(),
            KeyCode::Char('x') => self.toggle_collapse(),
            KeyCode::Char('g') => self.scroll_to_top(),
            KeyCode::Char('G') => self.scroll_to_bottom(),
            KeyCode::Char('o') => self.open_pr_in_browser(),
            KeyCode::Char('v') => {
                // Toggle visual mode
                if self.focus == Focus::Diff {
                    if self.visual_mode {
                        self.visual_mode = false;
                    } else {
                        self.visual_mode = true;
                        self.selection_anchor = self.diff_cursor;
                    }
                }
            }
            KeyCode::Char('c') => {
                // Start new comment
                if self.current_pr.is_some() {
                    let inline_context = if self.focus == Focus::Diff {
                        if self.visual_mode {
                            // Multi-line selection
                            if let Some((path, start, end)) = self.get_selection_line_info() {
                                if start == end {
                                    Some((path, end, None)) // Single line
                                } else {
                                    Some((path, end, Some(start))) // Range
                                }
                            } else {
                                None
                            }
                        } else {
                            // Single line
                            self.get_cursor_line_info().map(|(p, l)| (p, l, None))
                        }
                    } else {
                        None
                    };
                    self.visual_mode = false; // Exit visual mode when commenting
                    self.comment_mode = CommentMode::Editing {
                        text: String::new(),
                        inline_context,
                    };
                }
            }
            KeyCode::Char('C') => {
                // View pending comments
                if !self.pending_comments.is_empty() {
                    self.comment_mode = CommentMode::ViewingPending;
                    self.selected_pending_comment = 0;
                }
            }
            KeyCode::Char('S') => {
                // Submit all pending comments
                if !self.pending_comments.is_empty() && self.current_pr.is_some() {
                    self.submit_pending_comments();
                }
            }
            KeyCode::Char('t') => {
                // View comment threads
                if self.current_pr.is_some() && !self.comment_threads.is_empty() {
                    self.comment_mode = CommentMode::ViewingThreads {
                        selected: 0,
                        scroll: 0,
                    };
                }
            }
            KeyCode::Char('T') => {
                // Refresh comment threads
                if self.current_pr.is_some() {
                    self.load_comment_threads();
                }
            }
            KeyCode::Char('A') => {
                // Open review submission modal
                if self.current_pr.is_some() {
                    self.comment_mode = CommentMode::SubmittingReview {
                        selected_action: 0,
                        body: String::new(),
                        editing_body: false,
                        reviewing_drafts: false,
                        selected_draft: 0,
                        editing_draft: false,
                    };
                }
            }
            KeyCode::Char('?') => {
                self.help_mode = HelpMode::DiffView;
            }
            _ => {}
        }
    }

    fn open_pr_in_browser(&self) {
        if let Some(ref pr) = self.current_pr {
            self.open_pr_url_in_browser(&pr.repo_full_name(), pr.number);
        }
    }

    fn open_selected_pr_in_browser(&self) {
        let indices = self.current_filtered_indices();
        if indices.is_empty() {
            return;
        }
        let pr_list = self.current_pr_list();
        let selected = self.current_selected_pr();
        let pr = &pr_list[selected];
        self.open_pr_url_in_browser(&pr.repo_full_name(), pr.number);
    }

    fn open_pr_url_in_browser(&self, repo: &str, number: u32) {
        // Use gh CLI to open PR in browser
        let _ = std::process::Command::new("gh")
            .args(["pr", "view", &number.to_string(), "--repo", repo, "--web"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    // PR list navigation methods
    fn current_pr_list(&self) -> &Vec<ReviewPr> {
        match self.pr_tab {
            PrListTab::ForReview => &self.review_prs,
            PrListTab::MyPrs => &self.my_prs,
        }
    }

    fn current_filtered_indices(&self) -> &Vec<usize> {
        match self.pr_tab {
            PrListTab::ForReview => &self.filtered_review_pr_indices,
            PrListTab::MyPrs => &self.filtered_my_pr_indices,
        }
    }

    fn current_selected_pr(&self) -> usize {
        match self.pr_tab {
            PrListTab::ForReview => self.selected_review_pr,
            PrListTab::MyPrs => self.selected_my_pr,
        }
    }

    fn set_current_selected_pr(&mut self, idx: usize) {
        match self.pr_tab {
            PrListTab::ForReview => self.selected_review_pr = idx,
            PrListTab::MyPrs => self.selected_my_pr = idx,
        }
    }

    fn move_pr_down(&mut self) {
        let indices = self.current_filtered_indices();
        if indices.is_empty() {
            return;
        }
        let selected = self.current_selected_pr();
        if let Some(pos) = indices.iter().position(|&i| i == selected) {
            if pos < indices.len() - 1 {
                self.set_current_selected_pr(indices[pos + 1]);
            }
        }
    }

    fn move_pr_up(&mut self) {
        let indices = self.current_filtered_indices();
        if indices.is_empty() {
            return;
        }
        let selected = self.current_selected_pr();
        if let Some(pos) = indices.iter().position(|&i| i == selected) {
            if pos > 0 {
                self.set_current_selected_pr(indices[pos - 1]);
            }
        }
    }

    fn cycle_repo_filter(&mut self) {
        if self.available_repos.is_empty() {
            return;
        }

        self.repo_filter_index = (self.repo_filter_index + 1) % (self.available_repos.len() + 1);

        if self.repo_filter_index == 0 {
            self.repo_filter = None;
        } else {
            self.repo_filter = Some(self.available_repos[self.repo_filter_index - 1].clone());
        }

        self.update_filtered_pr_indices();
    }

    fn update_filtered_pr_indices(&mut self) {
        let query = self.pr_search_query.to_lowercase();

        // Update review PRs filter
        self.filtered_review_pr_indices = self
            .review_prs
            .iter()
            .enumerate()
            .filter(|(_, pr)| self.pr_matches_filter(pr, &query))
            .map(|(i, _)| i)
            .collect();

        // Update my PRs filter
        self.filtered_my_pr_indices = self
            .my_prs
            .iter()
            .enumerate()
            .filter(|(_, pr)| self.pr_matches_filter(pr, &query))
            .map(|(i, _)| i)
            .collect();

        // Ensure selected PR is in filtered list for current tab
        match self.pr_tab {
            PrListTab::ForReview => {
                if !self
                    .filtered_review_pr_indices
                    .contains(&self.selected_review_pr)
                {
                    if let Some(&first) = self.filtered_review_pr_indices.first() {
                        self.selected_review_pr = first;
                    }
                }
                self.review_pr_scroll = 0;
            }
            PrListTab::MyPrs => {
                if !self.filtered_my_pr_indices.contains(&self.selected_my_pr) {
                    if let Some(&first) = self.filtered_my_pr_indices.first() {
                        self.selected_my_pr = first;
                    }
                }
                self.my_pr_scroll = 0;
            }
        }
    }

    fn pr_matches_filter(&self, pr: &ReviewPr, query: &str) -> bool {
        // Filter by repo
        if let Some(ref filter) = self.repo_filter {
            if &pr.repo_full_name() != filter {
                return false;
            }
        }
        // Filter by search query
        if !query.is_empty() {
            let searchable = format!(
                "{} {} {} #{}",
                pr.title.to_lowercase(),
                pr.author.to_lowercase(),
                pr.repo_full_name().to_lowercase(),
                pr.number
            );
            if !searchable.contains(query) {
                return false;
            }
        }
        true
    }

    fn select_pr(&mut self) {
        let indices = self.current_filtered_indices();
        if indices.is_empty() {
            return;
        }

        let pr_list = self.current_pr_list();
        let selected = self.current_selected_pr();
        let pr = pr_list[selected].clone();
        self.current_pr = Some(pr.clone());
        self.load_current_drafts(); // Load any saved drafts for this PR
        self.loading =
            LoadingState::Loading(format!("Loading {}#{} ...", pr.repo_full_name(), pr.number));

        // Start async diff fetch
        let pr_info = pr.to_pr_info();
        let (tx, rx) = mpsc::channel();
        self.diff_receiver = Some(rx);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                // Fetch diff and head SHA in parallel
                let (diff_result, sha_result) = tokio::join!(
                    crate::github::fetch_pr_diff(&pr_info),
                    crate::github::fetch_pr_head_sha(&pr_info)
                );

                // Diff is required, head SHA is optional (for comment optimization)
                match diff_result {
                    Ok(diff_content) => {
                        let files = crate::parser::parse_diff(&diff_content);
                        let head_sha = sha_result.ok();
                        Ok((files, head_sha))
                    }
                    Err(e) => Err(e.to_string()),
                }
            });

            let _ = tx.send(result);
        });
    }

    fn refresh_pr_list(&mut self) {
        // Non-blocking async refresh - results are processed in event_loop
        self.loading = LoadingState::Loading("Refreshing PRs...".to_string());

        let (tx, rx) = mpsc::channel();
        self.pr_list_receiver = Some(rx);

        // Fetch both lists in parallel in a single background thread
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                let (review_result, my_result) = tokio::join!(
                    crate::github::fetch_review_prs(),
                    crate::github::fetch_my_prs()
                );

                match (review_result, my_result) {
                    (Ok(review_prs), Ok(my_prs)) => Ok((review_prs, my_prs)),
                    (Err(e), _) => Err(e.to_string()),
                    (_, Err(e)) => Err(e.to_string()),
                }
            });

            let _ = tx.send(result);
        });
    }

    fn submit_pending_comments(&mut self) {
        let Some(ref pr) = self.current_pr else {
            return;
        };

        if self.pending_comments.is_empty() {
            return;
        }

        let pr_info = pr.to_pr_info();
        let comments = self.pending_comments.clone();
        let count = comments.len();
        let head_sha = pr.head_sha.clone();

        self.loading = LoadingState::Loading(format!("Submitting {} comment(s)...", count));
        self.comment_mode = CommentMode::None;

        let (tx, rx) = mpsc::channel();
        self.comment_submit_receiver = Some(rx);

        // Submit comments in a separate thread - results processed in event_loop
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                crate::github::submit_pr_comments(&pr_info, &comments, head_sha.as_deref()).await
            });

            let _ = tx.send(result.map_err(|e| e.to_string()));
        });
    }

    /// Submit a reply to an existing comment thread
    fn submit_thread_reply(&mut self, thread_index: usize, body: &str) {
        let Some(ref pr) = self.current_pr else {
            return;
        };
        let Some(thread) = self.comment_threads.get(thread_index).cloned() else {
            return;
        };

        let pr_info = pr.to_pr_info();
        let body_clone = body.to_string();

        self.loading = LoadingState::Loading("Submitting reply...".to_string());

        let (tx, rx) = mpsc::channel();
        self.reply_submit_receiver = Some(rx);

        // Submit in a separate thread - results processed in event_loop
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(crate::github::submit_thread_reply(
                &pr_info,
                &thread,
                &body_clone,
            ));

            // Return thread_index on success so we can navigate back to the right thread
            let _ = tx.send(result.map(|_| thread_index).map_err(|e| e.to_string()));
        });
    }

    /// Submit a PR review (approve/request changes/comment) with all pending comments
    fn submit_review(&mut self, action: usize, body: Option<String>) {
        let Some(ref pr) = self.current_pr else {
            return;
        };

        let event = match action {
            0 => "APPROVE",
            1 => "REQUEST_CHANGES",
            _ => "COMMENT",
        };

        let action_label = match action {
            0 => "Approving",
            1 => "Requesting changes on",
            _ => "Commenting on",
        };

        let comments_count = self.pending_comments.len();
        let loading_msg = if comments_count > 0 {
            format!("{} PR with {} comment(s)...", action_label, comments_count)
        } else {
            format!("{} PR...", action_label)
        };

        let pr_info = pr.to_pr_info();
        let head_sha = pr.head_sha.clone();
        let pending_comments = self.pending_comments.clone();

        self.loading = LoadingState::Loading(loading_msg);
        self.comment_mode = CommentMode::None;

        let (tx, rx) = mpsc::channel();
        self.review_submit_receiver = Some(rx);

        let event_str = event.to_string();
        let body_clone = body;

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let comments_opt = if pending_comments.is_empty() {
                None
            } else {
                Some(pending_comments.as_slice())
            };
            let result = rt.block_on(crate::github::submit_pr_review(
                &pr_info,
                &event_str,
                body_clone.as_deref(),
                comments_opt,
                head_sha.as_deref(),
            ));

            // Return (event, comments_submitted) on success
            let _ = tx.send(
                result
                    .map(|count| (event_str, count))
                    .map_err(|e| e.to_string()),
            );
        });
    }

    /// Save current drafts to disk
    fn save_current_drafts(&self) {
        if let Some(ref pr) = self.current_pr {
            let pr_info = pr.to_pr_info();
            if let Err(e) = crate::drafts::save_drafts(&pr_info, &self.pending_comments) {
                // Silently ignore save errors - drafts are best-effort
                eprintln!("Warning: Failed to save drafts: {}", e);
            }
        }
    }

    /// Load drafts for the current PR from disk
    fn load_current_drafts(&mut self) {
        if let Some(ref pr) = self.current_pr {
            let pr_info = pr.to_pr_info();
            self.pending_comments = crate::drafts::load_drafts(&pr_info);
            self.selected_pending_comment = 0;
        }
    }

    /// Load comment threads for the current PR from GitHub (non-blocking)
    fn load_comment_threads(&mut self) {
        let Some(ref pr) = self.current_pr else {
            return;
        };

        let pr_info = pr.to_pr_info();

        let (tx, rx) = mpsc::channel();
        self.comment_threads_receiver = Some(rx);

        // Spawn a thread to fetch comments - results processed in event_loop
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(crate::github::fetch_all_comment_threads(&pr_info));

            let _ = tx.send(result.map_err(|e| e.to_string()));
        });
    }

    /// Build lookup map from (file, line) to thread indices
    fn build_line_to_threads_map(&mut self, threads: &[CommentThread]) {
        // Clear and shrink to prevent unbounded growth
        self.line_to_threads.clear();
        self.line_to_threads.shrink_to_fit();

        // Reserve capacity based on expected size
        self.line_to_threads.reserve(threads.len());

        for (idx, thread) in threads.iter().enumerate() {
            if let (Some(path), Some(line)) = (&thread.file_path, thread.line) {
                self.line_to_threads
                    .entry((path.clone(), line))
                    .or_default()
                    .push(idx);
            }
        }
    }

    /// Check if there are any threads at a given line
    fn has_threads_at_line(&self, file_path: &str, line: u32) -> bool {
        self.line_to_threads
            .contains_key(&(file_path.to_string(), line))
    }

    /// Get count of threads at a line
    fn thread_count_at_line(&self, file_path: &str, line: u32) -> usize {
        self.line_to_threads
            .get(&(file_path.to_string(), line))
            .map(|v| v.len())
            .unwrap_or(0)
    }

    fn update_filtered_indices(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_indices = (0..self.files.len()).collect();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_indices = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.path.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect();
        }
        // Ensure selected file is in filtered list, or select first match
        if !self.filtered_indices.contains(&self.selected_file) {
            if let Some(&first) = self.filtered_indices.first() {
                self.select_file(first);
            }
        }
        // Reset tree scroll
        self.tree_scroll = 0;
    }

    /// Get line info at the current cursor position for inline comments
    /// Returns (file_path, line_number) if cursor is on a commentable line
    fn get_cursor_line_info(&self) -> Option<(String, u32)> {
        let file = self.files.get(self.selected_file)?;

        // Build flattened line list (same as render_unified_direct)
        let mut line_idx = 0;
        for hunk in &file.hunks {
            // Skip hunk header
            if line_idx == self.diff_cursor {
                return None; // Can't comment on hunk header
            }
            line_idx += 1;

            for diff_line in &hunk.lines {
                if line_idx == self.diff_cursor {
                    // Found the line - return file path and new line number
                    // For inline comments, we use the "new" line number (RIGHT side)
                    if let Some(new_ln) = diff_line.new_ln {
                        return Some((file.path.clone(), new_ln));
                    } else if let Some(old_ln) = diff_line.old_ln {
                        // Deleted line - use old line number
                        return Some((file.path.clone(), old_ln));
                    }
                    return None;
                }
                line_idx += 1;
            }
        }
        None
    }

    /// Get total line count for current file (including hunk headers)
    fn get_diff_line_count(&self) -> usize {
        let Some(file) = self.files.get(self.selected_file) else {
            return 0;
        };
        file.hunks.iter().map(|h| 1 + h.lines.len()).sum()
    }

    /// Select a file and reset cursor/scroll
    fn select_file(&mut self, index: usize) {
        self.selected_file = index;
        self.selected_tree_item = None;
        self.scroll_offset = 0;
        self.diff_cursor = 0;
        self.visual_mode = false;
        self.selection_anchor = 0;
    }

    /// Get selection range (start_line, end_line) in visual mode
    fn get_selection_range(&self) -> (usize, usize) {
        let start = self.selection_anchor.min(self.diff_cursor);
        let end = self.selection_anchor.max(self.diff_cursor);
        (start, end)
    }

    /// Get line info for a range of lines (for multi-line comments)
    /// Returns (file_path, start_line, end_line) if valid
    fn get_selection_line_info(&self) -> Option<(String, u32, u32)> {
        let file = self.files.get(self.selected_file)?;
        let (sel_start, sel_end) = self.get_selection_range();

        let mut start_line_num: Option<u32> = None;
        let mut end_line_num: Option<u32> = None;

        let mut line_idx = 0;
        for hunk in &file.hunks {
            line_idx += 1; // Skip hunk header

            for diff_line in &hunk.lines {
                if line_idx >= sel_start && line_idx <= sel_end {
                    // This line is in selection
                    if let Some(new_ln) = diff_line.new_ln {
                        if start_line_num.is_none() {
                            start_line_num = Some(new_ln);
                        }
                        end_line_num = Some(new_ln);
                    } else if let Some(old_ln) = diff_line.old_ln {
                        if start_line_num.is_none() {
                            start_line_num = Some(old_ln);
                        }
                        end_line_num = Some(old_ln);
                    }
                }
                line_idx += 1;
            }
        }

        match (start_line_num, end_line_num) {
            (Some(start), Some(end)) => Some((file.path.clone(), start, end)),
            _ => None,
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            Focus::Tree => {
                self.move_to_next_tree_item();
            }
            Focus::Diff => {
                let max_line = self.get_diff_line_count().saturating_sub(1);
                if self.diff_cursor < max_line {
                    self.diff_cursor += 1;
                }
                // Auto-scroll to keep cursor visible
                self.scroll_offset = self.diff_cursor.saturating_sub(10);
            }
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Tree => {
                self.move_to_prev_tree_item();
            }
            Focus::Diff => {
                self.diff_cursor = self.diff_cursor.saturating_sub(1);
                // Auto-scroll to keep cursor visible
                if self.diff_cursor < self.scroll_offset {
                    self.scroll_offset = self.diff_cursor;
                }
            }
        }
    }

    fn next_file(&mut self) {
        // Move to next file (skip folders)
        self.move_to_next_file_only();
    }

    fn prev_file(&mut self) {
        // Move to previous file (skip folders)
        self.move_to_prev_file_only();
    }

    fn move_to_next_tree_item(&mut self) {
        let tree = self.build_tree();
        let flat_items = self.flatten_tree(&tree);

        if flat_items.is_empty() {
            return;
        }

        // Find current position
        let current_pos = flat_items
            .iter()
            .position(|item| match item {
                TreeItem::File { index, .. } => {
                    self.selected_tree_item.is_none() && *index == self.selected_file
                }
                TreeItem::Folder { path, .. } => self.selected_tree_item.as_ref() == Some(path),
            })
            .unwrap_or(0);

        if current_pos < flat_items.len() - 1 {
            match &flat_items[current_pos + 1] {
                TreeItem::File { index, .. } => {
                    self.select_file(*index);
                }
                TreeItem::Folder { path, .. } => {
                    self.selected_tree_item = Some(path.clone());
                }
            }
        }
    }

    fn move_to_prev_tree_item(&mut self) {
        let tree = self.build_tree();
        let flat_items = self.flatten_tree(&tree);

        if flat_items.is_empty() {
            return;
        }

        // Find current position
        let current_pos = flat_items
            .iter()
            .position(|item| match item {
                TreeItem::File { index, .. } => {
                    self.selected_tree_item.is_none() && *index == self.selected_file
                }
                TreeItem::Folder { path, .. } => self.selected_tree_item.as_ref() == Some(path),
            })
            .unwrap_or(0);

        if current_pos > 0 {
            match &flat_items[current_pos - 1] {
                TreeItem::File { index, .. } => {
                    self.select_file(*index);
                }
                TreeItem::Folder { path, .. } => {
                    self.selected_tree_item = Some(path.clone());
                }
            }
        }
    }

    fn move_to_next_file_only(&mut self) {
        let tree = self.build_tree();
        let flat_items = self.flatten_tree(&tree);

        // Find current position
        let current_pos = flat_items
            .iter()
            .position(|item| match item {
                TreeItem::File { index, .. } => {
                    self.selected_tree_item.is_none() && *index == self.selected_file
                }
                TreeItem::Folder { path, .. } => self.selected_tree_item.as_ref() == Some(path),
            })
            .unwrap_or(0);

        // Find next file after current position
        for item in flat_items.iter().skip(current_pos + 1) {
            if let TreeItem::File { index, .. } = item {
                self.select_file(*index);
                return;
            }
        }
    }

    fn move_to_prev_file_only(&mut self) {
        let tree = self.build_tree();
        let flat_items = self.flatten_tree(&tree);

        // Find current position
        let current_pos = flat_items
            .iter()
            .position(|item| match item {
                TreeItem::File { index, .. } => {
                    self.selected_tree_item.is_none() && *index == self.selected_file
                }
                TreeItem::Folder { path, .. } => self.selected_tree_item.as_ref() == Some(path),
            })
            .unwrap_or(0);

        // Find previous file before current position
        for item in flat_items.iter().take(current_pos).rev() {
            if let TreeItem::File { index, .. } = item {
                self.select_file(*index);
                return;
            }
        }
    }

    fn toggle_focus(&mut self) {
        // If a folder is selected in the tree, toggle its collapse instead
        if self.focus == Focus::Tree && self.selected_tree_item.is_some() {
            self.toggle_collapse();
            return;
        }
        self.focus = match self.focus {
            Focus::Tree => Focus::Diff,
            Focus::Diff => Focus::Tree,
        };
    }

    fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Unified => ViewMode::Split,
            ViewMode::Split => ViewMode::Unified,
        };
    }

    fn toggle_collapse(&mut self) {
        // If a folder is selected, toggle its collapse state
        if let Some(ref folder_path) = self.selected_tree_item {
            if self.collapsed_folders.contains(folder_path) {
                self.collapsed_folders.remove(folder_path);
            } else {
                self.collapsed_folders.insert(folder_path.clone());
            }
        } else {
            // File is selected - toggle file collapse in diff view
            if self.collapsed.contains(&self.selected_file) {
                self.collapsed.remove(&self.selected_file);
            } else {
                self.collapsed.insert(self.selected_file);
            }
        }
    }

    fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    fn scroll_to_bottom(&mut self) {
        if let Some(file) = self.files.get(self.selected_file) {
            self.scroll_offset = file.line_count().saturating_sub(20);
        }
    }

    fn scroll_half_page_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(15);
    }
    fn scroll_half_page_down(&mut self) {
        self.scroll_offset += 15;
    }

    fn render(&self, frame: &mut ratatui::Frame) {
        // Show loading/error overlay if active
        match &self.loading {
            LoadingState::Loading(msg) => {
                self.render_loading(frame, msg);
                return;
            }
            LoadingState::Success(msg) => {
                self.render_success(frame, msg);
                return;
            }
            LoadingState::Error(msg) => {
                self.render_error(frame, msg);
                return;
            }
            LoadingState::Idle => {}
        }

        match self.screen {
            Screen::PrList => self.render_pr_list(frame),
            Screen::DiffView => self.render_diff_view(frame),
        }

        // Render help overlay if active
        if self.help_mode != HelpMode::None {
            self.render_help(frame);
        }
    }

    fn render_help(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();

        // Create centered popup
        let popup_width = 60.min(area.width.saturating_sub(4));
        let popup_height = match self.help_mode {
            HelpMode::PrList => 18,
            HelpMode::DiffView => 28, // Increased for thread commands
            HelpMode::None => return,
        };
        let popup_height = popup_height.min(area.height.saturating_sub(4));

        let popup_x = area.x + (area.width - popup_width) / 2;
        let popup_y = area.y + (area.height - popup_height) / 2;

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        // Clear background
        let buf = frame.buffer_mut();
        for y in popup_area.y..popup_area.y + popup_area.height {
            for x in popup_area.x..popup_area.x + popup_area.width {
                buf.set_string(x, y, " ", Style::default().bg(Color::Rgb(30, 30, 40)));
            }
        }

        let title = " Keyboard Shortcuts (press any key to close) ";
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let inner_area = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let commands: Vec<(&str, &str)> = match self.help_mode {
            HelpMode::PrList => vec![
                ("j / ↓", "Move down"),
                ("k / ↑", "Move up"),
                ("Enter", "Open selected PR"),
                ("o", "Open PR in browser"),
                ("Tab / 1 / 2", "Switch tabs (For Review / My PRs)"),
                ("f", "Cycle repository filter"),
                ("/", "Search PRs"),
                ("R", "Refresh PR list"),
                ("Esc", "Clear filter/search or quit"),
                ("q", "Quit"),
                ("?", "Show this help"),
            ],
            HelpMode::DiffView => vec![
                ("j / ↓", "Move cursor down"),
                ("k / ↑", "Move cursor up"),
                ("h / ←", "Previous file"),
                ("l / →", "Next file"),
                ("Tab / Enter", "Toggle focus (tree/diff)"),
                ("g", "Go to top"),
                ("G", "Go to bottom"),
                ("Ctrl+u", "Half page up"),
                ("Ctrl+d", "Half page down"),
                ("d", "Toggle unified/split view"),
                ("x", "Collapse/expand folder"),
                ("/", "Search files"),
                ("v", "Enter visual selection mode"),
                ("c", "Comment on current line/selection"),
                ("C", "View pending comments"),
                ("S", "Submit all pending comments"),
                ("t", "View comment threads"),
                ("T", "Refresh comment threads"),
                ("A", "Submit PR review (approve/reject)"),
                ("o", "Open PR in browser"),
                ("Esc", "Exit visual mode / go back"),
                ("q", "Back to PR list / quit"),
                ("?", "Show this help"),
            ],
            HelpMode::None => return,
        };

        let buf = frame.buffer_mut();
        let key_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(Color::White);

        for (i, (key, desc)) in commands.iter().enumerate() {
            if i as u16 >= inner_area.height {
                break;
            }
            let y = inner_area.y + i as u16;

            // Key column (width 16)
            let key_display = format!("{:>14}  ", key);
            buf.set_string(
                inner_area.x,
                y,
                &key_display,
                key_style.bg(Color::Rgb(30, 30, 40)),
            );

            // Description
            let desc_x = inner_area.x + 16;
            let available = (inner_area.width as usize).saturating_sub(16);
            let desc_truncated: String = desc.chars().take(available).collect();
            buf.set_string(
                desc_x,
                y,
                &desc_truncated,
                desc_style.bg(Color::Rgb(30, 30, 40)),
            );
        }
    }

    fn render_loading(&self, frame: &mut ratatui::Frame, message: &str) {
        let area = frame.area();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let text = Paragraph::new(message)
            .block(block)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Yellow));

        // Center the loading message
        let popup_area = Rect {
            x: area.width / 4,
            y: area.height / 2 - 1,
            width: area.width / 2,
            height: 3,
        };

        frame.render_widget(text, popup_area);
    }

    fn render_error(&self, frame: &mut ratatui::Frame, message: &str) {
        let area = frame.area();
        let block = Block::default()
            .title(" Error ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red));

        let text = Paragraph::new(format!("{}\n\nPress any key to continue", message))
            .block(block)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Red));

        let popup_area = Rect {
            x: area.width / 6,
            y: area.height / 2 - 2,
            width: area.width * 2 / 3,
            height: 5,
        };

        frame.render_widget(text, popup_area);
    }

    fn render_success(&self, frame: &mut ratatui::Frame, message: &str) {
        let area = frame.area();
        let block = Block::default()
            .title(" Success ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green));

        let text = Paragraph::new(format!("{}\n\nPress any key to continue", message))
            .block(block)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Green));

        let popup_area = Rect {
            x: area.width / 6,
            y: area.height / 2 - 2,
            width: area.width * 2 / 3,
            height: 5,
        };

        frame.render_widget(text, popup_area);
    }

    fn render_pr_list(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let buf = frame.buffer_mut();

        // Render tab bar at top
        let tab_height = 2;
        let tab_y = area.y;

        // Tab bar background
        let tab_bg = Style::default().bg(Color::Rgb(30, 30, 40));
        for x in area.x..area.x + area.width {
            buf.set_string(x, tab_y, " ", tab_bg);
            buf.set_string(x, tab_y + 1, " ", tab_bg);
        }

        let name = "kensa";
        let name_style = Style::default()
            .fg(Color::Magenta)
            .bg(Color::Rgb(30, 30, 40))
            .add_modifier(Modifier::BOLD);
        buf.set_string(area.x + 1, tab_y, name, name_style);

        // Separator (account for kanji width - each kanji is 2 cells wide)
        let sep = " │ ";
        let sep_style = Style::default()
            .fg(Color::DarkGray)
            .bg(Color::Rgb(30, 30, 40));
        let name_width: u16 = 4 + 1 + 5; // 2 kanji (2 cells each) + space + "kensa"
        buf.set_string(area.x + 1 + name_width, tab_y, sep, sep_style);

        let tabs_start = area.x + 1 + name_width + sep.len() as u16;

        // Tab 1: For Review
        let tab1_style = if self.pr_tab == PrListTab::ForReview {
            Style::default()
                .fg(Color::Cyan)
                .bg(Color::Rgb(30, 30, 40))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::DarkGray)
                .bg(Color::Rgb(30, 30, 40))
        };
        let tab1_text = format!(
            " [1] For Review ({}) ",
            self.filtered_review_pr_indices.len()
        );
        buf.set_string(tabs_start, tab_y, &tab1_text, tab1_style);

        // Tab 2: My PRs
        let tab2_style = if self.pr_tab == PrListTab::MyPrs {
            Style::default()
                .fg(Color::Cyan)
                .bg(Color::Rgb(30, 30, 40))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::DarkGray)
                .bg(Color::Rgb(30, 30, 40))
        };
        let tab2_text = format!(" [2] My PRs ({}) ", self.filtered_my_pr_indices.len());
        let tab2_x = tabs_start + tab1_text.len() as u16 + 1;
        buf.set_string(tab2_x, tab_y, &tab2_text, tab2_style);

        // Tab hint
        let hint = "Tab: switch";
        let hint_x = area.x + area.width - hint.len() as u16 - 2;
        buf.set_string(
            hint_x,
            tab_y,
            hint,
            Style::default()
                .fg(Color::DarkGray)
                .bg(Color::Rgb(30, 30, 40)),
        );

        // Filter/search info on second line
        let filter_info = if self.pr_search_mode {
            format!(" /{}_ ", self.pr_search_query)
        } else {
            let mut info = String::new();
            if let Some(ref repo) = self.repo_filter {
                info.push_str(&format!(" repo:{} ", repo));
            }
            if !self.pr_search_query.is_empty() {
                info.push_str(&format!(" search:{} ", self.pr_search_query));
            }
            info
        };
        if !filter_info.is_empty() {
            buf.set_string(
                area.x + 1,
                tab_y + 1,
                &filter_info,
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::Rgb(30, 30, 40)),
            );
        }

        // Content area below tabs
        let content_area = Rect {
            x: area.x,
            y: area.y + tab_height,
            width: area.width,
            height: area.height.saturating_sub(tab_height),
        };

        let border_style = Style::default().fg(Color::Cyan);
        let block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_style(border_style);

        let inner_area = block.inner(content_area);
        frame.render_widget(block, content_area);

        // Get current tab's data
        let pr_list = self.current_pr_list();
        let filtered_indices = self.current_filtered_indices();
        let selected = self.current_selected_pr();
        let pr_scroll = match self.pr_tab {
            PrListTab::ForReview => self.review_pr_scroll,
            PrListTab::MyPrs => self.my_pr_scroll,
        };

        if filtered_indices.is_empty() {
            let msg = match self.pr_tab {
                PrListTab::ForReview => {
                    if self.review_prs.is_empty() {
                        "No PRs awaiting your review"
                    } else {
                        "No PRs match the current filter"
                    }
                }
                PrListTab::MyPrs => {
                    if self.my_prs.is_empty() {
                        "You have no open PRs"
                    } else {
                        "No PRs match the current filter"
                    }
                }
            };
            let text = Paragraph::new(msg)
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(text, inner_area);

            // Still render help footer
            self.render_pr_list_footer(frame, area);
            return;
        }

        let visible_height = inner_area.height.saturating_sub(1) as usize; // -1 for footer

        // Find position of selected PR
        let selected_pos = filtered_indices
            .iter()
            .position(|&i| i == selected)
            .unwrap_or(0);

        // Auto-scroll
        let scroll = if selected_pos < pr_scroll {
            selected_pos
        } else if selected_pos >= pr_scroll + visible_height {
            selected_pos.saturating_sub(visible_height - 1)
        } else {
            pr_scroll
        };

        // Group PRs by repo for display
        let mut current_repo: Option<String> = None;
        let mut row = 0;

        let buf = frame.buffer_mut();

        for &pr_idx in filtered_indices.iter().skip(scroll) {
            if row >= visible_height {
                break;
            }

            let pr = &pr_list[pr_idx];
            let repo = pr.repo_full_name();
            let y = inner_area.y + row as u16;

            // Show repo header if changed
            if current_repo.as_ref() != Some(&repo) {
                current_repo = Some(repo.clone());

                let header_style = Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD);

                for x in inner_area.x..inner_area.x + inner_area.width {
                    buf.set_string(x, y, " ", header_style);
                }

                let truncated_repo: String =
                    repo.chars().take(inner_area.width as usize - 1).collect();
                buf.set_string(inner_area.x, y, &truncated_repo, header_style);

                row += 1;
                if row >= visible_height {
                    break;
                }
            }

            // Render PR row
            let y = inner_area.y + row as u16;
            let is_selected = pr_idx == selected;

            let style = if is_selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            // Clear line
            for x in inner_area.x..inner_area.x + inner_area.width {
                buf.set_string(x, y, " ", style);
            }

            let mut x = inner_area.x;

            // Indent
            buf.set_string(x, y, "  ", style);
            x += 2;

            // PR number
            let num_str = format!("#{:<5}", pr.number);
            buf.set_string(x, y, &num_str, style.fg(Color::Green));
            x += 6;

            // Calculate space for title
            let author_age = format!("@{} {}", pr.author, pr.age());
            let author_age_len = author_age.chars().count();
            let title_max_width = (inner_area.x + inner_area.width)
                .saturating_sub(x)
                .saturating_sub(author_age_len as u16 + 2)
                as usize;

            // Title (truncated)
            let title: String = pr.title.chars().take(title_max_width).collect();
            let title_display = if pr.title.chars().count() > title_max_width {
                format!("{}...", &title[..title.len().saturating_sub(3)])
            } else {
                title
            };
            buf.set_string(x, y, &title_display, style);

            // Author and age (right-aligned)
            let right_x = inner_area.x + inner_area.width - author_age_len as u16 - 1;
            buf.set_string(right_x, y, &author_age, style.fg(Color::DarkGray));

            row += 1;
        }

        // Render help footer
        self.render_pr_list_footer(frame, area);

        // Scrollbar
        if filtered_indices.len() > visible_height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None);
            let mut scrollbar_state = ScrollbarState::new(filtered_indices.len()).position(scroll);
            frame.render_stateful_widget(
                scrollbar,
                content_area.inner(ratatui::layout::Margin {
                    horizontal: 0,
                    vertical: 1,
                }),
                &mut scrollbar_state,
            );
        }
    }

    fn render_pr_list_footer(&self, frame: &mut ratatui::Frame, area: Rect) {
        let buf = frame.buffer_mut();
        let help =
            " j/k:nav  Enter:view  o:browser  f:filter  /:search  R:refresh  ?:help  q:quit ";
        let help_y = area.y + area.height - 1;
        let help_x = area.x + 1;
        let help_style = Style::default().fg(Color::DarkGray);
        buf.set_string(help_x, help_y, help, help_style);
    }

    fn render_diff_view(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();

        // If we came from PR list, show a header bar with back navigation
        if let Some(ref pr) = self.current_pr {
            let header_height = 2;
            let content_area = Rect {
                x: area.x,
                y: area.y + header_height,
                width: area.width,
                height: area.height.saturating_sub(header_height),
            };

            // Render header
            let buf = frame.buffer_mut();

            // First line: PR info
            let pr_info = format!(" {} #{}: {}", pr.repo_full_name(), pr.number, pr.title);
            let truncated_info: String = pr_info.chars().take(area.width as usize - 1).collect();
            let info_style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD);

            for x in area.x..area.x + area.width {
                buf.set_string(x, area.y, " ", Style::default().bg(Color::Rgb(30, 30, 40)));
            }
            buf.set_string(
                area.x,
                area.y,
                &truncated_info,
                info_style.bg(Color::Rgb(30, 30, 40)),
            );

            // Show pending comment count on the right
            if !self.pending_comments.is_empty() {
                let comment_badge = format!(" {} draft(s) ", self.pending_comments.len());
                let badge_x = area.x + area.width - comment_badge.len() as u16 - 1;
                let badge_style = Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                buf.set_string(badge_x, area.y, &comment_badge, badge_style);
            }

            // Second line: navigation hints
            let nav_hint = if self.visual_mode {
                " -- VISUAL --  v:cancel  c:comment  j/k:extend  ?:help "
            } else if self.pending_comments.is_empty() {
                " q:back  o:browser  c:comment  v:visual  /:search  ?:help "
            } else {
                " q:back  c:comment  C:drafts  S:submit  ?:help "
            };
            let hint_style = Style::default()
                .fg(Color::Yellow)
                .bg(Color::Rgb(30, 30, 40));

            for x in area.x..area.x + area.width {
                buf.set_string(
                    x,
                    area.y + 1,
                    " ",
                    Style::default().bg(Color::Rgb(30, 30, 40)),
                );
            }
            buf.set_string(area.x, area.y + 1, nav_hint, hint_style);

            // Render diff content in remaining area
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(45), Constraint::Min(0)])
                .split(content_area);

            self.render_tree(frame, chunks[0]);
            self.render_diff(frame, chunks[1]);

            // Render comment overlay if active
            self.render_comment_overlay(frame, area);
        } else {
            // Direct PR URL mode - no header needed
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(45), Constraint::Min(0)])
                .split(area);

            self.render_tree(frame, chunks[0]);
            self.render_diff(frame, chunks[1]);
        }
    }

    fn render_comment_overlay(&self, frame: &mut ratatui::Frame, area: Rect) {
        match &self.comment_mode {
            CommentMode::None => {}
            CommentMode::Editing {
                text,
                inline_context,
            } => {
                self.render_comment_input(frame, area, text, inline_context.as_ref());
            }
            CommentMode::ViewingPending => {
                self.render_pending_comments(frame, area);
            }
            CommentMode::ViewingThreads { selected, scroll } => {
                self.render_threads_list(frame, area, *selected, *scroll);
            }
            CommentMode::ViewingThread {
                index,
                selected,
                scroll,
            } => {
                self.render_thread_detail(frame, area, *index, *selected, *scroll);
            }
            CommentMode::ReplyingToThread { index, text } => {
                // Show thread detail in background + reply input overlay
                self.render_thread_detail(frame, area, *index, 0, 0);
                self.render_reply_input(frame, area, text);
            }
            CommentMode::SubmittingReview {
                selected_action,
                body,
                editing_body,
                reviewing_drafts,
                selected_draft,
                editing_draft,
            } => {
                self.render_review_modal(
                    frame,
                    area,
                    *selected_action,
                    body,
                    *editing_body,
                    *reviewing_drafts,
                    *selected_draft,
                    *editing_draft,
                );
            }
        }
    }

    fn render_comment_input(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        text: &str,
        inline_context: Option<&(String, u32, Option<u32>)>,
    ) {
        // Create a centered popup for comment input
        let popup_width = (area.width * 2 / 3).min(80);
        let popup_height = 12;
        let popup_x = area.x + (area.width - popup_width) / 2;
        let popup_y = area.y + (area.height - popup_height) / 2;

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        let title = match inline_context {
            Some((path, end_line, Some(start_line))) => {
                let suffix = " (Ctrl+S, Esc) ";
                let line_info = format!(":{}-{}", start_line, end_line);
                let prefix = " Comment on ";
                let max_path_len = (popup_width as usize)
                    .saturating_sub(prefix.len() + line_info.len() + suffix.len() + 2);
                let display_path = if path.len() > max_path_len {
                    format!("...{}", &path[path.len().saturating_sub(max_path_len.saturating_sub(3))..])
                } else {
                    path.clone()
                };
                format!("{}{}{}{}", prefix, display_path, line_info, suffix)
            }
            Some((path, line, None)) => {
                let suffix = " (Ctrl+S, Esc) ";
                let line_info = format!(":{}", line);
                let prefix = " Comment on ";
                let max_path_len = (popup_width as usize)
                    .saturating_sub(prefix.len() + line_info.len() + suffix.len() + 2);
                let display_path = if path.len() > max_path_len {
                    format!("...{}", &path[path.len().saturating_sub(max_path_len.saturating_sub(3))..])
                } else {
                    path.clone()
                };
                format!("{}{}{}{}", prefix, display_path, line_info, suffix)
            }
            None => " New Comment (Ctrl+S, Esc) ".to_string(),
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let inner_area = block.inner(popup_area);

        // Clear background and render block
        {
            let buf = frame.buffer_mut();
            for y in popup_area.y..popup_area.y + popup_area.height {
                for x in popup_area.x..popup_area.x + popup_area.width {
                    buf.set_string(x, y, " ", Style::default().bg(Color::Rgb(40, 40, 50)));
                }
            }
        }

        frame.render_widget(block, popup_area);

        // Render the text with word wrapping and cursor
        let wrap_width = inner_area.width.saturating_sub(1) as usize;
        let mut wrapped_lines: Vec<String> = Vec::new();

        // Wrap each line of the input text
        for line in text.lines() {
            if line.is_empty() {
                wrapped_lines.push(String::new());
            } else {
                wrapped_lines.extend(Self::wrap_text(line, wrap_width));
            }
        }
        // Handle case where text ends with newline or is empty
        if text.is_empty() || text.ends_with('\n') {
            wrapped_lines.push(String::new());
        }

        // Add cursor to the last line
        if let Some(last) = wrapped_lines.last_mut() {
            last.push('_');
        }

        let buf = frame.buffer_mut();
        for (i, line) in wrapped_lines.iter().enumerate() {
            if i as u16 >= inner_area.height {
                break;
            }
            buf.set_string(
                inner_area.x,
                inner_area.y + i as u16,
                line,
                Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 50)),
            );
        }
    }

    fn render_pending_comments(&self, frame: &mut ratatui::Frame, area: Rect) {
        // Create a centered popup for viewing pending comments
        let popup_width = (area.width * 3 / 4).min(100);
        let popup_height = (area.height * 2 / 3).min(20);
        let popup_x = area.x + (area.width - popup_width) / 2;
        let popup_y = area.y + (area.height - popup_height) / 2;

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        let title = format!(
            " Pending Comments ({}) - j/k:nav  e:edit  d:delete  S:submit  Esc:close ",
            self.pending_comments.len()
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let inner_area = block.inner(popup_area);

        // Clear background
        {
            let buf = frame.buffer_mut();
            for y in popup_area.y..popup_area.y + popup_area.height {
                for x in popup_area.x..popup_area.x + popup_area.width {
                    buf.set_string(x, y, " ", Style::default().bg(Color::Rgb(40, 40, 50)));
                }
            }
        }

        frame.render_widget(block, popup_area);

        let buf = frame.buffer_mut();

        if self.pending_comments.is_empty() {
            buf.set_string(
                inner_area.x,
                inner_area.y,
                "No pending comments",
                Style::default()
                    .fg(Color::DarkGray)
                    .bg(Color::Rgb(40, 40, 50)),
            );
            return;
        }

        // Render each comment
        let mut y = inner_area.y;
        for (i, comment) in self.pending_comments.iter().enumerate() {
            if y >= inner_area.y + inner_area.height {
                break;
            }

            let is_selected = i == self.selected_pending_comment;
            let style = if is_selected {
                Style::default().fg(Color::White).bg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 50))
            };

            // Clear line
            for x in inner_area.x..inner_area.x + inner_area.width {
                buf.set_string(x, y, " ", style);
            }

            // Comment number and type indicator
            let type_indicator = if comment.is_inline() {
                let path = comment.file_path.as_ref().unwrap();
                let line = comment.line_number.unwrap();
                // Shorten path if too long
                let short_path: String = if path.len() > 20 {
                    format!("...{}", &path[path.len() - 17..])
                } else {
                    path.clone()
                };
                if let Some(start_line) = comment.start_line {
                    format!("{}. [{}:{}-{}] ", i + 1, short_path, start_line, line)
                } else {
                    format!("{}. [{}:{}] ", i + 1, short_path, line)
                }
            } else {
                format!("{}. [General] ", i + 1)
            };
            buf.set_string(inner_area.x, y, &type_indicator, style.fg(Color::Cyan));

            // Comment preview (first line, truncated)
            let available_width =
                (inner_area.width as usize).saturating_sub(type_indicator.len() + 2);
            let preview: String = comment
                .body
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(available_width)
                .collect();

            let has_more_lines = comment.body.lines().count() > 1;
            let display = if has_more_lines && preview.len() >= available_width.saturating_sub(3) {
                format!("{}...", &preview[..preview.len().saturating_sub(3)])
            } else if has_more_lines {
                format!("{}...", preview)
            } else {
                preview
            };

            buf.set_string(
                inner_area.x + type_indicator.len() as u16,
                y,
                &display,
                style,
            );

            y += 1;
        }
    }

    fn render_threads_list(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        selected: usize,
        scroll: usize,
    ) {
        let popup_width = (area.width * 3 / 4).min(100);
        let popup_height = (area.height * 2 / 3).min(25);
        let popup_x = area.x + (area.width - popup_width) / 2;
        let popup_y = area.y + (area.height - popup_height) / 2;

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        let title = format!(
            " Comment Threads ({}) - j/k:nav  Enter:view  r:reply  g:goto  Esc:close ",
            self.comment_threads.len()
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta));

        let inner_area = block.inner(popup_area);

        // Clear background
        let buf = frame.buffer_mut();
        for y in popup_area.y..popup_area.y + popup_area.height {
            for x in popup_area.x..popup_area.x + popup_area.width {
                buf.set_string(x, y, " ", Style::default().bg(Color::Rgb(35, 35, 45)));
            }
        }

        frame.render_widget(block, popup_area);

        if self.comment_threads.is_empty() {
            let buf = frame.buffer_mut();
            buf.set_string(
                inner_area.x,
                inner_area.y,
                "No comment threads on this PR",
                Style::default()
                    .fg(Color::DarkGray)
                    .bg(Color::Rgb(35, 35, 45)),
            );
            return;
        }

        let buf = frame.buffer_mut();

        // Render thread list
        for (row, (idx, thread)) in self
            .comment_threads
            .iter()
            .enumerate()
            .skip(scroll)
            .take(inner_area.height as usize)
            .enumerate()
        {
            let y = inner_area.y + row as u16;
            let is_selected = idx == selected;

            let style = if is_selected {
                Style::default().fg(Color::White).bg(Color::Rgb(60, 60, 80))
            } else {
                Style::default().fg(Color::White).bg(Color::Rgb(35, 35, 45))
            };

            // Clear line
            for x in inner_area.x..inner_area.x + inner_area.width {
                buf.set_string(x, y, " ", style);
            }

            // Thread location
            let location = if let Some(path) = &thread.file_path {
                let short_path = if path.len() > 25 {
                    format!("...{}", &path[path.len() - 22..])
                } else {
                    path.clone()
                };
                if let Some(line) = thread.line {
                    format!("[{}:{}]", short_path, line)
                } else {
                    format!("[{}]", short_path)
                }
            } else {
                "[General]".to_string()
            };

            let comment_count = format!(" ({})", thread.comment_count());
            let author = format!(" @{}", thread.author());

            // Render location
            buf.set_string(inner_area.x, y, &location, style.fg(Color::Cyan));

            // Render count
            let count_x = inner_area.x + location.len() as u16;
            buf.set_string(count_x, y, &comment_count, style.fg(Color::Yellow));

            // Render author
            let author_x = count_x + comment_count.len() as u16;
            buf.set_string(author_x, y, &author, style.fg(Color::Green));

            // Preview of first comment
            let preview_x = author_x + author.len() as u16 + 1;
            let available =
                (inner_area.width as usize).saturating_sub((preview_x - inner_area.x) as usize);
            let preview = thread.preview(available);
            buf.set_string(preview_x, y, &preview, style.fg(Color::Gray));
        }
    }

    fn render_thread_detail(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        thread_idx: usize,
        _selected: usize,
        scroll: usize,
    ) {
        let Some(thread) = self.comment_threads.get(thread_idx) else {
            return;
        };

        let popup_width = (area.width * 4 / 5).min(120);
        let popup_height = (area.height * 3 / 4).min(30);
        let popup_x = area.x + (area.width - popup_width) / 2;
        let popup_y = area.y + (area.height - popup_height) / 2;

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        let location = if let Some(path) = &thread.file_path {
            if let Some(line) = thread.line {
                format!("{}:{}", path, line)
            } else {
                path.clone()
            }
        } else {
            "General Comment".to_string()
        };

        let title = format!(" {} - j/k:scroll  r:reply  Esc:back ", location);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let inner_area = block.inner(popup_area);

        // Clear background
        let buf = frame.buffer_mut();
        for y in popup_area.y..popup_area.y + popup_area.height {
            for x in popup_area.x..popup_area.x + popup_area.width {
                buf.set_string(x, y, " ", Style::default().bg(Color::Rgb(30, 30, 40)));
            }
        }

        frame.render_widget(block, popup_area);

        // Pre-calculate all lines for scrolling
        let wrap_width = (inner_area.width as usize).saturating_sub(4);
        let mut all_lines: Vec<(String, Style)> = Vec::new();
        let header_style = Style::default()
            .fg(Color::Green)
            .bg(Color::Rgb(30, 30, 40))
            .add_modifier(Modifier::BOLD);
        let body_style = Style::default().fg(Color::White).bg(Color::Rgb(30, 30, 40));
        let code_style = Style::default().fg(Color::Yellow).bg(Color::Rgb(20, 20, 30));
        let separator_style = Style::default().bg(Color::Rgb(30, 30, 40));

        for comment in &thread.comments {
            // Author and timestamp
            let time_ago = Self::format_relative_time(&comment.created_at);
            let header = format!("@{} - {}", comment.author, time_ago);
            all_lines.push((header, header_style));

            // Comment body (word-wrapped, with code block detection)
            let wrapped = Self::wrap_text_with_code(&comment.body, wrap_width);
            for (line, is_code) in wrapped {
                if is_code {
                    // Convert tabs to spaces for proper terminal rendering
                    let display_line = line.replace('\t', "    ");
                    all_lines.push((format!("  │ {}", display_line), code_style));
                } else {
                    all_lines.push((format!(" {}", line), body_style));
                }
            }

            // Separator (empty line)
            all_lines.push((String::new(), separator_style));
        }

        let buf = frame.buffer_mut();
        let visible_height = inner_area.height as usize;
        let total_lines = all_lines.len();

        // Show scroll indicator if content is scrollable
        if total_lines > visible_height {
            let scroll_info = format!(" [{}/{}] ", scroll + 1, total_lines.saturating_sub(visible_height) + 1);
            let info_x = popup_area.x + popup_area.width - scroll_info.len() as u16 - 1;
            buf.set_string(
                info_x,
                popup_area.y,
                &scroll_info,
                Style::default().fg(Color::DarkGray).bg(Color::Rgb(30, 30, 40)),
            );
        }

        // Render visible lines with scroll offset
        for (i, (line, style)) in all_lines.iter().enumerate().skip(scroll).take(visible_height) {
            let y = inner_area.y + (i - scroll) as u16;
            if y < inner_area.y + inner_area.height {
                buf.set_string(inner_area.x, y, line, *style);
            }
        }
    }

    fn render_reply_input(&self, frame: &mut ratatui::Frame, area: Rect, text: &str) {
        let popup_width = (area.width * 2 / 3).min(80);
        let popup_height = 10;
        let popup_x = area.x + (area.width - popup_width) / 2;
        let popup_y = area.y + (area.height - popup_height) / 2;

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        let title = " Reply (Ctrl+S to send, Esc to cancel) ";
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green));

        let inner_area = block.inner(popup_area);

        // Clear and render
        let buf = frame.buffer_mut();
        for y in popup_area.y..popup_area.y + popup_area.height {
            for x in popup_area.x..popup_area.x + popup_area.width {
                buf.set_string(x, y, " ", Style::default().bg(Color::Rgb(40, 50, 40)));
            }
        }

        frame.render_widget(block, popup_area);

        // Render the text with word wrapping and cursor
        let wrap_width = inner_area.width.saturating_sub(1) as usize;
        let mut wrapped_lines: Vec<String> = Vec::new();

        for line in text.lines() {
            if line.is_empty() {
                wrapped_lines.push(String::new());
            } else {
                wrapped_lines.extend(Self::wrap_text(line, wrap_width));
            }
        }
        if text.is_empty() || text.ends_with('\n') {
            wrapped_lines.push(String::new());
        }

        if let Some(last) = wrapped_lines.last_mut() {
            last.push('_');
        }

        let buf = frame.buffer_mut();
        for (i, line) in wrapped_lines.iter().enumerate() {
            if i as u16 >= inner_area.height {
                break;
            }
            buf.set_string(
                inner_area.x,
                inner_area.y + i as u16,
                line,
                Style::default().fg(Color::White).bg(Color::Rgb(40, 50, 40)),
            );
        }
    }

    fn render_review_modal(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        selected_action: usize,
        body: &str,
        editing_body: bool,
        reviewing_drafts: bool,
        selected_draft: usize,
        editing_draft: bool,
    ) {
        let popup_width = (area.width * 2 / 3).min(80);
        let popup_height = if reviewing_drafts { 22 } else { 20 };
        let popup_x = area.x + (area.width - popup_width) / 2;
        let popup_y = area.y + (area.height - popup_height) / 2;

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        let title = if reviewing_drafts {
            " Review Draft Comments (Ctrl+Enter to submit, Esc to go back) "
        } else {
            " Submit Review (Ctrl+Enter to submit, Esc to cancel) "
        };

        let border_color = if editing_draft {
            Color::Green
        } else if reviewing_drafts {
            Color::Yellow
        } else if editing_body {
            Color::Green
        } else {
            Color::Cyan
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner_area = block.inner(popup_area);

        // Clear background
        let buf = frame.buffer_mut();
        for y in popup_area.y..popup_area.y + popup_area.height {
            for x in popup_area.x..popup_area.x + popup_area.width {
                buf.set_string(x, y, " ", Style::default().bg(Color::Rgb(35, 35, 50)));
            }
        }

        frame.render_widget(block, popup_area);

        if reviewing_drafts {
            // Render draft review screen
            self.render_draft_review(frame, inner_area, selected_draft, editing_draft);
        } else {
            // Render main review modal
            self.render_review_main(
                frame,
                inner_area,
                selected_action,
                body,
                editing_body,
                popup_area,
            );
        }
    }

    fn render_review_main(
        &self,
        frame: &mut ratatui::Frame,
        inner_area: Rect,
        selected_action: usize,
        body: &str,
        editing_body: bool,
        popup_area: Rect,
    ) {
        // Render action options
        let actions = [
            ("1", "Approve", Color::Green),
            ("2", "Request Changes", Color::Yellow),
            ("3", "Comment Only", Color::Blue),
        ];

        let buf = frame.buffer_mut();
        for (i, (key, label, color)) in actions.iter().enumerate() {
            let y = inner_area.y + i as u16;
            let is_selected = i == selected_action;

            let prefix = if is_selected { "> " } else { "  " };
            let style = if is_selected {
                Style::default().fg(*color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            // Dim action options when editing body
            let style = if editing_body {
                Style::default().fg(Color::DarkGray)
            } else {
                style
            };

            buf.set_string(inner_area.x, y, prefix, style);
            buf.set_string(inner_area.x + 2, y, format!("[{}] {}", key, label), style);
        }

        // Separator and comment label
        let separator_y = inner_area.y + 4;
        let comment_label = if editing_body {
            "Comment (editing - Esc to go back):"
        } else {
            "Comment (press 'c' or Enter to edit):"
        };
        let label_style = if editing_body {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        buf.set_string(inner_area.x, separator_y, comment_label, label_style);

        // Render comment body area with border effect
        let body_start_y = separator_y + 1;
        let body_height = 4u16;
        let body_bg = if editing_body {
            Color::Rgb(30, 40, 30)
        } else {
            Color::Rgb(30, 30, 40)
        };

        // Clear body area
        for y in body_start_y..body_start_y + body_height {
            if y >= inner_area.y + inner_area.height - 4 {
                break;
            }
            for x in inner_area.x..inner_area.x + inner_area.width {
                buf.set_string(x, y, " ", Style::default().bg(body_bg));
            }
        }

        // Render comment text with word wrapping
        let text_style = if editing_body {
            Style::default().fg(Color::White).bg(body_bg)
        } else if body.is_empty() {
            Style::default().fg(Color::DarkGray).bg(body_bg)
        } else {
            Style::default().fg(Color::Gray).bg(body_bg)
        };

        let wrap_width = inner_area.width.saturating_sub(3) as usize;

        if body.is_empty() && !editing_body {
            // Show placeholder
            buf.set_string(inner_area.x + 1, body_start_y, "(no comment)", text_style);
        } else {
            // Wrap and render comment text
            let mut wrapped_lines: Vec<String> = Vec::new();
            for line in body.lines() {
                if line.is_empty() {
                    wrapped_lines.push(String::new());
                } else {
                    wrapped_lines.extend(Self::wrap_text(line, wrap_width));
                }
            }
            if body.is_empty() || body.ends_with('\n') {
                wrapped_lines.push(String::new());
            }

            // Add cursor if editing
            if editing_body {
                if let Some(last) = wrapped_lines.last_mut() {
                    last.push('_');
                }
            }

            for (i, line) in wrapped_lines.iter().enumerate() {
                let y = body_start_y + i as u16;
                if y >= body_start_y + body_height || y >= inner_area.y + inner_area.height - 2 {
                    break;
                }
                buf.set_string(inner_area.x + 1, y, line, text_style);
            }
        }

        // Show draft comments count with 'd' to review
        let drafts_y = body_start_y + body_height + 1;
        let draft_count = self.pending_comments.len();
        if draft_count > 0 {
            let drafts_text = format!("Draft comments: {} (press 'd' to review)", draft_count);
            buf.set_string(
                inner_area.x,
                drafts_y,
                &drafts_text,
                Style::default().fg(Color::Yellow),
            );
        } else {
            buf.set_string(
                inner_area.x,
                drafts_y,
                "No draft comments",
                Style::default().fg(Color::DarkGray),
            );
        }

        // Instructions at bottom
        let instructions = if editing_body {
            "Type comment | Esc: back | Ctrl+Enter: submit"
        } else if draft_count > 0 {
            "j/k or 1/2/3: select | c: edit | d: review drafts | Ctrl+Enter: submit"
        } else {
            "j/k or 1/2/3: select | c/Enter: edit comment | Ctrl+Enter: submit"
        };
        let instr_y = popup_area.y + popup_area.height - 2;
        if instr_y > inner_area.y {
            buf.set_string(
                inner_area.x,
                instr_y,
                instructions,
                Style::default().fg(Color::DarkGray),
            );
        }
    }

    fn render_draft_review(
        &self,
        frame: &mut ratatui::Frame,
        inner_area: Rect,
        selected_draft: usize,
        editing_draft: bool,
    ) {
        let buf = frame.buffer_mut();
        let draft_count = self.pending_comments.len();

        // Header
        let header = format!("Review {} draft comment(s) before submission:", draft_count);
        buf.set_string(
            inner_area.x,
            inner_area.y,
            &header,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

        if draft_count == 0 {
            buf.set_string(
                inner_area.x,
                inner_area.y + 2,
                "No draft comments to review.",
                Style::default().fg(Color::DarkGray),
            );
            buf.set_string(
                inner_area.x,
                inner_area.y + 4,
                "Press Ctrl+Enter to submit review or Esc to go back.",
                Style::default().fg(Color::DarkGray),
            );
            return;
        }

        // Calculate visible drafts
        let list_start_y = inner_area.y + 2;
        let list_height = (inner_area.height - 5) as usize;
        let visible_drafts = list_height.min(draft_count);

        // Calculate scroll offset
        let scroll_offset = if selected_draft >= visible_drafts {
            selected_draft - visible_drafts + 1
        } else {
            0
        };

        // Render visible drafts
        for (i, draft) in self
            .pending_comments
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_drafts)
        {
            let y = list_start_y + (i - scroll_offset) as u16;
            let is_selected = i == selected_draft;

            let prefix = if is_selected { "> " } else { "  " };

            // Show location for inline comments
            let location = if let (Some(path), Some(line)) = (&draft.file_path, draft.line_number) {
                let filename = path.rsplit('/').next().unwrap_or(path);
                if let Some(start) = draft.start_line {
                    format!("{}:{}-{}", filename, start, line)
                } else {
                    format!("{}:{}", filename, line)
                }
            } else {
                "general".to_string()
            };

            // Format: "> [file:line] first line of comment..."
            let comment_preview: String = draft
                .body
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(40)
                .collect();
            let display = format!("[{}] {}", location, comment_preview);

            let style = if is_selected && editing_draft {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };

            let truncated: String = display
                .chars()
                .take(inner_area.width as usize - 4)
                .collect();
            buf.set_string(inner_area.x, y, prefix, style);
            buf.set_string(inner_area.x + 2, y, &truncated, style);
        }

        // Show scroll indicator if needed
        if draft_count > visible_drafts {
            let scroll_info = format!("[{}/{}]", selected_draft + 1, draft_count);
            buf.set_string(
                inner_area.x + inner_area.width - scroll_info.len() as u16 - 1,
                list_start_y,
                &scroll_info,
                Style::default().fg(Color::DarkGray),
            );
        }

        // If editing a draft, show the full text in a text area
        if editing_draft && selected_draft < draft_count {
            let edit_y = list_start_y + visible_drafts as u16 + 1;
            buf.set_string(
                inner_area.x,
                edit_y,
                "Editing (Esc to save):",
                Style::default().fg(Color::Green),
            );

            let edit_text = format!("{}_", self.pending_comments[selected_draft].body);
            let edit_text_y = edit_y + 1;
            let edit_bg = Color::Rgb(30, 40, 30);

            // Clear edit area
            for y in edit_text_y..edit_text_y + 3 {
                if y >= inner_area.y + inner_area.height - 2 {
                    break;
                }
                for x in inner_area.x..inner_area.x + inner_area.width {
                    buf.set_string(x, y, " ", Style::default().bg(edit_bg));
                }
            }

            // Render edit text
            for (i, line) in edit_text.lines().enumerate().take(3) {
                let y = edit_text_y + i as u16;
                if y >= inner_area.y + inner_area.height - 2 {
                    break;
                }
                let truncated: String = line.chars().take(inner_area.width as usize - 2).collect();
                buf.set_string(
                    inner_area.x + 1,
                    y,
                    &truncated,
                    Style::default().fg(Color::White).bg(edit_bg),
                );
            }
        }

        // Instructions at bottom
        let instructions = if editing_draft {
            "Type to edit | Esc: save changes"
        } else {
            "j/k: navigate | e/Enter: edit | d/x: delete | Ctrl+Enter: submit | Esc: back"
        };
        let instr_y = inner_area.y + inner_area.height - 1;
        buf.set_string(
            inner_area.x,
            instr_y,
            instructions,
            Style::default().fg(Color::DarkGray),
        );
    }

    /// Format time as relative (e.g., "2h ago", "3d ago")
    fn format_relative_time(iso_time: &str) -> String {
        chrono::DateTime::parse_from_rfc3339(iso_time)
            .map(|dt| {
                let now = chrono::Utc::now();
                let diff = now.signed_duration_since(dt);
                if diff.num_hours() < 1 {
                    format!("{}m ago", diff.num_minutes())
                } else if diff.num_days() < 1 {
                    format!("{}h ago", diff.num_hours())
                } else {
                    format!("{}d ago", diff.num_days())
                }
            })
            .unwrap_or_else(|_| iso_time.to_string())
    }

    /// Character-based text wrapping - breaks at width boundary
    /// Returns Vec of (line, is_code_block)
    fn wrap_text_with_code(text: &str, width: usize) -> Vec<(String, bool)> {
        if width == 0 {
            return vec![(text.to_string(), false)];
        }

        let mut result = Vec::new();
        let mut in_code_block = false;

        // First split by newlines, then wrap each line
        for line in text.split('\n') {
            // Detect code block markers
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                in_code_block = !in_code_block;
                // Skip the ``` marker line itself
                continue;
            }

            if line.is_empty() {
                result.push((String::new(), in_code_block));
                continue;
            }

            // For code blocks, preserve exact whitespace and don't wrap aggressively
            if in_code_block {
                // Don't wrap code lines, just truncate if needed
                if line.len() > width {
                    result.push((line[..width].to_string(), true));
                } else {
                    result.push((line.to_string(), true));
                }
            } else {
                let chars: Vec<char> = line.chars().collect();
                let mut i = 0;
                while i < chars.len() {
                    let end = (i + width).min(chars.len());
                    let wrapped_line: String = chars[i..end].iter().collect();
                    result.push((wrapped_line, false));
                    i = end;
                }
            }
        }

        if result.is_empty() {
            result.push((String::new(), false));
        }

        result
    }

    /// Simple wrap_text for non-code content
    fn wrap_text(text: &str, width: usize) -> Vec<String> {
        Self::wrap_text_with_code(text, width)
            .into_iter()
            .map(|(line, _)| line)
            .collect()
    }

    fn render_tree(&self, frame: &mut ratatui::Frame, area: Rect) {
        let border_style = if self.focus == Focus::Tree || self.search_mode {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        // Calculate inner area for content
        let title = if self.search_mode {
            format!(" /{}_ ", self.search_query)
        } else if !self.search_query.is_empty() {
            format!(
                " Files ({}/{}) [{}] ",
                self.filtered_indices.len(),
                self.files.len(),
                self.search_query
            )
        } else {
            format!(" Files ({}) ", self.files.len())
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        if self.filtered_indices.is_empty() {
            // No matches
            let no_match = Line::from(Span::styled(
                "No matching files",
                Style::default().fg(Color::DarkGray),
            ));
            let buf = frame.buffer_mut();
            buf.set_line(inner_area.x, inner_area.y, &no_match, inner_area.width);
            return;
        }

        // Build and flatten tree
        let tree = self.build_tree();
        let flat_items = self.flatten_tree(&tree);

        // Calculate visible range with scrolling
        let visible_height = inner_area.height as usize;

        // Find position of selected item in flattened list
        let selected_pos = flat_items
            .iter()
            .position(|item| match item {
                TreeItem::File { index, .. } => *index == self.selected_file,
                TreeItem::Folder { path, .. } => self.selected_tree_item.as_ref() == Some(path),
            })
            .unwrap_or(0);

        // Auto-scroll to keep selected item visible
        let tree_scroll = if selected_pos < self.tree_scroll {
            selected_pos
        } else if selected_pos >= self.tree_scroll + visible_height {
            selected_pos.saturating_sub(visible_height - 1)
        } else {
            self.tree_scroll
        };

        let buf = frame.buffer_mut();

        for (row_idx, item) in flat_items
            .iter()
            .skip(tree_scroll)
            .take(visible_height)
            .enumerate()
        {
            let y = inner_area.y + row_idx as u16;

            match item {
                TreeItem::Folder {
                    path,
                    name,
                    depth,
                    is_last,
                    ancestors_last,
                } => {
                    let is_selected = self.selected_tree_item.as_ref() == Some(path);
                    let is_collapsed = self.collapsed_folders.contains(path);

                    let prefix = self.get_tree_prefix(*depth, *is_last, ancestors_last);
                    let folder_icon = if is_collapsed { "+" } else { "-" };

                    let style = if is_selected {
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };

                    // Clear the line first
                    for x in inner_area.x..inner_area.x + inner_area.width {
                        buf.set_string(x, y, " ", style);
                    }

                    let mut x = inner_area.x;
                    // Draw prefix
                    buf.set_string(x, y, &prefix, Style::default().fg(Color::DarkGray));
                    x += prefix.chars().count() as u16;

                    // Draw folder icon
                    buf.set_string(x, y, folder_icon, Style::default().fg(Color::Yellow));
                    x += 1;

                    // Draw folder name
                    let display_name = format!(" {}/", name);
                    let available_width =
                        (inner_area.x + inner_area.width).saturating_sub(x) as usize;
                    let truncated_name: String =
                        display_name.chars().take(available_width).collect();
                    buf.set_string(x, y, &truncated_name, style.fg(Color::Yellow));
                }
                TreeItem::File {
                    index,
                    name,
                    depth,
                    is_last,
                    ancestors_last,
                } => {
                    let file = &self.files[*index];
                    let is_selected =
                        *index == self.selected_file && self.selected_tree_item.is_none();

                    let prefix = self.get_tree_prefix(*depth, *is_last, ancestors_last);

                    let style = if is_selected {
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };

                    // Clear the line first
                    for x in inner_area.x..inner_area.x + inner_area.width {
                        buf.set_string(x, y, " ", style);
                    }

                    let mut x = inner_area.x;
                    // Draw prefix
                    buf.set_string(x, y, &prefix, Style::default().fg(Color::DarkGray));
                    x += prefix.chars().count() as u16;

                    // Draw status badge
                    let badge = format!("{} ", file.status.badge());
                    buf.set_string(x, y, &badge, Style::default().fg(file.status.color()));
                    x += badge.chars().count() as u16;

                    // Draw file name
                    let available_width =
                        (inner_area.x + inner_area.width).saturating_sub(x) as usize;
                    let truncated_name: String = name.chars().take(available_width).collect();
                    buf.set_string(x, y, &truncated_name, style);
                }
            }
        }

        // Show scrollbar if needed
        if flat_items.len() > visible_height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None);
            let mut scrollbar_state = ScrollbarState::new(flat_items.len()).position(tree_scroll);
            frame.render_stateful_widget(
                scrollbar,
                area.inner(ratatui::layout::Margin {
                    horizontal: 0,
                    vertical: 1,
                }),
                &mut scrollbar_state,
            );
        }
    }

    fn render_diff(&self, frame: &mut ratatui::Frame, area: Rect) {
        // First, fill the ENTIRE area with background color directly in the buffer
        let buf = frame.buffer_mut();
        fill_area(buf, area, BG_COLOR);

        let Some(file) = self.files.get(self.selected_file) else {
            let block = Block::default()
                .title(" No file selected ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            frame.render_widget(block, area);
            return;
        };

        let border_style = if self.focus == Focus::Diff {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let title = if self.collapsed.contains(&self.selected_file) {
            format!(" {} [collapsed] ", file.path)
        } else if self.view_mode == ViewMode::Split {
            format!(" {} [split] ", file.path)
        } else {
            format!(" {} ", file.path)
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        if self.collapsed.contains(&self.selected_file) {
            return;
        }

        // Fill inner area with background
        fill_area(frame.buffer_mut(), inner_area, BG_COLOR);

        match self.view_mode {
            ViewMode::Unified => self.render_unified_direct(frame.buffer_mut(), inner_area, file),
            ViewMode::Split => self.render_split_direct(frame.buffer_mut(), inner_area, file),
        }

        // Scrollbar
        let total_lines = file.line_count() + file.hunks.len();
        if total_lines > inner_area.height as usize {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
            let mut scrollbar_state = ScrollbarState::new(total_lines).position(self.scroll_offset);
            frame.render_stateful_widget(
                scrollbar,
                area.inner(ratatui::layout::Margin {
                    horizontal: 0,
                    vertical: 1,
                }),
                &mut scrollbar_state,
            );
        }
    }

    fn render_unified_direct(&self, buf: &mut Buffer, area: Rect, file: &DiffFile) {
        let mut lines: Vec<DiffDisplayLine> = Vec::new();

        for hunk in &file.hunks {
            lines.push(DiffDisplayLine::Hunk(hunk.header.clone()));

            for diff_line in &hunk.lines {
                lines.push(DiffDisplayLine::Content {
                    kind: diff_line.kind,
                    old_ln: diff_line.old_ln,
                    new_ln: diff_line.new_ln,
                    content: diff_line.content.clone(),
                });
            }
        }

        let max_scroll = lines.len().saturating_sub(area.height as usize);
        let scroll = self.scroll_offset.min(max_scroll);

        for (row_idx, line) in lines
            .iter()
            .skip(scroll)
            .take(area.height as usize)
            .enumerate()
        {
            let y = area.y + row_idx as u16;
            let line_idx = scroll + row_idx;
            let (sel_start, sel_end) = self.get_selection_range();
            let is_in_selection = self.visual_mode
                && self.focus == Focus::Diff
                && line_idx >= sel_start
                && line_idx <= sel_end;
            let is_cursor_line =
                self.focus == Focus::Diff && line_idx == self.diff_cursor && !self.visual_mode;

            match line {
                DiffDisplayLine::Hunk(header) => {
                    let bg = if is_in_selection {
                        Color::Rgb(60, 60, 90) // Selection highlight
                    } else if is_cursor_line {
                        CURSOR_BG
                    } else {
                        BG_COLOR
                    };

                    // Cursor marker
                    let cursor_marker = if is_cursor_line || is_in_selection {
                        "▶"
                    } else {
                        " "
                    };
                    let marker_style = if is_cursor_line {
                        Style::default()
                            .fg(Color::Yellow)
                            .bg(CURSOR_GUTTER)
                            .add_modifier(Modifier::BOLD)
                    } else if is_in_selection {
                        Style::default().fg(Color::Cyan).bg(Color::Rgb(60, 60, 90))
                    } else {
                        Style::default().fg(Color::DarkGray).bg(bg)
                    };
                    buf.set_string(area.x, y, cursor_marker, marker_style);

                    let text = truncate_or_pad(header, (area.width as usize).saturating_sub(1));
                    buf.set_string(
                        area.x + 1,
                        y,
                        &text,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::DIM)
                            .bg(bg),
                    );
                }
                DiffDisplayLine::Content {
                    kind,
                    old_ln,
                    new_ln,
                    content,
                } => {
                    let base_bg = match kind {
                        LineKind::Add => ADD_BG,
                        LineKind::Del => DEL_BG,
                        LineKind::Context => BG_COLOR,
                    };
                    // Blend highlight with line background
                    let bg = if is_in_selection {
                        match kind {
                            LineKind::Add => Color::Rgb(50, 90, 70), // Green-ish selection
                            LineKind::Del => Color::Rgb(90, 50, 70), // Red-ish selection
                            _ => Color::Rgb(60, 60, 90),             // Blue-ish selection
                        }
                    } else if is_cursor_line {
                        match kind {
                            LineKind::Add => Color::Rgb(40, 80, 50),
                            LineKind::Del => Color::Rgb(80, 40, 50),
                            _ => CURSOR_BG,
                        }
                    } else {
                        base_bg
                    };

                    let (old_str, new_str) = match kind {
                        LineKind::Add => {
                            ("    ".to_string(), format!("{:>4}", new_ln.unwrap_or(0)))
                        }
                        LineKind::Del => {
                            (format!("{:>4}", old_ln.unwrap_or(0)), "    ".to_string())
                        }
                        LineKind::Context => (
                            format!("{:>4}", old_ln.unwrap_or(0)),
                            format!("{:>4}", new_ln.unwrap_or(0)),
                        ),
                    };

                    // Fill entire row with line's background first
                    for cx in area.x..area.x + area.width {
                        buf.set_string(cx, y, " ", Style::default().bg(bg));
                    }

                    // Cursor marker for current line
                    let cursor_marker = if is_cursor_line || is_in_selection {
                        "▶"
                    } else {
                        " "
                    };
                    let marker_style = if is_cursor_line {
                        Style::default()
                            .fg(Color::Yellow)
                            .bg(CURSOR_GUTTER)
                            .add_modifier(Modifier::BOLD)
                    } else if is_in_selection {
                        Style::default().fg(Color::Cyan).bg(Color::Rgb(60, 60, 90))
                    } else {
                        Style::default().fg(Color::DarkGray).bg(bg)
                    };
                    buf.set_string(area.x, y, cursor_marker, marker_style);

                    let gutter = format!("{} {} ", old_str, new_str);
                    let gutter_width = gutter.len() + 1; // +1 for cursor marker

                    // Render line numbers with special style for cursor line
                    let gutter_style = if is_cursor_line {
                        Style::default()
                            .fg(Color::White)
                            .bg(CURSOR_GUTTER)
                            .add_modifier(Modifier::BOLD)
                    } else if is_in_selection {
                        Style::default().fg(Color::White).bg(Color::Rgb(60, 60, 90))
                    } else {
                        Style::default().fg(Color::DarkGray).bg(bg)
                    };
                    buf.set_string(area.x + 1, y, &gutter, gutter_style);

                    // Comment thread indicator
                    let indicator_x = area.x + gutter_width as u16;
                    let has_comment = new_ln
                        .map(|ln| self.has_threads_at_line(&file.path, ln))
                        .unwrap_or(false);
                    if has_comment {
                        let count = new_ln
                            .map(|ln| self.thread_count_at_line(&file.path, ln))
                            .unwrap_or(0);
                        let indicator = if count > 1 {
                            format!("{}", count)
                        } else {
                            "C".to_string()
                        };
                        let indicator_style = Style::default()
                            .fg(Color::Yellow)
                            .bg(bg)
                            .add_modifier(Modifier::BOLD);
                        buf.set_string(indicator_x, y, &indicator, indicator_style);
                    }

                    let content_start_x = area.x + gutter_width as u16 + 2; // +2 for indicator space
                    let max_x = area.x + area.width;

                    // First render raw content as fallback (in case spans don't cover everything)
                    let default_style = Style::default().fg(Color::White).bg(bg);
                    let mut x_offset = content_start_x;
                    for ch in content.chars() {
                        if x_offset >= max_x {
                            break;
                        }
                        buf.set_string(x_offset, y, &ch.to_string(), default_style);
                        x_offset += 1;
                    }

                    // Then overlay syntax highlighted spans
                    let highlighted = self.highlighter.highlight_line(content, &file.path);
                    x_offset = content_start_x;

                    for span in highlighted.spans {
                        let span_style = span.style.bg(bg);
                        for ch in span.content.chars() {
                            if x_offset >= max_x {
                                break;
                            }
                            buf.set_string(x_offset, y, &ch.to_string(), span_style);
                            x_offset += 1;
                        }
                    }
                }
            }
        }
    }

    fn render_split_direct(&self, buf: &mut Buffer, area: Rect, file: &DiffFile) {
        let mid = area.width / 2;
        let left_area = Rect {
            x: area.x,
            y: area.y,
            width: mid,
            height: area.height,
        };
        let right_area = Rect {
            x: area.x + mid,
            y: area.y,
            width: area.width - mid,
            height: area.height,
        };

        // Build paired lines
        let mut paired: Vec<(Option<SplitLine>, Option<SplitLine>)> = Vec::new();

        for hunk in &file.hunks {
            paired.push((
                Some(SplitLine::Hunk(hunk.header.clone())),
                Some(SplitLine::Hunk(hunk.header.clone())),
            ));

            let mut pending_dels: Vec<SplitLine> = Vec::new();
            let mut pending_adds: Vec<SplitLine> = Vec::new();

            for diff_line in &hunk.lines {
                match diff_line.kind {
                    LineKind::Del => {
                        pending_dels.push(SplitLine::Del {
                            ln: diff_line.old_ln.unwrap_or(0),
                            content: diff_line.content.clone(),
                        });
                    }
                    LineKind::Add => {
                        pending_adds.push(SplitLine::Add {
                            ln: diff_line.new_ln.unwrap_or(0),
                            content: diff_line.content.clone(),
                        });
                    }
                    LineKind::Context => {
                        // Flush pending
                        let max_len = pending_dels.len().max(pending_adds.len());
                        for i in 0..max_len {
                            paired
                                .push((pending_dels.get(i).cloned(), pending_adds.get(i).cloned()));
                        }
                        pending_dels.clear();
                        pending_adds.clear();

                        paired.push((
                            Some(SplitLine::Context {
                                ln: diff_line.old_ln.unwrap_or(0),
                                content: diff_line.content.clone(),
                            }),
                            Some(SplitLine::Context {
                                ln: diff_line.new_ln.unwrap_or(0),
                                content: diff_line.content.clone(),
                            }),
                        ));
                    }
                }
            }

            // Flush remaining
            let max_len = pending_dels.len().max(pending_adds.len());
            for i in 0..max_len {
                paired.push((pending_dels.get(i).cloned(), pending_adds.get(i).cloned()));
            }
        }

        let max_scroll = paired.len().saturating_sub(area.height as usize);
        let scroll = self.scroll_offset.min(max_scroll);

        for (row_idx, (left, right)) in paired
            .iter()
            .skip(scroll)
            .take(area.height as usize)
            .enumerate()
        {
            let y = area.y + row_idx as u16;

            self.render_split_line(
                buf,
                left_area.x,
                y,
                left_area.width,
                left.as_ref(),
                &file.path,
            );
            self.render_split_line(
                buf,
                right_area.x,
                y,
                right_area.width,
                right.as_ref(),
                &file.path,
            );
        }
    }

    fn render_split_line(
        &self,
        buf: &mut Buffer,
        x: u16,
        y: u16,
        width: u16,
        line: Option<&SplitLine>,
        path: &str,
    ) {
        let max_x = x + width;

        match line {
            None => {
                // Empty line - fill with background
                for cx in x..max_x {
                    buf.set_string(cx, y, " ", Style::default().bg(BG_COLOR));
                }
            }
            Some(SplitLine::Hunk(header)) => {
                let text = truncate_or_pad(header, width as usize);
                buf.set_string(
                    x,
                    y,
                    &text,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::DIM)
                        .bg(BG_COLOR),
                );
            }
            Some(SplitLine::Del { ln, content }) => {
                self.render_split_content_line(buf, x, y, width, *ln, content, DEL_BG, path);
            }
            Some(SplitLine::Add { ln, content }) => {
                self.render_split_content_line(buf, x, y, width, *ln, content, ADD_BG, path);
            }
            Some(SplitLine::Context { ln, content }) => {
                self.render_split_content_line(buf, x, y, width, *ln, content, BG_COLOR, path);
            }
        }
    }

    fn render_split_content_line(
        &self,
        buf: &mut Buffer,
        x: u16,
        y: u16,
        width: u16,
        ln: u32,
        content: &str,
        bg: Color,
        path: &str,
    ) {
        let max_x = x + width;

        // Fill entire row with background first
        for cx in x..max_x {
            buf.set_string(cx, y, " ", Style::default().bg(bg));
        }

        let gutter = format!("{:>4} ", ln);
        let gutter_len = gutter.len() as u16;

        buf.set_string(x, y, &gutter, Style::default().fg(Color::DarkGray).bg(bg));

        // Comment thread indicator
        let indicator_x = x + gutter_len;
        if self.has_threads_at_line(path, ln) {
            let count = self.thread_count_at_line(path, ln);
            let indicator = if count > 1 {
                format!("{}", count)
            } else {
                "C".to_string()
            };
            let indicator_style = Style::default()
                .fg(Color::Yellow)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            buf.set_string(indicator_x, y, &indicator, indicator_style);
        }

        let content_start_x = x + gutter_len + 2; // +2 for indicator space

        // First render raw content as fallback (in case spans don't cover everything)
        let default_style = Style::default().fg(Color::White).bg(bg);
        let mut x_offset = content_start_x;
        for ch in content.chars() {
            if x_offset >= max_x {
                break;
            }
            buf.set_string(x_offset, y, &ch.to_string(), default_style);
            x_offset += 1;
        }

        // Then overlay syntax highlighted spans
        let highlighted = self.highlighter.highlight_line(content, path);
        x_offset = content_start_x;

        for span in highlighted.spans {
            let span_style = span.style.bg(bg);
            for ch in span.content.chars() {
                if x_offset >= max_x {
                    break;
                }
                buf.set_string(x_offset, y, &ch.to_string(), span_style);
                x_offset += 1;
            }
        }
    }
}

#[derive(Clone)]
enum DiffDisplayLine {
    Hunk(String),
    Content {
        kind: LineKind,
        old_ln: Option<u32>,
        new_ln: Option<u32>,
        content: String,
    },
}

#[derive(Clone)]
enum SplitLine {
    Hunk(String),
    Del { ln: u32, content: String },
    Add { ln: u32, content: String },
    Context { ln: u32, content: String },
}

/// Fill an entire area with a background color
fn fill_area(buf: &mut Buffer, area: Rect, color: Color) {
    let style = Style::default().bg(color);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            buf.set_string(x, y, " ", style);
        }
    }
}

/// Truncate or pad a string to exactly the given width
fn truncate_or_pad(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() >= width {
        chars[..width].iter().collect()
    } else {
        let mut result: String = chars.into_iter().collect();
        result.push_str(&" ".repeat(width - result.len()));
        result
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_text_with_code_preserves_whitespace() {
        let text = "Here is some code:\n```\n    indented line\n\tline with tab\n  two spaces\n```\nAfter code";
        let result = App::wrap_text_with_code(text, 80);

        // Find the code lines
        let code_lines: Vec<_> = result.iter().filter(|(_, is_code)| *is_code).collect();

        assert_eq!(code_lines.len(), 3, "Should have 3 code lines");
        assert_eq!(code_lines[0].0, "    indented line", "Should preserve 4-space indent");
        assert_eq!(code_lines[1].0, "\tline with tab", "Should preserve tab");
        assert_eq!(code_lines[2].0, "  two spaces", "Should preserve 2-space indent");
    }

    #[test]
    fn test_wrap_text_with_code_detects_blocks() {
        let text = "Normal text\n```rust\nfn main() {}\n```\nMore text";
        let result = App::wrap_text_with_code(text, 80);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0], ("Normal text".to_string(), false));
        assert_eq!(result[1], ("fn main() {}".to_string(), true));
        assert_eq!(result[2], ("More text".to_string(), false));
    }

    #[test]
    fn test_wrap_text_with_code_empty_lines_in_code() {
        let text = "```\nline1\n\nline2\n```";
        let result = App::wrap_text_with_code(text, 80);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0], ("line1".to_string(), true));
        assert_eq!(result[1], (String::new(), true)); // empty line in code block
        assert_eq!(result[2], ("line2".to_string(), true));
    }
}
