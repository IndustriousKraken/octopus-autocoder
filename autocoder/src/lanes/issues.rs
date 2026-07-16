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

/// One issue unit excluded from selection because it holds an
/// `.in-progress` lock, carrying the lock file's age. `stale` is true when
/// the age exceeds the busy-marker stale threshold — the walker recovers a
/// stale lock (crash leftover) on the same pass; a fresh lock stays excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedUnit {
    pub slug: String,
    pub age: std::time::Duration,
    pub stale: bool,
}

/// One issue unit excluded because it carries a `.perma-stuck.json` park
/// marker. `marked_at` and `detail` (the marker's `last_reason`) come from
/// the marker JSON; when the marker is unreadable `unreadable` is set —
/// `marked_at` falls back to now AND `detail` is empty, so the caller can
/// render the entry as unavailable without breaking the reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParkedUnit {
    pub slug: String,
    pub marked_at: chrono::DateTime<Utc>,
    pub detail: String,
    pub unreadable: bool,
}

/// The issues lane's per-pass enumeration: units ready for selection PLUS
/// the excluded units WITH their reasons (locked/parked). Both the walker
/// (which logs, recovers stale locks, and works the ready set) AND the
/// `repo_status` control-socket action consume this ONE source rather than
/// re-walking `issues/` and diverging (mirrors the changes lane's
/// `queue::list_marker_excluded` pattern).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssuesEnumeration {
    /// Ready units, sorted ascending (the lane's selection order).
    pub ready: Vec<String>,
    /// Locked units (fresh AND stale), sorted by slug.
    pub locked: Vec<LockedUnit>,
    /// Parked units, sorted by slug.
    pub parked: Vec<ParkedUnit>,
}

impl IssuesEnumeration {
    /// Slugs selectable this pass: genuinely-ready units PLUS stale-locked
    /// units (which the walker recovers on this same pass, making them
    /// selectable). Sorted ascending. Used by the precedence gate so a
    /// recoverable stale lock does not read as "no ready issue".
    pub fn selectable(&self) -> Vec<String> {
        let mut v = self.ready.clone();
        v.extend(
            self.locked
                .iter()
                .filter(|l| l.stale)
                .map(|l| l.slug.clone()),
        );
        v.sort();
        v
    }
}

/// Gather candidate unit slugs (EITHER form) under `<workspace>/issues/`. A
/// unit is a top-level `<slug>.md` FILE OR a non-`archive`, non-`.`-prefixed
/// `<slug>/` DIRECTORY. The lane's own marker siblings
/// (`<slug>.in-progress`, `<slug>.perma-stuck.json`) AND any other
/// non-`.md`, non-directory sibling are ignored — not mistaken for units.
/// Unsorted; callers sort their derived lists.
fn unit_slugs(workspace: &Path) -> Result<Vec<String>> {
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
    Ok(slugs)
}

/// The `.in-progress` lock file's age (from its mtime), or `None` when the
/// lock is absent OR its metadata cannot be read. `stale_threshold_secs`
/// comes from the busy-marker threshold (a lock older than it is a crash
/// leftover); `0` disables staleness entirely, matching the busy-marker
/// convention.
fn lock_age(workspace: &Path, slug: &str) -> Option<std::time::Duration> {
    let path = lock_path(workspace, slug);
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    Some(
        std::time::SystemTime::now()
            .duration_since(mtime)
            .unwrap_or_default(),
    )
}

/// Read a parked unit's `.perma-stuck.json` for `(marked_at, detail,
/// unreadable)`. `detail` is the marker's `last_reason`. A read/parse
/// failure logs a WARN AND returns `unreadable = true` (marked_at → now,
/// detail empty) so callers degrade the entry, never the whole enumeration.
/// The caller has already confirmed the marker exists (`is_perma_stuck`).
fn read_park_marker_meta(workspace: &Path, slug: &str) -> (chrono::DateTime<Utc>, String, bool) {
    let path = perma_stuck_marker_path(workspace, slug);
    match std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<IssuePermaStuckMarker>(&raw).ok())
    {
        Some(m) => (m.marked_stuck_at, m.last_reason, false),
        None => {
            tracing::warn!(
                slug,
                "issues lane: park marker {} is unreadable; rendering the entry as unavailable",
                path.display()
            );
            (Utc::now(), String::new(), true)
        }
    }
}

/// Enumerate the issues lane: ready units, locked units (with lock age AND
/// a stale flag), AND parked units (with marked-at + last-reason detail).
/// PURE classification — it removes nothing and posts no alert; the walker's
/// [`recover_stale_locks`] performs the side-effecting stale-lock recovery.
/// A locked unit is classified before a parked one (lock takes precedence,
/// matching the historical skip order); a malformed / unloadable unit is
/// excluded with a WARN, as before, and appears in none of the three lists.
pub fn enumerate(workspace: &Path, stale_threshold_secs: u64) -> Result<IssuesEnumeration> {
    let mut out = IssuesEnumeration::default();
    for name in unit_slugs(workspace)? {
        if lock_path(workspace, &name).exists() {
            let age = lock_age(workspace, &name).unwrap_or_default();
            let stale = stale_threshold_secs > 0 && age.as_secs() > stale_threshold_secs;
            out.locked.push(LockedUnit {
                slug: name,
                age,
                stale,
            });
            continue;
        }
        // A parked (perma-stuck) issue is excluded from selection until the
        // operator removes its marker, mirroring the changes lane's
        // `.perma-stuck.json` skip. For a single-file issue this consults
        // the sibling marker.
        if is_perma_stuck(workspace, &name) {
            let (marked_at, detail, unreadable) = read_park_marker_meta(workspace, &name);
            out.parked.push(ParkedUnit {
                slug: name,
                marked_at,
                detail,
                unreadable,
            });
            continue;
        }
        match load(workspace, &name) {
            Ok(_) => out.ready.push(name),
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
    out.ready.sort();
    out.locked.sort_by(|a, b| a.slug.cmp(&b.slug));
    out.parked.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(out)
}

/// Recover the stale locks found in `enumeration`: remove each stale
/// `.in-progress` lock file AND return the removed units (now selectable).
/// Pure filesystem — the walker logs the WARN + posts the chatops alert +
/// folds the recovered slugs into its ready set. Fresh locks and park
/// markers are left untouched (a park marker is operator-owned and NEVER
/// auto-removed). A removal that fails is logged AND left excluded.
pub fn recover_stale_locks(workspace: &Path, enumeration: &IssuesEnumeration) -> Vec<LockedUnit> {
    let mut recovered = Vec::new();
    for locked in enumeration.locked.iter().filter(|l| l.stale) {
        match unlock(workspace, &locked.slug) {
            Ok(()) => recovered.push(locked.clone()),
            Err(e) => tracing::warn!(
                slug = %locked.slug,
                "issues lane: failed to remove stale lock (unit stays excluded this pass): {e:#}"
            ),
        }
    }
    recovered
}

/// List ready issue slugs in EITHER form, sorted ascending. Thin wrapper
/// over [`enumerate`] with the stale threshold disabled (`0`), so every
/// `.in-progress` lock excludes exactly as before — the historical
/// selection semantics. The walker uses [`enumerate`] directly so it can
/// recover stale locks AND log exclusions; this stays for the many callers
/// that only need the ready set.
pub fn list_ready(workspace: &Path) -> Result<Vec<String>> {
    Ok(enumerate(workspace, 0)?.ready)
}

/// Slugs of parked issue units (carrying a `.perma-stuck.json` marker in
/// EITHER form), sorted ascending. The issues-lane analog of
/// `queue::list_marker_excluded`'s perma set — the ONE enumeration the
/// `clear-perma-stuck` sweep AND its exact-target fallback consume, so both
/// name exactly the parked issues on disk.
pub fn list_perma_stuck(workspace: &Path) -> Vec<String> {
    let mut out: Vec<String> = unit_slugs(workspace)
        .unwrap_or_default()
        .into_iter()
        .filter(|slug| is_perma_stuck(workspace, slug))
        .collect();
    out.sort();
    out
}

/// Resolve `input` to a parked issue slug for the exact-target
/// `clear-perma-stuck` fallback: an exact match wins; otherwise a UNIQUE
/// prefix match over parked issue slugs (honoring neither form specially —
/// removal is form-aware). `None` when nothing matches OR the prefix is
/// ambiguous, in which case the caller reports the changes-lane not-found
/// (the operator disambiguates by naming the full slug).
// ponytail: ambiguous issue-prefix falls through to the changes not-found
// error rather than surfacing an issues-specific multi-match; issue slugs
// are hand-curated and collisions are vanishingly rare.
pub fn resolve_perma_stuck_prefix(workspace: &Path, input: &str) -> Option<String> {
    let parked = list_perma_stuck(workspace);
    if parked.iter().any(|s| s == input) {
        return Some(input.to_string());
    }
    let mut hits = parked.iter().filter(|s| s.starts_with(input));
    let first = hits.next()?.clone();
    if hits.next().is_some() {
        return None; // ambiguous prefix
    }
    Some(first)
}

/// Remove `slug`'s `.perma-stuck.json` park marker, honoring the unit's form
/// (in-directory OR sibling). Errors when the marker is absent so the
/// operator is told precisely, mirroring `queue::remove_perma_stuck_marker`.
pub fn remove_perma_stuck(workspace: &Path, slug: &str) -> Result<()> {
    let path = perma_stuck_marker_path(workspace, slug);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("no perma-stuck marker for issue `{slug}`")
        }
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
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

    // ----- Enriched enumeration + stale-lock recovery
    // (issues-lane-exclusions-are-observable §1) -----

    /// Set the `.in-progress` lock's mtime `age_secs` into the past so a test
    /// can drive the stale-threshold branch (libc `utimensat`, mirroring
    /// `log_retention` tests — the `filetime` crate is not a dependency).
    fn age_lock(workspace: &Path, slug: &str, age_secs: u64) {
        let path = lock_path(workspace, slug);
        let mtime = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(age_secs))
            .unwrap();
        let dur = mtime
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap();
        let ts = libc::timespec {
            tv_sec: dur.as_secs() as libc::time_t,
            tv_nsec: i64::from(dur.subsec_nanos()),
        };
        let c = std::ffi::CString::new(path.as_os_str().to_string_lossy().as_bytes()).unwrap();
        let times = [ts, ts];
        let r = unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(r, 0, "utimensat failed: {}", std::io::Error::last_os_error());
    }

    /// A fresh lock excludes the unit AND reports `locked` (not stale) with a
    /// lock age; the unit is absent from `ready`.
    #[test]
    fn enumerate_reports_fresh_lock_with_age() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_issue(ws, "worked");
        make_issue(ws, "free");
        lock(ws, "worked").unwrap();

        let e = enumerate(ws, 600).unwrap();
        assert_eq!(e.ready, vec!["free".to_string()]);
        assert_eq!(e.locked.len(), 1);
        assert_eq!(e.locked[0].slug, "worked");
        assert!(!e.locked[0].stale, "a just-created lock is not stale");
        // Age is a real duration read from the lock's mtime (within reason).
        assert!(e.locked[0].age.as_secs() < 600);
        // `selectable()` excludes a fresh lock (only recoverable staleness
        // counts as selectable).
        assert_eq!(e.selectable(), vec!["free".to_string()]);
    }

    /// A stale lock (age past the threshold) is flagged, then removed by
    /// `recover_stale_locks`; the unit returns to `ready` on the next
    /// enumeration. `selectable()` counts it before recovery.
    #[test]
    fn stale_lock_is_recovered_and_unit_returns_to_ready() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_issue(ws, "crashed");
        lock(ws, "crashed").unwrap();
        age_lock(ws, "crashed", 1200); // older than the 600s threshold

        let e = enumerate(ws, 600).unwrap();
        assert!(e.ready.is_empty(), "still locked before recovery");
        assert_eq!(e.locked.len(), 1);
        assert!(e.locked[0].stale, "a 20-minute-old lock is stale at 600s");
        // A stale lock is selectable — the walker will recover it this pass.
        assert_eq!(e.selectable(), vec!["crashed".to_string()]);

        let recovered = recover_stale_locks(ws, &e);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].slug, "crashed");
        assert!(!lock_path(ws, "crashed").exists(), "stale lock removed");

        // Now selectable as a normal ready unit.
        let after = enumerate(ws, 600).unwrap();
        assert_eq!(after.ready, vec!["crashed".to_string()]);
        assert!(after.locked.is_empty());
    }

    /// A parked unit reports `parked` with the marker's marked-at + last
    /// reason, and is NEVER auto-removed by stale-lock recovery — no matter
    /// how old the marker is (park is operator-owned).
    #[test]
    fn enumerate_reports_parked_and_recovery_never_touches_it() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_issue(ws, "stuck");
        write_perma_stuck(ws, "stuck", 3, "agent gave up").unwrap();

        let e = enumerate(ws, 600).unwrap();
        assert!(e.ready.is_empty());
        assert!(e.locked.is_empty());
        assert_eq!(e.parked.len(), 1);
        assert_eq!(e.parked[0].slug, "stuck");
        assert_eq!(e.parked[0].detail, "agent gave up");
        assert!(!e.parked[0].unreadable);
        // A park marker is not "selectable" — recovery leaves it alone.
        assert!(e.selectable().is_empty());
        let recovered = recover_stale_locks(ws, &e);
        assert!(recovered.is_empty(), "park markers are never auto-removed");
        assert!(is_perma_stuck(ws, "stuck"), "marker survives recovery");
    }

    /// An unreadable park marker degrades the entry (unreadable flag set),
    /// never the enumeration — the unit is still reported parked.
    #[test]
    fn enumerate_degrades_unreadable_park_marker() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_issue(ws, "corrupt");
        // Write garbage where valid marker JSON is expected.
        std::fs::write(
            issue_dir(ws, "corrupt").join(PERMA_STUCK_FILE),
            "{ not valid json",
        )
        .unwrap();

        let e = enumerate(ws, 600).unwrap();
        assert_eq!(e.parked.len(), 1);
        assert_eq!(e.parked[0].slug, "corrupt");
        assert!(e.parked[0].unreadable, "corrupt marker flagged unreadable");
        assert!(e.parked[0].detail.is_empty());
    }

    /// The exact-target `clear-perma-stuck` issue helpers: enumerate parked
    /// slugs, resolve an exact-or-prefix match, AND remove a park marker in
    /// either form.
    #[test]
    fn perma_stuck_list_resolve_and_remove() {
        let td = TempDir::new().unwrap();
        let ws = td.path();
        make_issue(ws, "dir-parked");
        write_perma_stuck(ws, "dir-parked", 1, "x").unwrap();
        make_single_file_issue(ws, "file-parked");
        write_perma_stuck(ws, "file-parked", 1, "y").unwrap();
        make_issue(ws, "healthy"); // not parked

        assert_eq!(
            list_perma_stuck(ws),
            vec!["dir-parked".to_string(), "file-parked".to_string()]
        );
        // Exact match.
        assert_eq!(
            resolve_perma_stuck_prefix(ws, "dir-parked").as_deref(),
            Some("dir-parked")
        );
        // Unique prefix match.
        assert_eq!(
            resolve_perma_stuck_prefix(ws, "file-").as_deref(),
            Some("file-parked")
        );
        // A non-parked slug does not resolve (nothing to clear).
        assert!(resolve_perma_stuck_prefix(ws, "healthy").is_none());

        // Remove honors the single-file form (sibling marker).
        remove_perma_stuck(ws, "file-parked").unwrap();
        assert!(!is_perma_stuck(ws, "file-parked"));
        // Remove honors the directory form (in-directory marker).
        remove_perma_stuck(ws, "dir-parked").unwrap();
        assert!(!is_perma_stuck(ws, "dir-parked"));
        // Removing an absent marker is a clear error, not a silent success.
        assert!(remove_perma_stuck(ws, "dir-parked").is_err());
    }
}
