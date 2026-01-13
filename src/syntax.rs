use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Syntax highlighter using syntect
pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
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

impl Highlighter {
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
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
        &self.theme_set.themes["base16-eighties.dark"]
    }

    /// Convert syntect style to ratatui spans
    fn convert_to_spans(ranges: Vec<(syntect::highlighting::Style, &str)>) -> Vec<Span<'static>> {
        ranges
            .into_iter()
            .map(|(style, text)| {
                // Boost all colors to minimum brightness 180 for better readability
                let (r, g, b) = ensure_min_brightness(
                    style.foreground.r,
                    style.foreground.g,
                    style.foreground.b,
                    180,
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
            Ok(ranges) => Line::from(Self::convert_to_spans(ranges)),
            Err(_) => Line::from(content.to_string()),
        }
    }

}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}
