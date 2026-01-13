use std::fs;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// GitHub repository for kensa (owner/repo format)
const GITHUB_REPO: &str = "marlboro-red/kensa";

/// How often to check for updates (24 hours)
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Current version from Cargo.toml
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
struct UpdateCache {
    last_check: u64,
    latest_version: Option<String>,
}

/// Get the update cache file path
fn cache_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("kensa").join("update_cache.json"))
}

/// Load the cached update info
fn load_cache() -> Option<UpdateCache> {
    let path = cache_path()?;
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save update info to cache
fn save_cache(cache: &UpdateCache) {
    if let Some(path) = cache_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, serde_json::to_string(cache).unwrap_or_default());
    }
}

/// Get current timestamp in seconds
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse version string, stripping 'v' prefix if present
fn parse_version(v: &str) -> &str {
    v.strip_prefix('v').unwrap_or(v)
}

/// Compare two semver versions, returns true if `latest` is newer than `current`
fn is_newer_version(current: &str, latest: &str) -> bool {
    let parse = |v: &str| -> Option<(u32, u32, u32)> {
        let v = parse_version(v);
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() >= 3 {
            Some((
                parts[0].parse().ok()?,
                parts[1].parse().ok()?,
                parts[2].parse().ok()?,
            ))
        } else {
            None
        }
    };

    match (parse(current), parse(latest)) {
        (Some((c_maj, c_min, c_patch)), Some((l_maj, l_min, l_patch))) => {
            (l_maj, l_min, l_patch) > (c_maj, c_min, c_patch)
        }
        _ => false,
    }
}

/// Fetch the latest release version from GitHub using gh CLI
async fn fetch_latest_version() -> Option<String> {
    let output = Command::new("gh")
        .args([
            "api",
            &format!("repos/{}/releases/latest", GITHUB_REPO),
            "--jq",
            ".tag_name",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !version.is_empty() {
            return Some(version);
        }
    }

    None
}

/// Check for updates and return a message if an update is available
/// Uses caching to avoid checking too frequently unless `force` is true
pub async fn check_for_update(force: bool) -> Option<String> {
    let now = now_secs();

    // Check cache first (unless forcing)
    if !force {
        if let Some(cache) = load_cache() {
            if now - cache.last_check < CHECK_INTERVAL.as_secs() {
                // Use cached result
                if let Some(ref latest) = cache.latest_version {
                    if is_newer_version(VERSION, latest) {
                        return Some(format_update_message(latest));
                    }
                }
                return None;
            }
        }
    }

    // Fetch latest version from GitHub
    let latest = fetch_latest_version().await;

    // Save to cache
    save_cache(&UpdateCache {
        last_check: now,
        latest_version: latest.clone(),
    });

    // Check if update is available
    if let Some(ref latest_version) = latest {
        if is_newer_version(VERSION, latest_version) {
            return Some(format_update_message(latest_version));
        }
    }

    None
}

fn format_update_message(latest: &str) -> String {
    format!(
        "Update available: {} -> {} (https://github.com/{})",
        VERSION,
        parse_version(latest),
        GITHUB_REPO
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_version_basic() {
        assert!(is_newer_version("0.1.0", "0.2.0"));
        assert!(is_newer_version("0.1.0", "0.1.1"));
        assert!(is_newer_version("0.1.0", "1.0.0"));
        assert!(is_newer_version("1.2.3", "1.2.4"));
    }

    #[test]
    fn test_is_newer_version_same() {
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("1.0.0", "1.0.0"));
    }

    #[test]
    fn test_is_newer_version_older() {
        assert!(!is_newer_version("0.2.0", "0.1.0"));
        assert!(!is_newer_version("1.0.0", "0.9.9"));
    }

    #[test]
    fn test_is_newer_version_with_v_prefix() {
        assert!(is_newer_version("0.1.0", "v0.2.0"));
        assert!(is_newer_version("v0.1.0", "0.2.0"));
        assert!(is_newer_version("v0.1.0", "v0.2.0"));
    }

    #[test]
    fn test_parse_version_strips_v() {
        assert_eq!(parse_version("v1.2.3"), "1.2.3");
        assert_eq!(parse_version("1.2.3"), "1.2.3");
    }
}
