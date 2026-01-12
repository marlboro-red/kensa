use std::fs;
use std::path::PathBuf;

use crate::types::{PendingComment, PrInfo};

/// Get the drafts directory path (~/.config/kensa/drafts/)
fn drafts_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("kensa").join("drafts"))
}

/// Get the draft file path for a specific PR
fn draft_file_path(pr: &PrInfo) -> Option<PathBuf> {
    drafts_dir().map(|dir| dir.join(format!("{}_{}_{}.json", pr.owner, pr.repo, pr.number)))
}

/// Save drafts for a PR to disk
pub fn save_drafts(pr: &PrInfo, comments: &[PendingComment]) -> Result<(), String> {
    let dir = drafts_dir().ok_or("Could not determine config directory")?;
    let file_path = draft_file_path(pr).ok_or("Could not determine draft file path")?;

    // Create directory if it doesn't exist
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create drafts directory: {}", e))?;

    if comments.is_empty() {
        // Remove the file if no drafts
        if file_path.exists() {
            fs::remove_file(&file_path).map_err(|e| format!("Failed to remove draft file: {}", e))?;
        }
    } else {
        // Save drafts to file
        let json = serde_json::to_string_pretty(comments)
            .map_err(|e| format!("Failed to serialize drafts: {}", e))?;
        fs::write(&file_path, json).map_err(|e| format!("Failed to write draft file: {}", e))?;
    }

    Ok(())
}

/// Load drafts for a PR from disk
pub fn load_drafts(pr: &PrInfo) -> Vec<PendingComment> {
    let file_path = match draft_file_path(pr) {
        Some(p) => p,
        None => return Vec::new(),
    };

    if !file_path.exists() {
        return Vec::new();
    }

    let content = match fs::read_to_string(&file_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    serde_json::from_str(&content).unwrap_or_default()
}
