use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

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
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("kensa").join("config.toml"))
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
}
