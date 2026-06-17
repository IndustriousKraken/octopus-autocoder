//! Issues-lane artifact loading, validation, AND lifecycle (a009 §2).
//!
//! An issue is a directory `issues/<slug>/` containing
//! `issue.md` (the report + diagnosis AND the acceptance criteria stated
//! against the EXISTING specification) AND `tasks.md` (the fix steps),
//! with NO `specs/` directory — that absence is the contract that an
//! issue changes no spec. A unit that carries a `specs/` directory is
//! malformed (an issue carries no delta).
//!
//! The lane lives at the repository root (`issues/`), NOT under
//! `openspec/`: issues are autocoder's own construct, not an OpenSpec
//! artifact (the `openspec` CLI never reads them). On completion the issue
//! directory moves to `issues/archive/` (mirroring `changes/archive/`); NO
//! canonical spec is modified — the issues lane leaves an audit trail only.

use crate::lanes::shared;
use anyhow::{Context, Result};
use chrono::Utc;
use std::fmt;
use std::path::{Path, PathBuf};

/// Subdirectory under the workspace holding the issues lane, at the
/// repository root (mirroring `changes/` for the changes lane). Issues are
/// autocoder's own construct, not an OpenSpec artifact, so the lane lives at
/// the root rather than under `openspec/`.
pub const ISSUES_SUBDIR: &str = "issues";
/// Pre-relocation location of the issues lane. A repository that has not yet
/// been migrated (`git mv openspec/issues issues`) is still served from here
/// transitionally; see [`issues_dir`]. Removed in a later release.
const LEGACY_ISSUES_SUBDIR: &str = "openspec/issues";
const ARCHIVE_DIR: &str = "archive";
const ISSUE_FILE: &str = "issue.md";
const TASKS_FILE: &str = "tasks.md";
const SPECS_DIR: &str = "specs";

/// Optional file carrying the RAW, UNTRUSTED body of a public-origin
/// reported issue (a010). Its presence marks the unit as public-origin:
/// the implementer prompt quarantines this body as DATA, distinct from
/// the maintainer-approved task in `issue.md` / `tasks.md`. Curated
/// (a009) units have no such file AND are not quarantined.
pub const REPORT_BODY_FILE: &str = "report-body.md";

/// `<workspace>/issues/` — the canonical issues-lane root.
///
/// Transitional migration: if the canonical `issues/` directory does not
/// exist but a pre-relocation `openspec/issues/` directory does, this
/// resolves to the legacy location (for BOTH read and write) so a repository
/// that has not yet run `git mv openspec/issues issues` keeps working; a
/// one-time WARN names the remedy. Once `issues/` exists the legacy path is
/// ignored, and a fresh repository uses the canonical path. The legacy
/// fallback is removed in a later release.
pub fn issues_dir(workspace: &Path) -> PathBuf {
    let canonical = workspace.join(ISSUES_SUBDIR);
    if canonical.exists() {
        return canonical;
    }
    let legacy = workspace.join(LEGACY_ISSUES_SUBDIR);
    if legacy.is_dir() {
        warn_legacy_issues_dir_once(&legacy);
        return legacy;
    }
    canonical
}

/// Emit a single process-wide WARN the first time the issues lane resolves
/// to the pre-relocation `openspec/issues/` location, naming the migration.
fn warn_legacy_issues_dir_once(legacy: &Path) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            legacy = %legacy.display(),
            "issues lane resolved to the pre-relocation `openspec/issues/` location; \
             run `git mv openspec/issues issues` to migrate (the legacy fallback is \
             removed in a later release)"
        );
    });
}

/// `<workspace>/issues/<slug>/`.
pub fn issue_dir(workspace: &Path, slug: &str) -> PathBuf {
    issues_dir(workspace).join(slug)
}

/// `<workspace>/issues/archive/`.
pub fn archive_root(workspace: &Path) -> PathBuf {
    issues_dir(workspace).join(ARCHIVE_DIR)
}

/// Why an `issues/<slug>/` unit failed to load as a well-formed issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueLoadError {
    /// The unit directory does not exist.
    NotFound,
    /// The unit carries a `specs/` directory — an issue carries no spec
    /// delta, so this is malformed.
    MalformedHasSpecsDir,
    /// Required `issue.md` is missing.
    MissingIssueMd,
    /// Required `tasks.md` is missing.
    MissingTasksMd,
}

impl fmt::Display for IssueLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueLoadError::NotFound => write!(f, "issue directory not found"),
            IssueLoadError::MalformedHasSpecsDir => write!(
                f,
                "malformed issue: it carries a `specs/` directory, but an issue changes no spec (carries no delta)"
            ),
            IssueLoadError::MissingIssueMd => write!(f, "missing required {ISSUE_FILE}"),
            IssueLoadError::MissingTasksMd => write!(f, "missing required {TASKS_FILE}"),
        }
    }
}

/// A successfully-validated issue unit: its slug AND the bodies of its
/// two required files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedIssue {
    pub slug: String,
    pub issue_body: String,
    pub tasks_body: String,
    /// The raw, untrusted public report body, present ONLY when the unit
    /// carries a `report-body.md` (a010 public-origin path). `None` for a
    /// curated (a009) issue. When `Some`, the implementer prompt embeds it
    /// as quarantined DATA.
    pub report_body: Option<String>,
}

impl LoadedIssue {
    /// True when this is a public-origin reported issue (it carries a
    /// quarantined `report-body.md`). The task is always taken from
    /// `issue.md` / `tasks.md`; the body is data only.
    pub fn is_public_origin(&self) -> bool {
        self.report_body.is_some()
    }
}

/// True when `issues/<slug>/` carries a `specs/` directory (malformed —
/// an issue carries no delta). [`load`] is the authoritative validator;
/// this predicate is the standalone check used by callers that only need
/// the malformed signal.
#[allow(dead_code)]
pub fn is_malformed(workspace: &Path, slug: &str) -> bool {
    issue_dir(workspace, slug).join(SPECS_DIR).is_dir()
}

/// Load AND validate the `issues/<slug>/` unit. Validation order makes
/// the malformed-`specs/` case authoritative: a unit with a `specs/`
/// directory is rejected as malformed even if it also has `issue.md` +
/// `tasks.md`. Returns the file bodies on success.
pub fn load(workspace: &Path, slug: &str) -> std::result::Result<LoadedIssue, IssueLoadError> {
    let dir = issue_dir(workspace, slug);
    if !dir.is_dir() {
        return Err(IssueLoadError::NotFound);
    }
    // The absence of `specs/` is the issue contract. Check it first so a
    // delta-bearing unit is rejected as malformed before anything else.
    if dir.join(SPECS_DIR).is_dir() {
        return Err(IssueLoadError::MalformedHasSpecsDir);
    }
    let issue_path = dir.join(ISSUE_FILE);
    if !issue_path.is_file() {
        return Err(IssueLoadError::MissingIssueMd);
    }
    let tasks_path = dir.join(TASKS_FILE);
    if !tasks_path.is_file() {
        return Err(IssueLoadError::MissingTasksMd);
    }
    let issue_body = std::fs::read_to_string(&issue_path).map_err(|e| {
        tracing::warn!(slug, "reading {} failed: {e}", issue_path.display());
        IssueLoadError::MissingIssueMd
    })?;
    let tasks_body = std::fs::read_to_string(&tasks_path).map_err(|e| {
        tracing::warn!(slug, "reading {} failed: {e}", tasks_path.display());
        IssueLoadError::MissingTasksMd
    })?;
    // Optional public-origin quarantine body (a010). Its presence marks
    // the unit as public-origin; a read error is logged AND treated as
    // absent (curated path) rather than failing the load.
    let report_body = match std::fs::read_to_string(dir.join(REPORT_BODY_FILE)) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!(slug, "reading {REPORT_BODY_FILE} failed (treating as curated): {e}");
            None
        }
    };
    Ok(LoadedIssue {
        slug: slug.to_string(),
        issue_body,
        tasks_body,
        report_body,
    })
}

/// List ready issue slugs: direct subdirectories of
/// `<workspace>/issues/` that are not the `archive` directory,
/// do not begin with `.`, do not carry an `.in-progress` lock, AND load
/// as a well-formed issue. A malformed unit (one with a `specs/`
/// directory) is excluded with a one-line WARN — it is rejected, not
/// worked. Returned sorted ascending (alphabetical within the lane).
pub fn list_ready(workspace: &Path) -> Result<Vec<String>> {
    let root = issues_dir(workspace);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&root)
        .with_context(|| format!("reading {}", root.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if name == ARCHIVE_DIR || name.starts_with('.') {
            continue;
        }
        if entry.path().join(shared::LOCK_FILE).exists() {
            continue;
        }
        match load(workspace, &name) {
            Ok(_) => out.push(name),
            Err(IssueLoadError::MalformedHasSpecsDir) => {
                tracing::warn!(
                    slug = %name,
                    "issues lane: rejecting malformed `issues/{name}/` — it carries a `specs/` directory, but an issue changes no spec"
                );
            }
            Err(e) => {
                tracing::warn!(
                    slug = %name,
                    "issues lane: skipping `issues/{name}/` — {e}"
                );
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The `.in-progress` lock helpers, scoped to the issue's directory. The
/// lock-file shape is the shared queue-state primitive
/// ([`shared::acquire_lock`] / [`shared::release_lock`]); these wrappers
/// only resolve the issue directory.
pub fn lock(workspace: &Path, slug: &str) -> Result<()> {
    shared::acquire_lock(&issue_dir(workspace, slug))
}

pub fn unlock(workspace: &Path, slug: &str) -> Result<()> {
    shared::release_lock(&issue_dir(workspace, slug))
}

/// Archive a completed issue: move `issues/<slug>/` to
/// `issues/archive/<UTC-YYYY-MM-DD>-<slug>/`, mirroring
/// `changes/archive/`. Uses the shared dated-move-with-postcondition
/// primitive. This NEVER invokes `openspec` AND NEVER touches any
/// canonical spec — the issues lane leaves an audit trail only.
pub fn archive(workspace: &Path, slug: &str) -> Result<PathBuf> {
    let dated_name = format!("{}-{slug}", Utc::now().format("%Y-%m-%d"));
    shared::archive_dir_with_postcondition(
        &issue_dir(workspace, slug),
        &archive_root(workspace),
        &dated_name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a well-formed `issues/<slug>/` fixture (issue.md + tasks.md,
    /// no specs/).
    fn make_issue(workspace: &Path, slug: &str) {
        let dir = issue_dir(workspace, slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ISSUE_FILE), "## Report\nbug\n").unwrap();
        std::fs::write(dir.join(TASKS_FILE), "- [ ] 1.1 fix it\n").unwrap();
    }

    #[test]
    fn load_accepts_well_formed_issue() {
        let td = TempDir::new().unwrap();
        make_issue(td.path(), "fix-thing");
        let loaded = load(td.path(), "fix-thing").unwrap();
        assert_eq!(loaded.slug, "fix-thing");
        assert!(loaded.issue_body.contains("bug"));
        assert!(loaded.tasks_body.contains("fix it"));
    }

    #[test]
    fn load_rejects_specs_dir_as_malformed() {
        let td = TempDir::new().unwrap();
        make_issue(td.path(), "has-delta");
        // Add a specs/ directory — an issue carries no delta.
        std::fs::create_dir_all(issue_dir(td.path(), "has-delta").join("specs")).unwrap();
        assert_eq!(
            load(td.path(), "has-delta"),
            Err(IssueLoadError::MalformedHasSpecsDir)
        );
        assert!(is_malformed(td.path(), "has-delta"));
    }

    #[test]
    fn load_reads_optional_report_body_marking_public_origin() {
        let td = TempDir::new().unwrap();
        // Curated (a009): no report-body.md → not public-origin.
        make_issue(td.path(), "curated");
        let curated = load(td.path(), "curated").unwrap();
        assert!(curated.report_body.is_none());
        assert!(!curated.is_public_origin());

        // Public (a010): a report-body.md is present → public-origin.
        make_issue(td.path(), "public");
        std::fs::write(
            issue_dir(td.path(), "public").join(REPORT_BODY_FILE),
            "raw reporter body {{token}}",
        )
        .unwrap();
        let public = load(td.path(), "public").unwrap();
        assert_eq!(public.report_body.as_deref(), Some("raw reporter body {{token}}"));
        assert!(public.is_public_origin());
    }

    #[test]
    fn load_rejects_missing_issue_md() {
        let td = TempDir::new().unwrap();
        let dir = issue_dir(td.path(), "no-issue");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TASKS_FILE), "- [ ] 1.1\n").unwrap();
        assert_eq!(load(td.path(), "no-issue"), Err(IssueLoadError::MissingIssueMd));
    }

    #[test]
    fn load_rejects_missing_tasks_md() {
        let td = TempDir::new().unwrap();
        let dir = issue_dir(td.path(), "no-tasks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(ISSUE_FILE), "## Report\n").unwrap();
        assert_eq!(load(td.path(), "no-tasks"), Err(IssueLoadError::MissingTasksMd));
    }

    #[test]
    fn list_ready_excludes_malformed_archive_dotfiles_and_locked() {
        let td = TempDir::new().unwrap();
        make_issue(td.path(), "beta");
        make_issue(td.path(), "alpha");
        // Malformed (carries specs/) — excluded.
        make_issue(td.path(), "malformed");
        std::fs::create_dir_all(issue_dir(td.path(), "malformed").join("specs")).unwrap();
        // Locked — excluded.
        make_issue(td.path(), "locked");
        lock(td.path(), "locked").unwrap();
        // Dotfile-named — excluded.
        std::fs::create_dir_all(issue_dir(td.path(), ".hidden")).unwrap();
        // Archive subdir — excluded.
        std::fs::create_dir_all(archive_root(td.path()).join("2026-01-01-old")).unwrap();

        let ready = list_ready(td.path()).unwrap();
        assert_eq!(ready, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn list_ready_empty_when_dir_absent() {
        let td = TempDir::new().unwrap();
        assert!(list_ready(td.path()).unwrap().is_empty());
    }

    #[test]
    fn archive_moves_to_dated_issues_archive_without_touching_canon() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        // A canonical spec the issues lane must NOT modify.
        let canon = ws.join("openspec/specs/widget/spec.md");
        std::fs::create_dir_all(canon.parent().unwrap()).unwrap();
        std::fs::write(&canon, "CANON_CONTENTS").unwrap();
        make_issue(ws, "fix-widget");

        let dest = archive(ws, "fix-widget").unwrap();

        assert!(!issue_dir(ws, "fix-widget").exists(), "source moved");
        assert!(dest.is_dir());
        let today = Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(dest, archive_root(ws).join(format!("{today}-fix-widget")));
        // Canon untouched.
        assert_eq!(std::fs::read_to_string(&canon).unwrap(), "CANON_CONTENTS");
    }
}
