//! `autocoder changelog` — harvests release-note entries from the OpenSpec
//! archive.
//!
//! Pure-data extractor: walks `<workspace>/openspec/changes/archive/`, finds
//! archive directories added between two git refs, pulls the first
//! paragraph of `## Why` (or a frontmatter override) from each archive's
//! `proposal.md`, groups by primary capability, and renders markdown or
//! JSON to stdout. No LLM, no mutation, no daemon work — same archive
//! contents + same tag range produce the same output every invocation.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, ValueEnum};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Args, Debug, Clone)]
pub struct ChangelogArgs {
    /// Directory containing `openspec/changes/archive/`. Defaults to the
    /// current working directory.
    #[arg(long)]
    pub workspace: Option<PathBuf>,

    /// Lower bound (exclusive). Defaults to the most recent tag on
    /// `HEAD`'s ancestry. The literal `ever` is a sentinel meaning
    /// "from the beginning of archive history".
    #[arg(long)]
    pub since: Option<String>,

    /// Upper bound (inclusive). Defaults to `HEAD`.
    #[arg(long, default_value = "HEAD")]
    pub to: String,

    /// Output shape. Default `markdown`.
    #[arg(long, value_enum, default_value_t = ChangelogFormat::Markdown)]
    pub format: ChangelogFormat,
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum ChangelogFormat {
    Markdown,
    Json,
}

#[derive(Debug, Clone)]
pub struct TagRange {
    pub since_commit: Option<String>,
    pub since_label: String,
    pub to_commit: String,
    pub to_label: String,
    /// Commit date of `to_commit` in UTC, formatted `YYYY-MM-DD`. Used
    /// for the markdown header date so a release tagged on day N reports
    /// day N regardless of when the underlying archives shipped.
    pub to_date: String,
}

/// Which lane an entry came from. Changes carry a spec delta and an owning
/// capability; issues are corrections with neither. Serialized as
/// `"change"` / `"issue"` in JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Change,
    Issue,
}

impl Lane {
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Change => "change",
            Lane::Issue => "issue",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub slug: String,
    pub archive_dir: PathBuf,
    /// File the summary is read from: `proposal.md` for a change,
    /// `issue.md` for a directory-form issue, the `.md` file itself for a
    /// single-file issue.
    pub summary_path: PathBuf,
    pub lane: Lane,
    pub primary_capability: Option<String>,
    pub summary: String,
    pub shipped_commit: String,
    pub shipped_date: String,
}

/// A range commit that added no archive entry in either lane (a direct
/// bugfix/docs commit that bypassed both workflows). Merge commits are
/// excluded before this list is built.
#[derive(Debug, Clone)]
pub struct UnattributedCommit {
    pub sha: String,
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone)]
pub struct SkippedEntry {
    pub slug: String,
    pub reason: String,
}

/// Outcome of reading an archive's `proposal.md`: either a usable summary
/// (for the entries list) or a skip directive (for the skipped list). The
/// caller decides which bucket the result belongs in.
#[derive(Debug, Clone)]
pub enum ArchiveMetadataRaw {
    Entry { summary: String },
    Skip { reason: String },
}

pub async fn execute(args: ChangelogArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("resolving current working directory")?;
    let workspace = args.workspace.clone().unwrap_or(cwd);
    let stderr = std::io::stderr();
    let stdout = std::io::stdout();
    let mut stderr_handle = stderr.lock();
    let mut stdout_handle = stdout.lock();
    execute_into(
        &workspace,
        args.since.as_deref(),
        &args.to,
        args.format,
        &mut stdout_handle,
        &mut stderr_handle,
    )
}

/// IO-injected core of `execute`. Tests pass in-memory buffers so output
/// can be asserted without touching the process's real stdout/stderr.
pub fn execute_into(
    workspace: &Path,
    since: Option<&str>,
    to: &str,
    format: ChangelogFormat,
    stdout: &mut dyn std::io::Write,
    stderr: &mut dyn std::io::Write,
) -> Result<()> {
    let range = resolve_tag_range(workspace, since, to, stderr)?;
    let discovered = find_archives_in_range(workspace, &range)?;

    // Every commit that added a lane entry (change or issue), whether or
    // not the entry is later skipped — a `changelog: skip` archive still
    // means that commit is accounted for, so it is not "unattributed".
    let lane_shas: BTreeSet<String> =
        discovered.iter().map(|e| e.shipped_commit.clone()).collect();

    let mut entries: Vec<ArchiveEntry> = Vec::new();
    let mut skipped: Vec<SkippedEntry> = Vec::new();
    for raw in discovered {
        let slug = raw.slug.clone();
        let metadata = match read_archive_metadata(&raw.summary_path, raw.lane, stderr) {
            Ok(m) => m,
            Err(e) => {
                // Log but do not bail; the binary release should still
                // get a body even if one archive is malformed.
                let _ = writeln!(
                    stderr,
                    "changelog: skipping `{slug}`: failed to read {}: {e:#}",
                    raw.summary_path.display()
                );
                continue;
            }
        };
        match metadata {
            ArchiveMetadataRaw::Entry { summary } => entries.push(ArchiveEntry {
                summary,
                ..raw
            }),
            ArchiveMetadataRaw::Skip { reason } => skipped.push(SkippedEntry { slug, reason }),
        }
    }

    let unattributed = find_unattributed_commits(workspace, &range, &lane_shas)?;

    let version = range.to_label.clone();
    let output = match format {
        ChangelogFormat::Markdown => {
            render_markdown(&version, &range, &entries, &skipped, &unattributed)
        }
        ChangelogFormat::Json => {
            render_json(&version, &range, &entries, &skipped, &unattributed)?
        }
    };
    stdout
        .write_all(output.as_bytes())
        .context("writing changelog output to stdout")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tag-range resolution
// ---------------------------------------------------------------------------

/// Resolve `--since` / `--to` to concrete commit SHAs. Side-effect: may
/// emit one INFO line to `stderr` when falling back to "ever" because the
/// repo has no tags.
pub fn resolve_tag_range(
    workspace: &Path,
    since: Option<&str>,
    to: &str,
    stderr: &mut dyn std::io::Write,
) -> Result<TagRange> {
    let to_commit = rev_parse(workspace, to).with_context(|| {
        format!("resolving --to ref `{to}` in workspace {}", workspace.display())
    })?;
    let to_date = commit_date_utc(workspace, &to_commit).with_context(|| {
        format!("resolving commit date for --to ref `{to}`")
    })?;

    let (since_commit, since_label) = match since {
        Some("ever") => (None, "ever".to_string()),
        Some(tag) => {
            let sha = rev_parse(workspace, tag).with_context(|| {
                format!("--since tag `{tag}` does not resolve in workspace {}", workspace.display())
            })?;
            (Some(sha), tag.to_string())
        }
        None => match describe_most_recent_tag(workspace, &to_commit)? {
            Some(tag) => {
                let sha = rev_parse(workspace, &tag).with_context(|| {
                    format!("resolving discovered tag `{tag}`")
                })?;
                (Some(sha), tag)
            }
            None => {
                let _ = writeln!(
                    stderr,
                    "No tags found in this repo; emitting full archive history. Pass --since ever to suppress this notice."
                );
                (None, "ever (no prior tags found)".to_string())
            }
        },
    };

    // A resolved range whose `since` and `to` name the same commit is a
    // mistake (typically the same ref passed twice, or a tag that resolves
    // to `to`). An empty self-range silently misrepresents a release as
    // containing no work, so it is a hard error, not an empty document.
    if since_commit.as_deref() == Some(to_commit.as_str()) {
        bail!(
            "changelog: `--since {since_label}` and `--to {to}` resolve to the same commit \
             ({to_commit}); this empty range would document no changes. Pass an explicit `--since \
             <earlier-ref>`, or `--since ever` to document from the beginning of history."
        );
    }

    Ok(TagRange {
        since_commit,
        since_label,
        to_commit,
        to_label: to.to_string(),
        to_date,
    })
}

fn commit_date_utc(workspace: &Path, commit: &str) -> Result<String> {
    // `%cd` with `--date=format-local:%Y-%m-%d` forces UTC by passing the
    // empty TZ in the env. `TZ=UTC` is the portable incantation; git
    // honors it for all date formatting.
    let output = Command::new("git")
        .env("TZ", "UTC")
        .args([
            "show",
            "--no-patch",
            "--date=format-local:%Y-%m-%d",
            "--pretty=format:%cd",
            commit,
        ])
        .current_dir(workspace)
        .output()
        .with_context(|| format!("spawning `git show` for commit date of {commit}"))?;
    if !output.status.success() {
        let stderr_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git show for commit date failed: {stderr_text}");
    }
    let date = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if date.is_empty() {
        bail!("git show returned empty date for commit {commit}");
    }
    Ok(date)
}

fn rev_parse(workspace: &Path, refname: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", &format!("{refname}^{{commit}}")])
        .current_dir(workspace)
        .output()
        .with_context(|| format!("spawning `git rev-parse {refname}`"))?;
    if !output.status.success() {
        let stderr_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git rev-parse `{refname}` failed: {stderr_text}");
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        bail!("git rev-parse `{refname}` returned empty output");
    }
    Ok(sha)
}

/// Resolve the default `--since` tag: the most recent tag on `to_commit`'s
/// ancestry that does NOT point at `to_commit` itself. Tags naming
/// `to_commit` are passed to `git describe` as `--exclude` patterns so a
/// `--to <release-tag>` run resolves to the *previous* tag rather than the
/// empty self-range. Returns `None` when nothing remains (the caller then
/// falls back to "ever").
fn describe_most_recent_tag(workspace: &Path, to_commit: &str) -> Result<Option<String>> {
    let mut args: Vec<String> = vec![
        "describe".into(),
        "--tags".into(),
        "--abbrev=0".into(),
    ];
    for tag in tags_pointing_at(workspace, to_commit)? {
        args.push(format!("--exclude={tag}"));
    }
    args.push(to_commit.to_string());

    let output = Command::new("git")
        .args(args.iter().map(String::as_str))
        .current_dir(workspace)
        .output()
        .with_context(|| format!("spawning `git describe --tags --abbrev=0 {to_commit}`"))?;
    if output.status.success() {
        let tag = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if tag.is_empty() {
            Ok(None)
        } else {
            Ok(Some(tag))
        }
    } else {
        // `git describe` exits non-zero when no tags exist on the ref's
        // ancestry (or every candidate was excluded) — treat that as "no
        // prior tags" rather than a hard error.
        Ok(None)
    }
}

/// List the tags that point directly at `commit` (one per line).
fn tags_pointing_at(workspace: &Path, commit: &str) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["tag", "--points-at", commit])
        .current_dir(workspace)
        .output()
        .with_context(|| format!("spawning `git tag --points-at {commit}`"))?;
    if !output.status.success() {
        let stderr_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git tag --points-at `{commit}` failed: {stderr_text}");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

// ---------------------------------------------------------------------------
// Archive discovery
// ---------------------------------------------------------------------------

/// Walks `git log --diff-filter=A` to identify commits that added archive
/// directories within the range. Each top-level directory under
/// `openspec/changes/archive/` produces one `ArchiveEntry`.
///
/// The returned entries have placeholder `summary` and
/// `primary_capability` values; the caller is responsible for filling
/// those in via `read_archive_metadata` and `primary_capability`. This
/// keeps discovery cheap and testable independently of frontmatter
/// shapes.
pub fn find_archives_in_range(
    workspace: &Path,
    range: &TagRange,
) -> Result<Vec<ArchiveEntry>> {
    let change_prefix = "openspec/changes/archive/";
    let issue_prefix = "issues/archive/";
    let range_arg = match &range.since_commit {
        Some(sha) => format!("{sha}..{}", range.to_commit),
        None => range.to_commit.clone(),
    };

    let args = [
        "log".to_string(),
        "--diff-filter=A".to_string(),
        "--pretty=format:%H%x09%ad".to_string(),
        "--date=short".to_string(),
        "--reverse".to_string(),
        "--name-only".to_string(),
        range_arg,
        "--".to_string(),
        change_prefix.to_string(),
        issue_prefix.to_string(),
    ];

    let output = Command::new("git")
        .args(args.iter().map(String::as_str))
        .current_dir(workspace)
        .output()
        .with_context(|| format!("spawning `git log` in {}", workspace.display()))?;
    if !output.status.success() {
        let stderr_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git log for changelog failed: {stderr_text}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut entries: Vec<ArchiveEntry> = Vec::new();
    // Dedup key is the full lane-relative unit path, so a change directory
    // and an issue file that happen to share a name never collide.
    let mut seen_units: BTreeSet<String> = BTreeSet::new();
    let mut current_commit: Option<(String, String)> = None;

    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        if let Some((sha, date)) = parse_commit_header(line) {
            current_commit = Some((sha, date));
            continue;
        }
        let Some((sha, date)) = current_commit.as_ref() else {
            continue;
        };

        // A change: top-level directory under `openspec/changes/archive/`.
        if let Some(top_level) = top_level_archive_dir(line, change_prefix) {
            let key = format!("change:{top_level}");
            if !seen_units.insert(key) {
                continue;
            }
            let archive_dir = workspace
                .join(change_prefix.trim_end_matches('/'))
                .join(&top_level);
            let summary_path = archive_dir.join("proposal.md");
            let slug = derive_slug_from_dir_name(&top_level);
            let shipped_date = date_prefix_from_dir_name(&top_level)
                .unwrap_or_else(|| date.clone());
            entries.push(ArchiveEntry {
                slug,
                primary_capability: primary_capability(workspace, &archive_dir),
                summary_path,
                archive_dir,
                lane: Lane::Change,
                summary: String::new(),
                shipped_commit: sha.clone(),
                shipped_date,
            });
            continue;
        }

        // An issue: either `issues/archive/<slug>/…` (directory form,
        // summary from `issue.md`) or `issues/archive/<slug>.md`
        // (single-file form, summary from the file itself).
        if let Some((unit, is_dir)) = issue_unit(line, issue_prefix) {
            let key = format!("issue:{unit}");
            if !seen_units.insert(key) {
                continue;
            }
            let issue_root = workspace.join(issue_prefix.trim_end_matches('/'));
            let (archive_dir, summary_path, name_for_slug) = if is_dir {
                let dir = issue_root.join(&unit);
                let summary = dir.join("issue.md");
                (dir, summary, unit.clone())
            } else {
                // `unit` is the file name (`<slug>.md`); slug/date derive
                // from the stem.
                let file = issue_root.join(&unit);
                let stem = unit.strip_suffix(".md").unwrap_or(&unit).to_string();
                (file.clone(), file, stem)
            };
            let slug = derive_slug_from_dir_name(&name_for_slug);
            let shipped_date = date_prefix_from_dir_name(&name_for_slug)
                .unwrap_or_else(|| date.clone());
            entries.push(ArchiveEntry {
                slug,
                archive_dir,
                summary_path,
                lane: Lane::Issue,
                primary_capability: None,
                summary: String::new(),
                shipped_commit: sha.clone(),
                shipped_date,
            });
        }
    }
    Ok(entries)
}

/// Classify a path under `issues/archive/` into its top-level unit. Returns
/// `(unit_name, is_directory)`. Directory-form issues yield the directory
/// name; single-file issues yield the `<slug>.md` file name. Non-`.md`
/// files at the top level (a stray `README`, `.gitkeep`, etc.) are ignored.
fn issue_unit(path: &str, prefix: &str) -> Option<(String, bool)> {
    let rest = path.strip_prefix(prefix)?;
    if rest.is_empty() {
        return None;
    }
    match rest.split_once('/') {
        Some((top, _)) if !top.is_empty() => Some((top.to_string(), true)),
        Some(_) => None,
        None if rest.ends_with(".md") => Some((rest.to_string(), false)),
        None => None,
    }
}

// ---------------------------------------------------------------------------
// Unattributed-commit sweep
// ---------------------------------------------------------------------------

/// Report range commits (`--no-merges`) whose SHA is not among the
/// lane-adding commits. These added neither an OpenSpec archive nor an
/// issues-lane archive — direct commits that bypassed both workflows. The
/// extractor reports them; it does not classify them.
pub fn find_unattributed_commits(
    workspace: &Path,
    range: &TagRange,
    lane_shas: &BTreeSet<String>,
) -> Result<Vec<UnattributedCommit>> {
    let range_arg = match &range.since_commit {
        Some(sha) => format!("{sha}..{}", range.to_commit),
        None => range.to_commit.clone(),
    };
    let args = [
        "log",
        "--no-merges",
        "--format=%H%x09%ad%x09%s",
        "--date=short",
        "--reverse",
        &range_arg,
    ];
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .with_context(|| format!("spawning `git log --no-merges` in {}", workspace.display()))?;
    if !output.status.success() {
        let stderr_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git log (unattributed sweep) failed: {stderr_text}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut commits = Vec::new();
    for line in stdout.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(sha), Some(date), Some(subject)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if lane_shas.contains(sha) {
            continue;
        }
        commits.push(UnattributedCommit {
            sha: sha.to_string(),
            date: date.to_string(),
            subject: subject.to_string(),
        });
    }
    Ok(commits)
}

fn parse_commit_header(line: &str) -> Option<(String, String)> {
    // git log --pretty=format:%H%x09%ad --date=short emits
    // <sha>\t<YYYY-MM-DD>. Detect by tab + 40-char hex prefix.
    let (sha, date) = line.split_once('\t')?;
    if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((sha.to_string(), date.to_string()))
}

fn top_level_archive_dir(path: &str, prefix: &str) -> Option<String> {
    let rest = path.strip_prefix(prefix)?;
    // Only count it if a `/` follows (i.e. the path is within the dir,
    // not the dir name appearing as a sibling file).
    let (top, _rest_after_dir) = rest.split_once('/')?;
    if top.is_empty() {
        return None;
    }
    Some(top.to_string())
}

/// Convert a directory name like `2026-05-22-chat-request-triage` into the
/// archive's logical slug (`chat-request-triage`). If the prefix is
/// missing the date pattern, return the directory name verbatim.
fn derive_slug_from_dir_name(dir_name: &str) -> String {
    if has_date_prefix(dir_name) {
        dir_name[11..].to_string()
    } else {
        dir_name.to_string()
    }
}

/// Extract the `YYYY-MM-DD` prefix from an archive directory name, if
/// present. Returns `None` if the directory lacks the standard prefix
/// (e.g. a manually-renamed archive — callers fall back to the git
/// addition commit's date).
fn date_prefix_from_dir_name(dir_name: &str) -> Option<String> {
    if has_date_prefix(dir_name) {
        Some(dir_name[..10].to_string())
    } else {
        None
    }
}

fn has_date_prefix(dir_name: &str) -> bool {
    let bytes = dir_name.as_bytes();
    bytes.len() >= 11
        && bytes[..4].iter().all(|b| b.is_ascii_digit())
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(|b| b.is_ascii_digit())
        && bytes[10] == b'-'
}

// ---------------------------------------------------------------------------
// Frontmatter + summary extraction
// ---------------------------------------------------------------------------

/// Read a lane entry's summary file (`proposal.md` for a change,
/// `issue.md` or the single `.md` file for an issue), parse any
/// `---`-delimited frontmatter, and return either an entry summary or a
/// skip directive. Unrecognized `changelog:` values produce a WARN on
/// `stderr` and fall through to the default summary extraction. The default
/// summary is the first `## Why` paragraph for changes, or the first body
/// paragraph (after any leading heading) for issues.
pub fn read_archive_metadata(
    summary_path: &Path,
    lane: Lane,
    stderr: &mut dyn std::io::Write,
) -> Result<ArchiveMetadataRaw> {
    let raw = std::fs::read_to_string(summary_path)
        .with_context(|| format!("reading {}", summary_path.display()))?;

    let (frontmatter, body) = split_frontmatter(&raw);

    if let Some(fm) = frontmatter {
        match interpret_frontmatter(&fm, stderr, summary_path)? {
            FrontmatterDirective::Skip(reason) => {
                return Ok(ArchiveMetadataRaw::Skip { reason });
            }
            FrontmatterDirective::Summary(text) => {
                return Ok(ArchiveMetadataRaw::Entry { summary: text });
            }
            FrontmatterDirective::Default => {}
        }
    }

    let summary = match lane {
        Lane::Change => extract_why_paragraph(body, summary_path, stderr),
        // Issues have no `## Why` convention; the first body paragraph
        // (skipping any leading heading) is the closest equivalent.
        Lane::Issue => first_body_paragraph(body),
    };
    Ok(ArchiveMetadataRaw::Entry { summary })
}

enum FrontmatterDirective {
    Skip(String),
    Summary(String),
    Default,
}

fn split_frontmatter(raw: &str) -> (Option<String>, &str) {
    // YAML frontmatter must be the very first bytes of the file: `---\n`,
    // contents, then a closing `---\n`. Anything else is treated as no
    // frontmatter.
    let mut rest = raw;
    if let Some(stripped) = rest.strip_prefix("---\n") {
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix("---\r\n") {
        rest = stripped;
    } else {
        return (None, raw);
    }
    // Find the closing `---` on its own line.
    let mut split_at: Option<usize> = None;
    let mut search_from = 0usize;
    while let Some(rel) = rest[search_from..].find("---") {
        let abs = search_from + rel;
        let at_line_start = abs == 0 || rest.as_bytes()[abs - 1] == b'\n';
        let after = &rest[abs + 3..];
        let ends_line = after.is_empty() || after.starts_with('\n') || after.starts_with("\r\n");
        if at_line_start && ends_line {
            split_at = Some(abs);
            break;
        }
        search_from = abs + 3;
    }
    let Some(end) = split_at else {
        return (None, raw);
    };
    let frontmatter = rest[..end].to_string();
    let after_marker = &rest[end + 3..];
    let body = after_marker.strip_prefix("\r\n").unwrap_or_else(|| {
        after_marker.strip_prefix('\n').unwrap_or(after_marker)
    });
    (Some(frontmatter), body)
}

fn interpret_frontmatter(
    fm_yaml: &str,
    stderr: &mut dyn std::io::Write,
    location: &Path,
) -> Result<FrontmatterDirective> {
    let parsed: serde_yml::Value = match serde_yml::from_str(fm_yaml) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(
                stderr,
                "changelog: WARN: frontmatter in {} failed to parse as YAML: {e}; falling back to default summary",
                location.display()
            );
            return Ok(FrontmatterDirective::Default);
        }
    };
    let mapping = match parsed {
        serde_yml::Value::Mapping(m) => m,
        _ => return Ok(FrontmatterDirective::Default),
    };
    let value = match mapping.get(serde_yml::Value::String("changelog".to_string())) {
        Some(v) => v,
        None => return Ok(FrontmatterDirective::Default),
    };
    match value {
        serde_yml::Value::String(s) => {
            let lower = s.trim().to_ascii_lowercase();
            if matches!(lower.as_str(), "skip" | "internal" | "hidden") {
                Ok(FrontmatterDirective::Skip(format!("changelog: {lower}")))
            } else {
                let _ = writeln!(
                    stderr,
                    "changelog: WARN: unrecognized `changelog: {s}` value in {}; falling back to default summary",
                    location.display()
                );
                Ok(FrontmatterDirective::Default)
            }
        }
        serde_yml::Value::Mapping(inner) => {
            if let Some(serde_yml::Value::String(text)) =
                inner.get(serde_yml::Value::String("summary".to_string()))
            {
                Ok(FrontmatterDirective::Summary(text.clone()))
            } else {
                let _ = writeln!(
                    stderr,
                    "changelog: WARN: `changelog:` mapping in {} lacks a recognized field; falling back to default summary",
                    location.display()
                );
                Ok(FrontmatterDirective::Default)
            }
        }
        other => {
            let _ = writeln!(
                stderr,
                "changelog: WARN: unrecognized `changelog:` value type {:?} in {}; falling back to default summary",
                other, location.display()
            );
            Ok(FrontmatterDirective::Default)
        }
    }
}

fn extract_why_paragraph(
    body: &str,
    archive_dir: &Path,
    stderr: &mut dyn std::io::Write,
) -> String {
    // Scan for the literal line `## Why`. If absent, take the first
    // paragraph of the body and WARN.
    let mut lines = body.lines();
    let mut found = false;
    for line in lines.by_ref() {
        if line.trim() == "## Why" {
            found = true;
            break;
        }
    }
    if !found {
        let _ = writeln!(
            stderr,
            "changelog: WARN: no `## Why` heading in {}; using leading paragraph",
            archive_dir.display()
        );
        return first_paragraph(body);
    }
    // Skip blank lines.
    let mut buffer: Vec<&str> = Vec::new();
    let mut started = false;
    for line in lines {
        if !started && line.trim().is_empty() {
            continue;
        }
        if started && line.trim().is_empty() {
            break;
        }
        if line.starts_with("## ") {
            // Reached the next heading without finding a paragraph; bail.
            break;
        }
        started = true;
        buffer.push(line);
    }
    buffer.join("\n").trim_end().to_string()
}

fn first_paragraph(body: &str) -> String {
    let mut buffer: Vec<&str> = Vec::new();
    let mut started = false;
    for line in body.lines() {
        if !started && line.trim().is_empty() {
            continue;
        }
        if started && line.trim().is_empty() {
            break;
        }
        started = true;
        buffer.push(line);
    }
    buffer.join("\n").trim_end().to_string()
}

/// First prose paragraph of an issue body: skip leading blank AND heading
/// (`#`-prefixed) lines — an issue opens with a `# title` and often a `##
/// Issue`/`## Problem` heading — then collect until the next blank line.
fn first_body_paragraph(body: &str) -> String {
    let mut buffer: Vec<&str> = Vec::new();
    let mut started = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if !started && (trimmed.is_empty() || trimmed.starts_with('#')) {
            continue;
        }
        if started && trimmed.is_empty() {
            break;
        }
        started = true;
        buffer.push(line);
    }
    buffer.join("\n").trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Primary-capability detection
// ---------------------------------------------------------------------------

/// Returns the alphabetically-first capability directory under
/// `<archive_dir>/specs/`. Returns `None` for docs-only archives whose
/// `specs/` directory is missing or empty.
pub fn primary_capability(_workspace: &Path, archive_dir: &Path) -> Option<String> {
    let specs_dir = archive_dir.join("specs");
    let read = std::fs::read_dir(&specs_dir).ok()?;
    let mut names: Vec<String> = Vec::new();
    for entry in read.flatten() {
        if let Ok(ft) = entry.file_type()
            && ft.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_string());
        }
    }
    if names.is_empty() {
        return None;
    }
    names.sort();
    names.into_iter().next()
}

// ---------------------------------------------------------------------------
// Renderers
// ---------------------------------------------------------------------------

/// Render the markdown changelog. The header uses the `--to` commit date
/// in UTC (already populated on each entry; for the header we use the
/// most recent shipped_date, or fall back to today if there are no
/// entries — operators get a usable doc either way).
pub fn render_markdown(
    version: &str,
    range: &TagRange,
    entries: &[ArchiveEntry],
    skipped: &[SkippedEntry],
    unattributed: &[UnattributedCommit],
) -> String {
    let mut out = String::new();
    if range.to_date.is_empty() {
        out.push_str(&format!("## {version}\n\n"));
    } else {
        out.push_str(&format!("## {version} — {}\n\n", range.to_date));
    }

    if entries.is_empty() && skipped.is_empty() && unattributed.is_empty() {
        out.push_str(&format!(
            "_No archived changes between `{}` and `{}`._\n",
            range.since_label, range.to_label
        ));
        return out;
    }

    // Change entries group by capability; issue entries collect under a
    // single `Fixes` group (issues have no owning capability).
    let mut grouped: BTreeMap<String, Vec<&ArchiveEntry>> = BTreeMap::new();
    let mut fixes: Vec<&ArchiveEntry> = Vec::new();
    for entry in entries {
        match entry.lane {
            Lane::Change => {
                let cap = entry
                    .primary_capability
                    .clone()
                    .unwrap_or_else(|| "Other".to_string());
                grouped.entry(cap).or_default().push(entry);
            }
            Lane::Issue => fixes.push(entry),
        }
    }

    // Entries within a capability sort by shipped_commit order (insertion
    // order from git log --reverse).
    for (capability, capability_entries) in &grouped {
        out.push_str(&format!("### {capability}\n"));
        for entry in capability_entries {
            out.push_str(&format_entry_bullet(entry));
        }
        out.push('\n');
    }

    if !fixes.is_empty() {
        out.push_str("### Fixes\n");
        for entry in &fixes {
            out.push_str(&format_entry_bullet(entry));
        }
        out.push('\n');
    }

    if !skipped.is_empty() {
        out.push_str("### Skipped\n");
        for s in skipped {
            out.push_str(&format!("- `{}` — {}\n", s.slug, s.reason));
        }
        out.push('\n');
    }

    if !unattributed.is_empty() {
        out.push_str("### Other changes\n");
        for c in unattributed {
            out.push_str(&format!("- {} {}\n", short_sha(&c.sha), c.subject));
        }
        out.push('\n');
    }

    out
}

/// First 7 chars of a commit SHA (the conventional short form), tolerant of
/// shorter inputs.
fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

fn format_entry_bullet(entry: &ArchiveEntry) -> String {
    let summary = entry.summary.trim();
    let (head, rest) = match summary.find('\n') {
        Some(idx) => (&summary[..idx], summary[idx + 1..].trim()),
        None => (summary, ""),
    };
    if rest.is_empty() {
        format!("- **{head}** ({})\n", entry.slug)
    } else {
        let flattened = rest
            .lines()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        format!("- **{head}** ({}) — {}\n", entry.slug, flattened)
    }
}

/// Render the JSON shape documented in the spec. Pretty-printed (2-space
/// indent) for both human readability AND scripting.
pub fn render_json(
    version: &str,
    range: &TagRange,
    entries: &[ArchiveEntry],
    skipped: &[SkippedEntry],
    unattributed: &[UnattributedCommit],
) -> Result<String> {
    let entries_json: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "slug": e.slug,
                "archive_dir": e.archive_dir.display().to_string(),
                "primary_capability": e.primary_capability,
                "summary": e.summary,
                "shipped_commit": e.shipped_commit,
                "shipped_date": e.shipped_date,
                "lane": e.lane.as_str(),
            })
        })
        .collect();
    let skipped_json: Vec<serde_json::Value> = skipped
        .iter()
        .map(|s| serde_json::json!({"slug": s.slug, "reason": s.reason}))
        .collect();
    let unattributed_json: Vec<serde_json::Value> = unattributed
        .iter()
        .map(|c| serde_json::json!({"sha": c.sha, "date": c.date, "subject": c.subject}))
        .collect();
    let doc = serde_json::json!({
        "version": version,
        "date": range.to_date,
        "since": range.since_label,
        "to": range.to_label,
        "entries": entries_json,
        "skipped": skipped_json,
        "unattributed_commits": unattributed_json,
    });
    let mut text = serde_json::to_string_pretty(&doc)
        .map_err(|e| anyhow!("serializing changelog JSON: {e}"))?;
    text.push('\n');
    Ok(text)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    /// Set up a temporary git repo with the user identity required for
    /// `git commit` to succeed inside CI sandboxes.
    fn init_repo(dir: &Path) {
        run(dir, &["init", "--initial-branch=main"]);
        run(dir, &["config", "user.email", "test@example.com"]);
        run(dir, &["config", "user.name", "test"]);
        run(dir, &["config", "commit.gpgsign", "false"]);
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git invocation");
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: stderr={}, stdout={}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout),
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit_all(dir: &Path, message: &str) -> String {
        run(dir, &["add", "-A"]);
        run(dir, &["commit", "-m", message]);
        run(dir, &["rev-parse", "HEAD"])
    }

    fn write_file(dir: &Path, relative: &str, contents: &str) {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("creating parent");
        }
        fs::write(path, contents).expect("writing file");
    }

    fn write_archive(
        workspace: &Path,
        date_prefix: &str,
        slug: &str,
        proposal_md: &str,
        capabilities: &[&str],
    ) -> String {
        let dir_name = format!("{date_prefix}-{slug}");
        let archive_dir = workspace
            .join("openspec/changes/archive")
            .join(&dir_name);
        fs::create_dir_all(&archive_dir).unwrap();
        write_file(
            &archive_dir,
            "proposal.md",
            proposal_md,
        );
        for cap in capabilities {
            fs::create_dir_all(archive_dir.join("specs").join(cap)).unwrap();
            write_file(
                &archive_dir.join("specs").join(cap),
                "spec.md",
                "## ADDED Requirements\n\n### Requirement: stub\nstub.\n",
            );
        }
        dir_name
    }

    /// Write a directory-form archived issue under `issues/archive/`
    /// (`<date>-<slug>/issue.md`). Returns the directory name.
    fn write_issue_dir(
        workspace: &Path,
        date_prefix: &str,
        slug: &str,
        issue_md: &str,
    ) -> String {
        let dir_name = format!("{date_prefix}-{slug}");
        let issue_dir = workspace.join("issues/archive").join(&dir_name);
        fs::create_dir_all(&issue_dir).unwrap();
        write_file(&issue_dir, "issue.md", issue_md);
        write_file(&issue_dir, "tasks.md", "- [x] done\n");
        dir_name
    }

    // -----------------------------------------------------------------
    // resolve_tag_range
    // -----------------------------------------------------------------

    #[test]
    fn resolve_tag_range_with_prior_tag_defaults_to_that_tag() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "a.txt", "1");
        commit_all(ws, "first");
        run(ws, &["tag", "v0.1.0"]);
        write_file(ws, "b.txt", "2");
        let head = commit_all(ws, "second");

        let mut stderr = Vec::new();
        let range = resolve_tag_range(ws, None, "HEAD", &mut stderr).unwrap();
        assert_eq!(range.since_label, "v0.1.0");
        assert!(range.since_commit.is_some());
        assert_eq!(range.to_commit, head);
        assert!(
            stderr.is_empty(),
            "should not emit INFO line when a tag is found"
        );
    }

    #[test]
    fn resolve_tag_range_no_tags_falls_back_to_ever_with_info_line() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "a.txt", "1");
        commit_all(ws, "first");

        let mut stderr = Vec::new();
        let range = resolve_tag_range(ws, None, "HEAD", &mut stderr).unwrap();
        assert!(range.since_commit.is_none());
        assert!(range.since_label.starts_with("ever"));
        let stderr_text = String::from_utf8(stderr).unwrap();
        assert!(
            stderr_text.contains("No tags found in this repo"),
            "expected INFO line, got: {stderr_text}"
        );
        assert!(stderr_text.contains("--since ever to suppress"));
    }

    #[test]
    fn resolve_tag_range_explicit_ever_emits_no_info_line() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "a.txt", "1");
        commit_all(ws, "first");

        let mut stderr = Vec::new();
        let range = resolve_tag_range(ws, Some("ever"), "HEAD", &mut stderr).unwrap();
        assert!(range.since_commit.is_none());
        assert_eq!(range.since_label, "ever");
        assert!(
            String::from_utf8(stderr).unwrap().is_empty(),
            "explicit --since ever should not emit the INFO line"
        );
    }

    #[test]
    fn resolve_tag_range_explicit_tag_resolves_to_its_commit() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "a.txt", "1");
        let first = commit_all(ws, "first");
        run(ws, &["tag", "v0.1.0"]);
        write_file(ws, "b.txt", "2");
        commit_all(ws, "second");

        let mut stderr = Vec::new();
        let range = resolve_tag_range(ws, Some("v0.1.0"), "HEAD", &mut stderr).unwrap();
        assert_eq!(range.since_commit.as_deref(), Some(first.as_str()));
        assert_eq!(range.since_label, "v0.1.0");
    }

    #[test]
    fn resolve_tag_range_missing_tag_errors() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "a.txt", "1");
        commit_all(ws, "first");

        let mut stderr = Vec::new();
        let err = resolve_tag_range(ws, Some("v99.0.0"), "HEAD", &mut stderr).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("v99.0.0"),
            "error must name the missing tag; got: {msg}"
        );
    }

    // -----------------------------------------------------------------
    // find_archives_in_range
    // -----------------------------------------------------------------

    #[test]
    fn find_archives_detects_three_entries_in_three_commits() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");

        write_archive(
            ws,
            "2026-05-01",
            "alpha",
            "## Why\n\nAlpha rationale.\n",
            &["chatops-manager"],
        );
        commit_all(ws, "ship alpha");
        write_archive(
            ws,
            "2026-05-02",
            "beta",
            "## Why\n\nBeta rationale.\n",
            &["orchestrator-cli"],
        );
        commit_all(ws, "ship beta");
        write_archive(
            ws,
            "2026-05-03",
            "gamma",
            "## Why\n\nGamma rationale.\n",
            &["executor"],
        );
        commit_all(ws, "ship gamma");

        let mut stderr = Vec::new();
        let range = resolve_tag_range(ws, Some("ever"), "HEAD", &mut stderr).unwrap();
        let entries = find_archives_in_range(ws, &range).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].slug, "alpha");
        assert_eq!(entries[1].slug, "beta");
        assert_eq!(entries[2].slug, "gamma");
        assert_eq!(entries[0].primary_capability.as_deref(), Some("chatops-manager"));
        assert_eq!(entries[2].primary_capability.as_deref(), Some("executor"));
    }

    #[test]
    fn find_archives_groups_two_entries_in_one_commit() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");

        write_archive(
            ws,
            "2026-05-10",
            "first",
            "## Why\n\nFirst rationale.\n",
            &["chatops-manager"],
        );
        write_archive(
            ws,
            "2026-05-10",
            "second",
            "## Why\n\nSecond rationale.\n",
            &["orchestrator-cli"],
        );
        let sha = commit_all(ws, "bundle two archives");

        let mut stderr = Vec::new();
        let range = resolve_tag_range(ws, Some("ever"), "HEAD", &mut stderr).unwrap();
        let entries = find_archives_in_range(ws, &range).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].shipped_commit, sha);
        assert_eq!(entries[1].shipped_commit, sha);
    }

    #[test]
    fn find_archives_excludes_entries_before_since() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        write_archive(
            ws,
            "2026-05-01",
            "early",
            "## Why\n\nEarly rationale.\n",
            &["x"],
        );
        commit_all(ws, "ship early");
        run(ws, &["tag", "v0.1.0"]);
        write_archive(
            ws,
            "2026-05-02",
            "late",
            "## Why\n\nLate rationale.\n",
            &["y"],
        );
        commit_all(ws, "ship late");

        let mut stderr = Vec::new();
        let range = resolve_tag_range(ws, None, "HEAD", &mut stderr).unwrap();
        let entries = find_archives_in_range(ws, &range).unwrap();
        let slugs: Vec<_> = entries.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, vec!["late"], "early archive must be excluded");
    }

    // -----------------------------------------------------------------
    // read_archive_metadata
    // -----------------------------------------------------------------

    #[test]
    fn metadata_default_extracts_first_why_paragraph() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let name = write_archive(
            ws,
            "2026-05-01",
            "noprefix",
            "## Why\n\nFirst paragraph here.\n\nSecond paragraph.\n\n## What Changes\n\nDetail.\n",
            &["cap-a"],
        );
        let archive = ws.join("openspec/changes/archive").join(&name);
        let mut stderr = Vec::new();
        let meta =
            read_archive_metadata(&archive.join("proposal.md"), Lane::Change, &mut stderr).unwrap();
        match meta {
            ArchiveMetadataRaw::Entry { summary } => {
                assert_eq!(summary, "First paragraph here.");
            }
            other => panic!("expected entry, got {other:?}"),
        }
    }

    #[test]
    fn metadata_frontmatter_skip_yields_skip() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let proposal = "---\nchangelog: skip\n---\n## Why\n\nUnused.\n";
        let name = write_archive(ws, "2026-05-01", "skipme", proposal, &["cap"]);
        let archive = ws.join("openspec/changes/archive").join(&name);
        let mut stderr = Vec::new();
        let meta =
            read_archive_metadata(&archive.join("proposal.md"), Lane::Change, &mut stderr).unwrap();
        assert!(matches!(meta, ArchiveMetadataRaw::Skip { .. }));
    }

    #[test]
    fn metadata_frontmatter_summary_override_replaces_why() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let proposal = "---\nchangelog:\n  summary: \"Adds /healthz endpoint for liveness probes\"\n---\n## Why\n\nLong winded rationale.\n";
        let name = write_archive(ws, "2026-05-01", "healthz", proposal, &["cap"]);
        let archive = ws.join("openspec/changes/archive").join(&name);
        let mut stderr = Vec::new();
        let meta =
            read_archive_metadata(&archive.join("proposal.md"), Lane::Change, &mut stderr).unwrap();
        match meta {
            ArchiveMetadataRaw::Entry { summary } => {
                assert_eq!(summary, "Adds /healthz endpoint for liveness probes");
            }
            other => panic!("expected override entry, got {other:?}"),
        }
    }

    #[test]
    fn metadata_unrecognized_changelog_value_warns_and_uses_default() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let proposal = "---\nchangelog: bogus-value\n---\n## Why\n\nDefault wins.\n";
        let name = write_archive(ws, "2026-05-01", "bogus", proposal, &["cap"]);
        let archive = ws.join("openspec/changes/archive").join(&name);
        let mut stderr = Vec::new();
        let meta =
            read_archive_metadata(&archive.join("proposal.md"), Lane::Change, &mut stderr).unwrap();
        match meta {
            ArchiveMetadataRaw::Entry { summary } => assert_eq!(summary, "Default wins."),
            other => panic!("expected default entry, got {other:?}"),
        }
        let stderr_text = String::from_utf8(stderr).unwrap();
        assert!(
            stderr_text.contains("bogus-value"),
            "stderr should name the unrecognized value; got: {stderr_text}"
        );
    }

    #[test]
    fn metadata_internal_and_hidden_are_synonyms_for_skip() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        for value in ["internal", "hidden"] {
            let proposal = format!("---\nchangelog: {value}\n---\n## Why\n\nx.\n");
            let name = write_archive(ws, "2026-05-01", value, &proposal, &["cap"]);
            let archive = ws.join("openspec/changes/archive").join(&name);
            let mut stderr = Vec::new();
            let meta =
            read_archive_metadata(&archive.join("proposal.md"), Lane::Change, &mut stderr).unwrap();
            assert!(matches!(meta, ArchiveMetadataRaw::Skip { .. }), "{value}");
        }
    }

    #[test]
    fn metadata_no_why_heading_falls_back_to_leading_paragraph_with_warn() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let proposal = "Leading paragraph here.\n\nSecond paragraph.\n";
        let name = write_archive(ws, "2026-05-01", "nowhy", proposal, &["cap"]);
        let archive = ws.join("openspec/changes/archive").join(&name);
        let mut stderr = Vec::new();
        let meta =
            read_archive_metadata(&archive.join("proposal.md"), Lane::Change, &mut stderr).unwrap();
        match meta {
            ArchiveMetadataRaw::Entry { summary } => assert_eq!(summary, "Leading paragraph here."),
            other => panic!("expected default entry, got {other:?}"),
        }
        let stderr_text = String::from_utf8(stderr).unwrap();
        assert!(stderr_text.contains("no `## Why`"));
    }

    // -----------------------------------------------------------------
    // primary_capability
    // -----------------------------------------------------------------

    #[test]
    fn primary_capability_returns_alphabetically_first() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let name = write_archive(
            ws,
            "2026-05-01",
            "multi",
            "## Why\n\nx.\n",
            &["orchestrator-cli", "chatops-manager"],
        );
        let archive = ws.join("openspec/changes/archive").join(&name);
        assert_eq!(
            primary_capability(ws, &archive),
            Some("chatops-manager".to_string())
        );
    }

    #[test]
    fn primary_capability_returns_none_for_archive_without_specs() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let archive_dir = ws.join("openspec/changes/archive/2026-05-01-docs-only");
        fs::create_dir_all(&archive_dir).unwrap();
        write_file(&archive_dir, "proposal.md", "## Why\n\ndocs.\n");
        assert!(primary_capability(ws, &archive_dir).is_none());
    }

    // -----------------------------------------------------------------
    // renderers
    // -----------------------------------------------------------------

    fn fixture_entries() -> Vec<ArchiveEntry> {
        vec![
            ArchiveEntry {
                slug: "alpha".to_string(),
                archive_dir: PathBuf::from("openspec/changes/archive/2026-05-01-alpha"),
                summary_path: PathBuf::from(
                    "openspec/changes/archive/2026-05-01-alpha/proposal.md",
                ),
                lane: Lane::Change,
                primary_capability: Some("chatops-manager".to_string()),
                summary: "Alpha headline.\nAlpha tail.".to_string(),
                shipped_commit: "aaaaaaa".to_string(),
                shipped_date: "2026-05-01".to_string(),
            },
            ArchiveEntry {
                slug: "beta".to_string(),
                archive_dir: PathBuf::from("openspec/changes/archive/2026-05-02-beta"),
                summary_path: PathBuf::from(
                    "openspec/changes/archive/2026-05-02-beta/proposal.md",
                ),
                lane: Lane::Change,
                primary_capability: Some("orchestrator-cli".to_string()),
                summary: "Beta one-liner.".to_string(),
                shipped_commit: "bbbbbbb".to_string(),
                shipped_date: "2026-05-02".to_string(),
            },
        ]
    }

    fn fixture_range() -> TagRange {
        TagRange {
            since_commit: Some("a".repeat(40)),
            since_label: "v0.1.0".to_string(),
            to_commit: "b".repeat(40),
            to_label: "HEAD".to_string(),
            to_date: "2026-05-02".to_string(),
        }
    }

    #[test]
    fn render_markdown_matches_expected_text() {
        let entries = fixture_entries();
        let range = fixture_range();
        let out = render_markdown("v0.2.0", &range, &entries, &[], &[]);
        let expected = "## v0.2.0 — 2026-05-02\n\n### chatops-manager\n- **Alpha headline.** (alpha) — Alpha tail.\n\n### orchestrator-cli\n- **Beta one-liner.** (beta)\n\n";
        assert_eq!(out, expected, "got: {out}");
    }

    #[test]
    fn render_markdown_lists_skipped_when_present() {
        let entries = fixture_entries();
        let range = fixture_range();
        let skipped = vec![SkippedEntry {
            slug: "internal-thing".to_string(),
            reason: "changelog: skip".to_string(),
        }];
        let out = render_markdown("v0.2.0", &range, &entries, &skipped, &[]);
        assert!(out.contains("### Skipped\n- `internal-thing` — changelog: skip"));
    }

    #[test]
    fn render_markdown_empty_range_emits_placeholder() {
        let range = fixture_range();
        let out = render_markdown("v0.2.0", &range, &[], &[], &[]);
        assert!(out.contains("No archived changes between"));
    }

    #[test]
    fn render_json_parses_and_has_expected_top_level_fields() {
        let entries = fixture_entries();
        let range = fixture_range();
        let out = render_json("v0.2.0", &range, &entries, &[], &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["version"], "v0.2.0");
        assert_eq!(parsed["date"], "2026-05-02");
        assert_eq!(parsed["since"], "v0.1.0");
        assert_eq!(parsed["to"], "HEAD");
        assert_eq!(parsed["entries"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["skipped"].as_array().unwrap().len(), 0);
        assert_eq!(parsed["unattributed_commits"].as_array().unwrap().len(), 0);
        let entry0 = &parsed["entries"][0];
        assert_eq!(entry0["slug"], "alpha");
        assert_eq!(entry0["primary_capability"], "chatops-manager");
        assert_eq!(entry0["shipped_date"], "2026-05-01");
        assert_eq!(entry0["lane"], "change");
        assert!(out.contains("  "), "pretty-printed JSON should contain 2-space indents");
    }

    // -----------------------------------------------------------------
    // execute_into integration
    // -----------------------------------------------------------------

    #[test]
    fn execute_integration_three_archives_two_tags_one_skip_one_override() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        run(ws, &["tag", "v0.1.0"]);

        write_archive(
            ws,
            "2026-05-10",
            "default-one",
            "## Why\n\nDefault summary line.\n",
            &["chatops-manager"],
        );
        commit_all(ws, "ship default-one");

        write_archive(
            ws,
            "2026-05-11",
            "skipped-one",
            "---\nchangelog: skip\n---\n## Why\n\nUnused.\n",
            &["chatops-manager"],
        );
        commit_all(ws, "ship skipped-one");

        write_archive(
            ws,
            "2026-05-12",
            "override-one",
            "---\nchangelog:\n  summary: \"Manual summary wins\"\n---\n## Why\n\nIgnored body.\n",
            &["orchestrator-cli"],
        );
        commit_all(ws, "ship override-one");
        run(ws, &["tag", "v0.2.0"]);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        execute_into(
            ws,
            Some("v0.1.0"),
            "v0.2.0",
            ChangelogFormat::Markdown,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        let out = String::from_utf8(stdout).unwrap();
        // The header date is the --to commit's UTC date — that's the
        // day this test runs, not a date we control. Just confirm the
        // header starts with the version + an em-dash + an ISO date.
        let header_line = out.lines().next().unwrap();
        assert!(
            header_line.starts_with("## v0.2.0 — ") && header_line.len() >= "## v0.2.0 — 2026-01-01".len(),
            "expected `## v0.2.0 — YYYY-MM-DD`; got: {header_line}"
        );
        assert!(out.contains("### chatops-manager"), "got: {out}");
        assert!(out.contains("**Default summary line.** (default-one)"), "got: {out}");
        assert!(out.contains("### orchestrator-cli"), "got: {out}");
        assert!(out.contains("**Manual summary wins** (override-one)"), "got: {out}");
        assert!(!out.contains("skipped-one\n") || out.contains("### Skipped"), "got: {out}");
        assert!(
            out.contains("### Skipped\n- `skipped-one` — changelog: skip"),
            "got: {out}"
        );
    }

    #[test]
    fn execute_integration_json_format_round_trips() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        write_archive(
            ws,
            "2026-05-20",
            "only",
            "## Why\n\nThe only entry.\n",
            &["cap-a"],
        );
        commit_all(ws, "ship only");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        execute_into(
            ws,
            Some("ever"),
            "HEAD",
            ChangelogFormat::Json,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        let out = String::from_utf8(stdout).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["since"], "ever");
        assert_eq!(parsed["entries"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["entries"][0]["slug"], "only");
        assert_eq!(parsed["entries"][0]["summary"], "The only entry.");
    }

    // -----------------------------------------------------------------
    // 1.3 — tag-range resolution edge cases
    // -----------------------------------------------------------------

    /// `--to <tag>` with no `--since` documents the range ending at that
    /// tag (the tag pointing at `to` is excluded from default resolution),
    /// and the output is byte-identical to the explicit previous-tag run.
    #[test]
    fn to_tag_without_since_equals_explicit_previous_tag_run() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        run(ws, &["tag", "v1.0.0"]);
        write_archive(ws, "2026-06-01", "in-window", "## Why\n\nShipped work.\n", &["cap-a"]);
        commit_all(ws, "ship in-window");
        run(ws, &["tag", "v1.1.0"]);

        // Default `--since` with `--to v1.1.0`.
        let mut defaulted = Vec::new();
        let mut stderr_a = Vec::new();
        execute_into(
            ws,
            None,
            "v1.1.0",
            ChangelogFormat::Markdown,
            &mut defaulted,
            &mut stderr_a,
        )
        .unwrap();

        // Explicit `--since v1.0.0 --to v1.1.0`.
        let mut explicit = Vec::new();
        let mut stderr_b = Vec::new();
        execute_into(
            ws,
            Some("v1.0.0"),
            "v1.1.0",
            ChangelogFormat::Markdown,
            &mut explicit,
            &mut stderr_b,
        )
        .unwrap();

        let defaulted_s = String::from_utf8(defaulted).unwrap();
        assert!(defaulted_s.contains("**Shipped work.** (in-window)"), "got: {defaulted_s}");
        // Default resolution must NOT resolve the empty `(v1.1.0 .. v1.1.0]`.
        assert!(!defaulted_s.contains("No archived changes"), "got: {defaulted_s}");
        // Byte-identical to the explicit previous-tag run.
        assert_eq!(defaulted_s, String::from_utf8(explicit).unwrap());
        assert!(String::from_utf8(stderr_a).unwrap().is_empty());
    }

    /// An explicitly equal range (`--since v1.1.0 --to v1.1.0`) is a hard
    /// error naming both refs and the shared commit, never an empty doc.
    #[test]
    fn equal_explicit_range_errors_naming_both_refs() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        let head = commit_all(ws, "seed");
        run(ws, &["tag", "v1.1.0"]);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = execute_into(
            ws,
            Some("v1.1.0"),
            "v1.1.0",
            ChangelogFormat::Markdown,
            &mut stdout,
            &mut stderr,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("v1.1.0"), "must name the refs; got: {msg}");
        assert!(msg.contains(&head), "must name the shared commit; got: {msg}");
        assert!(msg.contains("--since"), "must suggest --since; got: {msg}");
        assert!(stdout.is_empty(), "no document on the error path");
    }

    /// A single-tag repo with `--to <that-tag>` falls back to "ever" (the
    /// only tag points at `to` and is excluded) with the INFO notice.
    #[test]
    fn single_tag_to_that_tag_falls_back_to_ever_with_notice() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        write_archive(ws, "2026-06-01", "only", "## Why\n\nThe work.\n", &["cap-a"]);
        commit_all(ws, "ship only");
        run(ws, &["tag", "v1.0.0"]);

        let mut stderr = Vec::new();
        let range = resolve_tag_range(ws, None, "v1.0.0", &mut stderr).unwrap();
        assert!(range.since_commit.is_none(), "must fall back to ever");
        assert!(range.since_label.starts_with("ever"));
        assert!(
            String::from_utf8(stderr).unwrap().contains("No tags found in this repo"),
            "expected the fallback notice"
        );
    }

    // -----------------------------------------------------------------
    // 2.4 — issues-lane harvesting
    // -----------------------------------------------------------------

    /// An archived issue appears under `### Fixes` with the first body
    /// paragraph of its `issue.md` as the summary, and carries
    /// `lane: "issue"` in JSON.
    #[test]
    fn archived_issue_appears_under_fixes() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        write_issue_dir(
            ws,
            "2026-06-10",
            "fix-timezone-drift",
            "# fix-timezone-drift\n\n## Issue\n\nDates rendered in local time instead of UTC.\n\nMore detail.\n",
        );
        commit_all(ws, "archive issue");

        // Markdown: under Fixes with the right summary.
        let mut md = Vec::new();
        let mut stderr = Vec::new();
        execute_into(ws, Some("ever"), "HEAD", ChangelogFormat::Markdown, &mut md, &mut stderr)
            .unwrap();
        let md_s = String::from_utf8(md).unwrap();
        assert!(md_s.contains("### Fixes"), "got: {md_s}");
        assert!(
            md_s.contains("**Dates rendered in local time instead of UTC.** (fix-timezone-drift)"),
            "got: {md_s}"
        );

        // JSON: entry carries lane == "issue".
        let mut js = Vec::new();
        let mut stderr2 = Vec::new();
        execute_into(ws, Some("ever"), "HEAD", ChangelogFormat::Json, &mut js, &mut stderr2)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&String::from_utf8(js).unwrap()).unwrap();
        let entry = &parsed["entries"][0];
        assert_eq!(entry["slug"], "fix-timezone-drift");
        assert_eq!(entry["lane"], "issue");
        assert_eq!(entry["summary"], "Dates rendered in local time instead of UTC.");
    }

    /// A single-file issue (`issues/archive/<slug>.md`) with
    /// `changelog: skip` frontmatter lands in `skipped`, not `entries`.
    #[test]
    fn issue_changelog_skip_lands_in_skipped() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        write_file(
            ws,
            "issues/archive/2026-06-11-internal-refactor.md",
            "---\nchangelog: skip\n---\n# internal-refactor\n\n## Problem\n\nInternal only.\n",
        );
        commit_all(ws, "archive internal issue");

        let mut js = Vec::new();
        let mut stderr = Vec::new();
        execute_into(ws, Some("ever"), "HEAD", ChangelogFormat::Json, &mut js, &mut stderr)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&String::from_utf8(js).unwrap()).unwrap();
        assert_eq!(parsed["entries"].as_array().unwrap().len(), 0, "must not appear in entries");
        let skipped = parsed["skipped"].as_array().unwrap();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0]["slug"], "internal-refactor");
        assert_eq!(skipped[0]["reason"], "changelog: skip");
    }

    /// A workspace with no `issues/archive/` behaves exactly as before:
    /// only change entries, no Fixes group, no error.
    #[test]
    fn workspace_without_issues_archive_behaves_as_today() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        write_archive(ws, "2026-06-01", "a-change", "## Why\n\nA change.\n", &["cap-a"]);
        commit_all(ws, "ship change");

        let mut md = Vec::new();
        let mut stderr = Vec::new();
        execute_into(ws, Some("ever"), "HEAD", ChangelogFormat::Markdown, &mut md, &mut stderr)
            .unwrap();
        let md_s = String::from_utf8(md).unwrap();
        assert!(md_s.contains("**A change.** (a-change)"), "got: {md_s}");
        assert!(!md_s.contains("### Fixes"), "no Fixes group without issues; got: {md_s}");
    }

    // -----------------------------------------------------------------
    // 3.2 — unattributed-commit sweep
    // -----------------------------------------------------------------

    /// A direct commit (no archive entry) appears in `unattributed_commits`
    /// (JSON) and the `### Other changes` markdown footer; merge commits do
    /// not appear; a lane-covered commit is not double-reported.
    #[test]
    fn direct_commit_is_reported_as_unattributed() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        run(ws, &["tag", "v1.0.0"]);
        // A lane commit (adds an archive) — must NOT be unattributed.
        write_archive(ws, "2026-06-01", "a-change", "## Why\n\nA change.\n", &["cap-a"]);
        commit_all(ws, "ship a-change");
        // A direct commit touching a source file — unattributed.
        write_file(ws, "src/fix.txt", "patch\n");
        let direct = commit_all(ws, "fix a direct bug");

        // JSON.
        let mut js = Vec::new();
        let mut stderr = Vec::new();
        execute_into(ws, Some("v1.0.0"), "HEAD", ChangelogFormat::Json, &mut js, &mut stderr)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&String::from_utf8(js).unwrap()).unwrap();
        let unattr = parsed["unattributed_commits"].as_array().unwrap();
        assert_eq!(unattr.len(), 1, "only the direct commit is unattributed: {unattr:?}");
        assert_eq!(unattr[0]["sha"], direct);
        assert_eq!(unattr[0]["subject"], "fix a direct bug");
        assert!(unattr[0]["date"].as_str().unwrap().starts_with("20"));

        // Markdown footer.
        let mut md = Vec::new();
        let mut stderr2 = Vec::new();
        execute_into(ws, Some("v1.0.0"), "HEAD", ChangelogFormat::Markdown, &mut md, &mut stderr2)
            .unwrap();
        let md_s = String::from_utf8(md).unwrap();
        assert!(md_s.contains("### Other changes"), "got: {md_s}");
        assert!(md_s.contains(&format!("{} fix a direct bug", &direct[..7])), "got: {md_s}");
    }

    /// A range fully covered by lane commits emits no footer and an empty
    /// `unattributed_commits` array. Merge commits never appear.
    #[test]
    fn range_covered_by_lanes_has_empty_unattributed() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        run(ws, &["tag", "v1.0.0"]);
        // A feature branch merged with --no-ff so a merge commit exists in
        // the range but has no archive entry — it must NOT be reported.
        run(ws, &["checkout", "-b", "feature"]);
        write_archive(ws, "2026-06-01", "only-change", "## Why\n\nOnly change.\n", &["cap-a"]);
        commit_all(ws, "ship only-change");
        run(ws, &["checkout", "main"]);
        run(ws, &["merge", "--no-ff", "-m", "merge feature", "feature"]);

        let mut js = Vec::new();
        let mut stderr = Vec::new();
        execute_into(ws, Some("v1.0.0"), "HEAD", ChangelogFormat::Json, &mut js, &mut stderr)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&String::from_utf8(js).unwrap()).unwrap();
        assert_eq!(
            parsed["unattributed_commits"].as_array().unwrap().len(),
            0,
            "the merge commit and the lane commit must both be excluded: {}",
            parsed["unattributed_commits"]
        );

        let mut md = Vec::new();
        let mut stderr2 = Vec::new();
        execute_into(ws, Some("v1.0.0"), "HEAD", ChangelogFormat::Markdown, &mut md, &mut stderr2)
            .unwrap();
        assert!(
            !String::from_utf8(md).unwrap().contains("### Other changes"),
            "no footer when nothing is unattributed"
        );
    }
}
