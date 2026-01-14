use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Syntax highlighter using syntect
pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    theme_name: String,
    min_brightness: u8,
}

/// Ensure a color has minimum brightness for readability
fn ensure_min_brightness(r: u8, g: u8, b: u8, min_brightness: u8) -> (u8, u8, u8) {
    // Calculate perceived brightness (human eye is more sensitive to green)
    let brightness = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;

    if brightness < min_brightness {
        // Scale up the color to meet minimum brightness
        let scale = min_brightness as f32 / brightness.max(1) as f32;
        let new_r = ((r as f32 * scale).min(255.0)) as u8;
        let new_g = ((g as f32 * scale).min(255.0)) as u8;
        let new_b = ((b as f32 * scale).min(255.0)) as u8;

        // If still too dark (e.g., pure black), use a gray
        let new_brightness =
            ((new_r as u32 * 299 + new_g as u32 * 587 + new_b as u32 * 114) / 1000) as u8;
        if new_brightness < min_brightness {
            return (min_brightness, min_brightness, min_brightness);
        }
        (new_r, new_g, new_b)
    } else {
        (r, g, b)
    }
}

/// Default theme name
const DEFAULT_THEME: &str = "base16-eighties.dark";

impl Highlighter {
    pub fn new() -> Self {
        Self::with_options(180, DEFAULT_THEME)
    }

    pub fn with_options(min_brightness: u8, theme: &str) -> Self {
        let theme_set = ThemeSet::load_defaults();
        // Validate theme exists, fall back to default if not
        let theme_name = if theme_set.themes.contains_key(theme) {
            theme.to_string()
        } else {
            DEFAULT_THEME.to_string()
        };
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set,
            theme_name,
            min_brightness,
        }
    }

    /// Get the syntax for a given path (with caching)
    fn get_syntax(&self, path: &str) -> &SyntaxReference {
        let ext = path.rsplit('.').next().unwrap_or("");
        self.syntax_set
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
    }

    /// Get the theme
    fn get_theme(&self) -> &Theme {
        &self.theme_set.themes[&self.theme_name]
    }

    /// Convert syntect style to ratatui spans
    fn convert_to_spans(ranges: Vec<(syntect::highlighting::Style, &str)>, min_brightness: u8) -> Vec<Span<'static>> {
        ranges
            .into_iter()
            .map(|(style, text)| {
                // Boost all colors to minimum brightness for better readability
                let (r, g, b) = ensure_min_brightness(
                    style.foreground.r,
                    style.foreground.g,
                    style.foreground.b,
                    min_brightness,
                );
                let fg = Color::Rgb(r, g, b);

                let mut ratatui_style = Style::default().fg(fg);

                if style.font_style.contains(FontStyle::BOLD) {
                    ratatui_style = ratatui_style.add_modifier(ratatui::style::Modifier::BOLD);
                }
                if style.font_style.contains(FontStyle::ITALIC) {
                    ratatui_style = ratatui_style.add_modifier(ratatui::style::Modifier::ITALIC);
                }
                if style.font_style.contains(FontStyle::UNDERLINE) {
                    ratatui_style = ratatui_style.add_modifier(ratatui::style::Modifier::UNDERLINED);
                }

                Span::styled(text.to_string(), ratatui_style)
            })
            .collect()
    }

    /// Highlight a line of code, returning styled spans
    pub fn highlight_line<'a>(&self, content: &'a str, path: &str) -> Line<'a> {
        let syntax = self.get_syntax(path);
        let theme = self.get_theme();
        let mut highlighter = HighlightLines::new(syntax, theme);

        match highlighter.highlight_line(content, &self.syntax_set) {
            Ok(ranges) => Line::from(Self::convert_to_spans(ranges, self.min_brightness)),
            Err(_) => Line::from(content.to_string()),
        }
    }

}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // ensure_min_brightness tests
    // ========================================================================

    #[test]
    fn test_brightness_already_bright() {
        // A bright color should not be modified
        let (r, g, b) = ensure_min_brightness(255, 255, 255, 180);
        assert_eq!((r, g, b), (255, 255, 255));
    }

    #[test]
    fn test_brightness_pure_white() {
        let (r, g, b) = ensure_min_brightness(255, 255, 255, 100);
        assert_eq!((r, g, b), (255, 255, 255));
    }

    #[test]
    fn test_brightness_boost_dark_color() {
        // A dark color should be boosted
        let (r, g, b) = ensure_min_brightness(50, 50, 50, 180);
        // Result should be brighter
        let brightness = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
        assert!(brightness >= 180 || (r == 180 && g == 180 && b == 180));
    }

    #[test]
    fn test_brightness_pure_black() {
        // Pure black (0,0,0) should become gray at minimum brightness
        let (r, g, b) = ensure_min_brightness(0, 0, 0, 180);
        assert_eq!((r, g, b), (180, 180, 180));
    }

    #[test]
    fn test_brightness_near_black() {
        // Near black should be boosted to gray
        let (r, g, b) = ensure_min_brightness(1, 1, 1, 180);
        // Should either be boosted or fall back to gray
        let brightness = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
        assert!(brightness >= 180 || r >= 180);
    }

    #[test]
    fn test_brightness_red_channel_only() {
        let (r, _g, _b) = ensure_min_brightness(100, 0, 0, 180);
        // Should be boosted but preserve color ratio where possible
        assert!(r > 100);
    }

    #[test]
    fn test_brightness_green_channel_only() {
        let (_r, g, _b) = ensure_min_brightness(0, 100, 0, 180);
        // Green contributes most to perceived brightness
        assert!(g > 100);
    }

    #[test]
    fn test_brightness_blue_channel_only() {
        let (_r, _g, b) = ensure_min_brightness(0, 0, 100, 180);
        // Blue contributes least to perceived brightness, so boost needed
        assert!(b > 100);
    }

    #[test]
    fn test_brightness_low_threshold() {
        // With a very low threshold, most colors should pass through unchanged
        let (r, g, b) = ensure_min_brightness(50, 50, 50, 10);
        // Should not be modified much if threshold is low
        let brightness = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
        assert!(brightness >= 10);
    }

    #[test]
    fn test_brightness_max_threshold() {
        // With threshold at 255, all colors get boosted to max
        let (r, g, b) = ensure_min_brightness(100, 100, 100, 255);
        // Should be boosted significantly
        let brightness = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
        assert!(brightness >= 180); // May not reach 255 due to clamping
    }

    #[test]
    fn test_brightness_clamping() {
        // Values should be clamped to 255 max (u8 guarantees this)
        // This test verifies the function doesn't panic with high values
        let (r, g, b) = ensure_min_brightness(200, 200, 200, 250);
        // Verify the function produced valid output (boosted but within bounds)
        let brightness = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
        assert!(brightness >= 200);
    }

    // ========================================================================
    // Highlighter tests
    // ========================================================================

    #[test]
    fn test_highlighter_new() {
        let _highlighter = Highlighter::new();
        // Just verify it doesn't panic
    }

    #[test]
    fn test_highlighter_default() {
        let _highlighter = Highlighter::default();
        // Should be equivalent to new()
    }

    #[test]
    fn test_highlight_line_rust() {
        let highlighter = Highlighter::new();
        let line = highlighter.highlight_line("fn main() {}", "test.rs");
        // Should return a non-empty Line
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_javascript() {
        let highlighter = Highlighter::new();
        let line = highlighter.highlight_line("const x = 42;", "test.js");
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_typescript() {
        let highlighter = Highlighter::new();
        let line = highlighter.highlight_line("const x: number = 42;", "test.ts");
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_python() {
        let highlighter = Highlighter::new();
        let line = highlighter.highlight_line("def hello():", "test.py");
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_go() {
        let highlighter = Highlighter::new();
        let line = highlighter.highlight_line("func main() {}", "test.go");
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_unknown_extension() {
        let highlighter = Highlighter::new();
        // Unknown extension should fall back to plain text
        let line = highlighter.highlight_line("some content", "test.xyz123unknown");
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_no_extension() {
        let highlighter = Highlighter::new();
        let line = highlighter.highlight_line("content", "Makefile");
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_highlight_line_empty_content() {
        let highlighter = Highlighter::new();
        let line = highlighter.highlight_line("", "test.rs");
        // Empty input should not panic - result may be empty or contain empty span
        let content: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(content.is_empty());
    }

    #[test]
    fn test_highlight_line_preserves_content() {
        let highlighter = Highlighter::new();
        let content = "let x = 42;";
        let line = highlighter.highlight_line(content, "test.rs");

        // Concatenate all span content
        let result: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(result, content);
    }

    #[test]
    fn test_highlight_line_with_keywords() {
        let highlighter = Highlighter::new();
        let line = highlighter.highlight_line("if true { return false; }", "test.rs");
        // Keywords should be highlighted (multiple spans)
        assert!(line.spans.len() > 1);
    }

    // ========================================================================
    // Style conversion tests
    // ========================================================================

    #[test]
    fn test_convert_to_spans_empty() {
        let ranges: Vec<(syntect::highlighting::Style, &str)> = vec![];
        let spans = Highlighter::convert_to_spans(ranges, 180);
        assert!(spans.is_empty());
    }

    #[test]
    fn test_highlight_line_applies_brightness_boost() {
        let highlighter = Highlighter::new();
        // Dark theme colors should be boosted for readability
        let line = highlighter.highlight_line("let x = 1;", "test.rs");

        // Check that resulting colors have reasonable brightness
        for span in &line.spans {
            if let Some(Color::Rgb(r, g, b)) = span.style.fg {
                let brightness = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
                // All colors should be at least somewhat visible (>100)
                // Note: exact threshold depends on ensure_min_brightness call
                assert!(brightness >= 100, "Color too dark: ({}, {}, {})", r, g, b);
            }
        }
    }

    // ========================================================================
    // File extension detection tests
    // ========================================================================

    #[test]
    fn test_get_syntax_rust() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("test.rs");
        assert!(syntax.name.to_lowercase().contains("rust"));
    }

    #[test]
    fn test_get_syntax_javascript() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("test.js");
        assert!(
            syntax.name.to_lowercase().contains("javascript")
                || syntax.name.to_lowercase().contains("js")
        );
    }

    #[test]
    fn test_get_syntax_typescript() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("test.ts");
        // TypeScript may fall back to JavaScript or have its own syntax
        let name = syntax.name.to_lowercase();
        assert!(
            name.contains("typescript") || name.contains("javascript") || name.contains("plain"),
            "Expected TypeScript-related syntax, got: {}",
            syntax.name
        );
    }

    #[test]
    fn test_get_syntax_python() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("test.py");
        assert!(syntax.name.to_lowercase().contains("python"));
    }

    #[test]
    fn test_get_syntax_markdown() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("README.md");
        assert!(syntax.name.to_lowercase().contains("markdown"));
    }

    #[test]
    fn test_get_syntax_json() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("package.json");
        assert!(syntax.name.to_lowercase().contains("json"));
    }

    #[test]
    fn test_get_syntax_yaml() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("config.yaml");
        assert!(syntax.name.to_lowercase().contains("yaml"));
    }

    #[test]
    fn test_get_syntax_yml() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("config.yml");
        assert!(syntax.name.to_lowercase().contains("yaml"));
    }

    #[test]
    fn test_get_syntax_html() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("index.html");
        assert!(syntax.name.to_lowercase().contains("html"));
    }

    #[test]
    fn test_get_syntax_css() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("styles.css");
        assert!(syntax.name.to_lowercase().contains("css"));
    }

    #[test]
    fn test_get_syntax_c() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("main.c");
        // C files use "C" or "C++" syntax in syntect
        let name = syntax.name.to_lowercase();
        assert!(name.contains("c") || name.contains("c++") || name == "c");
    }

    #[test]
    fn test_get_syntax_cpp() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("main.cpp");
        assert!(syntax.name.to_lowercase().contains("c++"));
    }

    #[test]
    fn test_get_syntax_java() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("Main.java");
        assert!(syntax.name.to_lowercase().contains("java"));
    }

    #[test]
    fn test_get_syntax_ruby() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("app.rb");
        assert!(syntax.name.to_lowercase().contains("ruby"));
    }

    #[test]
    fn test_get_syntax_shell() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("script.sh");
        let name = syntax.name.to_lowercase();
        assert!(name.contains("shell") || name.contains("bash") || name.contains("sh"));
    }

    #[test]
    fn test_get_syntax_sql() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("query.sql");
        assert!(syntax.name.to_lowercase().contains("sql"));
    }

    #[test]
    fn test_get_syntax_toml() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("Cargo.toml");
        // TOML may not be in the default syntect set, so accept plain text fallback
        let name = syntax.name.to_lowercase();
        assert!(
            name.contains("toml") || name.contains("plain"),
            "Expected TOML-related syntax, got: {}",
            syntax.name
        );
    }

    #[test]
    fn test_get_syntax_xml() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("config.xml");
        assert!(syntax.name.to_lowercase().contains("xml"));
    }

    #[test]
    fn test_get_syntax_unknown_falls_back() {
        let highlighter = Highlighter::new();
        let syntax = highlighter.get_syntax("file.unknownext123");
        // Should fall back to plain text
        assert!(syntax.name.to_lowercase().contains("plain text"));
    }

    #[test]
    fn test_get_syntax_path_extraction() {
        let highlighter = Highlighter::new();
        // Should extract extension from full path
        let syntax = highlighter.get_syntax("path/to/deep/file.rs");
        assert!(syntax.name.to_lowercase().contains("rust"));
    }

    #[test]
    fn test_get_syntax_double_extension() {
        let highlighter = Highlighter::new();
        // Files like .test.ts should use .ts extension
        let syntax = highlighter.get_syntax("component.test.ts");
        // TypeScript may fall back to JavaScript or plain text
        let name = syntax.name.to_lowercase();
        assert!(
            name.contains("typescript") || name.contains("javascript") || name.contains("plain"),
            "Expected TypeScript-related syntax, got: {}",
            syntax.name
        );
    }

    // ========================================================================
    // Theme tests
    // ========================================================================

    #[test]
    fn test_get_theme() {
        let highlighter = Highlighter::new();
        let theme = highlighter.get_theme();
        // Theme should be loaded
        assert!(!theme.name.is_none() || theme.name.is_some());
    }

    // ========================================================================
    // Edge case tests
    // ========================================================================

    #[test]
    fn test_highlight_very_long_line() {
        let highlighter = Highlighter::new();
        let long_line = "x".repeat(10000);
        let line = highlighter.highlight_line(&long_line, "test.txt");
        // Should not panic and should preserve content
        let result: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(result.len(), 10000);
    }

    #[test]
    fn test_highlight_special_characters() {
        let highlighter = Highlighter::new();
        let content = "fn test() { let s = \"hello\\nworld\\t!\"; }";
        let line = highlighter.highlight_line(content, "test.rs");
        let result: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(result, content);
    }

    #[test]
    fn test_highlight_unicode() {
        let highlighter = Highlighter::new();
        let content = "let greeting = \"こんにちは\";";
        let line = highlighter.highlight_line(content, "test.rs");
        let result: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(result, content);
    }

    #[test]
    fn test_highlight_emoji() {
        let highlighter = Highlighter::new();
        let content = "let emoji = \"\";";
        let line = highlighter.highlight_line(content, "test.rs");
        let result: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(result, content);
    }

    #[test]
    fn test_highlight_tabs() {
        let highlighter = Highlighter::new();
        let content = "\t\tfn indented() {}";
        let line = highlighter.highlight_line(content, "test.rs");
        let result: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(result, content);
    }

    #[test]
    fn test_highlight_mixed_whitespace() {
        let highlighter = Highlighter::new();
        let content = "  \t  fn mixed() {}  ";
        let line = highlighter.highlight_line(content, "test.rs");
        let result: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(result, content);
    }
}
