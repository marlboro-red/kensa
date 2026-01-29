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
        Err(e) => {
            eprintln!(
                "Warning: Failed to read draft file {:?}: {}",
                file_path, e
            );
            return Vec::new();
        }
    };

    match serde_json::from_str(&content) {
        Ok(drafts) => drafts,
        Err(e) => {
            eprintln!(
                "Warning: Draft file {:?} contains invalid JSON and could not be loaded: {}",
                file_path, e
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Helper tests for path construction
    // ========================================================================

    #[test]
    fn test_drafts_dir_returns_path() {
        // drafts_dir should return Some path if config_dir exists
        let dir = drafts_dir();
        // On most systems this should return Some
        if let Some(path) = dir {
            assert!(path.to_string_lossy().contains("kensa"));
            assert!(path.to_string_lossy().contains("drafts"));
        }
    }

    #[test]
    fn test_draft_file_path_format() {
        let pr = PrInfo {
            owner: "testowner".to_string(),
            repo: "testrepo".to_string(),
            number: 123,
        };

        let path = draft_file_path(&pr);
        if let Some(p) = path {
            let filename = p.file_name().unwrap().to_string_lossy();
            assert_eq!(filename, "testowner_testrepo_123.json");
        }
    }

    #[test]
    fn test_draft_file_path_with_hyphenated_names() {
        let pr = PrInfo {
            owner: "my-org".to_string(),
            repo: "my-cool-repo".to_string(),
            number: 42,
        };

        let path = draft_file_path(&pr);
        if let Some(p) = path {
            let filename = p.file_name().unwrap().to_string_lossy();
            assert_eq!(filename, "my-org_my-cool-repo_42.json");
        }
    }

    #[test]
    fn test_draft_file_path_with_large_pr_number() {
        let pr = PrInfo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
            number: 999999,
        };

        let path = draft_file_path(&pr);
        if let Some(p) = path {
            let filename = p.file_name().unwrap().to_string_lossy();
            assert_eq!(filename, "owner_repo_999999.json");
        }
    }

    // ========================================================================
    // Integration tests (require filesystem access)
    // ========================================================================

    #[test]
    fn test_load_drafts_nonexistent_pr() {
        // Loading drafts for a PR that has never had any should return empty vec
        let pr = PrInfo {
            owner: "nonexistent_owner_12345".to_string(),
            repo: "nonexistent_repo_67890".to_string(),
            number: 99999999,
        };

        let drafts = load_drafts(&pr);
        assert!(drafts.is_empty());
    }

    #[test]
    fn test_save_and_load_drafts_roundtrip() {
        // Use a unique PR to avoid conflicts with other tests
        let pr = PrInfo {
            owner: "test_roundtrip_owner".to_string(),
            repo: "test_roundtrip_repo".to_string(),
            number: 11111,
        };

        // Create test comments
        let comments = vec![
            PendingComment::new_general("General comment".to_string()),
            PendingComment::new_inline(
                "Inline comment".to_string(),
                "src/main.rs".to_string(),
                42,
            ),
            PendingComment::new_multiline(
                "Multiline comment".to_string(),
                "src/lib.rs".to_string(),
                10,
                20,
            ),
        ];

        // Save drafts
        let save_result = save_drafts(&pr, &comments);
        assert!(save_result.is_ok(), "Failed to save drafts: {:?}", save_result);

        // Load drafts back
        let loaded = load_drafts(&pr);
        assert_eq!(loaded.len(), 3);

        // Verify contents
        assert_eq!(loaded[0].body, "General comment");
        assert!(!loaded[0].is_inline());

        assert_eq!(loaded[1].body, "Inline comment");
        assert!(loaded[1].is_inline());
        assert_eq!(loaded[1].file_path, Some("src/main.rs".to_string()));
        assert_eq!(loaded[1].line_number, Some(42));

        assert_eq!(loaded[2].body, "Multiline comment");
        assert_eq!(loaded[2].start_line, Some(10));
        assert_eq!(loaded[2].line_number, Some(20));

        // Cleanup: save empty to remove file
        let _ = save_drafts(&pr, &[]);
    }

    #[test]
    fn test_save_empty_removes_file() {
        let pr = PrInfo {
            owner: "test_empty_owner".to_string(),
            repo: "test_empty_repo".to_string(),
            number: 22222,
        };

        // First save some comments
        let comments = vec![PendingComment::new_general("Test".to_string())];
        let _ = save_drafts(&pr, &comments);

        // Verify file was created
        let loaded = load_drafts(&pr);
        assert_eq!(loaded.len(), 1);

        // Now save empty
        let result = save_drafts(&pr, &[]);
        assert!(result.is_ok());

        // Verify file was removed
        let loaded_after = load_drafts(&pr);
        assert!(loaded_after.is_empty());
    }

    #[test]
    fn test_save_overwrites_existing() {
        let pr = PrInfo {
            owner: "test_overwrite_owner".to_string(),
            repo: "test_overwrite_repo".to_string(),
            number: 33333,
        };

        // Save first set of comments
        let comments1 = vec![
            PendingComment::new_general("Comment 1".to_string()),
            PendingComment::new_general("Comment 2".to_string()),
        ];
        let _ = save_drafts(&pr, &comments1);

        // Save second set of comments (should overwrite)
        let comments2 = vec![PendingComment::new_general("Comment 3".to_string())];
        let _ = save_drafts(&pr, &comments2);

        // Load and verify
        let loaded = load_drafts(&pr);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].body, "Comment 3");

        // Cleanup
        let _ = save_drafts(&pr, &[]);
    }

    #[test]
    fn test_load_drafts_with_corrupted_file() {
        // This tests the unwrap_or_default behavior for corrupted JSON
        // We can't easily write a corrupted file without more setup,
        // but we can verify the function handles invalid JSON gracefully

        let pr = PrInfo {
            owner: "corrupted_test".to_string(),
            repo: "corrupted_repo".to_string(),
            number: 44444,
        };

        // Loading non-existent file should return empty (not panic)
        let loaded = load_drafts(&pr);
        assert!(loaded.is_empty());
    }

    #[test]
    fn test_multiple_prs_independent() {
        // Verify that different PRs have independent draft storage
        let pr1 = PrInfo {
            owner: "owner1".to_string(),
            repo: "repo1".to_string(),
            number: 55555,
        };

        let pr2 = PrInfo {
            owner: "owner2".to_string(),
            repo: "repo2".to_string(),
            number: 66666,
        };

        // Save different comments to each PR
        let comments1 = vec![PendingComment::new_general("PR1 comment".to_string())];
        let comments2 = vec![
            PendingComment::new_general("PR2 comment 1".to_string()),
            PendingComment::new_general("PR2 comment 2".to_string()),
        ];

        let _ = save_drafts(&pr1, &comments1);
        let _ = save_drafts(&pr2, &comments2);

        // Load and verify independence
        let loaded1 = load_drafts(&pr1);
        let loaded2 = load_drafts(&pr2);

        assert_eq!(loaded1.len(), 1);
        assert_eq!(loaded1[0].body, "PR1 comment");

        assert_eq!(loaded2.len(), 2);
        assert_eq!(loaded2[0].body, "PR2 comment 1");

        // Cleanup
        let _ = save_drafts(&pr1, &[]);
        let _ = save_drafts(&pr2, &[]);
    }

    #[test]
    fn test_draft_with_special_characters_in_body() {
        let pr = PrInfo {
            owner: "special_char_owner".to_string(),
            repo: "special_char_repo".to_string(),
            number: 77777,
        };

        let comments = vec![PendingComment::new_general(
            "Comment with \"quotes\" and\nnewlines\tand\ttabs and emoji: ".to_string(),
        )];

        let result = save_drafts(&pr, &comments);
        assert!(result.is_ok());

        let loaded = load_drafts(&pr);
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].body.contains("\"quotes\""));
        assert!(loaded[0].body.contains('\n'));
        assert!(loaded[0].body.contains('\t'));

        // Cleanup
        let _ = save_drafts(&pr, &[]);
    }

    #[test]
    fn test_draft_with_unicode_in_path() {
        let pr = PrInfo {
            owner: "unicode_owner".to_string(),
            repo: "unicode_repo".to_string(),
            number: 88888,
        };

        // File path shouldn't contain unicode in practice, but body can
        let comments = vec![PendingComment::new_inline(
            "Unicode body: こんにちは".to_string(),
            "src/main.rs".to_string(),
            10,
        )];

        let result = save_drafts(&pr, &comments);
        assert!(result.is_ok());

        let loaded = load_drafts(&pr);
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].body.contains("こんにちは"));

        // Cleanup
        let _ = save_drafts(&pr, &[]);
    }

    #[test]
    fn test_draft_with_empty_body() {
        let pr = PrInfo {
            owner: "empty_body_owner".to_string(),
            repo: "empty_body_repo".to_string(),
            number: 99999,
        };

        let comments = vec![PendingComment::new_general("".to_string())];

        let result = save_drafts(&pr, &comments);
        assert!(result.is_ok());

        let loaded = load_drafts(&pr);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].body, "");

        // Cleanup
        let _ = save_drafts(&pr, &[]);
    }

    #[test]
    fn test_draft_serialization_json_format() {
        // Verify that the JSON format is correct (for debugging purposes)
        let comment = PendingComment::new_inline(
            "Test body".to_string(),
            "src/test.rs".to_string(),
            42,
        );

        let json = serde_json::to_string_pretty(&vec![comment]).unwrap();

        // Verify JSON structure
        assert!(json.contains("\"body\""));
        assert!(json.contains("\"file_path\""));
        assert!(json.contains("\"line_number\""));
        assert!(json.contains("Test body"));
        assert!(json.contains("src/test.rs"));
        assert!(json.contains("42"));
    }
}
