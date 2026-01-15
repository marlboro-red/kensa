use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::ReviewPr;

/// Cached PR lists
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PrCache {
    pub review_prs: Vec<ReviewPr>,
    pub my_prs: Vec<ReviewPr>,
    /// Unix timestamp when cache was saved
    #[serde(default)]
    pub cached_at: u64,
}

impl PrCache {
    /// Returns how many seconds ago the cache was saved
    pub fn age_seconds(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(self.cached_at)
    }

    /// Returns a human-readable age string like "2m ago" or "1h ago"
    pub fn age_display(&self) -> String {
        let secs = self.age_seconds();
        if secs < 60 {
            format!("{}s ago", secs)
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else if secs < 86400 {
            format!("{}h ago", secs / 3600)
        } else {
            format!("{}d ago", secs / 86400)
        }
    }
}

/// Get the cache file path (~/.config/kensa/cache.json)
fn cache_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("kensa").join("cache.json"))
}

/// Save PR lists to cache
pub fn save_cache(review_prs: &[ReviewPr], my_prs: &[ReviewPr]) {
    let Some(file_path) = cache_file_path() else {
        return;
    };

    // Create parent directory if needed
    if let Some(parent) = file_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let cached_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let cache = PrCache {
        review_prs: review_prs.to_vec(),
        my_prs: my_prs.to_vec(),
        cached_at,
    };

    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = fs::write(&file_path, json);
    }
}

/// Load PR lists from cache (returns None if no cache or error)
pub fn load_cache() -> Option<PrCache> {
    let file_path = cache_file_path()?;

    if !file_path.exists() {
        return None;
    }

    let content = fs::read_to_string(&file_path).ok()?;
    serde_json::from_str(&content).ok()
}
