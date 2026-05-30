//! Acceptance-scan helper for the implementer flow (a27a2).
//!
//! Parses `tasks.md` line-by-line AND returns the unchecked tasks the
//! implementer left behind. The recovery loop uses this list to direct
//! the agent at exactly the items it forgot to either finish or signal.
//!
//! Parsing rules (per the executor capability deltas):
//! - Lines matching `^[ \t]*- \[ \] ` outside fenced code blocks count
//!   as unchecked.
//! - Lines matching `^[ \t]*- \[x\] ` (case-insensitive on `x`) count
//!   as checked AND are ignored.
//! - Content inside ` ``` ` fenced blocks is ignored entirely.
//! - An absent OR unreadable tasks.md returns zero unchecked (the
//!   caller treats this as scan-skipped).

use std::path::Path;

/// One unchecked task extracted from `tasks.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncheckedTask {
    /// 1-based line number in the source file.
    pub line_number: usize,
    /// Everything after the `- [ ] ` marker, trimmed of trailing
    /// whitespace.
    pub trailing_text: String,
}

/// Read `tasks.md` at the per-change canonical path AND return the
/// unchecked task list. Missing OR unreadable file → empty vec.
pub fn scan_change_tasks_md(workspace: &Path, change: &str) -> Vec<UncheckedTask> {
    let path = workspace
        .join("openspec/changes")
        .join(change)
        .join("tasks.md");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    parse_unchecked_tasks(&raw)
}

/// Parse the contents of a tasks.md file AND return the unchecked
/// tasks. Exposed for unit testing.
pub fn parse_unchecked_tasks(content: &str) -> Vec<UncheckedTask> {
    let mut out: Vec<UncheckedTask> = Vec::new();
    let mut in_fence = false;
    for (idx, raw_line) in content.lines().enumerate() {
        let line_number = idx + 1;
        if is_fence_line(raw_line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(trailing) = extract_unchecked(raw_line) {
            out.push(UncheckedTask {
                line_number,
                trailing_text: trailing.to_string(),
            });
        }
    }
    out
}

/// A line opens or closes a fenced code block when its first
/// non-whitespace characters are ```` ``` ```` (three backticks). Per
/// CommonMark, the fence may be followed by an info string (e.g.
/// ` ```rust `) — we accept anything after the three backticks.
fn is_fence_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```")
}

/// If `line` matches `^[ \t]*- \[ \] (.*)` return the captured tail.
/// Lines matching the checked form `[x]` (case-insensitive) return
/// `None`. Lines that look like neither return `None`.
fn extract_unchecked(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    // Need at least `- [ ] ` after the indent: 6 bytes.
    if i + 6 > bytes.len() {
        return None;
    }
    if bytes[i] != b'-' || bytes[i + 1] != b' ' || bytes[i + 2] != b'[' {
        return None;
    }
    let marker = bytes[i + 3];
    let close = bytes[i + 4];
    let space = bytes[i + 5];
    if close != b']' || space != b' ' {
        return None;
    }
    match marker {
        b' ' => {
            // Unchecked. Return the trailing text after `- [ ] `.
            let tail = &line[i + 6..];
            Some(tail.trim_end())
        }
        b'x' | b'X' => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn three_unchecked_four_checked_returns_three_entries() {
        let content = "\
# Tasks

- [ ] 1.1 first unchecked task
- [x] 1.2 first checked
- [ ] 1.3 second unchecked
- [x] 1.4 second checked
- [X] 1.5 capital-X checked (case-insensitive)
- [ ] 1.6 third unchecked
- [x] 1.7 fourth checked
";
        let out = parse_unchecked_tasks(content);
        assert_eq!(out.len(), 3, "expected 3 unchecked, got {out:?}");
        assert_eq!(out[0].trailing_text, "1.1 first unchecked task");
        assert_eq!(out[1].trailing_text, "1.3 second unchecked");
        assert_eq!(out[2].trailing_text, "1.6 third unchecked");
    }

    #[test]
    fn fenced_block_content_is_ignored() {
        let content = "\
# Tasks

- [ ] 1.1 real unchecked

```
- [ ] foo inside fence
- [ ] bar inside fence
```

- [x] 1.2 done
";
        let out = parse_unchecked_tasks(content);
        assert_eq!(out.len(), 1, "fenced unchecked items must not count: {out:?}");
        assert_eq!(out[0].trailing_text, "1.1 real unchecked");
    }

    #[test]
    fn fenced_block_with_only_unchecked_items_returns_zero() {
        let content = "\
# Tasks

```
- [ ] foo
- [ ] bar
- [ ] baz
```
";
        let out = parse_unchecked_tasks(content);
        assert!(out.is_empty(), "fenced-only content must yield zero: {out:?}");
    }

    #[test]
    fn no_checkbox_content_returns_zero() {
        let content = "\
# Tasks

Just narrative text. No checkboxes at all.

1. A numbered item.
2. Another numbered item.
";
        let out = parse_unchecked_tasks(content);
        assert!(out.is_empty(), "non-checkbox content must yield zero: {out:?}");
    }

    #[test]
    fn absent_tasks_md_returns_zero() {
        let tmp = TempDir::new().unwrap();
        let out = scan_change_tasks_md(tmp.path(), "missing-change");
        assert!(out.is_empty(), "absent tasks.md must yield zero");
    }

    #[test]
    fn unreadable_tasks_md_returns_zero() {
        // Simulate via a non-existent path. On a real fs an "unreadable"
        // file (permissions denied) would also produce Err from read_to_string;
        // the defensive default is the same.
        let tmp = TempDir::new().unwrap();
        let out = scan_change_tasks_md(tmp.path(), "definitely-not-there");
        assert!(out.is_empty());
    }

    #[test]
    fn nested_checkboxes_count_as_separate() {
        let content = "\
- [ ] 1 parent unchecked
  - [ ] 1a nested unchecked
  - [x] 1b nested checked
- [x] 2 parent checked
";
        let out = parse_unchecked_tasks(content);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].trailing_text, "1 parent unchecked");
        assert_eq!(out[1].trailing_text, "1a nested unchecked");
    }

    #[test]
    fn line_numbers_track_source() {
        let content = "\
header line 1
header line 2
- [ ] task on line 3
intervening text
- [ ] task on line 5
";
        let out = parse_unchecked_tasks(content);
        assert_eq!(out[0].line_number, 3);
        assert_eq!(out[1].line_number, 5);
    }

    #[test]
    fn trailing_whitespace_trimmed() {
        let content = "- [ ] task with trailing spaces   \n";
        let out = parse_unchecked_tasks(content);
        assert_eq!(out[0].trailing_text, "task with trailing spaces");
    }

    #[test]
    fn tab_indented_unchecked_is_counted() {
        let content = "\t- [ ] tab-indented task\n";
        let out = parse_unchecked_tasks(content);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].trailing_text, "tab-indented task");
    }

    #[test]
    fn fence_with_info_string_still_toggles() {
        // The fence might carry a language identifier (`rust`, `bash`).
        let content = "\
```rust
- [ ] inside rust fence
```

- [ ] outside fence
";
        let out = parse_unchecked_tasks(content);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].trailing_text, "outside fence");
    }

    #[test]
    fn real_tasks_md_round_trips_through_actual_file() {
        // Scan against an on-disk fixture to exercise scan_change_tasks_md.
        let tmp = TempDir::new().unwrap();
        let change_dir = tmp.path().join("openspec/changes/x");
        std::fs::create_dir_all(&change_dir).unwrap();
        std::fs::write(
            change_dir.join("tasks.md"),
            "- [ ] 1.1 alpha\n- [x] 1.2 beta\n- [ ] 1.3 gamma\n",
        )
        .unwrap();
        let out = scan_change_tasks_md(tmp.path(), "x");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].trailing_text, "1.1 alpha");
        assert_eq!(out[1].trailing_text, "1.3 gamma");
    }
}
