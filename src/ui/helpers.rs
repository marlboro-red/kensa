//! Utility helper functions for the UI module.

use std::io::{self, Stdout};

use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::Terminal;

/// Fill an entire area with a background color
pub fn fill_area(buf: &mut Buffer, area: Rect, color: Color) {
    let style = Style::default().bg(color);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            buf.set_string(x, y, " ", style);
        }
    }
}

/// Truncate or pad a string to exactly the given width
pub fn truncate_or_pad(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() >= width {
        chars[..width].iter().collect()
    } else {
        let mut result: String = chars.into_iter().collect();
        result.push_str(&" ".repeat(width - result.len()));
        result
    }
}

/// Set up the terminal for TUI mode
pub fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore terminal to normal mode
pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

/// Format time as relative (e.g., "2h ago", "3d ago")
pub fn format_relative_time(iso_time: &str) -> String {
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
pub fn wrap_text_with_code(text: &str, width: usize) -> Vec<(String, bool)> {
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
            // Expand tabs first, then truncate if needed
            let expanded = line.replace('\t', "    ");
            let chars: Vec<char> = expanded.chars().collect();
            if chars.len() > width {
                result.push((chars[..width].iter().collect(), true));
            } else {
                result.push((expanded, true));
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
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    wrap_text_with_code(text, width)
        .into_iter()
        .map(|(line, _)| line)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_text_with_code_preserves_whitespace() {
        let text = "Here is some code:\n```\n    indented line\n\tline with tab\n  two spaces\n```\nAfter code";
        let result = wrap_text_with_code(text, 80);

        // Find the code lines
        let code_lines: Vec<_> = result.iter().filter(|(_, is_code)| *is_code).collect();

        assert_eq!(code_lines.len(), 3, "Should have 3 code lines");
        assert_eq!(
            code_lines[0].0, "    indented line",
            "Should preserve 4-space indent"
        );
        assert_eq!(
            code_lines[1].0, "    line with tab",
            "Tabs should be expanded to 4 spaces"
        );
        assert_eq!(
            code_lines[2].0, "  two spaces",
            "Should preserve 2-space indent"
        );
    }

    #[test]
    fn test_wrap_text_with_code_detects_blocks() {
        let text = "Normal text\n```rust\nfn main() {}\n```\nMore text";
        let result = wrap_text_with_code(text, 80);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0], ("Normal text".to_string(), false));
        assert_eq!(result[1], ("fn main() {}".to_string(), true));
        assert_eq!(result[2], ("More text".to_string(), false));
    }

    #[test]
    fn test_wrap_text_with_code_empty_lines_in_code() {
        let text = "```\nline1\n\nline2\n```";
        let result = wrap_text_with_code(text, 80);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0], ("line1".to_string(), true));
        assert_eq!(result[1], (String::new(), true)); // empty line in code block
        assert_eq!(result[2], ("line2".to_string(), true));
    }

    #[test]
    fn test_truncate_or_pad_truncate() {
        assert_eq!(truncate_or_pad("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_or_pad_pad() {
        assert_eq!(truncate_or_pad("hi", 5), "hi   ");
    }

    #[test]
    fn test_truncate_or_pad_exact() {
        assert_eq!(truncate_or_pad("hello", 5), "hello");
    }
}
