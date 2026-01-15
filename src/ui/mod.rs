//! TUI module for the kensa PR review application.

mod helpers;
mod tree;
mod types;

use std::collections::{HashMap, HashSet};
use std::io::Stdout;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Terminal;

use crate::config::Config;
use crate::syntax::Highlighter;
use crate::types::{CommentThread, DiffFile, LineKind, PendingComment, ReviewPr};

// Re-export public types
pub use types::{CommentMode, HelpMode, LoadingState, PrListTab, Screen, ViewMode};

// Internal type imports
use types::{Focus, TreeItem, TreeNode};

// Type aliases to reduce complexity warnings
type DiffResultReceiver =
    mpsc::Receiver<Result<(Vec<DiffFile>, Option<String>, Option<String>), String>>;
type PrListReceiver = mpsc::Receiver<Result<(Vec<ReviewPr>, Vec<ReviewPr>), String>>;

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
    author_filter: Option<String>, // When viewing PRs by a specific user

    // Diff view state
    pub files: Vec<DiffFile>,
    pub selected_file: usize,
    pub scroll_offset: usize,
    pub horizontal_scroll: usize,  // Horizontal scroll offset for long lines
    pub view_mode: ViewMode,
    pub collapsed: HashSet<usize>,
    pub highlighter: Highlighter,
    pub config: Config,
    focus: Focus,
    should_quit: bool,
    confirm_quit: bool,
    tree_scroll: usize,
    search_mode: bool,
    search_query: String,
    filtered_indices: Vec<usize>,
    collapsed_folders: HashSet<String>,
    selected_tree_item: Option<String>,

    // For async diff loading
    diff_receiver: Option<DiffResultReceiver>, // (files, head_sha, body)
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

    // PR description display
    show_pr_description: bool,
    pr_description_scroll: usize,

    // Comment threads (existing comments from GitHub)
    comment_threads: Vec<CommentThread>,
    line_to_threads: HashMap<(String, u32), Vec<usize>>, // Quick lookup: (file_path, line) -> thread indices

    // Async receivers for non-blocking operations
    comment_threads_receiver: Option<mpsc::Receiver<Result<Vec<CommentThread>, String>>>,
    pr_list_receiver: Option<PrListReceiver>,
    comment_submit_receiver: Option<mpsc::Receiver<Result<usize, String>>>,
    reply_submit_receiver: Option<mpsc::Receiver<Result<usize, String>>>, // thread_index on success
    review_submit_receiver: Option<mpsc::Receiver<Result<(String, usize), String>>>, // (review action, comments count) on success

    // Cached tree structure to avoid rebuilding on every navigation
    cached_tree: Option<Vec<TreeNode>>,
    cached_flat_items: Option<Vec<TreeItem>>,
}

impl App {
    /// Create app in diff view mode (for direct PR URL)
    pub fn new(files: Vec<DiffFile>) -> Self {
        let file_count = files.len();
        let config = Config::load();
        let view_mode = if config.is_split_view_default() {
            ViewMode::Split
        } else {
            ViewMode::Unified
        };
        let highlighter = Highlighter::with_options(config.display.min_brightness, &config.display.theme);

        // Initialize collapsed folders based on config
        let (collapsed_folders, selected_tree_item) =
            if config.navigation.collapse_folders_by_default {
                let mut folders = HashSet::new();
                let mut root_folders = HashSet::new();
                for file in &files {
                    let parts: Vec<&str> = file.path.split('/').collect();
                    for i in 1..parts.len() {
                        let folder_path = parts[..i].join("/");
                        if i == 1 {
                            root_folders.insert(folder_path.clone());
                        }
                        folders.insert(folder_path);
                    }
                }
                // Select the first root folder alphabetically if there are folders
                let first_folder = root_folders.into_iter().min();
                (folders, first_folder)
            } else {
                (HashSet::new(), None)
            };

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
            author_filter: None,

            // Diff view state
            files,
            selected_file: 0,
            scroll_offset: 0,
            horizontal_scroll: 0,
            view_mode,
            collapsed: HashSet::new(),
            highlighter,
            config,
            focus: Focus::Tree,
            should_quit: false,
            confirm_quit: false,
            tree_scroll: 0,
            search_mode: false,
            search_query: String::new(),
            filtered_indices: (0..file_count).collect(),
            collapsed_folders,
            selected_tree_item,

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

            show_pr_description: false,
            pr_description_scroll: 0,

            comment_threads: Vec::new(),
            line_to_threads: HashMap::new(),

            comment_threads_receiver: None,
            pr_list_receiver: None,
            comment_submit_receiver: None,
            reply_submit_receiver: None,
            review_submit_receiver: None,

            cached_tree: None,
            cached_flat_items: None,
        }
    }

    /// Create app in diff view mode with PR context (for direct PR URL with review support)
    pub fn new_with_pr(files: Vec<DiffFile>, pr: ReviewPr) -> Self {
        let mut app = Self::new(files);
        app.current_pr = Some(pr);
        app.load_current_drafts(); // Load any saved drafts for this PR
        app.load_comment_threads(); // Load existing comments from GitHub
        app
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

        let config = Config::load();
        let view_mode = if config.is_split_view_default() {
            ViewMode::Split
        } else {
            ViewMode::Unified
        };
        let highlighter = Highlighter::with_options(config.display.min_brightness, &config.display.theme);

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
            author_filter: None,

            // Diff view state (empty until a PR is selected)
            files: Vec::new(),
            selected_file: 0,
            scroll_offset: 0,
            horizontal_scroll: 0,
            view_mode,
            collapsed: HashSet::new(),
            highlighter,
            config,
            focus: Focus::Tree,
            should_quit: false,
            confirm_quit: false,
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

            show_pr_description: false,
            pr_description_scroll: 0,

            comment_threads: Vec::new(),
            line_to_threads: HashMap::new(),

            comment_threads_receiver: None,
            pr_list_receiver: None,
            comment_submit_receiver: None,
            reply_submit_receiver: None,
            review_submit_receiver: None,

            cached_tree: None,
            cached_flat_items: None,
        }
    }

    /// Create app in PR list mode showing PRs by a specific author
    pub fn new_with_author_prs(author: String, prs: Vec<ReviewPr>) -> Self {
        let mut app = Self::new_with_prs(Vec::new(), prs);
        app.author_filter = Some(author);
        app.pr_tab = PrListTab::MyPrs; // Show the author's PRs tab by default
        app
    }

    // ========================================================================
    // Config Color Helpers
    // ========================================================================

    /// Get the background color for context lines
    fn bg_color(&self) -> Color {
        let c = &self.config.colors.context_bg;
        Color::Rgb(c.r, c.g, c.b)
    }

    /// Get the background color for deleted lines
    fn del_bg(&self) -> Color {
        let c = &self.config.colors.del_bg;
        Color::Rgb(c.r, c.g, c.b)
    }

    /// Get the background color for added lines
    fn add_bg(&self) -> Color {
        let c = &self.config.colors.add_bg;
        Color::Rgb(c.r, c.g, c.b)
    }

    /// Get the background color for cursor line
    fn cursor_bg(&self) -> Color {
        let c = &self.config.colors.cursor_bg;
        Color::Rgb(c.r, c.g, c.b)
    }

    /// Get the gutter color for cursor line
    fn cursor_gutter(&self) -> Color {
        let c = &self.config.colors.cursor_gutter;
        Color::Rgb(c.r, c.g, c.b)
    }

    /// Get the accent color for highlights, active elements
    fn accent_color(&self) -> Color {
        let c = &self.config.colors.accent;
        Color::Rgb(c.r, c.g, c.b)
    }

    // ========================================================================
    // UI Helper Functions
    // ========================================================================

    /// Create a centered popup area within the given container
    fn centered_popup(container: Rect, width: u16, height: u16) -> Rect {
        let popup_width = width.min(container.width.saturating_sub(4));
        let popup_height = height.min(container.height.saturating_sub(4));
        let popup_x = container.x + (container.width - popup_width) / 2;
        let popup_y = container.y + (container.height - popup_height) / 2;
        Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        }
    }

    /// Clear the background of an area with a solid color
    fn clear_popup_background(buf: &mut Buffer, area: Rect, color: Color) {
        let style = Style::default().bg(color);
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                buf.set_string(x, y, " ", style);
            }
        }
    }

    pub fn run(&mut self) -> Result<()> {
        let mut terminal = helpers::setup_terminal()?;
        let result = self.event_loop(&mut terminal);
        helpers::restore_terminal(&mut terminal)?;
        result
    }

    fn event_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        loop {
            // Check for async diff loading completion
            if let Some(ref receiver) = self.diff_receiver
                && let Ok(result) = receiver.try_recv() {
                    match result {
                        Ok((files, head_sha, body)) => {
                            let file_count = files.len();
                            self.files = files;
                            self.filtered_indices = (0..file_count).collect();
                            self.selected_file = 0;
                            self.scroll_offset = 0;
                            self.diff_cursor = 0;
                            self.collapsed.clear();
                            self.init_collapsed_folders();
                            self.invalidate_tree_cache(); // Cache invalidated when files change
                            self.screen = Screen::DiffView;
                            self.loading = LoadingState::Idle;
                            // Store head SHA and body for the PR
                            if let Some(ref mut pr) = self.current_pr {
                                pr.head_sha = head_sha;
                                pr.body = body;
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

            // Check for async comment threads loading completion
            if let Some(ref receiver) = self.comment_threads_receiver
                && let Ok(result) = receiver.try_recv() {
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

            // Check for async PR list refresh completion
            if let Some(ref receiver) = self.pr_list_receiver
                && let Ok(result) = receiver.try_recv() {
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

            // Check for async comment submission completion
            if let Some(ref receiver) = self.comment_submit_receiver
                && let Ok(result) = receiver.try_recv() {
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

            // Check for async reply submission completion
            if let Some(ref receiver) = self.reply_submit_receiver
                && let Ok(result) = receiver.try_recv() {
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

            // Check for async review submission completion
            if let Some(ref receiver) = self.review_submit_receiver
                && let Ok(result) = receiver.try_recv() {
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

            terminal.draw(|f| self.render(f))?;

            if event::poll(Duration::from_millis(50))?
                && let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press {
                        self.handle_key(key);
                    }

            if self.should_quit {
                break;
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Handle quit confirmation dialog
        if self.confirm_quit {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.should_quit = true;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_quit = false;
                }
                _ => {}
            }
            return;
        }

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
            KeyCode::Char('q') => self.request_quit(),
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
                    self.request_quit();
                }
            }
            KeyCode::Char('/') => {
                self.pr_search_mode = true;
                self.pr_search_query.clear();
            }
            KeyCode::Char('1') => {
                // Ignore in author mode (no tabs)
                if self.author_filter.is_none() {
                    self.pr_tab = PrListTab::ForReview;
                    self.update_filtered_pr_indices();
                }
            }
            KeyCode::Char('2') => {
                // Ignore in author mode (no tabs)
                if self.author_filter.is_none() {
                    self.pr_tab = PrListTab::MyPrs;
                    self.update_filtered_pr_indices();
                }
            }
            KeyCode::Tab => {
                // Ignore in author mode (no tabs)
                if self.author_filter.is_none() {
                    self.toggle_pr_tab();
                }
            }
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

        // Handle PR description view
        if self.show_pr_description {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('i') => {
                    self.show_pr_description = false;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.pr_description_scroll = self.pr_description_scroll.saturating_add(1);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.pr_description_scroll = self.pr_description_scroll.saturating_sub(1);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.pr_description_scroll = self.pr_description_scroll.saturating_add(10);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.pr_description_scroll = self.pr_description_scroll.saturating_sub(10);
                }
                KeyCode::Char('g') => {
                    self.pr_description_scroll = 0;
                }
                KeyCode::Char('G') => {
                    // Will be capped in render
                    self.pr_description_scroll = usize::MAX;
                }
                _ => {}
            }
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
        if let CommentMode::ViewingThreads { selected, scroll: _ } = self.comment_mode {
            // selected is a visual index (position in the display order)
            // Get visual order and count before matching on key
            let visual_order = self.thread_visual_order();
            let visual_count = visual_order.len();
            let current_selected = selected;
            let thread_idx = visual_order.get(current_selected).copied();

            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.comment_mode = CommentMode::None;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if visual_count > 0 && current_selected < visual_count - 1 {
                        self.comment_mode = CommentMode::ViewingThreads {
                            selected: current_selected + 1,
                            scroll: 0,
                        };
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if current_selected > 0 {
                        self.comment_mode = CommentMode::ViewingThreads {
                            selected: current_selected - 1,
                            scroll: 0,
                        };
                    }
                }
                KeyCode::Enter => {
                    // Open thread detail view - convert visual index to original index
                    if let Some(idx) = thread_idx {
                        self.comment_mode = CommentMode::ViewingThread {
                            index: idx,
                            selected: current_selected, // Store visual index for returning
                            scroll: 0,
                        };
                    }
                }
                KeyCode::Char('r') => {
                    // Start reply - convert visual index to original index
                    if let Some(idx) = thread_idx {
                        self.comment_mode = CommentMode::ReplyingToThread {
                            index: idx,
                            text: String::new(),
                        };
                    }
                }
                KeyCode::Char('g') => {
                    // Jump to thread location in diff - convert visual index to original index
                    if let Some(idx) = thread_idx {
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
                }
                _ => {}
            }
            return;
        }

        // Handle viewing single thread detail mode
        if let CommentMode::ViewingThread {
            index,
            selected: visual_idx,
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
                    total_lines += helpers::wrap_text(&comment.body, wrap_width).len();
                    total_lines += 1; // Separator
                }
                total_lines.saturating_sub(20) // Approximate visible height
            }).unwrap_or(0);

            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    // Go back to thread list - use stored visual index
                    self.comment_mode = CommentMode::ViewingThreads {
                        selected: visual_idx,
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
                // If we came from PR list (normal or author mode), go back; otherwise quit
                if !self.review_prs.is_empty() || !self.my_prs.is_empty() || self.author_filter.is_some() {
                    self.screen = Screen::PrList;
                    self.current_pr = None;
                } else {
                    self.request_quit();
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
                } else if !self.review_prs.is_empty() || !self.my_prs.is_empty() || self.author_filter.is_some() {
                    self.screen = Screen::PrList;
                    self.current_pr = None;
                } else {
                    self.request_quit();
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
            KeyCode::Char('H') => self.scroll_left(),
            KeyCode::Char('L') => self.scroll_right(),
            KeyCode::Enter | KeyCode::Tab => self.toggle_focus(),
            KeyCode::Char('d') => self.toggle_view_mode(),
            KeyCode::Char('x') => self.toggle_collapse(),
            KeyCode::Char('g') => self.scroll_to_top(),
            KeyCode::Char('G') => self.scroll_to_bottom(),
            KeyCode::Char('o') => self.open_pr_in_browser(),
            KeyCode::Char('i') => {
                // Toggle PR description view
                self.show_pr_description = !self.show_pr_description;
                self.pr_description_scroll = 0;
            }
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
        if let Some(pos) = indices.iter().position(|&i| i == selected)
            && pos < indices.len() - 1 {
                self.set_current_selected_pr(indices[pos + 1]);
            }
    }

    fn move_pr_up(&mut self) {
        let indices = self.current_filtered_indices();
        if indices.is_empty() {
            return;
        }
        let selected = self.current_selected_pr();
        if let Some(pos) = indices.iter().position(|&i| i == selected)
            && pos > 0 {
                self.set_current_selected_pr(indices[pos - 1]);
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
                    && let Some(&first) = self.filtered_review_pr_indices.first() {
                        self.selected_review_pr = first;
                    }
                self.review_pr_scroll = 0;
            }
            PrListTab::MyPrs => {
                if !self.filtered_my_pr_indices.contains(&self.selected_my_pr)
                    && let Some(&first) = self.filtered_my_pr_indices.first() {
                        self.selected_my_pr = first;
                    }
                self.my_pr_scroll = 0;
            }
        }
    }

    fn pr_matches_filter(&self, pr: &ReviewPr, query: &str) -> bool {
        // Filter by repo
        if let Some(ref filter) = self.repo_filter
            && &pr.repo_full_name() != filter {
                return false;
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
                // Fetch diff and PR details (including body) in parallel
                let (diff_result, details_result) = tokio::join!(
                    crate::github::fetch_pr_diff(&pr_info),
                    crate::github::fetch_pr_details(&pr_info)
                );

                // Diff is required, details are optional (for head_sha and body)
                match diff_result {
                    Ok(diff_content) => {
                        let files = crate::parser::parse_diff(&diff_content);
                        let (head_sha, body) = details_result
                            .map(|d| (d.head_sha, d.body))
                            .unwrap_or((None, None));
                        Ok((files, head_sha, body))
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

    /// Save current drafts to disk (non-blocking)
    /// Spawns a background thread to avoid blocking the UI during file I/O
    fn save_current_drafts(&self) {
        if let Some(ref pr) = self.current_pr {
            let pr_info = pr.to_pr_info();
            let comments = self.pending_comments.clone();
            // Spawn background thread for file I/O to avoid blocking UI
            std::thread::spawn(move || {
                if let Err(e) = crate::drafts::save_drafts(&pr_info, &comments) {
                    // Silently ignore save errors - drafts are best-effort
                    eprintln!("Warning: Failed to save drafts: {}", e);
                }
            });
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

    /// Get count of pending comments for a file
    fn pending_comment_count_for_file(&self, file_path: &str) -> usize {
        self.pending_comments
            .iter()
            .filter(|c| c.file_path.as_deref() == Some(file_path))
            .count()
    }

    /// Get the visual order of thread indices (current threads first, then outdated)
    /// Returns a Vec where each element is the original index in self.comment_threads
    fn thread_visual_order(&self) -> Vec<usize> {
        let mut order = Vec::new();
        // Current threads first
        for (idx, thread) in self.comment_threads.iter().enumerate() {
            if !thread.outdated {
                order.push(idx);
            }
        }
        // Then outdated threads
        for (idx, thread) in self.comment_threads.iter().enumerate() {
            if thread.outdated {
                order.push(idx);
            }
        }
        order
    }

    /// Convert a visual index to the original thread index
    fn visual_to_thread_index(&self, visual_idx: usize) -> Option<usize> {
        self.thread_visual_order().get(visual_idx).copied()
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
        if !self.filtered_indices.contains(&self.selected_file)
            && let Some(&first) = self.filtered_indices.first() {
                self.select_file(first);
            }
        // Reset tree scroll
        self.tree_scroll = 0;
        // Invalidate tree cache when filter changes
        self.invalidate_tree_cache();
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
        self.horizontal_scroll = 0;
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
        // Ensure cache is populated, then work with the cached data
        self.ensure_flat_items_cached();
        let flat_items = self.get_flat_items();

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
            // Extract the values we need before modifying self
            let next_item_info = match &flat_items[current_pos + 1] {
                TreeItem::File { index, .. } => Some((true, *index, String::new())),
                TreeItem::Folder { path, .. } => Some((false, 0, path.clone())),
            };

            if let Some((is_file, index, path)) = next_item_info {
                if is_file {
                    self.select_file(index);
                } else {
                    self.selected_tree_item = Some(path);
                }
            }
        }
    }

    fn move_to_prev_tree_item(&mut self) {
        // Ensure cache is populated, then work with the cached data
        self.ensure_flat_items_cached();
        let flat_items = self.get_flat_items();

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
            // Extract the values we need before modifying self
            let prev_item_info = match &flat_items[current_pos - 1] {
                TreeItem::File { index, .. } => Some((true, *index, String::new())),
                TreeItem::Folder { path, .. } => Some((false, 0, path.clone())),
            };

            if let Some((is_file, index, path)) = prev_item_info {
                if is_file {
                    self.select_file(index);
                } else {
                    self.selected_tree_item = Some(path);
                }
            }
        }
    }

    fn move_to_next_file_only(&mut self) {
        // Ensure cache is populated, then work with the cached data
        self.ensure_flat_items_cached();
        let flat_items = self.get_flat_items();

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
        let next_file_idx = flat_items.iter().skip(current_pos + 1).find_map(|item| {
            if let TreeItem::File { index, .. } = item {
                Some(*index)
            } else {
                None
            }
        });

        if let Some(idx) = next_file_idx {
            self.select_file(idx);
        }
    }

    fn move_to_prev_file_only(&mut self) {
        // Ensure cache is populated, then work with the cached data
        self.ensure_flat_items_cached();
        let flat_items = self.get_flat_items();

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
        let prev_file_idx = flat_items.iter().take(current_pos).rev().find_map(|item| {
            if let TreeItem::File { index, .. } = item {
                Some(*index)
            } else {
                None
            }
        });

        if let Some(idx) = prev_file_idx {
            self.select_file(idx);
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
            // Invalidate tree cache when folder collapse state changes
            self.invalidate_tree_cache();
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

    fn scroll_left(&mut self) {
        let amount = self.config.navigation.horizontal_scroll_columns;
        self.horizontal_scroll = self.horizontal_scroll.saturating_sub(amount);
    }

    fn scroll_right(&mut self) {
        let amount = self.config.navigation.horizontal_scroll_columns;
        self.horizontal_scroll = self.horizontal_scroll.saturating_add(amount);
    }

    fn scroll_half_page_up(&mut self) {
        let amount = self.config.navigation.scroll_lines;
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }
    fn scroll_half_page_down(&mut self) {
        let amount = self.config.navigation.scroll_lines;
        self.scroll_offset += amount;
    }

    /// Request to quit the application, respecting the confirm_quit config setting
    fn request_quit(&mut self) {
        if self.config.navigation.confirm_quit {
            self.confirm_quit = true;
        } else {
            self.should_quit = true;
        }
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

        // Render PR description overlay if active
        if self.show_pr_description {
            self.render_pr_description(frame);
        }

        // Render quit confirmation dialog if active
        if self.confirm_quit {
            self.render_confirm_quit(frame);
        }
    }

    fn render_help(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let bg = Color::Rgb(25, 28, 38);

        let popup_height = match self.help_mode {
            HelpMode::PrList => 16,
            HelpMode::DiffView => 28,
            HelpMode::None => return,
        };

        let popup_area = Self::centered_popup(area, 65, popup_height);
        Self::clear_popup_background(frame.buffer_mut(), popup_area, bg);

        let title = match self.help_mode {
            HelpMode::PrList => " PR List Shortcuts ",
            HelpMode::DiffView => " Diff View Shortcuts ",
            HelpMode::None => return,
        };

        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(Color::Rgb(100, 200, 255)).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(70, 80, 100)));

        let inner_area = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        // Group commands by category
        let commands: Vec<(&str, Vec<(&str, &str)>)> = match self.help_mode {
            HelpMode::PrList => vec![
                ("Navigation", vec![
                    ("j/k", "Move up/down"),
                    ("Enter", "Open PR"),
                    ("Tab", "Switch tabs"),
                ]),
                ("Actions", vec![
                    ("o", "Open in browser"),
                    ("R", "Refresh list"),
                    ("f", "Filter by repo"),
                    ("/", "Search"),
                ]),
                ("General", vec![
                    ("Esc", "Clear filter"),
                    ("q", "Quit"),
                ]),
            ],
            HelpMode::DiffView => vec![
                ("Navigation", vec![
                    ("j/k", "Scroll up/down"),
                    ("h/l", "Prev/next file"),
                    ("H/L", "Scroll left/right"),
                    ("g/G", "Top/bottom"),
                    ("Ctrl+u/d", "Half page"),
                ]),
                ("View", vec![
                    ("Tab", "Toggle tree/diff"),
                    ("d", "Toggle split view"),
                    ("i", "View PR description"),
                    ("x", "Collapse folder"),
                    ("/", "Search files"),
                ]),
                ("Comments", vec![
                    ("c", "Add comment"),
                    ("v", "Visual select"),
                    ("C", "View drafts"),
                    ("t", "View threads"),
                    ("S", "Submit comments"),
                    ("A", "Submit review"),
                ]),
                ("General", vec![
                    ("o", "Open in browser"),
                    ("q", "Back/quit"),
                ]),
            ],
            HelpMode::None => return,
        };

        let buf = frame.buffer_mut();
        let mut y = inner_area.y;

        for (category, items) in commands {
            // Category header
            buf.set_string(
                inner_area.x + 1,
                y,
                category,
                Style::default()
                    .fg(Color::Rgb(180, 140, 220))
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            );
            y += 1;

            // Items in two columns
            let col_width = (inner_area.width / 2) as usize;
            for chunk in items.chunks(2) {
                if y >= inner_area.y + inner_area.height {
                    break;
                }

                // Left column
                if let Some((key, desc)) = chunk.first() {
                    buf.set_string(
                        inner_area.x + 2,
                        y,
                        key,
                        Style::default().fg(Color::Rgb(240, 200, 100)).bg(bg),
                    );
                    buf.set_string(
                        inner_area.x + 2 + key.len() as u16 + 1,
                        y,
                        desc,
                        Style::default().fg(Color::Rgb(180, 180, 200)).bg(bg),
                    );
                }

                // Right column
                if let Some((key, desc)) = chunk.get(1) {
                    buf.set_string(
                        inner_area.x + col_width as u16 + 1,
                        y,
                        key,
                        Style::default().fg(Color::Rgb(240, 200, 100)).bg(bg),
                    );
                    buf.set_string(
                        inner_area.x + col_width as u16 + 1 + key.len() as u16 + 1,
                        y,
                        desc,
                        Style::default().fg(Color::Rgb(180, 180, 200)).bg(bg),
                    );
                }
                y += 1;
            }
            y += 1; // Space between categories
        }

        // Footer hint
        let hint = "Press any key to close";
        let hint_x = popup_area.x + (popup_area.width.saturating_sub(hint.len() as u16)) / 2;
        buf.set_string(
            hint_x,
            popup_area.y + popup_area.height - 1,
            hint,
            Style::default().fg(Color::Rgb(80, 80, 100)).bg(bg),
        );
    }

    fn render_pr_description(&self, frame: &mut ratatui::Frame) {
        let Some(pr) = &self.current_pr else {
            return;
        };

        let area = frame.area();
        let bg = Color::Rgb(25, 28, 38);
        let accent = self.accent_color();

        // Use most of the screen for the description
        let popup_width = (area.width as f32 * 0.8) as u16;
        let popup_height = (area.height as f32 * 0.8) as u16;
        let popup_area = Self::centered_popup(area, popup_width, popup_height);

        // Clear popup background
        Self::clear_popup_background(frame.buffer_mut(), popup_area, bg);

        // Title with PR info
        let title = format!(" #{} - {} ", pr.number, pr.title);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(accent));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let buf = frame.buffer_mut();

        // PR metadata header
        let meta_line = format!("@{} | {} | {}", pr.author, pr.repo_full_name(), pr.age());
        buf.set_string(
            inner.x + 1,
            inner.y,
            &meta_line,
            Style::default().fg(Color::Rgb(150, 150, 180)).bg(bg),
        );

        // Separator line
        let separator: String = "─".repeat((inner.width.saturating_sub(2)) as usize);
        buf.set_string(
            inner.x + 1,
            inner.y + 1,
            &separator,
            Style::default().fg(Color::Rgb(60, 60, 80)).bg(bg),
        );

        // PR description body with text wrapping
        let body = pr.body.as_deref().unwrap_or("(No description provided)");
        let max_width = (inner.width.saturating_sub(2)) as usize;
        let wrapped_lines = helpers::wrap_text_with_code(body, max_width);
        let content_height = (inner.height.saturating_sub(4)) as usize; // Leave room for header, separator, footer
        let total_lines = wrapped_lines.len();

        // Cap scroll offset
        let max_scroll = total_lines.saturating_sub(content_height);
        let scroll = self.pr_description_scroll.min(max_scroll);

        // Render visible lines
        let visible_lines = wrapped_lines.iter().skip(scroll).take(content_height);
        let content_start_y = inner.y + 2;
        let code_bg = Color::Rgb(35, 38, 48);

        for (i, (line, is_code)) in visible_lines.enumerate() {
            let y = content_start_y + i as u16;
            if y >= inner.y + inner.height - 1 {
                break;
            }

            let line_bg = if *is_code { code_bg } else { bg };
            let line_fg = if *is_code {
                Color::Rgb(180, 220, 180) // Greenish for code
            } else {
                Color::Rgb(220, 220, 230)
            };

            // Clear line background for code blocks
            if *is_code {
                for x in inner.x + 1..inner.x + inner.width - 1 {
                    buf.set_string(x, y, " ", Style::default().bg(line_bg));
                }
            }

            buf.set_string(
                inner.x + 1,
                y,
                line,
                Style::default().fg(line_fg).bg(line_bg),
            );
        }

        // Footer with scroll indicator and help
        let scroll_info = if total_lines > content_height {
            format!(" {}/{} ", scroll + 1, total_lines.saturating_sub(content_height) + 1)
        } else {
            String::new()
        };
        let hint = format!("{}j/k scroll | q/Esc close", scroll_info);
        let hint_x = popup_area.x + (popup_area.width.saturating_sub(hint.len() as u16)) / 2;
        buf.set_string(
            hint_x,
            popup_area.y + popup_area.height - 1,
            &hint,
            Style::default().fg(Color::Rgb(80, 80, 100)).bg(bg),
        );
    }

    fn render_loading(&self, frame: &mut ratatui::Frame, message: &str) {
        let area = frame.area();
        // Size popup based on message length, with min/max bounds
        let msg_len = message.chars().count() as u16;
        let popup_width = (msg_len + 4).clamp(30, area.width.saturating_sub(4));
        let popup_area = Self::centered_popup(area, popup_width, 5);

        // Clear popup background
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(30, 35, 50));

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(100, 180, 220)));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let buf = frame.buffer_mut();
        let bg = Color::Rgb(30, 35, 50);

        // Loading indicator
        let spinner = "*";
        buf.set_string(
            inner.x + (inner.width / 2).saturating_sub(1),
            inner.y,
            spinner,
            Style::default().fg(Color::Rgb(100, 200, 255)).bg(bg).add_modifier(Modifier::BOLD),
        );

        // Message (truncate if needed)
        let max_msg_width = inner.width.saturating_sub(2) as usize;
        let display_msg: String = if message.chars().count() > max_msg_width {
            message.chars().take(max_msg_width.saturating_sub(1)).collect::<String>() + "…"
        } else {
            message.to_string()
        };
        let display_width = display_msg.chars().count() as u16;
        let msg_x = inner.x + (inner.width.saturating_sub(display_width)) / 2;
        buf.set_string(
            msg_x,
            inner.y + 2,
            &display_msg,
            Style::default().fg(Color::Rgb(200, 200, 220)).bg(bg),
        );
    }

    fn render_error(&self, frame: &mut ratatui::Frame, message: &str) {
        let area = frame.area();
        let popup_width = 60u16.min(area.width.saturating_sub(4));
        let popup_area = Self::centered_popup(area, popup_width, 7);

        // Clear popup background
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(50, 30, 35));

        let block = Block::default()
            .title(" Error ")
            .title_style(Style::default().fg(Color::Rgb(255, 100, 100)).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(200, 80, 80)));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let buf = frame.buffer_mut();
        let bg = Color::Rgb(50, 30, 35);

        // Wrap message if too long
        let max_width = inner.width.saturating_sub(2) as usize;
        let lines: Vec<String> = message
            .chars()
            .collect::<Vec<_>>()
            .chunks(max_width)
            .map(|c| c.iter().collect())
            .collect();

        for (i, line) in lines.iter().take(3).enumerate() {
            buf.set_string(
                inner.x + 1,
                inner.y + i as u16,
                line,
                Style::default().fg(Color::Rgb(255, 180, 180)).bg(bg),
            );
        }

        // Continue hint
        buf.set_string(
            inner.x + 1,
            inner.y + inner.height - 1,
            "Press any key to continue",
            Style::default().fg(Color::Rgb(150, 100, 100)).bg(bg),
        );
    }

    fn render_confirm_quit(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let popup_width = 40u16.min(area.width.saturating_sub(4));
        let popup_area = Self::centered_popup(area, popup_width, 5);

        // Clear popup background
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(35, 35, 50));

        let block = Block::default()
            .title(" Quit ")
            .title_style(Style::default().fg(Color::Rgb(200, 200, 220)).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(100, 100, 140)));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let buf = frame.buffer_mut();
        let bg = Color::Rgb(35, 35, 50);

        // Question
        let msg = "Are you sure you want to quit?";
        let msg_x = inner.x + (inner.width.saturating_sub(msg.len() as u16)) / 2;
        buf.set_string(
            msg_x,
            inner.y,
            msg,
            Style::default().fg(Color::Rgb(200, 200, 220)).bg(bg),
        );

        // Options
        let options = "(y)es  (n)o";
        let opt_x = inner.x + (inner.width.saturating_sub(options.len() as u16)) / 2;
        buf.set_string(
            opt_x,
            inner.y + 2,
            options,
            Style::default().fg(Color::Rgb(140, 140, 180)).bg(bg),
        );
    }

    fn render_success(&self, frame: &mut ratatui::Frame, message: &str) {
        let area = frame.area();
        let popup_width = 50u16.min(area.width.saturating_sub(4));
        let popup_area = Self::centered_popup(area, popup_width, 6);

        // Clear popup background
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(30, 50, 40));

        let block = Block::default()
            .title(" Success ")
            .title_style(Style::default().fg(Color::Rgb(100, 220, 140)).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(80, 180, 120)));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let buf = frame.buffer_mut();
        let bg = Color::Rgb(30, 50, 40);

        // Message
        let msg_width = message.chars().count() as u16;
        let msg_x = inner.x + (inner.width.saturating_sub(msg_width)) / 2;
        buf.set_string(
            msg_x.max(inner.x),
            inner.y + 1,
            message,
            Style::default().fg(Color::Rgb(180, 255, 200)).bg(bg),
        );

        // Continue hint
        let hint = "Press any key to continue";
        let hint_x = inner.x + (inner.width.saturating_sub(hint.len() as u16)) / 2;
        buf.set_string(
            hint_x,
            inner.y + 3,
            hint,
            Style::default().fg(Color::Rgb(100, 140, 110)).bg(bg),
        );
    }

    fn render_pr_list(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let buf = frame.buffer_mut();

        // Render header bar at top
        let tab_height = 3; // Increased for underline indicator
        let tab_y = area.y;

        // Header bar background - slightly darker for better contrast
        let header_bg = Color::Rgb(25, 25, 35);
        let tab_bg = Style::default().bg(header_bg);
        for x in area.x..area.x + area.width {
            buf.set_string(x, tab_y, " ", tab_bg);
            buf.set_string(x, tab_y + 1, " ", tab_bg);
            buf.set_string(x, tab_y + 2, " ", Style::default().bg(Color::Rgb(35, 35, 45)));
        }

        // App name
        let name = "kensa";
        let name_style = Style::default()
            .fg(Color::Rgb(180, 140, 255)) // Softer purple
            .bg(header_bg)
            .add_modifier(Modifier::BOLD);
        buf.set_string(area.x + 1, tab_y, name, name_style);

        // Separator
        let sep = " | ";
        let sep_style = Style::default()
            .fg(Color::Rgb(60, 60, 70))
            .bg(header_bg);
        let name_width: u16 = 5; // "kensa"
        buf.set_string(area.x + 1 + name_width, tab_y, sep, sep_style);

        let tabs_start = area.x + 1 + name_width + sep.len() as u16;

        // Author mode: simple header without tabs
        if let Some(ref author) = self.author_filter {
            let header_style = Style::default()
                .fg(Color::Rgb(100, 200, 255)) // Softer cyan
                .bg(header_bg)
                .add_modifier(Modifier::BOLD);
            let header_text = format!("  @{}'s PRs ({}) ", author, self.filtered_my_pr_indices.len());
            buf.set_string(tabs_start, tab_y, &header_text, header_style);
            // Underline indicator
            for x in tabs_start..(tabs_start + header_text.len() as u16) {
                buf.set_string(x, tab_y + 1, "─", Style::default().fg(Color::Rgb(100, 200, 255)).bg(header_bg));
            }
        } else {
            // Normal mode: show tabs with visual indicators
            // Tab 1: For Review
            let tab1_active = self.pr_tab == PrListTab::ForReview;
            let tab1_style = if tab1_active {
                Style::default()
                    .fg(Color::Rgb(100, 200, 255)) // Bright cyan
                    .bg(header_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Rgb(100, 100, 110))
                    .bg(header_bg)
            };
            let tab1_text = format!(
                " For Review ({}) ",
                self.filtered_review_pr_indices.len()
            );
            buf.set_string(tabs_start, tab_y, &tab1_text, tab1_style);
            // Underline for active tab
            if tab1_active {
                for x in tabs_start..(tabs_start + tab1_text.len() as u16) {
                    buf.set_string(x, tab_y + 1, "━", Style::default().fg(Color::Rgb(100, 200, 255)).bg(header_bg));
                }
            }

            // Tab 2: My PRs
            let tab2_active = self.pr_tab == PrListTab::MyPrs;
            let tab2_style = if tab2_active {
                Style::default()
                    .fg(Color::Rgb(100, 200, 255))
                    .bg(header_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Rgb(100, 100, 110))
                    .bg(header_bg)
            };
            let tab2_text = format!(" My PRs ({}) ", self.filtered_my_pr_indices.len());
            let tab2_x = tabs_start + tab1_text.len() as u16;
            buf.set_string(tab2_x, tab_y, &tab2_text, tab2_style);
            // Underline for active tab
            if tab2_active {
                for x in tab2_x..(tab2_x + tab2_text.len() as u16) {
                    buf.set_string(x, tab_y + 1, "━", Style::default().fg(Color::Rgb(100, 200, 255)).bg(header_bg));
                }
            }

            // Help hint (right-aligned)
            let hint = "?:help  Tab:switch";
            let hint_x = area.x + area.width - hint.len() as u16 - 2;
            buf.set_string(
                hint_x,
                tab_y,
                hint,
                Style::default()
                    .fg(Color::Rgb(80, 80, 90))
                    .bg(header_bg),
            );
        }

        // Filter/search info on third line (separator line)
        let filter_info = if self.pr_search_mode {
            format!("  /{}_ ", self.pr_search_query)
        } else {
            let mut info = String::new();
            if let Some(ref repo) = self.repo_filter {
                info.push_str(&format!(" repo:{} ", repo));
            }
            if !self.pr_search_query.is_empty() {
                info.push_str(&format!(" filter:{} ", self.pr_search_query));
            }
            info
        };
        if !filter_info.is_empty() {
            buf.set_string(
                area.x + 1,
                tab_y + 2,
                &filter_info,
                Style::default()
                    .fg(Color::Rgb(220, 180, 100)) // Warm yellow
                    .bg(Color::Rgb(35, 35, 45)),
            );
        }

        // Content area below tabs
        let content_area = Rect {
            x: area.x,
            y: area.y + tab_height,
            width: area.width,
            height: area.height.saturating_sub(tab_height),
        };

        let border_style = Style::default().fg(Color::Rgb(60, 60, 80));
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

                let header_bg = Color::Rgb(28, 28, 38);
                let header_style = Style::default()
                    .fg(Color::Rgb(130, 160, 200)) // Softer blue
                    .bg(header_bg)
                    .add_modifier(Modifier::BOLD);

                for x in inner_area.x..inner_area.x + inner_area.width {
                    buf.set_string(x, y, " ", Style::default().bg(header_bg));
                }

                // Repo name
                let repo_display = format!("  {}", repo);
                let truncated_repo: String =
                    repo_display.chars().take(inner_area.width as usize - 1).collect();
                buf.set_string(inner_area.x, y, &truncated_repo, header_style);

                row += 1;
                if row >= visible_height {
                    break;
                }
            }

            // Render PR row
            let y = inner_area.y + row as u16;
            let is_selected = pr_idx == selected;

            // Enhanced selection styling
            let (style, row_bg) = if is_selected {
                (
                    Style::default()
                        .bg(Color::Rgb(50, 60, 80)) // Distinctive blue-gray selection
                        .fg(Color::Rgb(240, 240, 250))
                        .add_modifier(Modifier::BOLD),
                    Color::Rgb(50, 60, 80)
                )
            } else {
                (
                    Style::default()
                        .bg(Color::Rgb(22, 22, 28)) // Subtle row background
                        .fg(Color::Rgb(200, 200, 210)),
                    Color::Rgb(22, 22, 28)
                )
            };

            // Clear line with row background
            for x in inner_area.x..inner_area.x + inner_area.width {
                buf.set_string(x, y, " ", Style::default().bg(row_bg));
            }

            let mut x = inner_area.x;

            // Selection indicator
            if is_selected {
                buf.set_string(x, y, " ▸", Style::default().fg(Color::Rgb(100, 200, 255)).bg(row_bg));
            } else {
                buf.set_string(x, y, "  ", Style::default().bg(row_bg));
            }
            x += 2;

            // PR number with accent color
            let num_str = format!("#{:<6}", pr.number);
            let accent = self.accent_color();
            let num_style = if is_selected {
                Style::default().fg(accent).bg(row_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(accent).bg(row_bg)
            };
            buf.set_string(x, y, &num_str, num_style);
            x += 7;

            // Calculate space for title
            let author_age = format!("@{} · {}", pr.author, pr.age());
            let author_age_len = author_age.chars().count();
            let title_max_width = (inner_area.x + inner_area.width)
                .saturating_sub(x)
                .saturating_sub(author_age_len as u16 + 2)
                as usize;

            // Title (truncated) with better styling
            let title: String = pr.title.chars().take(title_max_width).collect();
            let title_display = if pr.title.chars().count() > title_max_width {
                format!("{}…", &title[..title.len().saturating_sub(1)])
            } else {
                title
            };
            buf.set_string(x, y, &title_display, style);

            // Author and age (right-aligned) with softer styling
            let right_x = inner_area.x + inner_area.width - author_age_len as u16 - 1;
            let author_style = if is_selected {
                Style::default().fg(Color::Rgb(150, 150, 170)).bg(row_bg)
            } else {
                Style::default().fg(Color::Rgb(90, 90, 110)).bg(row_bg)
            };
            buf.set_string(right_x, y, &author_age, author_style);

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
        let help_y = area.y + area.height - 1;
        let footer_bg = Color::Rgb(25, 25, 32);

        // Fill footer background
        for x in area.x..area.x + area.width {
            buf.set_string(x, help_y, " ", Style::default().bg(footer_bg));
        }

        // Key hints with better formatting
        let hints = vec![
            ("j/k", "navigate"),
            ("Enter", "open"),
            ("o", "browser"),
            ("f", "filter"),
            ("/", "search"),
            ("R", "refresh"),
            ("?", "help"),
        ];

        let mut x = area.x + 1;
        for (key, action) in hints {
            // Key
            buf.set_string(x, help_y, key, Style::default().fg(Color::Rgb(130, 180, 220)).bg(footer_bg));
            x += key.len() as u16;
            // Colon
            buf.set_string(x, help_y, ":", Style::default().fg(Color::Rgb(60, 60, 70)).bg(footer_bg));
            x += 1;
            // Action
            buf.set_string(x, help_y, action, Style::default().fg(Color::Rgb(100, 100, 120)).bg(footer_bg));
            x += action.len() as u16 + 2;

            if x >= area.x + area.width - 10 {
                break;
            }
        }

        // Show item count on the right
        let filtered = self.current_filtered_indices();
        let total = match self.pr_tab {
            PrListTab::ForReview => self.review_prs.len(),
            PrListTab::MyPrs => self.my_prs.len(),
        };
        let count_str = if filtered.len() == total {
            format!("{} PRs ", total)
        } else {
            format!("{}/{} PRs ", filtered.len(), total)
        };
        let count_x = area.x + area.width - count_str.len() as u16 - 1;
        buf.set_string(count_x, help_y, &count_str, Style::default().fg(Color::Rgb(80, 80, 100)).bg(footer_bg));
    }

    fn render_diff_view(&self, frame: &mut ratatui::Frame) {
        let area = frame.area();

        // If we came from PR list, show a header bar with back navigation
        if let Some(ref pr) = self.current_pr {
            let header_height = 3; // Increased for status bar
            let content_area = Rect {
                x: area.x,
                y: area.y + header_height,
                width: area.width,
                height: area.height.saturating_sub(header_height),
            };

            // Render header
            let buf = frame.buffer_mut();
            let header_bg = Color::Rgb(25, 25, 35);
            let status_bg = Color::Rgb(30, 30, 42);

            // First line: PR info with repo icon
            for x in area.x..area.x + area.width {
                buf.set_string(x, area.y, " ", Style::default().bg(header_bg));
            }

            // PR number badge with accent color
            let pr_num = format!(" #{} ", pr.number);
            buf.set_string(
                area.x + 1,
                area.y,
                &pr_num,
                Style::default()
                    .fg(Color::Rgb(25, 25, 35))
                    .bg(self.accent_color())
                    .add_modifier(Modifier::BOLD),
            );

            // PR title
            let title_start = area.x + 1 + pr_num.len() as u16 + 1;
            let max_title_width = (area.width as usize).saturating_sub(title_start as usize + 20);
            let title: String = pr.title.chars().take(max_title_width).collect();
            let title_display = if pr.title.chars().count() > max_title_width {
                format!("{}…", &title[..title.len().saturating_sub(1)])
            } else {
                title
            };
            buf.set_string(
                title_start,
                area.y,
                &title_display,
                Style::default().fg(Color::Rgb(220, 220, 230)).bg(header_bg),
            );

            // Repo name (right side)
            let repo_str = format!(" {} ", pr.repo_full_name());
            let repo_x = area.x + area.width - repo_str.len() as u16 - 1;
            buf.set_string(
                repo_x,
                area.y,
                &repo_str,
                Style::default().fg(Color::Rgb(100, 100, 120)).bg(header_bg),
            );

            // Second line: Status bar with position info
            for x in area.x..area.x + area.width {
                buf.set_string(x, area.y + 1, " ", Style::default().bg(status_bg));
            }

            // Mode indicator (prominent for visual mode)
            let mode_x = area.x + 1;
            if self.visual_mode {
                buf.set_string(
                    mode_x,
                    area.y + 1,
                    " VISUAL ",
                    Style::default()
                        .fg(Color::Rgb(25, 25, 35))
                        .bg(Color::Rgb(255, 180, 100))
                        .add_modifier(Modifier::BOLD),
                );
            } else {
                let focus_mode = if self.focus == Focus::Tree { "TREE" } else { "DIFF" };
                buf.set_string(
                    mode_x,
                    area.y + 1,
                    format!(" {} ", focus_mode),
                    Style::default()
                        .fg(Color::Rgb(180, 180, 200))
                        .bg(Color::Rgb(45, 45, 60)),
                );
            }

            // Line position indicator
            let line_info = if let Some(file) = self.files.get(self.selected_file) {
                let total_lines = file.line_count();
                let current_line = self.diff_cursor + 1;
                format!(" Ln {}/{} ", current_line.min(total_lines), total_lines)
            } else {
                " Ln --/-- ".to_string()
            };
            let line_x = area.x + 12;
            buf.set_string(
                line_x,
                area.y + 1,
                &line_info,
                Style::default().fg(Color::Rgb(140, 140, 160)).bg(status_bg),
            );

            // File progress indicator
            let file_info = format!(" File {}/{} ", self.selected_file + 1, self.files.len());
            let file_x = line_x + line_info.len() as u16 + 1;
            buf.set_string(
                file_x,
                area.y + 1,
                &file_info,
                Style::default().fg(Color::Rgb(140, 140, 160)).bg(status_bg),
            );

            // View mode indicator
            let view_mode = match self.view_mode {
                ViewMode::Unified => " unified ",
                ViewMode::Split => " split ",
            };
            let view_x = file_x + file_info.len() as u16 + 1;
            buf.set_string(
                view_x,
                area.y + 1,
                view_mode,
                Style::default().fg(Color::Rgb(100, 180, 140)).bg(status_bg),
            );

            // Pending comments badge (right side)
            if !self.pending_comments.is_empty() {
                let comment_badge = format!(" {} drafts ", self.pending_comments.len());
                let badge_x = area.x + area.width - comment_badge.len() as u16 - 1;
                buf.set_string(
                    badge_x,
                    area.y + 1,
                    &comment_badge,
                    Style::default()
                        .fg(Color::Rgb(30, 30, 40))
                        .bg(Color::Rgb(240, 200, 80))
                        .add_modifier(Modifier::BOLD),
                );
            }

            // Third line: Key hints
            for x in area.x..area.x + area.width {
                buf.set_string(x, area.y + 2, " ", Style::default().bg(Color::Rgb(22, 22, 28)));
            }

            let hints = if self.visual_mode {
                vec![("Esc", "cancel"), ("c", "comment"), ("j/k", "extend")]
            } else if self.pending_comments.is_empty() {
                vec![("q", "back"), ("c", "comment"), ("v", "visual"), ("/", "search"), ("?", "help")]
            } else {
                vec![("q", "back"), ("c", "comment"), ("C", "drafts"), ("S", "submit"), ("?", "help")]
            };

            let mut hint_x = area.x + 1;
            for (key, action) in hints {
                buf.set_string(
                    hint_x,
                    area.y + 2,
                    key,
                    Style::default().fg(Color::Rgb(130, 180, 220)).bg(Color::Rgb(22, 22, 28)),
                );
                hint_x += key.len() as u16;
                buf.set_string(
                    hint_x,
                    area.y + 2,
                    ":",
                    Style::default().fg(Color::Rgb(60, 60, 70)).bg(Color::Rgb(22, 22, 28)),
                );
                hint_x += 1;
                buf.set_string(
                    hint_x,
                    area.y + 2,
                    action,
                    Style::default().fg(Color::Rgb(100, 100, 120)).bg(Color::Rgb(22, 22, 28)),
                );
                hint_x += action.len() as u16 + 2;
            }

            // Render diff content in remaining area
            let tree_width = self.config.navigation.tree_width;
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(tree_width), Constraint::Min(0)])
                .split(content_area);

            self.render_tree(frame, chunks[0]);
            self.render_diff(frame, chunks[1]);

            // Render comment overlay if active
            self.render_comment_overlay(frame, area);
        } else {
            // Direct PR URL mode - minimal header with status bar
            let header_height = 1;
            let content_area = Rect {
                x: area.x,
                y: area.y + header_height,
                width: area.width,
                height: area.height.saturating_sub(header_height),
            };

            let buf = frame.buffer_mut();
            let status_bg = Color::Rgb(25, 25, 35);

            // Status bar
            for x in area.x..area.x + area.width {
                buf.set_string(x, area.y, " ", Style::default().bg(status_bg));
            }

            // Mode indicator
            let focus_mode = if self.focus == Focus::Tree { "TREE" } else { "DIFF" };
            buf.set_string(
                area.x + 1,
                area.y,
                format!(" {} ", focus_mode),
                Style::default()
                    .fg(Color::Rgb(180, 180, 200))
                    .bg(Color::Rgb(45, 45, 60)),
            );

            // Line and file info
            let line_info = if let Some(file) = self.files.get(self.selected_file) {
                let total_lines = file.line_count();
                format!(" Ln {}/{} ", (self.diff_cursor + 1).min(total_lines), total_lines)
            } else {
                " Ln --/-- ".to_string()
            };
            buf.set_string(
                area.x + 10,
                area.y,
                &line_info,
                Style::default().fg(Color::Rgb(140, 140, 160)).bg(status_bg),
            );

            let file_info = format!(" File {}/{} ", self.selected_file + 1, self.files.len());
            buf.set_string(
                area.x + 10 + line_info.len() as u16,
                area.y,
                &file_info,
                Style::default().fg(Color::Rgb(140, 140, 160)).bg(status_bg),
            );

            let tree_width = self.config.navigation.tree_width;
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(tree_width), Constraint::Min(0)])
                .split(content_area);

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
        let popup_area = Self::centered_popup(area, popup_width, 12);

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
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(40, 40, 50));
        frame.render_widget(block, popup_area);

        // Render the text with word wrapping and cursor
        let wrap_width = inner_area.width.saturating_sub(1) as usize;
        let mut wrapped_lines: Vec<String> = Vec::new();

        // Wrap each line of the input text
        for line in text.lines() {
            if line.is_empty() {
                wrapped_lines.push(String::new());
            } else {
                wrapped_lines.extend(helpers::wrap_text(line, wrap_width));
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
        let popup_width = (area.width * 3 / 4).min(100);
        let popup_height = (area.height * 2 / 3).min(20);
        let popup_area = Self::centered_popup(area, popup_width, popup_height);

        let title = format!(
            " Pending Comments ({}) - j/k:nav  e:edit  d:delete  S:submit  Esc:close ",
            self.pending_comments.len()
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let inner_area = block.inner(popup_area);
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(40, 40, 50));
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
        // Make popup wider to accommodate preview pane
        let popup_width = (area.width * 4 / 5).min(140);
        let popup_height = (area.height * 3 / 4).min(30);
        let popup_area = Self::centered_popup(area, popup_width, popup_height);

        let current_count = self.comment_threads.iter().filter(|t| !t.outdated).count();
        let outdated_count = self.comment_threads.iter().filter(|t| t.outdated).count();
        let title = if outdated_count > 0 {
            format!(
                " Comment Threads ({} current, {} outdated) - j/k:nav  Enter:view  r:reply  g:goto  q/Esc:close ",
                current_count, outdated_count
            )
        } else {
            format!(
                " Comment Threads ({}) - j/k:nav  Enter:view  r:reply  g:goto  q/Esc:close ",
                current_count
            )
        };
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta));

        let inner_area = block.inner(popup_area);
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(35, 35, 45));
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

        // Split into list (left, 45%) and preview (right, 55%)
        let list_width = (inner_area.width * 45 / 100).max(30);
        let preview_width = inner_area.width.saturating_sub(list_width + 1); // +1 for separator

        let list_area = Rect {
            x: inner_area.x,
            y: inner_area.y,
            width: list_width,
            height: inner_area.height,
        };

        let preview_area = Rect {
            x: inner_area.x + list_width + 1,
            y: inner_area.y,
            width: preview_width,
            height: inner_area.height,
        };

        let buf = frame.buffer_mut();

        // Draw separator line
        let sep_x = inner_area.x + list_width;
        for y in inner_area.y..inner_area.y + inner_area.height {
            buf.set_string(sep_x, y, "│", Style::default().fg(Color::DarkGray).bg(Color::Rgb(35, 35, 45)));
        }

        // Separate threads into current and outdated, preserving original indices
        let current_threads: Vec<(usize, &crate::types::CommentThread)> = self
            .comment_threads
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.outdated)
            .collect();
        let outdated_threads: Vec<(usize, &crate::types::CommentThread)> = self
            .comment_threads
            .iter()
            .enumerate()
            .filter(|(_, t)| t.outdated)
            .collect();

        // Build display list: current section header, current threads, outdated section header, outdated threads
        struct DisplayItem<'a> {
            kind: DisplayKind<'a>,
            visual_idx: Option<usize>, // Visual index for selection tracking
        }
        enum DisplayKind<'a> {
            SectionHeader(&'static str),
            Thread(&'a crate::types::CommentThread),
        }

        let mut display_items: Vec<DisplayItem> = Vec::new();
        let mut visual_idx = 0usize;

        // Add current section if there are current threads
        if !current_threads.is_empty() {
            display_items.push(DisplayItem {
                kind: DisplayKind::SectionHeader("── Current ──"),
                visual_idx: None,
            });
            for (_idx, thread) in &current_threads {
                display_items.push(DisplayItem {
                    kind: DisplayKind::Thread(thread),
                    visual_idx: Some(visual_idx),
                });
                visual_idx += 1;
            }
        }

        // Add outdated section if there are outdated threads
        if !outdated_threads.is_empty() {
            display_items.push(DisplayItem {
                kind: DisplayKind::SectionHeader("── Outdated ──"),
                visual_idx: None,
            });
            for (_idx, thread) in &outdated_threads {
                display_items.push(DisplayItem {
                    kind: DisplayKind::Thread(thread),
                    visual_idx: Some(visual_idx),
                });
                visual_idx += 1;
            }
        }

        // Render thread list (left pane)
        for (row, item) in display_items
            .iter()
            .skip(scroll)
            .take(list_area.height as usize)
            .enumerate()
        {
            let y = list_area.y + row as u16;

            match &item.kind {
                DisplayKind::SectionHeader(header) => {
                    let style = Style::default().fg(Color::DarkGray).bg(Color::Rgb(35, 35, 45));
                    // Clear line
                    for x in list_area.x..list_area.x + list_area.width {
                        buf.set_string(x, y, " ", style);
                    }
                    buf.set_string(list_area.x, y, header, style);
                }
                DisplayKind::Thread(thread) => {
                    let is_selected = item.visual_idx == Some(selected);

                    let style = if is_selected {
                        Style::default().fg(Color::White).bg(Color::Rgb(60, 60, 80))
                    } else {
                        Style::default().fg(Color::White).bg(Color::Rgb(35, 35, 45))
                    };

                    // Clear line in list area
                    for x in list_area.x..list_area.x + list_area.width {
                        buf.set_string(x, y, " ", style);
                    }

                    // Thread location - show just filename, not full path
                    let location = if let Some(path) = &thread.file_path {
                        let filename = std::path::Path::new(path)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(path);
                        if let Some(line) = thread.line {
                            format!("{}:{}", filename, line)
                        } else {
                            filename.to_string()
                        }
                    } else {
                        "[General]".to_string()
                    };

                    let comment_count = format!(" ({})", thread.comment_count());
                    let author = format!(" @{}", thread.author());

                    // Render location
                    buf.set_string(list_area.x + 1, y, &location, style.fg(Color::Cyan));

                    // Render count
                    let next_x = list_area.x + 1 + location.len() as u16;
                    if next_x < list_area.x + list_area.width {
                        buf.set_string(next_x, y, &comment_count, style.fg(Color::Yellow));
                    }

                    // Render author if space permits
                    let author_x = next_x + comment_count.len() as u16;
                    if author_x + 5 < list_area.x + list_area.width {
                        let available = (list_area.width as usize).saturating_sub((author_x - list_area.x) as usize);
                        let truncated_author: String = author.chars().take(available).collect();
                        buf.set_string(author_x, y, &truncated_author, style.fg(Color::Green));
                    }
                }
            }
        }

        // Render preview pane (right side) - show selected thread's content
        // Convert visual index to original thread index
        let thread_idx = self.visual_to_thread_index(selected);
        if let Some(thread) = thread_idx.and_then(|idx| self.comment_threads.get(idx)) {
            let preview_bg = Color::Rgb(30, 30, 40);

            // Clear preview area with slightly different background
            for y in preview_area.y..preview_area.y + preview_area.height {
                for x in preview_area.x..preview_area.x + preview_area.width {
                    buf.set_string(x, y, " ", Style::default().bg(preview_bg));
                }
            }

            // Preview header
            let header = format!(" Preview - {} comment(s) ", thread.comment_count());
            buf.set_string(
                preview_area.x,
                preview_area.y,
                &header,
                Style::default().fg(Color::Cyan).bg(preview_bg).add_modifier(Modifier::BOLD),
            );

            // Render first comment preview with word wrapping
            if let Some(first_comment) = thread.comments.first() {
                let content_start_y = preview_area.y + 2;
                let available_height = preview_area.height.saturating_sub(2) as usize;
                let wrap_width = (preview_area.width as usize).saturating_sub(2);

                // Author line
                let author_line = format!("@{}", first_comment.author);
                buf.set_string(
                    preview_area.x + 1,
                    preview_area.y + 1,
                    &author_line,
                    Style::default().fg(Color::Green).bg(preview_bg),
                );

                // Word-wrap the comment body
                let wrapped = helpers::wrap_text_with_code(&first_comment.body, wrap_width);

                for (line_idx, (line, is_code)) in wrapped.iter().take(available_height).enumerate() {
                    let y = content_start_y + line_idx as u16;
                    let style = if *is_code {
                        Style::default().fg(Color::Yellow).bg(preview_bg)
                    } else {
                        Style::default().fg(Color::White).bg(preview_bg)
                    };

                    buf.set_string(preview_area.x + 1, y, line, style);
                }

                // Show "more" indicator if there are more comments or lines
                let total_lines = wrapped.len();
                if total_lines > available_height || thread.comments.len() > 1 {
                    let more_msg = if thread.comments.len() > 1 {
                        format!("... +{} more comment(s)", thread.comments.len() - 1)
                    } else {
                        "... (more)".to_string()
                    };
                    let msg_y = preview_area.y + preview_area.height - 1;
                    buf.set_string(
                        preview_area.x + 1,
                        msg_y,
                        &more_msg,
                        Style::default().fg(Color::DarkGray).bg(preview_bg),
                    );
                }
            }
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
        let popup_area = Self::centered_popup(area, popup_width, popup_height);

        let location = if let Some(path) = &thread.file_path {
            let filename = std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(path);
            if let Some(line) = thread.line {
                format!("{}:{}", filename, line)
            } else {
                filename.to_string()
            }
        } else {
            "General Comment".to_string()
        };

        let title = format!(" {} - j/k:scroll  r:reply  q/Esc:back ", location);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let inner_area = block.inner(popup_area);
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(30, 30, 40));
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
            let time_ago = helpers::format_relative_time(&comment.created_at);
            let header = format!("@{} - {}", comment.author, time_ago);
            all_lines.push((header, header_style));

            // Comment body (word-wrapped, with code block detection)
            let wrapped = helpers::wrap_text_with_code(&comment.body, wrap_width);
            for (line, is_code) in wrapped {
                if is_code {
                    all_lines.push((format!("  │ {}", line), code_style));
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
        let popup_area = Self::centered_popup(area, popup_width, 10);

        let title = " Reply (Ctrl+S to send, Esc to cancel) ";
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green));

        let inner_area = block.inner(popup_area);
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(40, 50, 40));
        frame.render_widget(block, popup_area);

        // Render the text with word wrapping and cursor
        let wrap_width = inner_area.width.saturating_sub(1) as usize;
        let mut wrapped_lines: Vec<String> = Vec::new();

        for line in text.lines() {
            if line.is_empty() {
                wrapped_lines.push(String::new());
            } else {
                wrapped_lines.extend(helpers::wrap_text(line, wrap_width));
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

    #[allow(clippy::too_many_arguments)]
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
        let popup_area = Self::centered_popup(area, popup_width, popup_height);

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
        Self::clear_popup_background(frame.buffer_mut(), popup_area, Color::Rgb(35, 35, 50));
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
                    wrapped_lines.extend(helpers::wrap_text(line, wrap_width));
                }
            }
            if body.is_empty() || body.ends_with('\n') {
                wrapped_lines.push(String::new());
            }

            // Add cursor if editing
            if editing_body
                && let Some(last) = wrapped_lines.last_mut() {
                    last.push('_');
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

    fn render_tree(&self, frame: &mut ratatui::Frame, area: Rect) {
        let is_focused = self.focus == Focus::Tree || self.search_mode;
        let border_style = if is_focused {
            Style::default().fg(self.accent_color()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        // Calculate inner area for content
        let focus_indicator = if is_focused && !self.search_mode { "▶ " } else { "" };
        let title = if self.search_mode {
            format!(" /{}_ ", self.search_query)
        } else if !self.search_query.is_empty() {
            format!(
                " {}Files ({}/{}) [{}] ",
                focus_indicator,
                self.filtered_indices.len(),
                self.files.len(),
                self.search_query
            )
        } else {
            format!(" {}Files ({}) ", focus_indicator, self.files.len())
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

                    // Enhanced selection styling
                    let (style, row_bg) = if is_selected {
                        (
                            Style::default()
                                .bg(Color::Rgb(50, 60, 80))
                                .fg(Color::Rgb(240, 240, 250))
                                .add_modifier(Modifier::BOLD),
                            Color::Rgb(50, 60, 80)
                        )
                    } else {
                        (Style::default(), Color::Reset)
                    };

                    // Clear the line first
                    for cx in inner_area.x..inner_area.x + inner_area.width {
                        buf.set_string(cx, y, " ", Style::default().bg(row_bg));
                    }

                    let mut x = inner_area.x;
                    // Draw prefix
                    buf.set_string(x, y, &prefix, Style::default().fg(Color::Rgb(80, 80, 100)).bg(row_bg));
                    x += prefix.chars().count() as u16;

                    // Draw status badge with improved colors
                    let badge = format!("{} ", file.status.badge());
                    buf.set_string(x, y, &badge, Style::default().fg(file.status.color()).bg(row_bg));
                    x += badge.chars().count() as u16;

                    // Check for pending comments on this file
                    let pending_count = self.pending_comment_count_for_file(&file.path);
                    let thread_count = self.comment_threads
                        .iter()
                        .filter(|t| t.file_path.as_deref() == Some(&file.path))
                        .count();

                    // Calculate space for badges at end
                    let mut end_badges = String::new();
                    if pending_count > 0 {
                        end_badges.push_str(&format!(" +{}", pending_count));
                    }
                    if thread_count > 0 {
                        end_badges.push_str(&format!(" c{}", thread_count));
                    }
                    let badges_width = end_badges.chars().count();

                    // Draw file name (reserve space for badges)
                    let available_width = (inner_area.x + inner_area.width)
                        .saturating_sub(x)
                        .saturating_sub(badges_width as u16 + 1) as usize;
                    let truncated_name: String = name.chars().take(available_width).collect();
                    buf.set_string(x, y, &truncated_name, style.bg(row_bg));

                    // Draw badges at the end
                    if !end_badges.is_empty() {
                        let badge_x = inner_area.x + inner_area.width - badges_width as u16 - 1;
                        let mut bx = badge_x;
                        if pending_count > 0 {
                            let draft_badge = format!("+{}", pending_count);
                            buf.set_string(
                                bx,
                                y,
                                &draft_badge,
                                Style::default()
                                    .fg(Color::Rgb(240, 200, 80))
                                    .bg(row_bg)
                                    .add_modifier(Modifier::BOLD),
                            );
                            bx += draft_badge.len() as u16 + 1;
                        }
                        if thread_count > 0 {
                            let thread_badge = format!("c{}", thread_count);
                            buf.set_string(
                                bx,
                                y,
                                &thread_badge,
                                Style::default()
                                    .fg(Color::Rgb(140, 180, 220))
                                    .bg(row_bg),
                            );
                        }
                    }
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
        helpers::fill_area(buf, area, self.bg_color());

        let Some(file) = self.files.get(self.selected_file) else {
            let block = Block::default()
                .title(" No file selected ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            frame.render_widget(block, area);
            return;
        };

        let is_focused = self.focus == Focus::Diff;
        let border_style = if is_focused {
            Style::default().fg(self.accent_color()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let focus_indicator = if is_focused { "▶ " } else { "" };
        let title = if self.collapsed.contains(&self.selected_file) {
            format!(" {}{} [collapsed] ", focus_indicator, file.path)
        } else if self.view_mode == ViewMode::Split {
            format!(" {}{} [split] ", focus_indicator, file.path)
        } else {
            format!(" {}{} ", focus_indicator, file.path)
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
        helpers::fill_area(frame.buffer_mut(), inner_area, self.bg_color());

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
                        self.cursor_bg()
                    } else {
                        self.bg_color()
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
                            .bg(self.cursor_gutter())
                            .add_modifier(Modifier::BOLD)
                    } else if is_in_selection {
                        Style::default().fg(Color::Cyan).bg(Color::Rgb(60, 60, 90))
                    } else {
                        Style::default().fg(Color::DarkGray).bg(bg)
                    };
                    buf.set_string(area.x, y, cursor_marker, marker_style);

                    let text = helpers::truncate_or_pad(header, (area.width as usize).saturating_sub(1));
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
                        LineKind::Add => self.add_bg(),
                        LineKind::Del => self.del_bg(),
                        LineKind::Context => self.bg_color(),
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
                            _ => self.cursor_bg(),
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
                            .bg(self.cursor_gutter())
                            .add_modifier(Modifier::BOLD)
                    } else if is_in_selection {
                        Style::default().fg(Color::Cyan).bg(Color::Rgb(60, 60, 90))
                    } else {
                        Style::default().fg(Color::DarkGray).bg(bg)
                    };
                    buf.set_string(area.x, y, cursor_marker, marker_style);

                    let gutter_width = if self.config.display.show_line_numbers {
                        let gutter = format!("{} {} ", old_str, new_str);
                        // Render line numbers with special style for cursor line
                        let gutter_style = if is_cursor_line {
                            Style::default()
                                .fg(Color::White)
                                .bg(self.cursor_gutter())
                                .add_modifier(Modifier::BOLD)
                        } else if is_in_selection {
                            Style::default().fg(Color::White).bg(Color::Rgb(60, 60, 90))
                        } else {
                            Style::default().fg(Color::DarkGray).bg(bg)
                        };
                        buf.set_string(area.x + 1, y, &gutter, gutter_style);
                        gutter.len() + 1 // +1 for cursor marker
                    } else {
                        1 // Just the cursor marker
                    };

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

                    // Expand tabs based on config
                    let expanded_content = self.config.expand_tabs(content, &file.path);

                    // Apply horizontal scroll - skip first N characters
                    let h_scroll = self.horizontal_scroll;

                    // First render raw content as fallback (in case spans don't cover everything)
                    let default_style = Style::default().fg(Color::White).bg(bg);
                    let mut x_offset = content_start_x;
                    for (char_idx, ch) in expanded_content.chars().enumerate() {
                        if char_idx < h_scroll {
                            continue;  // Skip scrolled characters
                        }
                        if x_offset >= max_x {
                            break;
                        }
                        buf.set_string(x_offset, y, ch.to_string(), default_style);
                        x_offset += 1;
                    }

                    // Then overlay syntax highlighted spans (if enabled)
                    if self.config.display.syntax_highlighting {
                        let highlighted = self.highlighter.highlight_line(&expanded_content, &file.path);
                        x_offset = content_start_x;
                        let mut char_idx = 0usize;

                        for span in highlighted.spans {
                            let span_style = span.style.bg(bg);
                            for ch in span.content.chars() {
                                if char_idx < h_scroll {
                                    char_idx += 1;
                                    continue;  // Skip scrolled characters
                                }
                                if x_offset >= max_x {
                                    break;
                                }
                                buf.set_string(x_offset, y, ch.to_string(), span_style);
                                x_offset += 1;
                                char_idx += 1;
                            }
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
                    buf.set_string(cx, y, " ", Style::default().bg(self.bg_color()));
                }
            }
            Some(SplitLine::Hunk(header)) => {
                let text = helpers::truncate_or_pad(header, width as usize);
                buf.set_string(
                    x,
                    y,
                    &text,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::DIM)
                        .bg(self.bg_color()),
                );
            }
            Some(SplitLine::Del { ln, content }) => {
                self.render_split_content_line(buf, x, y, width, *ln, content, self.del_bg(), path);
            }
            Some(SplitLine::Add { ln, content }) => {
                self.render_split_content_line(buf, x, y, width, *ln, content, self.add_bg(), path);
            }
            Some(SplitLine::Context { ln, content }) => {
                self.render_split_content_line(buf, x, y, width, *ln, content, self.bg_color(), path);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
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

        let gutter_len = if self.config.display.show_line_numbers {
            let gutter = format!("{:>4} ", ln);
            buf.set_string(x, y, &gutter, Style::default().fg(Color::DarkGray).bg(bg));
            gutter.len() as u16
        } else {
            0
        };

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

        // Expand tabs based on config
        let expanded_content = self.config.expand_tabs(content, path);

        // Apply horizontal scroll - skip first N characters
        let h_scroll = self.horizontal_scroll;

        // First render raw content as fallback (in case spans don't cover everything)
        let default_style = Style::default().fg(Color::White).bg(bg);
        let mut x_offset = content_start_x;
        for (char_idx, ch) in expanded_content.chars().enumerate() {
            if char_idx < h_scroll {
                continue;  // Skip scrolled characters
            }
            if x_offset >= max_x {
                break;
            }
            buf.set_string(x_offset, y, ch.to_string(), default_style);
            x_offset += 1;
        }

        // Then overlay syntax highlighted spans (if enabled)
        if self.config.display.syntax_highlighting {
            let highlighted = self.highlighter.highlight_line(&expanded_content, path);
            x_offset = content_start_x;
            let mut char_idx = 0usize;

            for span in highlighted.spans {
                let span_style = span.style.bg(bg);
                for ch in span.content.chars() {
                    if char_idx < h_scroll {
                        char_idx += 1;
                        continue;  // Skip scrolled characters
                    }
                    if x_offset >= max_x {
                        break;
                    }
                    buf.set_string(x_offset, y, ch.to_string(), span_style);
                    x_offset += 1;
                    char_idx += 1;
                }
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

