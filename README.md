# kensa

A fast TUI for reviewing GitHub PRs.

## Requirements

- [GitHub CLI](https://cli.github.com/) (`gh`) installed and authenticated

## Installation

```bash
cargo install --git https://github.com/marlboro-red/kensa
```

Or build from source:

```bash
git clone https://github.com/marlboro-red/kensa
cd kensa
cargo install --path .
```

## Usage

```bash
# List PRs awaiting your review
kensa

# View a specific PR
kensa https://github.com/owner/repo/pull/123

# List PRs by a specific GitHub user
kensa --user <username>
kensa -u <username>

# Generate default config file
kensa --init-config

# Open config in your editor
kensa --edit-config
kensa -e

# Check for version upgrades
kensa --upgrade
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
| `i` | View PR description |
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

- PR description viewer with markdown support
- Syntax highlighting
- Inline and multi-line comments
- Comment drafts (persisted to `~/.config/kensa/drafts/`)
- Batch comment submission (single API call)
- File tree navigation
- Vim-style keybindings

## Configuration

kensa can be configured via a TOML file at:
- **Linux/macOS:** `~/.config/kensa/config.toml`
- **Windows:** `%APPDATA%\Roaming\kensa\config.toml`

### Quick Setup

```bash
# Generate a default config file with all options documented
kensa --init-config

# Open config in your editor ($EDITOR or $VISUAL)
kensa --edit-config
kensa -e
```

See [`config.toml.example`](config.toml.example) for a complete example with all options.

### Display Settings

```toml
[display]
show_line_numbers = true              # Show line numbers in diff view
default_view_mode = "unified"         # "unified" or "split"
syntax_highlighting = true            # Enable syntax highlighting
min_brightness = 180                  # Minimum color brightness (0-255)
theme = "base16-eighties.dark"        # Syntax highlighting theme
```

Available themes: `base16-ocean.dark`, `base16-eighties.dark`, `base16-mocha.dark`, `base16-ocean.light`, `InspiredGitHub`, `Solarized (dark)`, `Solarized (light)`

### Diff Colors

Customize diff colors using RGB values (0-255):

```toml
[colors]
add_bg = { r = 30, g = 60, b = 30 }       # Added lines background
del_bg = { r = 60, g = 30, b = 30 }       # Deleted lines background
context_bg = { r = 22, g = 22, b = 22 }   # Context lines background
cursor_bg = { r = 45, g = 45, b = 65 }    # Cursor line background
cursor_gutter = { r = 100, g = 100, b = 180 } # Cursor gutter color
accent = { r = 0, g = 200, b = 200 }      # Accent color (PR numbers, focused borders)
```

### Navigation Settings

```toml
[navigation]
scroll_lines = 15               # Lines to scroll with Ctrl+u/d
horizontal_scroll_columns = 10  # Columns to scroll with h/l
tree_width = 45                 # File tree panel width
collapse_folders_by_default = false # Start with folders collapsed
confirm_quit = true             # Show confirmation dialog on quit
```

### Tab/Indentation Settings

```toml
default_tab_width = 4  # Default tab width

# Language-specific tab widths (by file extension)
[languages.go]
tab_width = 8

[languages.py]
tab_width = 4

[languages.js]
tab_width = 2
```

## License

MIT
