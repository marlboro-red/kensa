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
    let mut old_path: Option<String> = None;
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
            old_path = Some(line[12..].to_string());
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
            old_path,
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
    let old_count: u32 = caps.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1));
    let new_start: u32 = caps.get(3)?.as_str().parse().ok()?;
    let new_count: u32 = caps.get(4).map_or(1, |m| m.as_str().parse().unwrap_or(1));

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
            old_start,
            old_count,
            new_start,
            new_count,
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
        assert_eq!(files[0].old_path, Some("old.txt".to_string()));
    }
}
