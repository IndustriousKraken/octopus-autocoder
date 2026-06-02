//! CI-enforced rule (per `a42-audit-logs-carry-repo-url`): every
//! `tracing::warn!`, `tracing::info!`, AND `tracing::error!` call site
//! under `autocoder/src/audits/**/*.rs` SHALL carry a structured `url`
//! field naming the repository the audit is running against — OR be
//! explicitly annotated with a `// no-url: <reason>` comment on the line
//! immediately preceding the macro invocation.
//!
//! Why: in a multi-repo deployment, an audit WARN/INFO/ERROR with no
//! repo attribution forces the operator to guess which of N configured
//! repositories produced the line. Threading `url = %ctx.repo.url`
//! (or `url = %repo_url` when a helper takes the URL directly) into the
//! existing structured-field set makes `journalctl -u autocoder | grep
//! <repo-url>` a reliable per-repo filter.
//!
//! The annotation is the escape hatch for genuinely repo-agnostic
//! sites (daemon-start registry/state reload, daemon-global thread-state
//! prune, pure value parsers with no `AuditContext` in scope). Forcing
//! the choice to be explicit keeps the convention self-enforcing: a new
//! tracing call added later with neither a `url` field nor an annotation
//! fails this test in CI, so the contributor must make the attribution
//! decision deliberately.
//!
//! Scope: ONLY `src/audits/**/*.rs` AND ONLY the three named levels.
//! `tracing::debug!` / `tracing::trace!` sites are out of scope (they
//! are operator-diagnostic, not the audit-failure surface this rule
//! targets). Other modules (`polling_loop.rs`, `chatops/`, `executor/`)
//! follow their own tracing conventions and are not scanned here.
//!
//! Determinism: the test reads source files only — no clock, no env
//! mutation, no network. It produces a SINGLE combined failure summary
//! listing every offending site so an operator fixing many at once sees
//! them all in one run rather than fix-then-re-run.

use std::fs;
use std::path::{Path, PathBuf};

/// The three tracing levels this rule governs. A line is a "tracing
/// site" when it contains one of these exact macro-invocation markers.
const TRACING_MARKERS: &[&str] = &["tracing::warn!", "tracing::info!", "tracing::error!"];

#[test]
fn every_audit_tracing_site_carries_url_or_is_annotated() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let audits_root = crate_root.join("src").join("audits");
    assert!(
        audits_root.is_dir(),
        "src/audits/ must exist under {}",
        crate_root.display()
    );

    let mut rs_files = Vec::new();
    collect_rs_files(&audits_root, &mut rs_files);
    rs_files.sort();
    assert!(
        !rs_files.is_empty(),
        "expected at least one .rs file under {}",
        audits_root.display()
    );

    let mut violations: Vec<String> = Vec::new();
    for path in &rs_files {
        let rel = path
            .strip_prefix(&crate_root)
            .expect("walker returns paths under crate root")
            .to_string_lossy()
            .replace('\\', "/");
        let contents = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => panic!("could not read {}: {e}", path.display()),
        };
        scan_source(&rel, &contents, &mut violations);
    }

    assert!(
        violations.is_empty(),
        "Audit-module tracing call(s) missing a `url` field AND a `// no-url:` annotation \
         ({} site(s)):\n\n{}\n\n\
         Fix each site by EITHER adding the repo URL to its structured-field set \
         (`url = %ctx.repo.url` when the function has an `&AuditContext`, or \
         `url = %repo_url` when it takes the URL as a `&str` parameter), OR — if the \
         call is genuinely repo-agnostic (daemon-start reload, daemon-global maintenance, \
         a pure parser with no repo context) — adding a `// no-url: <reason>` comment on \
         the line immediately preceding the macro invocation. See \
         `a42-audit-logs-carry-repo-url`.",
        violations.len(),
        violations.join("\n"),
    );
}

/// Self-test: the scanner must FLAG an unattributed site and PASS both a
/// `url`-bearing site and a `// no-url:`-annotated one. Guards against a
/// silent-pass regression where the matcher logic drifts out of sync
/// with the rule it is supposed to enforce (spec scenario: "Regression
/// test catches a new tracing call added without attribution").
#[test]
fn scanner_flags_unattributed_and_passes_compliant_sites() {
    let synthetic = r#"
fn compliant(ctx: &Ctx) {
    tracing::warn!(
        url = %ctx.repo.url,
        audit_type = "x",
        "this one is attributed"
    );
}

fn annotated() {
    // no-url: daemon-start global reload, no per-repo context
    tracing::info!("this one is explicitly repo-agnostic");
}

fn single_line_attributed(repo_url: &str) {
    tracing::error!(url = %repo_url, "single-line, has the field");
}

fn offender() {
    tracing::warn!("this one has neither a url field nor an annotation");
}
"#;
    let mut violations = Vec::new();
    scan_source("tests/synthetic.rs", synthetic, &mut violations);

    assert_eq!(
        violations.len(),
        1,
        "exactly the one unattributed site should be flagged; got: {violations:?}"
    );
    assert!(
        violations[0].contains("neither a url field nor an annotation"),
        "the flagged excerpt should be the offending macro line; got: {}",
        violations[0]
    );

    // A `repo_url` FIELD (not the `url` field) must NOT satisfy the rule:
    // the word-boundary matcher rejects `repo_url =` as the `url` field.
    let near_miss = r#"
fn near_miss() {
    tracing::warn!(
        repo_url = %something,
        "repo_url is not the canonical `url` field name"
    );
}
"#;
    let mut nm_violations = Vec::new();
    scan_source("tests/near_miss.rs", near_miss, &mut nm_violations);
    assert_eq!(
        nm_violations.len(),
        1,
        "a `repo_url` field must not be mistaken for the `url` field; got: {nm_violations:?}"
    );
}

/// Core scanner: append a `rel:lineno: <excerpt>` entry to `violations`
/// for every `tracing::(warn|info|error)!` site in `contents` that lacks
/// both a `url` field (anywhere inside the macro's argument span) AND a
/// `// no-url:` annotation on the immediately-preceding source line.
fn scan_source(rel: &str, contents: &str, violations: &mut Vec<String>) {
    let lines: Vec<&str> = contents.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !is_tracing_site(line) {
            continue;
        }
        // The macro's argument span runs from this line through the line
        // that closes its parentheses (bounded so a malformed source can
        // never run away). `url` may appear on the opening line (single-
        // line calls) or on any continuation line within the span.
        let end = macro_arg_end(&lines, i);
        let has_url = (i..=end).any(|j| line_has_url_field(lines[j]));
        if has_url {
            continue;
        }
        // Escape hatch: a `// no-url:` comment on the line immediately
        // above the macro invocation marks a deliberate repo-agnostic
        // site.
        let annotated = i > 0 && lines[i - 1].contains("// no-url:");
        if annotated {
            continue;
        }
        violations.push(format!(
            "{}:{}: tracing call missing `url` field AND no `// no-url:` annotation: {}",
            rel,
            i + 1,
            line.trim()
        ));
    }
}

/// True when `line` invokes one of the governed tracing macros.
fn is_tracing_site(line: &str) -> bool {
    TRACING_MARKERS.iter().any(|m| line.contains(m))
}

/// Index of the line that closes the macro invocation starting at
/// `start`, found by balancing parentheses. Capped at 15 lines past the
/// start so a source file that is somehow malformed cannot cause an
/// unbounded scan. Parens inside the string-literal message bodies of
/// these macros are balanced in practice, so a naive count is correct
/// for the controlled source this scanner walks.
fn macro_arg_end(lines: &[&str], start: usize) -> usize {
    let cap = (start + 15).min(lines.len().saturating_sub(1));
    let mut depth: i32 = 0;
    let mut seen_open = false;
    for (j, line) in lines.iter().enumerate().take(cap + 1).skip(start) {
        for ch in line.chars() {
            match ch {
                '(' => {
                    depth += 1;
                    seen_open = true;
                }
                ')' => depth -= 1,
                _ => {}
            }
        }
        if seen_open && depth <= 0 {
            return j;
        }
    }
    cap
}

/// True when `line` contains the structured field named exactly `url`
/// (i.e. a `url` token, NOT preceded by an identifier character so
/// `repo_url`/`upstream_url` do not match, followed by optional
/// whitespace and `=`).
fn line_has_url_field(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut search_from = 0;
    while let Some(rel_pos) = line[search_from..].find("url") {
        let abs = search_from + rel_pos;
        let left_ok = abs == 0 || !is_ident_byte(bytes[abs - 1]);
        let after = abs + 3;
        if left_ok {
            let rest = line[after..].trim_start();
            if rest.starts_with('=') {
                return true;
            }
        }
        search_from = after;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Recursively collect every `*.rs` file under `dir` into `out`. Errors
/// reading a directory abort the test loudly because they indicate a
/// broken sandbox rather than a legitimate empty result.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => panic!("read_dir {}: {e}", dir.display()),
    };
    for entry in entries {
        let entry = entry.expect("read_dir entry must succeed");
        let path = entry.path();
        let file_type = entry.file_type().expect("file_type must succeed");
        if file_type.is_dir() {
            collect_rs_files(&path, out);
        } else if file_type.is_file()
            && path.extension().and_then(|s| s.to_str()) == Some("rs")
        {
            out.push(path);
        }
    }
}
