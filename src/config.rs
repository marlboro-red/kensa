use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// RGB color representation for config
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Display settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    /// Show line numbers in diff view
    pub show_line_numbers: bool,

    /// Default view mode: "unified" or "split"
    pub default_view_mode: String,

    /// Enable syntax highlighting
    pub syntax_highlighting: bool,

    /// Minimum brightness for syntax colors (0-255)
    /// Higher values make colors more visible on dark backgrounds
    pub min_brightness: u8,

    /// Syntax highlighting theme name
    /// Available themes: base16-ocean.dark, base16-eighties.dark, base16-mocha.dark,
    /// base16-ocean.light, InspiredGitHub, Solarized (dark), Solarized (light)
    pub theme: String,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            show_line_numbers: true,
            default_view_mode: "unified".to_string(),
            syntax_highlighting: true,
            min_brightness: 180,
            theme: "base16-eighties.dark".to_string(),
        }
    }
}

/// Diff color settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffColors {
    /// Background color for added lines
    pub add_bg: RgbColor,

    /// Background color for deleted lines
    pub del_bg: RgbColor,

    /// Background color for context lines
    pub context_bg: RgbColor,

    /// Background color for the cursor line
    pub cursor_bg: RgbColor,

    /// Gutter color for cursor line
    pub cursor_gutter: RgbColor,

    /// Accent color used for highlights, PR numbers, active elements
    pub accent: RgbColor,
}

impl Default for DiffColors {
    fn default() -> Self {
        Self {
            add_bg: RgbColor::new(30, 60, 30),
            del_bg: RgbColor::new(60, 30, 30),
            context_bg: RgbColor::new(22, 22, 22),
            cursor_bg: RgbColor::new(45, 45, 65),
            cursor_gutter: RgbColor::new(100, 100, 180),
            accent: RgbColor::new(0, 255, 135), // Green (similar to Color::Green)
        }
    }
}

/// Navigation settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NavigationSettings {
    /// Number of lines to scroll with Page Up/Down
    pub scroll_lines: usize,

    /// Number of columns to scroll horizontally
    pub horizontal_scroll_columns: usize,

    /// Width of the file tree panel
    pub tree_width: u16,

    /// Collapse folders by default in the file tree
    pub collapse_folders_by_default: bool,
}

impl Default for NavigationSettings {
    fn default() -> Self {
        Self {
            scroll_lines: 15,
            horizontal_scroll_columns: 10,
            tree_width: 45,
            collapse_folders_by_default: false,
        }
    }
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default tab width used when no language-specific setting exists
    pub default_tab_width: usize,

    /// Language-specific indentation settings
    /// The key is the file extension (without dot), e.g., "rs", "py", "go"
    #[serde(default)]
    pub languages: HashMap<String, LanguageConfig>,

    /// Display settings
    #[serde(default)]
    pub display: DisplaySettings,

    /// Diff color settings
    #[serde(default)]
    pub colors: DiffColors,

    /// Navigation settings
    #[serde(default)]
    pub navigation: NavigationSettings,
}

/// Language-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LanguageConfig {
    /// Tab width for this language (how many spaces a tab should render as)
    pub tab_width: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_tab_width: 4,
            languages: HashMap::new(),
            display: DisplaySettings::default(),
            colors: DiffColors::default(),
            navigation: NavigationSettings::default(),
        }
    }
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self { tab_width: 4 }
    }
}

impl Config {
    /// Get the config file path (~/.config/kensa/config.toml)
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("kensa").join("config.toml"))
    }

    /// Initialize a new config file with default settings
    /// Returns Ok(path) on success, or an error message
    pub fn init(force: bool) -> Result<PathBuf, String> {
        let path = Self::config_path()
            .ok_or_else(|| "Could not determine config directory".to_string())?;

        // Check if config already exists
        if path.exists() && !force {
            return Err(format!(
                "Config file already exists at: {}\nUse --force to overwrite",
                path.display()
            ));
        }

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }

        // Write the default config with documentation
        let config_content = Self::default_config_content();
        fs::write(&path, config_content)
            .map_err(|e| format!("Failed to write config file: {}", e))?;

        Ok(path)
    }

    /// Generate the default config file content with documentation
    fn default_config_content() -> String {
        r#"# Kensa Configuration File
# Generated with: kensa --init-config
# All settings are optional - defaults will be used for any missing values.

# =============================================================================
# INDENTATION SETTINGS
# =============================================================================

# Default tab width used when no language-specific setting exists
# Set to 0 to preserve tabs as-is (no expansion)
default_tab_width = 4

# Language-specific tab width settings
# The key is the file extension (without the dot)
[languages.go]
tab_width = 8

[languages.py]
tab_width = 4

[languages.rs]
tab_width = 4

[languages.js]
tab_width = 2

[languages.ts]
tab_width = 2

[languages.yaml]
tab_width = 2

[languages.yml]
tab_width = 2

# Preserve tabs in Makefiles (tab_width = 0 disables expansion)
[languages.mk]
tab_width = 0

# =============================================================================
# DISPLAY SETTINGS
# =============================================================================

[display]
# Show line numbers in the diff view
show_line_numbers = true

# Default view mode: "unified" or "split"
# Press 'v' during review to toggle between modes
default_view_mode = "unified"

# Enable syntax highlighting for code
syntax_highlighting = true

# Minimum brightness for syntax colors (0-255)
# Higher values make colors more visible on dark backgrounds
min_brightness = 180

# Syntax highlighting theme
# Available themes:
#   - base16-ocean.dark
#   - base16-eighties.dark (default)
#   - base16-mocha.dark
#   - base16-ocean.light
#   - InspiredGitHub
#   - Solarized (dark)
#   - Solarized (light)
theme = "base16-eighties.dark"

# =============================================================================
# DIFF COLORS (RGB values: 0-255)
# =============================================================================

[colors]
# Background color for added lines (default: dark green)
add_bg = { r = 30, g = 60, b = 30 }

# Background color for deleted lines (default: dark red)
del_bg = { r = 60, g = 30, b = 30 }

# Background color for context lines (default: dark gray)
context_bg = { r = 22, g = 22, b = 22 }

# Background color for the cursor line (default: dark blue)
cursor_bg = { r = 45, g = 45, b = 65 }

# Gutter color for the cursor line (default: light blue)
cursor_gutter = { r = 100, g = 100, b = 180 }

# Accent color for highlights, PR numbers, focused borders (default: green)
accent = { r = 0, g = 255, b = 135 }

# =============================================================================
# NAVIGATION SETTINGS
# =============================================================================

[navigation]
# Number of lines to scroll with Page Up/Down (Ctrl+u/Ctrl+d)
scroll_lines = 15

# Number of columns to scroll horizontally with h/l
horizontal_scroll_columns = 10

# Width of the file tree panel in characters
tree_width = 45

# Start with all folders collapsed in the file tree
collapse_folders_by_default = false
"#.to_string()
    }

    /// Open the config file in the user's preferred editor
    /// Creates a default config if one doesn't exist
    pub fn edit() -> Result<(), String> {
        let path = Self::config_path()
            .ok_or_else(|| "Could not determine config directory".to_string())?;

        // Create config with defaults if it doesn't exist
        if !path.exists() {
            Self::init(false)?;
            eprintln!(
                "\x1b[33mCreated new config file at:\x1b[0m {}",
                path.display()
            );
        }

        // Get editor from environment, with platform-specific fallbacks
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| Self::default_editor().to_string());

        eprintln!("Opening {} with {}...", path.display(), editor);

        // Split editor command in case it contains arguments (e.g., "code --wait")
        let mut parts = editor.split_whitespace();
        let cmd = parts.next().ok_or("Empty editor command")?;
        let args: Vec<&str> = parts.collect();

        let status = Command::new(cmd)
            .args(&args)
            .arg(&path)
            .status()
            .map_err(|e| format!("Failed to open editor '{}': {}", editor, e))?;

        if !status.success() {
            return Err(format!("Editor exited with status: {}", status));
        }

        Ok(())
    }

    /// Get the default editor for the current platform
    fn default_editor() -> &'static str {
        if cfg!(windows) {
            "notepad"
        } else {
            // Try common editors, vi is most likely to exist
            "vi"
        }
    }

    /// Load configuration from file, or return default if not found
    pub fn load() -> Self {
        let path = match Self::config_path() {
            Some(p) => p,
            None => return Self::default(),
        };

        if !path.exists() {
            return Self::default();
        }

        match fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Get the tab width for a given file path based on its extension
    pub fn tab_width_for_file(&self, path: &str) -> usize {
        // Extract extension from path
        let ext = path.rsplit('.').next().unwrap_or("");

        self.languages
            .get(ext)
            .map(|lang| lang.tab_width)
            .unwrap_or(self.default_tab_width)
    }

    /// Expand tabs in content to spaces based on the configured tab width
    pub fn expand_tabs(&self, content: &str, path: &str) -> String {
        let tab_width = self.tab_width_for_file(path);
        if tab_width == 0 {
            // Tab width of 0 means don't expand tabs
            return content.to_string();
        }

        let mut result = String::with_capacity(content.len());
        let mut column = 0;

        for ch in content.chars() {
            if ch == '\t' {
                // Calculate spaces needed to reach next tab stop
                let spaces_to_add = tab_width - (column % tab_width);
                for _ in 0..spaces_to_add {
                    result.push(' ');
                }
                column += spaces_to_add;
            } else {
                result.push(ch);
                column += 1;
            }
        }

        result
    }

    /// Check if default view mode is split
    pub fn is_split_view_default(&self) -> bool {
        self.display.default_view_mode.to_lowercase() == "split"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_tab_width, 4);
        assert!(config.languages.is_empty());
    }

    #[test]
    fn test_tab_width_for_file_default() {
        let config = Config::default();
        assert_eq!(config.tab_width_for_file("test.rs"), 4);
        assert_eq!(config.tab_width_for_file("test.py"), 4);
    }

    #[test]
    fn test_tab_width_for_file_custom() {
        let mut config = Config::default();
        config.languages.insert(
            "go".to_string(),
            LanguageConfig { tab_width: 8 },
        );
        config.languages.insert(
            "py".to_string(),
            LanguageConfig { tab_width: 2 },
        );

        assert_eq!(config.tab_width_for_file("test.go"), 8);
        assert_eq!(config.tab_width_for_file("test.py"), 2);
        assert_eq!(config.tab_width_for_file("test.rs"), 4); // Uses default
    }

    #[test]
    fn test_expand_tabs_simple() {
        let config = Config::default();
        let result = config.expand_tabs("\thello", "test.rs");
        assert_eq!(result, "    hello");
    }

    #[test]
    fn test_expand_tabs_multiple() {
        let config = Config::default();
        let result = config.expand_tabs("\t\thello", "test.rs");
        assert_eq!(result, "        hello");
    }

    #[test]
    fn test_expand_tabs_mid_line() {
        let config = Config::default();
        // Tab after 2 chars should expand to 2 spaces (to reach column 4)
        let result = config.expand_tabs("ab\tc", "test.rs");
        assert_eq!(result, "ab  c");
    }

    #[test]
    fn test_expand_tabs_custom_width() {
        let mut config = Config::default();
        config.languages.insert(
            "go".to_string(),
            LanguageConfig { tab_width: 8 },
        );

        let result = config.expand_tabs("\thello", "test.go");
        assert_eq!(result, "        hello");
    }

    #[test]
    fn test_expand_tabs_no_tabs() {
        let config = Config::default();
        let result = config.expand_tabs("hello world", "test.rs");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_expand_tabs_empty() {
        let config = Config::default();
        let result = config.expand_tabs("", "test.rs");
        assert_eq!(result, "");
    }

    #[test]
    fn test_expand_tabs_zero_width() {
        let mut config = Config::default();
        config.languages.insert(
            "mk".to_string(),
            LanguageConfig { tab_width: 0 },
        );

        // Tab width of 0 should preserve tabs
        let result = config.expand_tabs("\thello", "Makefile.mk");
        assert_eq!(result, "\thello");
    }

    #[test]
    fn test_parse_toml() {
        let toml_str = r#"
default_tab_width = 4

[languages.go]
tab_width = 8

[languages.py]
tab_width = 2
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_tab_width, 4);
        assert_eq!(config.tab_width_for_file("test.go"), 8);
        assert_eq!(config.tab_width_for_file("test.py"), 2);
    }

    #[test]
    fn test_parse_toml_partial() {
        // Should use defaults for missing fields
        let toml_str = r#"
[languages.go]
tab_width = 8
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_tab_width, 4); // Default
        assert_eq!(config.tab_width_for_file("test.go"), 8);
    }

    #[test]
    fn test_config_path_extraction() {
        let config = Config::default();
        // Should handle nested paths
        assert_eq!(config.tab_width_for_file("src/deep/nested/file.rs"), 4);
    }

    #[test]
    fn test_config_no_extension() {
        let config = Config::default();
        // Files without extension should use default
        assert_eq!(config.tab_width_for_file("Makefile"), 4);
    }

    #[test]
    fn test_display_settings_defaults() {
        let config = Config::default();
        assert!(config.display.show_line_numbers);
        assert_eq!(config.display.default_view_mode, "unified");
        assert!(config.display.syntax_highlighting);
        assert_eq!(config.display.min_brightness, 180);
        assert_eq!(config.display.theme, "base16-eighties.dark");
    }

    #[test]
    fn test_diff_colors_defaults() {
        let config = Config::default();
        assert_eq!(config.colors.add_bg.r, 30);
        assert_eq!(config.colors.add_bg.g, 60);
        assert_eq!(config.colors.add_bg.b, 30);
        assert_eq!(config.colors.del_bg.r, 60);
        assert_eq!(config.colors.del_bg.g, 30);
        assert_eq!(config.colors.del_bg.b, 30);
    }

    #[test]
    fn test_navigation_settings_defaults() {
        let config = Config::default();
        assert_eq!(config.navigation.scroll_lines, 15);
        assert_eq!(config.navigation.horizontal_scroll_columns, 10);
        assert_eq!(config.navigation.tree_width, 45);
        assert!(!config.navigation.collapse_folders_by_default);
    }

    #[test]
    fn test_parse_toml_with_display_settings() {
        let toml_str = r#"
[display]
show_line_numbers = false
default_view_mode = "split"
syntax_highlighting = false
min_brightness = 200
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.display.show_line_numbers);
        assert_eq!(config.display.default_view_mode, "split");
        assert!(!config.display.syntax_highlighting);
        assert_eq!(config.display.min_brightness, 200);
    }

    #[test]
    fn test_parse_toml_with_colors() {
        let toml_str = r#"
[colors]
add_bg = { r = 0, g = 100, b = 0 }
del_bg = { r = 100, g = 0, b = 0 }
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.colors.add_bg.g, 100);
        assert_eq!(config.colors.del_bg.r, 100);
    }

    #[test]
    fn test_parse_toml_with_navigation() {
        let toml_str = r#"
[navigation]
scroll_lines = 20
horizontal_scroll_columns = 5
tree_width = 60
collapse_folders_by_default = true
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.navigation.scroll_lines, 20);
        assert_eq!(config.navigation.horizontal_scroll_columns, 5);
        assert_eq!(config.navigation.tree_width, 60);
        assert!(config.navigation.collapse_folders_by_default);
    }

    #[test]
    fn test_is_split_view_default() {
        let mut config = Config::default();
        assert!(!config.is_split_view_default());

        config.display.default_view_mode = "split".to_string();
        assert!(config.is_split_view_default());

        config.display.default_view_mode = "SPLIT".to_string();
        assert!(config.is_split_view_default());
    }

    #[test]
    fn test_rgb_color_new() {
        let color = RgbColor::new(255, 128, 64);
        assert_eq!(color.r, 255);
        assert_eq!(color.g, 128);
        assert_eq!(color.b, 64);
    }
}
