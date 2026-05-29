//! Architecture-brightline audit. Pure-code metrics; no LLM invocation,
//! no network. `requires_head_change = true`, `WritePolicy::None`.
//!
//! Surfaces structural metrics that frequently signal drift in a code
//! base: oversize source files and identical function signatures across
//! files. The set is intentionally small in the foundation change;
//! future audits can plug in more checks via additional `Audit`
//! implementations or by extending this module's metric list.
//!
//! The `🔍 created proposal` chatops notification documented in
//! `a02-audit-proposal-created-notification` does NOT fire from this
//! audit — brightline produces pure-data findings and does not
//! generate an LLM proposal under `openspec/changes/<slug>/`, so
//! there is no proposal-creation event to signal.

use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::{
    Audit, AuditContext, AuditOutcome, Finding, Severity, WritePolicy, workspace_is_valid,
    workspace_unavailable_outcome,
};
use crate::config::AuditSettings;

pub mod ignore;

/// Subject prefix used for stale `.brightline-ignore` entries. The
/// chatops top-line formatter (`format_audit_top_line`) counts findings
/// whose subject starts with this prefix to render the trailing
/// `; <K> stale ignore entries to clean up` clause.
pub const STALE_IGNORE_SUBJECT_PREFIX: &str = "stale ignore entry: ";

const DEFAULT_FILE_LINES_THRESHOLD: u64 = 800;
const SETTINGS_KEY_FILE_LINES: &str = "file_lines_threshold";

/// Directories to skip entirely. Vendored / generated trees would
/// dominate the findings otherwise.
const EXCLUDED_DIR_COMPONENTS: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "out",
    ".git",
    ".cache",
    ".venv",
    "venv",
    "__pycache__",
];

/// Extensions the scanner examines. Anything else is ignored; binary
/// formats and asset blobs would just clutter the report.
const SCANNED_EXTENSIONS: &[&str] = &[
    "rs", "py", "cs", "ts", "tsx", "js", "jsx", "go", "java", "kt", "swift",
];

#[derive(Clone)]
pub struct ArchitectureBrightlineAudit {
    file_lines_threshold: u64,
}

impl ArchitectureBrightlineAudit {
    /// Build the audit, pulling thresholds out of `audit_settings`
    /// (under the audit's slug key in `settings.extra`). Falls back to
    /// the compile-time defaults when a knob is unset.
    pub fn new(audit_settings: &HashMap<String, AuditSettings>) -> Self {
        let file_lines_threshold = audit_settings
            .get(Self::TYPE)
            .and_then(|s| s.extra.get(SETTINGS_KEY_FILE_LINES))
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_FILE_LINES_THRESHOLD);
        Self {
            file_lines_threshold,
        }
    }

    pub const TYPE: &'static str = "architecture_brightline";

    /// Run both metrics against `workspace`. Returned findings sort by
    /// severity (high → medium → low) then by subject for stability
    /// across invocations.
    pub fn analyze(&self, workspace: &Path) -> Result<Vec<Finding>> {
        let scanned = collect_source_files(workspace)?;
        let ignore_entries = ignore::load(workspace);
        let mut findings = Vec::new();
        for path in &scanned {
            if let Some(f) = check_file_size(path, workspace, self.file_lines_threshold) {
                findings.push(f);
            }
        }
        findings.extend(check_signature_duplicates(&scanned, workspace, &ignore_entries));
        // Validate every loaded ignore entry against the current
        // workspace state. Stale entries surface as findings with a
        // dedicated subject prefix that the chatops top-line formatter
        // counts separately to render the trailing
        // `; <K> stale ignore entries to clean up` clause. The audit
        // does NOT modify the on-disk file (WritePolicy::None is
        // unchanged); cleanup is operator-driven.
        let stale = ignore::collect_stale(workspace, &ignore_entries);
        for entry in stale {
            findings.push(stale_finding(&entry));
        }
        // Deterministic ordering: severity (high first), then subject.
        findings.sort_by(|a, b| {
            severity_rank(b.severity)
                .cmp(&severity_rank(a.severity))
                .then(a.subject.cmp(&b.subject))
        });
        Ok(findings)
    }
}

fn stale_finding(entry: &ignore::IgnoreEntry) -> Finding {
    let file = entry.file.to_string_lossy();
    let subject = format!(
        "{prefix}{file} :: {function} — {reason}",
        prefix = STALE_IGNORE_SUBJECT_PREFIX,
        file = file,
        function = entry.function,
        reason = entry.reason,
    );
    let body = format!(
        "file: {file}\nfunction: {function}\nreason: {reason}",
        file = file,
        function = entry.function,
        reason = entry.reason,
    );
    Finding {
        severity: Severity::Low,
        subject,
        body,
        anchor: None,
    }
}

#[async_trait]
impl Audit for ArchitectureBrightlineAudit {
    fn audit_type(&self) -> &'static str {
        Self::TYPE
    }

    fn description(&self) -> &'static str {
        "file-size / module-size guidelines (architecture brightline)"
    }

    fn requires_head_change(&self) -> bool {
        true
    }

    fn write_policy(&self) -> WritePolicy {
        WritePolicy::None
    }

    async fn run(&self, ctx: &mut AuditContext<'_>) -> Result<AuditOutcome> {
        // Workspace-validity gate (see `audits-require-valid-workspace`).
        // Brightline doesn't write proposals, but running it against a
        // missing workspace produces garbage zero-file counts and is
        // gated uniformly with every other audit type so the framework
        // contract holds.
        if !workspace_is_valid(ctx.workspace) {
            return Ok(workspace_unavailable_outcome(Self::TYPE, ctx.workspace));
        }
        let findings = self.analyze(ctx.workspace)?;
        let _ = ctx.log_writer.write_section(
            "brightline_summary",
            &format!(
                "file_lines_threshold: {}\nfindings_count: {}",
                self.file_lines_threshold,
                findings.len()
            ),
        );
        // The architecture_brightline audit is pure-data file-line-counting
        // — it does NOT invoke an LLM and does NOT write proposals. The
        // post-write `openspec validate --strict` retry machinery in
        // `audits::validate_with_retry` does not apply here. (See change
        // `a01-audit-proposal-self-validation`.)
        Ok(AuditOutcome::reported(findings))
    }
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::High => 3,
        Severity::Medium => 2,
        Severity::Low => 1,
    }
}

fn collect_source_files(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(workspace, workspace, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if EXCLUDED_DIR_COMPONENTS.iter().any(|d| *d == name) {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_dir() {
            walk(root, &path, out)?;
        } else if ft.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if SCANNED_EXTENSIONS.contains(&ext) {
                    out.push(path);
                }
            }
        }
    }
    Ok(())
}

fn relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

fn check_file_size(path: &Path, root: &Path, threshold: u64) -> Option<Finding> {
    let contents = std::fs::read_to_string(path).ok()?;
    let n = contents.lines().count() as u64;
    if n <= threshold {
        return None;
    }
    let rel = relative_path(path, root);
    Some(Finding {
        severity: Severity::Medium,
        subject: format!("file {rel} is {n} lines (threshold: {threshold})"),
        body: format!("path: {rel}\nlines: {n}\nthreshold: {threshold}"),
        anchor: Some(format!("{rel}:1")),
    })
}

/// One occurrence of a function signature in a file. Used to apply
/// `.brightline-ignore` match-suppression per-site (before grouping).
#[derive(Debug, Clone)]
struct SignatureSite {
    rel_path: String,
    line_number: usize,
    function: String,
    signature_line: String,
}

/// Detect identical function/method signatures across files. We use a
/// simple regex per language and stay deliberately approximate — the
/// audit's value is fast smoke-testing, not full parsing.
///
/// `ignore_entries` carries the parsed `.brightline-ignore` content;
/// every constituent site of a duplicate-signature finding is matched
/// against the ignore list before the finding is emitted. A finding
/// whose every site matches an ignore entry is dropped entirely. A
/// finding where only some sites match is emitted with the unmatched
/// sites only, plus a "(N suppressed by .brightline-ignore)" tail in
/// the subject.
fn check_signature_duplicates(
    files: &[PathBuf],
    root: &Path,
    ignore_entries: &[ignore::IgnoreEntry],
) -> Vec<Finding> {
    // signature_key → list of SignatureSite
    let mut occurrences: BTreeMap<String, Vec<SignatureSite>> = BTreeMap::new();
    for path in files {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e,
            None => continue,
        };
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        // Strip Rust `mod tests { ... }` blocks (brace-counted) so test
        // helpers don't pollute the duplicate set.
        let stripped = if ext == "rs" {
            strip_rust_tests_modules(&contents)
        } else {
            contents.clone()
        };
        for (lineno, sig_key, function, signature_line) in extract_signature_sites(&stripped, ext) {
            occurrences
                .entry(sig_key)
                .or_default()
                .push(SignatureSite {
                    rel_path: relative_path(path, root),
                    line_number: lineno,
                    function,
                    signature_line,
                });
        }
    }
    let mut findings = Vec::new();
    for (sig_key, places) in occurrences {
        if places.len() < 2 {
            continue;
        }
        // Group by file: a signature appearing twice in the SAME file is
        // not a cross-file collision and isn't what this metric is for.
        let mut files_seen: BTreeMap<String, Vec<&SignatureSite>> = BTreeMap::new();
        for site in &places {
            files_seen.entry(site.rel_path.clone()).or_default().push(site);
        }
        if files_seen.len() < 2 {
            continue;
        }
        // Partition the distinct files into "matches an ignore entry"
        // and "doesn't". A file is considered matched when at least one
        // of its occurrences matches an entry — sites in the same file
        // are treated as one site per the audit's grouping rule above.
        let mut unmatched_files: Vec<(String, &SignatureSite)> = Vec::new();
        let mut suppressed_count: usize = 0;
        for (file, sites) in &files_seen {
            let any_matched = sites.iter().any(|s| {
                ignore_entries.iter().any(|e| {
                    ignore::entry_matches_site(e, &s.rel_path, &s.function, &s.signature_line)
                })
            });
            if any_matched {
                suppressed_count += 1;
            } else {
                let first = sites.first().copied().expect("non-empty per construction");
                unmatched_files.push((file.clone(), first));
            }
        }
        if unmatched_files.is_empty() {
            // Every constituent site is intentional — drop the finding.
            continue;
        }
        let mut subject_locations: Vec<String> = unmatched_files
            .iter()
            .map(|(p, site)| format!("{p}:{ln}", ln = site.line_number))
            .collect();
        subject_locations.sort();
        let mut body = subject_locations.join("\n");
        if suppressed_count > 0 {
            body.push_str(&format!(
                "\n({suppressed_count} site(s) suppressed by .brightline-ignore)"
            ));
        }
        let unmatched_count = unmatched_files.len();
        let subject = if suppressed_count > 0 {
            format!(
                "duplicate signature `{sig_key}` across {n} files ({suppressed_count} suppressed by .brightline-ignore)",
                n = unmatched_count,
            )
        } else {
            format!(
                "duplicate signature `{sig_key}` across {n} files",
                n = unmatched_count,
            )
        };
        findings.push(Finding {
            severity: Severity::Low,
            subject,
            body,
            anchor: subject_locations.first().cloned(),
        });
    }
    findings
}

#[allow(dead_code)]
fn extract_signatures(contents: &str, ext: &str) -> Vec<(usize, String)> {
    extract_signature_sites(contents, ext)
        .into_iter()
        .map(|(line, key, _name, _line_text)| (line, key))
        .collect()
}

/// Like [`extract_signatures`] but also returns the parsed function
/// name and the raw signature line — both needed to apply
/// `.brightline-ignore` match-suppression.
fn extract_signature_sites(contents: &str, ext: &str) -> Vec<(usize, String, String, String)> {
    let re = match signature_regex(ext) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for (idx, line) in contents.lines().enumerate() {
        if let Some(caps) = re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let params = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            // Normalize whitespace in the parameter list so trivial
            // formatting differences don't dodge the duplicate check.
            let normalized_params: String = params.split_whitespace().collect::<Vec<_>>().join(" ");
            let key = format!("{name}({normalized_params})");
            out.push((idx + 1, key, name.to_string(), line.to_string()));
        }
    }
    out
}

pub(super) fn signature_regex(ext: &str) -> Option<Regex> {
    let pattern = match ext {
        "rs" => Some(r"^\s*(?:pub\s+(?:\([^)]*\)\s+)?)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)"),
        "py" => Some(r"^\s*(?:async\s+)?def\s+(\w+)\s*\(([^)]*)\)\s*(?:->[^:]+)?\s*:"),
        "cs" => Some(r"^\s*(?:public|private|protected|internal)?\s*(?:static\s+)?(?:async\s+)?\w[\w<>?]*\s+(\w+)\s*\(([^)]*)\)"),
        "ts" | "tsx" | "js" | "jsx" => Some(
            r"^\s*(?:export\s+)?(?:async\s+)?function\s+(\w+)\s*\(([^)]*)\)",
        ),
        "go" => Some(r"^\s*func\s+(?:\([^)]+\)\s+)?(\w+)\s*\(([^)]*)\)"),
        "java" | "kt" => Some(
            r"^\s*(?:public|private|protected)?\s*(?:static\s+)?(?:async\s+)?[\w<>?\[\]]+\s+(\w+)\s*\(([^)]*)\)",
        ),
        "swift" => Some(r"^\s*(?:public|private|internal|fileprivate)?\s*func\s+(\w+)\s*\(([^)]*)\)"),
        _ => None,
    };
    pattern.and_then(|p| Regex::new(p).ok())
}

/// Strip every `mod tests { ... }` block from Rust source, brace-matched
/// so nested braces don't trip the scanner. The block can be preceded by
/// `#[cfg(test)]` or any other attribute. Returns the source with the
/// matched ranges replaced by empty lines (preserving line numbers for
/// the duplicate-signature anchors).
pub(super) fn strip_rust_tests_modules(src: &str) -> String {
    let re = match Regex::new(r"(?m)^\s*(?:#\[[^\]]+\]\s*)?mod\s+tests\s*\{") {
        Ok(r) => r,
        Err(_) => return src.to_string(),
    };
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    while let Some(m) = re.find_at(src, last) {
        out.push_str(&src[last..m.start()]);
        // Walk forward from the opening brace position to find the
        // matching closing brace.
        let body_start = m.end(); // position right after `{`
        let mut depth: i64 = 1;
        let mut idx = body_start;
        let bytes = src.as_bytes();
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        while idx < bytes.len() {
            let b = bytes[idx];
            if in_line_comment {
                if b == b'\n' {
                    in_line_comment = false;
                }
                idx += 1;
                continue;
            }
            if in_block_comment {
                if b == b'*' && idx + 1 < bytes.len() && bytes[idx + 1] == b'/' {
                    in_block_comment = false;
                    idx += 2;
                    continue;
                }
                idx += 1;
                continue;
            }
            if b == b'/' && idx + 1 < bytes.len() {
                let next = bytes[idx + 1];
                if next == b'/' {
                    in_line_comment = true;
                    idx += 2;
                    continue;
                } else if next == b'*' {
                    in_block_comment = true;
                    idx += 2;
                    continue;
                }
            }
            if b == b'{' {
                depth += 1;
            } else if b == b'}' {
                depth -= 1;
                if depth == 0 {
                    idx += 1;
                    break;
                }
            }
            idx += 1;
        }
        // Replace the entire `mod tests { ... }` span with blank lines
        // matching the original newline count, preserving line numbers.
        let span = &src[m.start()..idx];
        let newlines = span.bytes().filter(|b| *b == b'\n').count();
        for _ in 0..newlines {
            out.push('\n');
        }
        last = idx;
    }
    out.push_str(&src[last..]);
    out
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

    fn settings_with_threshold(t: u64) -> HashMap<String, AuditSettings> {
        let mut extra = HashMap::new();
        extra.insert(
            SETTINGS_KEY_FILE_LINES.to_string(),
            serde_yml::Value::Number(serde_yml::Number::from(t)),
        );
        let mut s = HashMap::new();
        s.insert(
            ArchitectureBrightlineAudit::TYPE.to_string(),
            AuditSettings {
                prompt_path: None,
                notify_on_clean: false,
                extra,
            },
        );
        s
    }

    #[test]
    fn file_size_metric_flags_long_files() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        // 1200 lines under src/ — exceeds default 800.
        let big: String = (0..1200).map(|i| format!("// line {i}\n")).collect();
        write(&ws.join("src/big.rs"), &big);
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        assert!(
            findings
                .iter()
                .any(|f| f.subject.contains("src/big.rs") && f.subject.contains("1200")),
            "expected a finding for src/big.rs; got: {findings:?}"
        );
    }

    #[test]
    fn file_size_metric_respects_threshold_override() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let medium: String = (0..600).map(|i| format!("// line {i}\n")).collect();
        write(&ws.join("src/medium.rs"), &medium);
        // Default (800) → no finding.
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        assert!(
            !findings.iter().any(|f| f.subject.contains("medium.rs")),
            "default threshold should not flag 600-line file: {findings:?}"
        );
        // Override (400) → finding.
        let audit2 = ArchitectureBrightlineAudit::new(&settings_with_threshold(400));
        let findings2 = audit2.analyze(ws).unwrap();
        assert!(
            findings2.iter().any(|f| f.subject.contains("medium.rs")),
            "override threshold should flag 600-line file: {findings2:?}"
        );
    }

    #[test]
    fn file_size_metric_ignores_excluded_dirs() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let big: String = (0..2000).map(|i| format!("// l {i}\n")).collect();
        write(&ws.join("node_modules/lib/big.js"), &big);
        write(&ws.join("target/debug/big.rs"), &big);
        write(&ws.join("vendor/dep/big.go"), &big);
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        assert!(
            findings.is_empty(),
            "excluded dirs must not contribute findings: {findings:?}"
        );
    }

    #[test]
    fn signature_duplicate_metric_flags_cross_file_collisions_rust() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        write(
            &ws.join("src/a.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x + y }\n",
        );
        write(
            &ws.join("src/b.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x * y }\n",
        );
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        assert!(
            findings.iter().any(|f| f.subject.contains("helper")),
            "expected a duplicate-signature finding for `helper`: {findings:?}"
        );
    }

    #[test]
    fn signature_duplicate_metric_ignores_tests_module() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        write(
            &ws.join("src/a.rs"),
            "pub fn alpha() {}\n#[cfg(test)]\nmod tests { fn alpha() {} }\n",
        );
        write(
            &ws.join("src/b.rs"),
            "#[cfg(test)]\nmod tests { fn alpha() {} }\n",
        );
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        // `alpha` appears as a real fn only in src/a.rs; the others are
        // inside `mod tests { ... }` and must be stripped → no cross-file
        // collision.
        assert!(
            !findings
                .iter()
                .any(|f| f.subject.contains("duplicate signature") && f.subject.contains("alpha")),
            "tests module signatures must be ignored: {findings:?}"
        );
    }

    #[test]
    fn audit_returns_no_findings_on_clean_codebase() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        // A small file, no duplicates.
        write(&ws.join("src/lib.rs"), "pub fn one() {}\n");
        write(&ws.join("src/main.rs"), "fn two() {}\nfn main() { one(); two(); }\n");
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        assert!(findings.is_empty(), "clean codebase: {findings:?}");
    }

    #[test]
    fn audit_returns_findings_for_known_violations() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        // Both: file-size violation AND signature duplicate.
        let big: String = (0..1500)
            .map(|i| format!("fn shared_name() {{ /* {i} */ }}\n"))
            .collect();
        write(&ws.join("src/giant.rs"), &big);
        write(&ws.join("src/other.rs"), "fn shared_name() {}\n");
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        assert!(findings.len() >= 2);
        let mut subjects: Vec<&String> = findings.iter().map(|f| &f.subject).collect();
        subjects.sort();
        let joined = subjects
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            joined.contains("giant.rs is 1500 lines"),
            "expected size finding for giant.rs: {joined}"
        );
        assert!(
            joined.contains("duplicate signature `shared_name"),
            "expected duplicate-signature finding: {joined}"
        );
    }

    #[test]
    fn excluded_dirs_skipped_during_walk() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        write(&ws.join("src/a.rs"), "fn x() {}\n");
        write(&ws.join("node_modules/a.js"), "function x() {}\n");
        write(&ws.join(".git/HEAD"), "ref: refs/heads/main\n");
        let collected = collect_source_files(ws).unwrap();
        let rels: Vec<String> = collected
            .iter()
            .map(|p| relative_path(p, ws))
            .collect();
        assert_eq!(rels, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn signature_regex_parses_async_pub_rust() {
        let re = signature_regex("rs").unwrap();
        let captures = re
            .captures("    pub async fn do_thing(a: u32) -> Result<()> {")
            .unwrap();
        assert_eq!(&captures[1], "do_thing");
        assert_eq!(captures[2].trim(), "a: u32");
    }

    /// Workspace-validity gate (see `audits-require-valid-workspace`):
    /// brightline must skip cleanly when the workspace is missing, even
    /// though it doesn't write proposals. The gate is uniform across
    /// every audit type for framework-contract consistency.
    #[tokio::test]
    async fn workspace_unavailable_when_path_does_not_exist() {
        use crate::audits::{AuditContext, AuditLogWriter};
        use crate::config::RepositoryConfig;

        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("never-existed");
        assert!(!workspace.exists());

        let log_writer = AuditLogWriter::open(tmp.path(), ArchitectureBrightlineAudit::TYPE)
            .expect("log writer opens");
        let log_path = log_writer.path().to_path_buf();
        let repo = RepositoryConfig {
            url: "git@github.com:test/repo.git".into(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
            spec_storage: None,
            upstream: None,
            auto_submit_pr: true,
        };
        let mut ctx = AuditContext {
            workspace: &workspace,
            repo: &repo,
            chatops_ctx: None,
            log_writer,
            max_validation_retries: 0,
        };
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let outcome = audit.run(&mut ctx).await.expect("gate returns Ok");
        match outcome {
            AuditOutcome::WorkspaceUnavailable {
                audit_type,
                workspace_path,
                reason,
            } => {
                assert_eq!(audit_type, ArchitectureBrightlineAudit::TYPE);
                assert_eq!(workspace_path, workspace);
                assert_eq!(reason, "workspace directory does not exist");
            }
            other => panic!("expected WorkspaceUnavailable, got {other:?}"),
        }
        assert!(!workspace.exists());
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::remove_dir_all(parent.parent().unwrap_or(parent));
        }
    }

    /// Workspace-validity gate: existing directory without `.git/` →
    /// WorkspaceUnavailable; the audit must not contribute zero-file
    /// garbage findings.
    #[tokio::test]
    async fn workspace_unavailable_when_dot_git_missing() {
        use crate::audits::{AuditContext, AuditLogWriter};
        use crate::config::RepositoryConfig;

        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("ws-no-git");
        std::fs::create_dir_all(&workspace).unwrap();
        let before: Vec<std::ffi::OsString> = std::fs::read_dir(&workspace)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();

        let log_writer = AuditLogWriter::open(tmp.path(), ArchitectureBrightlineAudit::TYPE)
            .expect("log writer opens");
        let log_path = log_writer.path().to_path_buf();
        let repo = RepositoryConfig {
            url: "git@github.com:test/repo.git".into(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
            spec_storage: None,
            upstream: None,
            auto_submit_pr: true,
        };
        let mut ctx = AuditContext {
            workspace: &workspace,
            repo: &repo,
            chatops_ctx: None,
            log_writer,
            max_validation_retries: 0,
        };
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let outcome = audit.run(&mut ctx).await.expect("gate returns Ok");
        match outcome {
            AuditOutcome::WorkspaceUnavailable { reason, .. } => {
                assert_eq!(reason, "workspace exists but has no .git/ subdirectory");
            }
            other => panic!("expected WorkspaceUnavailable, got {other:?}"),
        }
        let after: Vec<std::ffi::OsString> = std::fs::read_dir(&workspace)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(before, after);
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::remove_dir_all(parent.parent().unwrap_or(parent));
        }
    }

    #[test]
    fn ignore_fully_matching_finding_is_suppressed() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        write(
            &ws.join("src/a.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x + y }\n",
        );
        write(
            &ws.join("src/b.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x * y }\n",
        );
        write(
            &ws.join(".brightline-ignore"),
            r#"ignore:
  - file: src/a.rs
    function: helper
    signature_match: "fn helper(x: u32"
    reason: "intentional"
  - file: src/b.rs
    function: helper
    signature_match: "fn helper(x: u32"
    reason: "intentional"
"#,
        );
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        assert!(
            !findings.iter().any(|f| f.subject.contains("duplicate signature")),
            "all sites matched; finding should be suppressed: {findings:?}"
        );
    }

    #[test]
    fn ignore_partial_match_emits_unmatched_sites_only() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        write(
            &ws.join("src/a.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x + y }\n",
        );
        write(
            &ws.join("src/b.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x * y }\n",
        );
        write(
            &ws.join("src/c.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x - y }\n",
        );
        write(
            &ws.join(".brightline-ignore"),
            r#"ignore:
  - file: src/a.rs
    function: helper
    signature_match: "fn helper(x: u32"
    reason: "intentional"
  - file: src/b.rs
    function: helper
    signature_match: "fn helper(x: u32"
    reason: "intentional"
"#,
        );
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        let dupes: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.subject.starts_with("duplicate signature"))
            .collect();
        assert_eq!(dupes.len(), 1, "expected one partial-suppression finding: {findings:?}");
        let f = dupes[0];
        assert!(
            f.subject.contains("across 1 files"),
            "subject should reflect unmatched site count: {}",
            f.subject
        );
        assert!(
            f.subject.contains("2 suppressed by .brightline-ignore"),
            "subject should note the suppressed count: {}",
            f.subject
        );
        assert!(
            f.body.contains("src/c.rs"),
            "body should name the unmatched site: {}",
            f.body
        );
        assert!(
            !f.body.contains("src/a.rs") && !f.body.contains("src/b.rs"),
            "body should omit suppressed sites: {}",
            f.body
        );
        assert!(
            f.body.contains("2 site(s) suppressed by .brightline-ignore"),
            "body should mention suppressed count: {}",
            f.body
        );
    }

    #[test]
    fn ignore_no_matches_behaves_like_today() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        write(
            &ws.join("src/a.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x + y }\n",
        );
        write(
            &ws.join("src/b.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x * y }\n",
        );
        write(
            &ws.join("src/c.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x - y }\n",
        );
        // No .brightline-ignore.
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        let dupes: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.subject.starts_with("duplicate signature"))
            .collect();
        assert_eq!(dupes.len(), 1, "expected one finding: {findings:?}");
        let f = dupes[0];
        assert!(
            f.subject.contains("across 3 files"),
            "subject should list all sites: {}",
            f.subject
        );
        assert!(
            f.body.contains("src/a.rs")
                && f.body.contains("src/b.rs")
                && f.body.contains("src/c.rs"),
            "body should list every site: {}",
            f.body
        );
        assert!(
            !f.body.contains("suppressed"),
            "body should NOT mention suppression when no entries match: {}",
            f.body
        );
    }

    #[test]
    fn ignore_empty_list_no_suppression() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        write(
            &ws.join("src/a.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x + y }\n",
        );
        write(
            &ws.join("src/b.rs"),
            "pub fn helper(x: u32, y: u32) -> u32 { x * y }\n",
        );
        // Empty top-level `ignore` list.
        write(
            &ws.join(".brightline-ignore"),
            "ignore: []\n",
        );
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        let dupes: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.subject.starts_with("duplicate signature"))
            .collect();
        assert_eq!(dupes.len(), 1, "expected one finding: {findings:?}");
    }

    #[test]
    fn stale_entries_emit_findings_with_documented_prefix() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        // src/present.rs has `kept`; src/gone.rs is missing.
        write(&ws.join("src/present.rs"), "pub fn kept(x: u32) -> u32 { x }\n");
        write(
            &ws.join(".brightline-ignore"),
            r#"ignore:
  - file: src/gone.rs
    function: vanished
    signature_match: "fn vanished("
    reason: "this file was deleted"
  - file: src/present.rs
    function: kept
    signature_match: "fn kept(x: u32"
    reason: "still here"
"#,
        );
        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let findings = audit.analyze(ws).unwrap();
        let stale: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.subject.starts_with(STALE_IGNORE_SUBJECT_PREFIX))
            .collect();
        assert_eq!(stale.len(), 1, "expected exactly one stale entry: {findings:?}");
        let f = stale[0];
        assert!(
            f.subject.contains("src/gone.rs") && f.subject.contains("vanished"),
            "stale subject should name file + function: {}",
            f.subject
        );
        assert!(
            f.subject.contains("this file was deleted"),
            "stale subject should carry the reason: {}",
            f.subject
        );
        assert!(
            f.body.contains("file: src/gone.rs"),
            "stale body should name file: {}",
            f.body
        );
    }

    /// Regression guard for `a02-audit-proposal-created-notification`.
    /// `architecture_brightline` does NOT generate an LLM proposal, so
    /// the `🔍 created proposal` chatops notification must NEVER fire
    /// from this audit — even when it produces a non-empty findings
    /// set. The test runs the full `Audit::run` entry point through
    /// the trait and asserts that the recording chatops backend
    /// captured zero notifications.
    #[tokio::test]
    async fn brightline_does_not_post_proposal_created_notification() {
        use super::super::test_support::{RecordingBackend, make_recording_ctx};
        use crate::audits::{AuditContext, AuditLogWriter};
        use crate::config::RepositoryConfig;
        use std::sync::Arc;

        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        // Workspace-validity gate (see `audits-require-valid-workspace`)
        // requires a `.git/` subdirectory; without it the audit would
        // short-circuit to `WorkspaceUnavailable` and never reach the
        // Reported branch this test exercises.
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        // Force at least one finding (size + duplicate signature) so
        // the audit returns Reported with non-empty findings.
        let big: String = (0..1500)
            .map(|i| format!("fn shared_name() {{ /* {i} */ }}\n"))
            .collect();
        write(&ws.join("src/giant.rs"), &big);
        write(&ws.join("src/other.rs"), "fn shared_name() {}\n");

        let backend = Arc::new(RecordingBackend::new());
        let chatops = make_recording_ctx(backend.clone());

        let audit = ArchitectureBrightlineAudit::new(&HashMap::new());
        let repo = RepositoryConfig {
            url: "git@github.com:test/repo.git".into(),
            local_path: None,
            base_branch: "main".into(),
            agent_branch: "agent-q".into(),
            poll_interval_sec: 60,
            chatops_channel_id: None,
            max_changes_per_pr: None,
            audits: None,
            spec_storage: None,
            upstream: None,
            auto_submit_pr: true,
        };
        let log_writer =
            AuditLogWriter::open(ws, ArchitectureBrightlineAudit::TYPE)
                .expect("log writer opens");
        let log_path = log_writer.path().to_path_buf();
        let mut ctx = AuditContext {
            workspace: ws,
            repo: &repo,
            chatops_ctx: Some(&chatops),
            log_writer,
            max_validation_retries: 0,
        };
        let outcome = audit.run(&mut ctx).await.expect("brightline runs");
        match outcome {
            AuditOutcome::Reported { findings, .. } => {
                assert!(
                    !findings.is_empty(),
                    "fixture must produce findings so the no-fire assertion is meaningful"
                );
            }
            other => panic!("brightline must return Reported, got {other:?}"),
        }
        let calls = backend.calls();
        assert!(
            calls.is_empty(),
            "🔍 created proposal must NOT fire from architecture_brightline; got: {calls:?}"
        );

        if let Some(parent) = log_path.parent() {
            let _ = std::fs::remove_dir_all(parent.parent().unwrap_or(parent));
        }
    }
}
