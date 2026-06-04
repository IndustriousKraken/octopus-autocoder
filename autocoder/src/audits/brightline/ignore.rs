//! `.brightline-ignore` schema, loader, and per-entry validation.
//!
//! The file lives at `<workspace_root>/.brightline-ignore` and lists
//! intentional duplicate-signature occurrences brightline should not
//! report. Anchors are `file + function + signature_match` (substring) —
//! never line numbers, which shift on every edit. Each entry also
//! carries a one-line `reason` so operators (and reviewers of a `send
//! it` PR) understand why the duplication is deliberate.
//!
//! Loader contract:
//! - Missing file → empty list (no ignores).
//! - Empty file → empty list (no ignores).
//! - Malformed YAML (or missing top-level `ignore` key) → WARN log
//!   naming the parse error, treat as empty (no suppression).
//! - Per-entry validation failures (missing field) are NOT loader
//!   failures; they are caught by [`IgnoreEntry::is_valid`] callers and
//!   emit a per-entry WARN. (`serde_yml` rejects missing required
//!   fields outright, so missing-field entries hit the malformed-file
//!   path today.)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::{signature_regex, strip_rust_tests_modules};

pub const IGNORE_FILE_NAME: &str = ".brightline-ignore";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BrightlineIgnoreFile {
    #[serde(default)]
    pub ignore: Vec<IgnoreEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct IgnoreEntry {
    pub file: PathBuf,
    pub function: String,
    pub signature_match: String,
    pub reason: String,
}

/// Load `<workspace>/.brightline-ignore`. Returns the parsed entry list
/// on success, an empty list otherwise. Never errors: per the loader
/// contract above, every failure mode degrades to "no ignores."
pub fn load(workspace: &Path) -> Vec<IgnoreEntry> {
    let path = workspace.join(IGNORE_FILE_NAME);
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            // no-url: ignore-file loader keyed on workspace path, no repo URL in scope
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "brightline: reading .brightline-ignore failed; treating as empty"
            );
            return Vec::new();
        }
    };
    if contents.trim().is_empty() {
        return Vec::new();
    }
    match serde_yml::from_str::<BrightlineIgnoreFile>(&contents) {
        Ok(f) => f.ignore,
        Err(e) => {
            // no-url: ignore-file loader keyed on workspace path, no repo URL in scope
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "brightline: parsing .brightline-ignore failed; treating as empty (no suppression)"
            );
            Vec::new()
        }
    }
}

/// Whether `entry` matches a duplicate-signature site. The site's
/// `file` is the workspace-relative path; `function` is the parsed
/// function name; `signature_line` is the raw line where the signature
/// regex matched.
pub fn entry_matches_site(
    entry: &IgnoreEntry,
    site_file: &str,
    site_function: &str,
    signature_line: &str,
) -> bool {
    let entry_file = entry.file.to_string_lossy();
    entry_file == site_file
        && entry.function == site_function
        && signature_line.contains(&entry.signature_match)
}

/// Validate every entry against the current workspace state. An entry
/// is stale when (a) the named file doesn't exist, (b) the file
/// doesn't contain a function with the named name, OR (c) the
/// function's signature line no longer contains `signature_match`.
///
/// Returns the subset of `entries` that failed validation, preserving
/// input order so the chatops body lists them deterministically.
pub fn collect_stale(workspace: &Path, entries: &[IgnoreEntry]) -> Vec<IgnoreEntry> {
    let mut stale = Vec::new();
    for entry in entries {
        if is_stale(workspace, entry) {
            stale.push(entry.clone());
        }
    }
    stale
}

fn is_stale(workspace: &Path, entry: &IgnoreEntry) -> bool {
    let abs = workspace.join(&entry.file);
    let contents = match std::fs::read_to_string(&abs) {
        Ok(c) => c,
        Err(_) => return true, // file missing or unreadable
    };
    let ext = match abs.extension().and_then(|e| e.to_str()) {
        Some(e) => e,
        None => return true, // no extension → no regex → can't match
    };
    let re = match signature_regex(ext) {
        Some(r) => r,
        None => return true, // language not supported by brightline → treat as stale
    };
    let stripped = if ext == "rs" {
        strip_rust_tests_modules(&contents)
    } else {
        contents
    };
    for line in stripped.lines() {
        if let Some(caps) = re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if name == entry.function && line.contains(&entry.signature_match) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(p: &Path, contents: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let entries = load(dir.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn load_empty_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join(IGNORE_FILE_NAME), "");
        let entries = load(dir.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn load_valid_file_returns_entries() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join(IGNORE_FILE_NAME),
            r#"ignore:
  - file: examples/site-a/auth.ts
    function: handleAuthCallback
    signature_match: "async function handleAuthCallback(req"
    reason: "All example sites implement the same auth contract"
  - file: examples/site-b/auth.ts
    function: handleAuthCallback
    signature_match: "async function handleAuthCallback(req"
    reason: "All example sites implement the same auth contract"
"#,
        );
        let entries = load(dir.path());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].function, "handleAuthCallback");
        assert_eq!(
            entries[0].file.to_string_lossy(),
            "examples/site-a/auth.ts"
        );
        assert_eq!(
            entries[0].signature_match,
            "async function handleAuthCallback(req"
        );
    }

    #[test]
    fn load_malformed_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        // not valid yaml-with-the-expected-shape
        write(
            &dir.path().join(IGNORE_FILE_NAME),
            "ignore:\n  - file: x.rs\n    function: foo\n",
        );
        let entries = load(dir.path());
        // Missing required fields → serde_yml rejects → empty
        assert!(entries.is_empty());
    }

    #[test]
    fn entry_matches_site_requires_all_three_fields() {
        let e = IgnoreEntry {
            file: PathBuf::from("src/a.rs"),
            function: "foo".to_string(),
            signature_match: "fn foo(x: u32".to_string(),
            reason: "intentional".to_string(),
        };
        assert!(entry_matches_site(
            &e,
            "src/a.rs",
            "foo",
            "pub fn foo(x: u32, y: u32) -> u32 {"
        ));
        // Wrong file
        assert!(!entry_matches_site(
            &e,
            "src/b.rs",
            "foo",
            "pub fn foo(x: u32, y: u32) -> u32 {"
        ));
        // Wrong function name
        assert!(!entry_matches_site(
            &e,
            "src/a.rs",
            "bar",
            "pub fn foo(x: u32, y: u32) -> u32 {"
        ));
        // signature_match not in line
        assert!(!entry_matches_site(
            &e,
            "src/a.rs",
            "foo",
            "pub fn foo(z: String) -> String {"
        ));
    }

    #[test]
    fn collect_stale_marks_missing_file() {
        let dir = TempDir::new().unwrap();
        let e = IgnoreEntry {
            file: PathBuf::from("src/gone.rs"),
            function: "foo".to_string(),
            signature_match: "fn foo".to_string(),
            reason: "x".to_string(),
        };
        let stale = collect_stale(dir.path(), std::slice::from_ref(&e));
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0], e);
    }

    #[test]
    fn collect_stale_marks_missing_function() {
        let dir = TempDir::new().unwrap();
        write(&dir.path().join("src/a.rs"), "pub fn bar() {}\n");
        let e = IgnoreEntry {
            file: PathBuf::from("src/a.rs"),
            function: "foo".to_string(),
            signature_match: "fn foo".to_string(),
            reason: "x".to_string(),
        };
        let stale = collect_stale(dir.path(), std::slice::from_ref(&e));
        assert_eq!(stale.len(), 1);
    }

    #[test]
    fn collect_stale_marks_signature_drift() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("src/a.rs"),
            "pub fn foo(z: String) -> String { z }\n",
        );
        let e = IgnoreEntry {
            file: PathBuf::from("src/a.rs"),
            function: "foo".to_string(),
            signature_match: "fn foo(x: u32".to_string(),
            reason: "x".to_string(),
        };
        let stale = collect_stale(dir.path(), std::slice::from_ref(&e));
        assert_eq!(stale.len(), 1);
    }

    #[test]
    fn collect_stale_omits_valid_entries() {
        let dir = TempDir::new().unwrap();
        write(
            &dir.path().join("src/a.rs"),
            "pub fn foo(x: u32, y: u32) -> u32 { x + y }\n",
        );
        let e = IgnoreEntry {
            file: PathBuf::from("src/a.rs"),
            function: "foo".to_string(),
            signature_match: "fn foo(x: u32".to_string(),
            reason: "x".to_string(),
        };
        let stale = collect_stale(dir.path(), std::slice::from_ref(&e));
        assert!(stale.is_empty());
    }
}
