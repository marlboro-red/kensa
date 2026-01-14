# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

kensa is a fast TUI (Terminal User Interface) for reviewing GitHub PRs, built in Rust. It requires the GitHub CLI (`gh`) to be installed and authenticated.

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build (with LTO, codegen-units=1, strip)
cargo run                # Run debug build
cargo install --path .   # Install locally
```

## Testing

```bash
cargo test               # Run all tests
cargo test <test_name>   # Run a specific test
cargo test --lib         # Run only library tests
```

## Linting and Formatting

```bash
cargo clippy             # Run linter
cargo fmt                # Format code
cargo fmt -- --check     # Check formatting without changing files
```

## Architecture

### Module Structure

- **main.rs** - Entry point, CLI parsing (clap), startup modes (PR list, direct URL, user PRs)
- **ui/mod.rs** - TUI application state and rendering (ratatui/crossterm). Contains the `App` struct which manages:
  - Screen state (PR list vs diff view)
  - PR list tabs (For Review / My PRs)
  - Diff viewer with file tree, syntax highlighting, and cursor navigation
  - Comment drafting system (inline, multi-line, general)
  - Visual mode for selecting line ranges
- **github.rs** - GitHub API interactions via `gh` CLI subprocess calls. Handles PR fetching, diff retrieval, and comment submission
- **parser.rs** - Unified diff parser that extracts files, hunks, and diff lines with line numbers
- **types.rs** - Core data types: `DiffFile`, `DiffLine`, `Hunk`, `ReviewPr`, `PendingComment`, `CommentThread`
- **syntax.rs** - Syntax highlighting using syntect
- **config.rs** - TOML configuration loading from `~/.config/kensa/config.toml`
- **drafts.rs** - Draft comment persistence to `~/.config/kensa/drafts/`
- **update.rs** - Version checking against GitHub releases

### Key Patterns

- Async operations use tokio for subprocess execution (`gh` CLI calls)
- All GitHub API calls go through the `gh` CLI, not direct HTTP
- The UI uses a single-threaded event loop with mpsc channels for async diff loading
- Comment drafts are persisted per-PR to allow resuming reviews
- Configuration uses serde for TOML serialization with sensible defaults

### State Machine

The app has two main screens:
1. **PrList** - Shows tabs for "For Review" and "My PRs" with search/filter
2. **DiffView** - File tree + diff viewer with comment modes (editing, viewing pending, viewing threads, submitting review)
