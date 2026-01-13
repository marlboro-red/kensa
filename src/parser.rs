use regex::Regex;

use crate::types::{DiffFile, DiffLine, FileStatus, Hunk, LineKind};

/// Parse a unified diff string into structured DiffFile objects
pub fn parse_diff(diff: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let lines: Vec<&str> = diff.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        // Look for diff --git header
        if lines[i].starts_with("diff --git ") {
            if let Some((file, consumed)) = parse_file(&lines[i..]) {
                files.push(file);
                i += consumed;
                continue;
            }
        }
        i += 1;
    }

    files
}

fn parse_file(lines: &[&str]) -> Option<(DiffFile, usize)> {
    if lines.is_empty() || !lines[0].starts_with("diff --git ") {
        return None;
    }

    let mut i = 0;
    let mut path = String::new();
    let mut status = FileStatus::Modified;
    let mut hunks = Vec::new();

    // Parse diff --git line to get path
    // Format: diff --git a/path/to/file b/path/to/file
    let git_line = lines[i];
    if let Some(b_idx) = git_line.find(" b/") {
        path = git_line[b_idx + 3..].to_string();
    }
    i += 1;

    // Parse metadata lines
    while i < lines.len() {
        let line = lines[i];

        if line.starts_with("new file mode") {
            status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            status = FileStatus::Deleted;
        } else if line.starts_with("rename from ") {
            status = FileStatus::Renamed;
        } else if line.starts_with("similarity index ") {
            status = FileStatus::Renamed;
        } else if line.starts_with("--- ") {
            // Start of actual diff content, skip this line
            i += 1;
            // Skip +++ line too
            if i < lines.len() && lines[i].starts_with("+++ ") {
                i += 1;
            }
            break;
        } else if line.starts_with("diff --git ") {
            // Next file started
            break;
        } else if line.starts_with("Binary files") {
            // Binary file, skip
            i += 1;
            break;
        }
        i += 1;
    }

    // Parse hunks
    while i < lines.len() {
        let line = lines[i];

        if line.starts_with("diff --git ") {
            // Next file
            break;
        }

        if line.starts_with("@@ ") {
            if let Some((hunk, consumed)) = parse_hunk(&lines[i..]) {
                hunks.push(hunk);
                i += consumed;
                continue;
            }
        }

        i += 1;
    }

    Some((
        DiffFile {
            path,
            status,
            hunks,
        },
        i,
    ))
}

fn parse_hunk(lines: &[&str]) -> Option<(Hunk, usize)> {
    if lines.is_empty() || !lines[0].starts_with("@@ ") {
        return None;
    }

    let header = lines[0].to_string();

    // Parse @@ -old_start,old_count +new_start,new_count @@ optional context
    let hunk_re = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap();
    let caps = hunk_re.captures(&header)?;

    let old_start: u32 = caps.get(1)?.as_str().parse().ok()?;
    let _old_count: u32 = caps.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1));
    let new_start: u32 = caps.get(3)?.as_str().parse().ok()?;
    let _new_count: u32 = caps.get(4).map_or(1, |m| m.as_str().parse().unwrap_or(1));

    let mut diff_lines = Vec::new();
    let mut i = 1;
    let mut old_ln = old_start;
    let mut new_ln = new_start;

    while i < lines.len() {
        let line = lines[i];

        // Stop at next hunk or next file
        if line.starts_with("@@ ") || line.starts_with("diff --git ") {
            break;
        }

        let (kind, old, new) = if line.starts_with('+') {
            let ln = new_ln;
            new_ln += 1;
            (LineKind::Add, None, Some(ln))
        } else if line.starts_with('-') {
            let ln = old_ln;
            old_ln += 1;
            (LineKind::Del, Some(ln), None)
        } else if line.starts_with(' ') || line.is_empty() {
            let o = old_ln;
            let n = new_ln;
            old_ln += 1;
            new_ln += 1;
            (LineKind::Context, Some(o), Some(n))
        } else if line.starts_with('\\') {
            // "\ No newline at end of file"
            i += 1;
            continue;
        } else {
            // Unknown line, might be end of hunk
            break;
        };

        // Strip the leading +/- / space
        let content = if line.is_empty() {
            String::new()
        } else {
            line[1..].to_string()
        };

        diff_lines.push(DiffLine {
            kind,
            content,
            old_ln: old,
            new_ln: new,
        });

        i += 1;
    }

    Some((
        Hunk {
            header,
            lines: diff_lines,
        },
        i,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_diff() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
     println!("World");
 }
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[0].hunks[0].lines.len(), 4);
    }

    #[test]
    fn test_parse_new_file() {
        let diff = r#"diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+new content
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, FileStatus::Added);
    }

    #[test]
    fn test_parse_renamed_file() {
        let diff = r#"diff --git a/old.txt b/new.txt
similarity index 100%
rename from old.txt
rename to new.txt
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, FileStatus::Renamed);
    }

    // ========================================================================
    // Additional comprehensive tests
    // ========================================================================

    #[test]
    fn test_parse_empty_diff() {
        let diff = "";
        let files = parse_diff(diff);
        assert!(files.is_empty());
    }

    #[test]
    fn test_parse_diff_with_only_whitespace() {
        let diff = "   \n\n   \n";
        let files = parse_diff(diff);
        assert!(files.is_empty());
    }

    #[test]
    fn test_parse_deleted_file() {
        let diff = r#"diff --git a/deleted.txt b/deleted.txt
deleted file mode 100644
index 1234567..0000000
--- a/deleted.txt
+++ /dev/null
@@ -1,3 +0,0 @@
-line 1
-line 2
-line 3
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "deleted.txt");
        assert_eq!(files[0].status, FileStatus::Deleted);
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[0].hunks[0].lines.len(), 3);
        for line in &files[0].hunks[0].lines {
            assert_eq!(line.kind, LineKind::Del);
        }
    }

    #[test]
    fn test_parse_multiple_files() {
        let diff = r#"diff --git a/file1.txt b/file1.txt
index 1234567..abcdefg 100644
--- a/file1.txt
+++ b/file1.txt
@@ -1,2 +1,3 @@
 line 1
+added line
 line 2
diff --git a/file2.txt b/file2.txt
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/file2.txt
@@ -0,0 +1,2 @@
+new file line 1
+new file line 2
diff --git a/file3.txt b/file3.txt
deleted file mode 100644
index 1234567..0000000
--- a/file3.txt
+++ /dev/null
@@ -1 +0,0 @@
-removed content
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 3);

        assert_eq!(files[0].path, "file1.txt");
        assert_eq!(files[0].status, FileStatus::Modified);

        assert_eq!(files[1].path, "file2.txt");
        assert_eq!(files[1].status, FileStatus::Added);

        assert_eq!(files[2].path, "file3.txt");
        assert_eq!(files[2].status, FileStatus::Deleted);
    }

    #[test]
    fn test_parse_multiple_hunks() {
        let diff = r#"diff --git a/multi_hunk.rs b/multi_hunk.rs
index 1234567..abcdefg 100644
--- a/multi_hunk.rs
+++ b/multi_hunk.rs
@@ -1,3 +1,4 @@
 fn first() {
+    // added in first hunk
 }

@@ -10,3 +11,4 @@
 fn second() {
+    // added in second hunk
 }

@@ -20,3 +22,4 @@
 fn third() {
+    // added in third hunk
 }
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks.len(), 3);

        // Check hunk starts
        assert_eq!(files[0].hunks[0].old_start, 1);
        assert_eq!(files[0].hunks[0].new_start, 1);

        assert_eq!(files[0].hunks[1].old_start, 10);
        assert_eq!(files[0].hunks[1].new_start, 11);

        assert_eq!(files[0].hunks[2].old_start, 20);
        assert_eq!(files[0].hunks[2].new_start, 22);
    }

    #[test]
    fn test_parse_hunk_line_numbers() {
        let diff = r#"diff --git a/numbered.rs b/numbered.rs
index 1234567..abcdefg 100644
--- a/numbered.rs
+++ b/numbered.rs
@@ -5,6 +5,7 @@
 context line 1
 context line 2
+added line
 context line 3
-removed line
 context line 4
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);

        let lines = &files[0].hunks[0].lines;
        assert_eq!(lines.len(), 6);

        // Context line 1: old=5, new=5
        assert_eq!(lines[0].kind, LineKind::Context);
        assert_eq!(lines[0].old_ln, Some(5));
        assert_eq!(lines[0].new_ln, Some(5));

        // Context line 2: old=6, new=6
        assert_eq!(lines[1].kind, LineKind::Context);
        assert_eq!(lines[1].old_ln, Some(6));
        assert_eq!(lines[1].new_ln, Some(6));

        // Added line: old=None, new=7
        assert_eq!(lines[2].kind, LineKind::Add);
        assert_eq!(lines[2].old_ln, None);
        assert_eq!(lines[2].new_ln, Some(7));

        // Context line 3: old=7, new=8
        assert_eq!(lines[3].kind, LineKind::Context);
        assert_eq!(lines[3].old_ln, Some(7));
        assert_eq!(lines[3].new_ln, Some(8));

        // Removed line: old=8, new=None
        assert_eq!(lines[4].kind, LineKind::Del);
        assert_eq!(lines[4].old_ln, Some(8));
        assert_eq!(lines[4].new_ln, None);

        // Context line 4: old=9, new=9
        assert_eq!(lines[5].kind, LineKind::Context);
        assert_eq!(lines[5].old_ln, Some(9));
        assert_eq!(lines[5].new_ln, Some(9));
    }

    #[test]
    fn test_parse_hunk_header_single_line() {
        // When a hunk changes only one line, the count may be omitted
        let diff = r#"diff --git a/single.txt b/single.txt
index 1234567..abcdefg 100644
--- a/single.txt
+++ b/single.txt
@@ -1 +1 @@
-old content
+new content
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks.len(), 1);

        let hunk = &files[0].hunks[0];
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 1);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_count, 1);
    }

    #[test]
    fn test_parse_hunk_with_context_text() {
        // Hunk headers can include optional context (function name)
        let diff = r#"diff --git a/func.rs b/func.rs
index 1234567..abcdefg 100644
--- a/func.rs
+++ b/func.rs
@@ -10,3 +10,4 @@ fn my_function() {
     let x = 1;
+    let y = 2;
     return x;
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks.len(), 1);

        let hunk = &files[0].hunks[0];
        assert!(hunk.header.contains("fn my_function()"));
        assert_eq!(hunk.old_start, 10);
        assert_eq!(hunk.new_start, 10);
    }

    #[test]
    fn test_parse_binary_file() {
        let diff = r#"diff --git a/image.png b/image.png
new file mode 100644
index 0000000..1234567
Binary files /dev/null and b/image.png differ
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "image.png");
        assert_eq!(files[0].status, FileStatus::Added);
        assert!(files[0].hunks.is_empty()); // Binary files have no hunks
    }

    #[test]
    fn test_parse_no_newline_at_end() {
        let diff = r#"diff --git a/no_newline.txt b/no_newline.txt
index 1234567..abcdefg 100644
--- a/no_newline.txt
+++ b/no_newline.txt
@@ -1,2 +1,2 @@
 line 1
-old line 2
\ No newline at end of file
+new line 2
\ No newline at end of file
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);

        let lines = &files[0].hunks[0].lines;
        // Should have context, del, add - the "\ No newline" marker should be skipped
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].kind, LineKind::Context);
        assert_eq!(lines[1].kind, LineKind::Del);
        assert_eq!(lines[2].kind, LineKind::Add);
    }

    #[test]
    fn test_parse_renamed_with_modifications() {
        let diff = r#"diff --git a/old_name.txt b/new_name.txt
similarity index 85%
rename from old_name.txt
rename to new_name.txt
index 1234567..abcdefg 100644
--- a/old_name.txt
+++ b/new_name.txt
@@ -1,3 +1,4 @@
 unchanged line
+added during rename
 another unchanged
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new_name.txt");
        assert_eq!(files[0].old_path, Some("old_name.txt".to_string()));
        assert_eq!(files[0].status, FileStatus::Renamed);
        assert_eq!(files[0].hunks.len(), 1);
    }

    #[test]
    fn test_parse_path_with_spaces() {
        let diff = r#"diff --git a/path with spaces/file.txt b/path with spaces/file.txt
index 1234567..abcdefg 100644
--- a/path with spaces/file.txt
+++ b/path with spaces/file.txt
@@ -1 +1,2 @@
 original
+added
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "path with spaces/file.txt");
    }

    #[test]
    fn test_parse_deeply_nested_path() {
        let diff = r#"diff --git a/src/components/ui/buttons/primary/PrimaryButton.tsx b/src/components/ui/buttons/primary/PrimaryButton.tsx
index 1234567..abcdefg 100644
--- a/src/components/ui/buttons/primary/PrimaryButton.tsx
+++ b/src/components/ui/buttons/primary/PrimaryButton.tsx
@@ -1,2 +1,3 @@
 export const PrimaryButton = () => {
+  // comment
 };
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path,
            "src/components/ui/buttons/primary/PrimaryButton.tsx"
        );
    }

    #[test]
    fn test_parse_empty_content_lines() {
        // Lines that are just context but empty (just a space)
        let diff = r#"diff --git a/empty_lines.txt b/empty_lines.txt
index 1234567..abcdefg 100644
--- a/empty_lines.txt
+++ b/empty_lines.txt
@@ -1,5 +1,6 @@
 line 1

+added line

 line 4
"#;
        let files = parse_diff(diff);
        assert_eq!(files.len(), 1);

        let lines = &files[0].hunks[0].lines;
        // line 1, empty context, added, empty context, line 4
        assert_eq!(lines.len(), 5);

        // The empty context lines should be detected correctly
        assert_eq!(lines[1].kind, LineKind::Context);
        assert_eq!(lines[1].content, "");
    }

    #[test]
    fn test_parse_diff_line_content_stripped() {
        let diff = r#"diff --git a/strip.txt b/strip.txt
index 1234567..abcdefg 100644
--- a/strip.txt
+++ b/strip.txt
@@ -1,2 +1,2 @@
-old line with content
+new line with content
"#;
        let files = parse_diff(diff);
        let lines = &files[0].hunks[0].lines;

        // The leading +/- should be stripped from content
        assert_eq!(lines[0].content, "old line with content");
        assert_eq!(lines[1].content, "new line with content");
    }

    #[test]
    fn test_parse_hunk_counts() {
        let diff = r#"diff --git a/counts.txt b/counts.txt
index 1234567..abcdefg 100644
--- a/counts.txt
+++ b/counts.txt
@@ -1,10 +1,15 @@
 context
"#;
        let files = parse_diff(diff);
        let hunk = &files[0].hunks[0];

        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 10);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_count, 15);
    }

    #[test]
    fn test_parse_file_returns_none_for_empty() {
        let lines: Vec<&str> = vec![];
        assert!(parse_file(&lines).is_none());
    }

    #[test]
    fn test_parse_file_returns_none_for_non_diff() {
        let lines = vec!["not a diff line", "another line"];
        assert!(parse_file(&lines).is_none());
    }

    #[test]
    fn test_parse_hunk_returns_none_for_empty() {
        let lines: Vec<&str> = vec![];
        assert!(parse_hunk(&lines).is_none());
    }

    #[test]
    fn test_parse_hunk_returns_none_for_non_hunk() {
        let lines = vec!["not a hunk header", "some content"];
        assert!(parse_hunk(&lines).is_none());
    }

    #[test]
    fn test_parse_hunk_invalid_header_format() {
        // Missing valid line numbers
        let lines = vec!["@@ invalid @@ context"];
        assert!(parse_hunk(&lines).is_none());
    }

    #[test]
    fn test_parse_large_line_numbers() {
        let diff = r#"diff --git a/large.txt b/large.txt
index 1234567..abcdefg 100644
--- a/large.txt
+++ b/large.txt
@@ -99999,3 +100000,4 @@
 context at large line
+added at large line
 more context
"#;
        let files = parse_diff(diff);
        let hunk = &files[0].hunks[0];

        assert_eq!(hunk.old_start, 99999);
        assert_eq!(hunk.new_start, 100000);

        let lines = &hunk.lines;
        assert_eq!(lines[0].old_ln, Some(99999));
        assert_eq!(lines[0].new_ln, Some(100000));
    }

    #[test]
    fn test_parse_consecutive_adds_and_deletes() {
        let diff = r#"diff --git a/consec.txt b/consec.txt
index 1234567..abcdefg 100644
--- a/consec.txt
+++ b/consec.txt
@@ -1,5 +1,5 @@
-del1
-del2
-del3
+add1
+add2
+add3
"#;
        let files = parse_diff(diff);
        let lines = &files[0].hunks[0].lines;

        assert_eq!(lines.len(), 6);

        // First three are deletions
        assert_eq!(lines[0].kind, LineKind::Del);
        assert_eq!(lines[0].old_ln, Some(1));
        assert_eq!(lines[1].kind, LineKind::Del);
        assert_eq!(lines[1].old_ln, Some(2));
        assert_eq!(lines[2].kind, LineKind::Del);
        assert_eq!(lines[2].old_ln, Some(3));

        // Next three are additions
        assert_eq!(lines[3].kind, LineKind::Add);
        assert_eq!(lines[3].new_ln, Some(1));
        assert_eq!(lines[4].kind, LineKind::Add);
        assert_eq!(lines[4].new_ln, Some(2));
        assert_eq!(lines[5].kind, LineKind::Add);
        assert_eq!(lines[5].new_ln, Some(3));
    }

    #[test]
    fn test_parse_mixed_changes() {
        let diff = r#"diff --git a/mixed.txt b/mixed.txt
index 1234567..abcdefg 100644
--- a/mixed.txt
+++ b/mixed.txt
@@ -1,7 +1,8 @@
 line1
-removed
+added1
+added2
 line4
-removed2
 line6
+added3
 line7
"#;
        let files = parse_diff(diff);
        let lines = &files[0].hunks[0].lines;

        // Count line types
        let context_count = lines.iter().filter(|l| l.kind == LineKind::Context).count();
        let add_count = lines.iter().filter(|l| l.kind == LineKind::Add).count();
        let del_count = lines.iter().filter(|l| l.kind == LineKind::Del).count();

        assert_eq!(context_count, 4); // line1, line4, line6, line7
        assert_eq!(add_count, 3); // added1, added2, added3
        assert_eq!(del_count, 2); // removed, removed2
    }
}
