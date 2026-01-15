use std::fs;
use std::path::PathBuf;

use crate::types::ReviewPr;

/// Cached PR lists
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PrCache {
    pub review_prs: Vec<ReviewPr>,
    pub my_prs: Vec<ReviewPr>,
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

    let cache = PrCache {
        review_prs: review_prs.to_vec(),
        my_prs: my_prs.to_vec(),
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
