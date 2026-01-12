# kensa

A fast TUI for reviewing GitHub PRs.

## Requirements

- [GitHub CLI](https://cli.github.com/) (`gh`) installed and authenticated

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# List PRs awaiting your review
kensa

# View a specific PR
kensa https://github.com/owner/repo/pull/123
```

## Key Bindings

### PR List

| Key | Action |
|-----|--------|
| `j/k` | Navigate |
| `Enter` | Open PR diff |
| `Tab` | Switch between "For Review" / "My PRs" |
| `r` | Refresh list |
| `q` | Quit |

### Diff View

| Key | Action |
|-----|--------|
| `j/k` | Scroll diff |
| `h/l` | Previous/next file |
| `Tab` | Toggle file tree |
| `/` | Search files |
| `c` | Comment on current line |
| `v` | Visual mode (select lines) |
| `p` | View pending comments |
| `S` | Submit all comments |
| `o` | Open PR in browser |
| `?` | Help |
| `q` | Back to PR list |

### Comments

| Key | Action |
|-----|--------|
| `Ctrl+S` | Save comment |
| `Esc` | Cancel |
| `e` | Edit selected draft |
| `d` | Delete selected draft |

## Features

- Syntax highlighting
- Inline and multi-line comments
- Comment drafts (persisted to `~/.config/kensa/drafts/`)
- Batch comment submission (single API call)
- File tree navigation
- Vim-style keybindings

## License

MIT
