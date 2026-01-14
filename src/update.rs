use std::process::Stdio;

use tokio::process::Command;

/// GitHub repository for kensa (owner/repo format)
const GITHUB_REPO: &str = "marlboro-red/kensa";

/// Current version from Cargo.toml
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
            // Handle pre-release versions like "1.0.0-beta" by taking only the numeric part
            let patch_str = parts[2].split('-').next().unwrap_or(parts[2]);
            Some((
                parts[0].parse().ok()?,
                parts[1].parse().ok()?,
                patch_str.parse().ok()?,
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
pub async fn check_for_update() -> Option<String> {
    let latest = fetch_latest_version().await?;

    if is_newer_version(VERSION, &latest) {
        Some(format_update_message(&latest))
    } else {
        None
    }
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

    #[test]
    fn test_is_newer_version_prerelease() {
        // Pre-release versions should parse correctly (ignoring pre-release suffix)
        assert!(is_newer_version("1.0.0", "1.0.1-beta"));
        assert!(is_newer_version("1.0.0-alpha", "1.0.1"));
        assert!(!is_newer_version("1.0.1", "1.0.0-beta"));
    }
}
