//! Issues-lane artifact loading, validation, AND lifecycle (a009 §2).
//!
//! An issue takes ONE of two on-disk forms:
//!   - **Single file** `issues/<slug>.md` — a description plus an OPTIONAL
//!     `## Tasks` checklist. The default form for a small, curated
//!     correction. Its per-issue markers are SIBLING files
//!     (`issues/<slug>.in-progress`, `issues/<slug>.perma-stuck.json`).
//!   - **Directory** `issues/<slug>/` containing `issue.md` (the report +
//!     diagnosis AND the acceptance criteria stated against the EXISTING
//!     specification) AND `tasks.md` (the fix steps). Required when the
//!     unit must carry a separate artifact (in particular a quarantined
//!     public `report-body.md`). Its markers live INSIDE the directory.
//!
//! NEITHER form carries a `specs/` directory — that absence is the
//! contract that an issue changes no spec. A directory unit that carries a
//! `specs/` directory is malformed (an issue carries no delta).
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
const ARCHIVE_DIR: &str = "archive";
const ISSUE_FILE: &str = "issue.md";
const TASKS_FILE: &str = "tasks.md";
const SPECS_DIR: &str = "specs";
/// Park marker for a non-progressing issue, mirroring the changes lane's
/// per-change marker. Reuses the `.perma-stuck.json` name already registered
/// in `.git/info/exclude` (workspace init), so it is gitignored at any depth
/// AND survives the per-iteration branch reset AND `git clean`.
const PERMA_STUCK_FILE: &str = ".perma-stuck.json";

/// Optional file carrying the RAW, UNTRUSTED body of a public-origin
/// reported issue (a010). Its presence marks the unit as public-origin:
/// the implementer prompt quarantines this body as DATA, distinct from
/// the maintainer-approved task in `issue.md` / `tasks.md`. Curated
/// (a009) units have no such file AND are not quarantined.
pub const REPORT_BODY_FILE: &str = "report-body.md";

/// `<workspace>/issues/` — the canonical issues-lane root (mirrors `changes/`).
pub fn issues_dir(workspace: &Path) -> PathBuf {
    workspace.join(ISSUES_SUBDIR)
}

/// `<workspace>/issues/<slug>/` — the directory-form unit path.
pub fn issue_dir(workspace: &Path, slug: &str) -> PathBuf {
    issues_dir(workspace).join(slug)
}

/// `<workspace>/issues/<slug>.md` — the single-file-form unit path.
pub fn issue_file(workspace: &Path, slug: &str) -> PathBuf {
    issues_dir(workspace).join(format!("{slug}.md"))
}

/// `<workspace>/issues/archive/`.
pub fn archive_root(workspace: &Path) -> PathBuf {
    issues_dir(workspace).join(ARCHIVE_DIR)
}

/// The two on-disk shapes an issue unit can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueForm {
    /// A single file `issues/<slug>.md`; markers are siblings.
    SingleFile,
    /// A directory `issues/<slug>/`; markers live inside.
    Directory,
}

/// Resolve the on-disk form of `slug` in the active `issues/` tree.
/// `Some(SingleFile)` when `issues/<slug>.md` exists, `Some(Directory)`
/// when `issues/<slug>/` exists, `None` when neither does. The directory
/// form is preferred if (anomalously) both exist, so a unit that carries a
/// separate artifact is never mistaken for a bare single file.
pub fn resolve_form(workspace: &Path, slug: &str) -> Option<IssueForm> {
    if issue_dir(workspace, slug).is_dir() {
        Some(IssueForm::Directory)
    } else if issue_file(workspace, slug).is_file() {
        Some(IssueForm::SingleFile)
    } else {
        None
    }
}

/// Resolve a per-issue marker path for `slug`, honoring the unit's form:
/// a sibling `issues/<slug><suffix>` for a single-file issue, OR the
/// in-directory `issues/<slug>/<dot_name>` for a directory issue. When the
/// unit is not yet on disk (e.g. resolving a lock path before the unit is
/// written), the directory form is assumed — the historical default.
///
/// `suffix` is the sibling-form filename tail (`.in-progress`,
/// `.perma-stuck.json`); `dot_name` is the in-directory filename (the same
/// string — both forms use the leading-dot name, one as a sibling tail and
/// one as a contained file).
fn marker_path(workspace: &Path, slug: &str, dot_name: &str) -> PathBuf {
    match resolve_form(workspace, slug) {
        Some(IssueForm::SingleFile) => issues_dir(workspace).join(format!("{slug}{dot_name}")),
        _ => issue_dir(workspace, slug).join(dot_name),
    }
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

/// True when a directory-form `issues/<slug>/` carries a `specs/`
/// directory (malformed — an issue carries no delta). A single-file issue
/// can never carry a `specs/` directory, so it is never malformed in this
/// sense. [`load`] is the authoritative validator; this predicate is the
/// standalone check used by callers that only need the malformed signal.
#[allow(dead_code)]
pub fn is_malformed(workspace: &Path, slug: &str) -> bool {
    issue_dir(workspace, slug).join(SPECS_DIR).is_dir()
}

/// Serialized park-marker content. Mirrors the changes lane's
/// `PermaStuckMarker`: an operator-readable record of why the issue is
/// parked AND how to unpark it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IssuePermaStuckMarker {
    pub slug: String,
    pub consecutive_failures: u32,
    pub last_reason: String,
    pub marked_stuck_at: chrono::DateTime<Utc>,
    pub operator_action: String,
}

/// The park-marker path for `slug`, honoring the unit's form: the sibling
/// `issues/<slug>.perma-stuck.json` for a single-file issue, OR the
/// in-directory `issues/<slug>/.perma-stuck.json` for a directory issue.
fn perma_stuck_marker_path(workspace: &Path, slug: &str) -> PathBuf {
    marker_path(workspace, slug, PERMA_STUCK_FILE)
}

/// True when `slug` carries a `.perma-stuck.json` park marker (in-directory
/// OR sibling, per its form) — the presence-only flag [`list_ready`]
/// consults to exclude a parked issue.
pub fn is_perma_stuck(workspace: &Path, slug: &str) -> bool {
    perma_stuck_marker_path(workspace, slug).exists()
}

/// Atomically write the park marker for `slug`. The issue unit must already
/// exist (the caller is parking a unit it just worked); the marker is
/// written in-directory for a directory issue, OR as a sibling for a
/// single-file issue.
pub fn write_perma_stuck(
    workspace: &Path,
    slug: &str,
    consecutive_failures: u32,
    last_reason: &str,
) -> Result<()> {
    let path = perma_stuck_marker_path(workspace, slug);
    let parent = path
        .parent()
        .with_context(|| format!("park-marker path has no parent: {}", path.display()))?;
    if !parent.is_dir() {
        anyhow::bail!("issue marker parent does not exist: {}", parent.display());
    }
    // The unit itself must exist (the directory for a directory issue, the
    // sibling file for a single-file issue).
    if resolve_form(workspace, slug).is_none() {
        anyhow::bail!("issue unit does not exist: {slug}");
    }
    let marker = IssuePermaStuckMarker {
        slug: slug.to_string(),
        consecutive_failures,
        last_reason: last_reason.to_string(),
        marked_stuck_at: Utc::now(),
        operator_action: "Delete this file to retry the issue.".to_string(),
    };
    let tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating tempfile in {}", parent.display()))?;
    serde_json::to_writer_pretty(&tmp, &marker)
        .with_context(|| format!("serializing park marker for {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| anyhow::anyhow!("atomically persisting {}: {e}", path.display()))?;
    Ok(())
}

/// Split a single-file issue body into the description AND an optional
/// `## Tasks` checklist. The `## Tasks` heading (case-insensitive, allowing
/// leading/trailing whitespace on the heading line) separates the two; the
/// description is everything before it. When there is no `## Tasks` heading,
/// the whole body is the description AND the task list is empty.
fn split_single_file_body(body: &str) -> (String, String) {
    for (idx, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("##") {
            if rest.trim().eq_ignore_ascii_case("tasks") {
                // Description is every line before this heading.
                let desc: Vec<&str> = body.lines().take(idx).collect();
                // Tasks body is every line AFTER the heading.
                let tasks: Vec<&str> = body.lines().skip(idx + 1).collect();
                return (desc.join("\n"), tasks.join("\n"));
            }
        }
    }
    (body.to_string(), String::new())
}

/// Split a single-file issue body into `(description, tasks)` for callers
/// that read an archived single-file unit directly (e.g. the reviewer's
/// issue brief), mirroring how [`load`] splits an active single-file unit.
pub fn split_brief(body: &str) -> (String, String) {
    split_single_file_body(body)
}

/// Load AND validate the `issues/<slug>` unit in EITHER form. A single-file
/// `issues/<slug>.md` is read as a description plus an optional `## Tasks`
/// checklist; a directory `issues/<slug>/` carries `issue.md` + `tasks.md`.
/// Validation order makes the malformed-`specs/` case authoritative for a
/// directory unit: it is rejected as malformed even if it also has
/// `issue.md` + `tasks.md`. Returns the file bodies on success.
pub fn load(workspace: &Path, slug: &str) -> std::result::Result<LoadedIssue, IssueLoadError> {
    match resolve_form(workspace, slug) {
        Some(IssueForm::SingleFile) => load_single_file(workspace, slug),
        Some(IssueForm::Directory) => load_directory(workspace, slug),
        None => Err(IssueLoadError::NotFound),
    }
}

/// Load a single-file issue `issues/<slug>.md`. A single file can never
/// carry a `specs/` directory, so it is never malformed; a `## Tasks`
/// section is the fix-step list. A single-file issue is curated/trusted, so
/// it carries no quarantined public body.
fn load_single_file(
    workspace: &Path,
    slug: &str,
) -> std::result::Result<LoadedIssue, IssueLoadError> {
    let path = issue_file(workspace, slug);
    let body = std::fs::read_to_string(&path).map_err(|e| {
        tracing::warn!(slug, "reading {} failed: {e}", path.display());
        IssueLoadError::NotFound
    })?;
    let (issue_body, tasks_body) = split_single_file_body(&body);
    Ok(LoadedIssue {
        slug: slug.to_string(),
        issue_body,
        tasks_body,
        report_body: None,
    })
}

/// Load AND validate the directory-form `issues/<slug>/` unit.
fn load_directory(
    workspace: &Path,
    slug: &str,
) -> std::result::Result<LoadedIssue, IssueLoadError> {
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

/// List ready issue slugs in EITHER form. A unit is a top-level
/// `<slug>.md` FILE OR a non-`archive`, non-`.`-prefixed `<slug>/`
/// DIRECTORY under `<workspace>/issues/`. The lane's own marker siblings
/// (`<slug>.in-progress`, `<slug>.perma-stuck.json`) AND any other
/// non-`.md`, non-directory sibling are ignored — not mistaken for units.
/// A unit that carries an `.in-progress` lock OR a `.perma-stuck.json`
/// park marker (in-directory OR sibling, per its form) is skipped. A
/// malformed directory unit (one with a `specs/` directory) is excluded
/// with a one-line WARN. Returned sorted ascending (alphabetical).
pub fn list_ready(workspace: &Path) -> Result<Vec<String>> {
    let root = issues_dir(workspace);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut slugs: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&root)
        .with_context(|| format!("reading {}", root.display()))?
    {
        let entry = entry?;
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            // A directory unit: `<slug>/`, excluding `archive` and dotdirs.
            if name == ARCHIVE_DIR || name.starts_with('.') {
                continue;
            }
            slugs.push(name);
        } else if let Some(slug) = name.strip_suffix(".md") {
            // A single-file unit: `<slug>.md`. (Marker siblings end in
            // `.in-progress` / `.perma-stuck.json`, not `.md`, so they are
            // ignored here; any other non-`.md` sibling is ignored too.)
            if slug.is_empty() || slug.starts_with('.') {
                continue;
            }
            slugs.push(slug.to_string());
        }
        // Every other sibling (marker files, attachments) is ignored.
    }

    let mut out: Vec<String> = Vec::new();
    for name in slugs {
        if lock_path(workspace, &name).exists() {
            continue;
        }
        // A parked (perma-stuck) issue is excluded from selection until the
        // operator removes its marker, mirroring the changes lane's
        // `.perma-stuck.json` skip. For a single-file issue this consults
        // the sibling marker.
        if is_perma_stuck(workspace, &name) {
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
                    "issues lane: skipping `issues/{name}` — {e}"
                );
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The `.in-progress` lock path for `slug`, honoring its form: the sibling
/// `issues/<slug>.in-progress` for a single-file issue, OR the in-directory
/// `issues/<slug>/.in-progress` for a directory issue.
fn lock_path(workspace: &Path, slug: &str) -> PathBuf {
    marker_path(workspace, slug, shared::LOCK_FILE)
}

/// Acquire the `.in-progress` lock for `slug`, honoring its form. The
/// lock-file write/remove is the shared queue-state primitive; these
/// wrappers only resolve the form-aware lock path. For a directory issue
/// the lock lives inside; for a single-file issue it is a sibling.
pub fn lock(workspace: &Path, slug: &str) -> Result<()> {
    let path = lock_path(workspace, slug);
    std::fs::File::create(&path)
        .with_context(|| format!("creating lock file {}", path.display()))?;
    Ok(())
}

pub fn unlock(workspace: &Path, slug: &str) -> Result<()> {
    let path = lock_path(workspace, slug);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing lock file {}", path.display())),
    }
}

/// Archive a completed issue in EITHER form, mirroring `changes/archive/`.
/// A single-file issue moves `issues/<slug>.md` →
/// `issues/archive/<UTC-date>-<slug>.md`; a directory issue moves
/// `issues/<slug>/` → `issues/archive/<UTC-date>-<slug>/`. Transient marker
/// siblings (`.in-progress`) of a single-file issue are dropped, not
/// archived — the body file is the self-contained archive entry. This NEVER
/// invokes `openspec` AND NEVER touches any canonical spec.
pub fn archive(workspace: &Path, slug: &str) -> Result<PathBuf> {
    let date = Utc::now().format("%Y-%m-%d");
    match resolve_form(workspace, slug) {
        Some(IssueForm::SingleFile) => {
            // Drop the transient sibling lock before the move (it is not
            // part of the archive entry).
            let _ = std::fs::remove_file(lock_path(workspace, slug));
            let dated_name = format!("{date}-{slug}.md");
            shared::archive_file_with_postcondition(
                &issue_file(workspace, slug),
                &archive_root(workspace),
                &dated_name,
            )
        }
        _ => {
            let dated_name = format!("{date}-{slug}");
            shared::archive_dir_with_postcondition(
                &issue_dir(workspace, slug),
                &archive_root(workspace),
                &dated_name,
            )
        }
    }
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

    /// Build a well-formed single-file `issues/<slug>.md` fixture
    /// (description + `## Tasks`).
    fn make_single_file_issue(workspace: &Path, slug: &str) {
        std::fs::create_dir_all(issues_dir(workspace)).unwrap();
        std::fs::write(
            issue_file(workspace, slug),
            "## Report\nbug in the parser\n\n## Tasks\n\n- [ ] 1.1 fix it\n",
        )
        .unwrap();
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
    fn list_ready_excludes_parked_issue_until_marker_removed() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_issue(ws, "parked");
        assert!(
            list_ready(ws).unwrap().contains(&"parked".to_string()),
            "selectable before parking"
        );
        // Park it.
        write_perma_stuck(ws, "parked", 2, "agent gave up").unwrap();
        assert!(is_perma_stuck(ws, "parked"));
        assert!(
            !list_ready(ws).unwrap().contains(&"parked".to_string()),
            "a parked issue is excluded from selection"
        );
        // The operator unparks by removing the marker.
        std::fs::remove_file(issue_dir(ws, "parked").join(PERMA_STUCK_FILE)).unwrap();
        assert!(
            list_ready(ws).unwrap().contains(&"parked".to_string()),
            "removing the marker re-selects the issue"
        );
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

    // ----- Single-file form (single-file-issues §4) -----

    /// 4.1: a single-file issue loads, lists ready, works, AND archives to a
    /// dated `.md` file.
    #[test]
    fn single_file_issue_loads_lists_and_archives() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_single_file_issue(ws, "fix-parser");

        // Loads: description split from the `## Tasks` checklist.
        let loaded = load(ws, "fix-parser").unwrap();
        assert_eq!(loaded.slug, "fix-parser");
        assert!(loaded.issue_body.contains("bug in the parser"));
        assert!(!loaded.issue_body.contains("## Tasks"));
        assert!(loaded.tasks_body.contains("1.1 fix it"));
        // A curated single-file issue is never public-origin.
        assert!(loaded.report_body.is_none());
        assert!(!loaded.is_public_origin());

        // Lists ready (the `.md` file is the unit).
        assert_eq!(list_ready(ws).unwrap(), vec!["fix-parser".to_string()]);

        // Works (lock/unlock via sibling marker — see marker test).
        lock(ws, "fix-parser").unwrap();
        unlock(ws, "fix-parser").unwrap();

        // Archives to a dated `.md` file.
        let dest = archive(ws, "fix-parser").unwrap();
        let today = Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(dest, archive_root(ws).join(format!("{today}-fix-parser.md")));
        assert!(dest.is_file());
        assert!(!issue_file(ws, "fix-parser").exists(), "source moved");
        assert!(std::fs::read_to_string(&dest).unwrap().contains("bug in the parser"));
    }

    /// A single-file issue with NO `## Tasks` section loads with an empty
    /// task list and the whole body as the description.
    #[test]
    fn single_file_issue_without_tasks_section_loads() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        std::fs::create_dir_all(issues_dir(ws)).unwrap();
        std::fs::write(issue_file(ws, "tiny"), "just fix the typo on line 3\n").unwrap();
        let loaded = load(ws, "tiny").unwrap();
        assert!(loaded.issue_body.contains("typo on line 3"));
        assert!(loaded.tasks_body.trim().is_empty());
    }

    /// 4.4: a single-file issue's lock/perma-stuck markers are SIBLINGS, are
    /// NOT mistaken for units by `list_ready`, AND a parked single-file
    /// issue is skipped via its sibling `.perma-stuck.json`.
    #[test]
    fn single_file_markers_are_siblings_and_not_units() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_single_file_issue(ws, "fix-parser");

        // Lock writes a SIBLING `.in-progress`, not an in-directory file.
        lock(ws, "fix-parser").unwrap();
        let sibling_lock = issues_dir(ws).join("fix-parser.in-progress");
        assert!(sibling_lock.is_file(), "lock is a sibling file");
        assert!(!issue_dir(ws, "fix-parser").exists(), "no unit directory exists");
        // The locked unit is skipped, and the sibling marker is not a unit.
        assert!(list_ready(ws).unwrap().is_empty());
        unlock(ws, "fix-parser").unwrap();
        assert!(!sibling_lock.exists());
        assert_eq!(list_ready(ws).unwrap(), vec!["fix-parser".to_string()]);

        // Park writes a SIBLING `.perma-stuck.json`; the parked unit is
        // skipped; the marker is not a unit.
        write_perma_stuck(ws, "fix-parser", 2, "gave up").unwrap();
        let sibling_park = issues_dir(ws).join("fix-parser.perma-stuck.json");
        assert!(sibling_park.is_file(), "park marker is a sibling file");
        assert!(is_perma_stuck(ws, "fix-parser"));
        assert!(
            list_ready(ws).unwrap().is_empty(),
            "a parked single-file issue is skipped via its sibling marker"
        );
        // Removing the marker re-selects it.
        std::fs::remove_file(&sibling_park).unwrap();
        assert_eq!(list_ready(ws).unwrap(), vec!["fix-parser".to_string()]);
    }

    /// A directory issue's markers stay INSIDE the directory (regression).
    #[test]
    fn directory_issue_markers_stay_inside() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_issue(ws, "fix-thing");
        lock(ws, "fix-thing").unwrap();
        assert!(
            issue_dir(ws, "fix-thing").join(shared::LOCK_FILE).is_file(),
            "directory lock is in-directory"
        );
        assert!(
            !issues_dir(ws).join("fix-thing.in-progress").exists(),
            "no sibling lock for a directory issue"
        );
        unlock(ws, "fix-thing").unwrap();
        write_perma_stuck(ws, "fix-thing", 2, "x").unwrap();
        assert!(issue_dir(ws, "fix-thing").join(PERMA_STUCK_FILE).is_file());
        assert!(!issues_dir(ws).join("fix-thing.perma-stuck.json").exists());
    }

    /// `list_ready` lists BOTH forms together, sorted, ignoring marker
    /// siblings of either.
    #[test]
    fn list_ready_mixes_both_forms() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_single_file_issue(ws, "single-b");
        make_issue(ws, "dir-a");
        // A stray sibling marker for a not-yet-existent slug must be ignored.
        std::fs::write(issues_dir(ws).join("ghost.perma-stuck.json"), "{}").unwrap();
        let ready = list_ready(ws).unwrap();
        assert_eq!(ready, vec!["dir-a".to_string(), "single-b".to_string()]);
    }
}
