//! Polling-iteration handler for chat-driven changelog requests.
//!
//! The chatops `changelog` verb writes a `ChangelogRequestState` to disk
//! AND pushes a `ChangelogRequest` onto the per-repo queue. The polling
//! loop drains the queue once per iteration and calls
//! `process_changelog_requests`. For each request the handler:
//!
//! 1. Resolves which versions a flagless run documents (a72): with no
//!    `--since`/`--to` it runs tag-driven gap-fill — every stable release
//!    tag missing from `CHANGELOG.md`, oldest-first, each as its own
//!    `(previous stable tag … this tag]` section — and combines the
//!    per-tag results into one `{ "sections": [ … ] }` payload. An
//!    explicit `--since`/`--to` keeps the single-range path. When nothing
//!    is undocumented (or there are no stable tags), the run is a friendly
//!    no-op: no stylist, no PR, a short thread reply, terminal status.
//!    Either way the deterministic `a05` extractor produces each section's
//!    data (no subprocess — its data-producing helpers are called
//!    directly).
//! 2. Builds the stylist prompt from the JSON output AND invokes the
//!    executor's `run_changelog` method.
//! 3. Validates the resulting diff's path scope: must touch only
//!    `CHANGELOG.md` AND/OR `openspec/changes/archive/<slug>/proposal.md`
//!    paths. Reject otherwise.
//! 4. Commits the diff to a `changelog-<short-hash>` branch, pushes,
//!    AND opens a single PR.
//! 5. Posts a threaded reply in the lifecycle thread naming the PR URL.

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeSet;
use std::path::Path;

use crate::changelog_requests::{
    self, ChangelogRequestState, ChangelogStatus,
};
use crate::chatops::operator_commands::{ParsedChangelogArgs, parse_changelog_args};
use crate::cli::changelog::{
    self as cli_changelog, ArchiveEntry, ArchiveMetadataRaw, SkippedEntry, render_json,
    resolve_tag_range,
};
use crate::config::{GithubConfig, RepositoryConfig};
use crate::executor::{ChangelogContext, Executor, ExecutorOutcome};
use crate::{git, github};

/// Per-request branch-name prefix. The full branch name appends a short
/// hash of the request_id so concurrent runs cannot collide.
const CHANGELOG_BRANCH_PREFIX: &str = "changelog-";

/// Path-scope validation: a diff entry is accepted iff it touches
/// `CHANGELOG.md` (at the workspace root) OR
/// `openspec/changes/archive/<slug>/proposal.md` (any depth, any slug).
fn is_in_scope(path: &str) -> bool {
    if path == "CHANGELOG.md" {
        return true;
    }
    if let Some(rest) = path.strip_prefix("openspec/changes/archive/")
        && let Some(idx) = rest.find('/')
    {
        let after = &rest[idx + 1..];
        if after == "proposal.md" {
            return true;
        }
    }
    false
}

/// Run the deterministic `a05` extractor over `(since … to]` AND return
/// the rendered section as a JSON value (the single-section shape:
/// `{version, date, since, to, entries, skipped}`). Calls the extractor's
/// data-producing helpers directly (no subprocess).
fn extract_section_json(
    workspace: &Path,
    since: Option<&str>,
    to: &str,
) -> Result<serde_json::Value> {
    let mut stderr_buf: Vec<u8> = Vec::new();
    let range = resolve_tag_range(workspace, since, to, &mut stderr_buf)
        .with_context(|| "changelog-stylist: resolving tag range".to_string())?;
    let discovered = cli_changelog::find_archives_in_range(workspace, &range)
        .with_context(|| "changelog-stylist: discovering archives".to_string())?;

    let mut entries: Vec<ArchiveEntry> = Vec::new();
    let mut skipped: Vec<SkippedEntry> = Vec::new();
    for raw in discovered {
        let slug = raw.slug.clone();
        let metadata = match cli_changelog::read_archive_metadata(
            workspace,
            &raw.archive_dir,
            &mut stderr_buf,
        ) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    "changelog-stylist: skipping `{slug}`: failed to read proposal.md: {e:#}"
                );
                continue;
            }
        };
        match metadata {
            ArchiveMetadataRaw::Entry { summary } => entries.push(ArchiveEntry {
                summary,
                ..raw
            }),
            ArchiveMetadataRaw::Skip { reason } => {
                skipped.push(SkippedEntry { slug, reason })
            }
        }
    }
    let version = range.to_label.clone();
    let json = render_json(&version, &range, &entries, &skipped)
        .map_err(|e| anyhow!("rendering changelog JSON: {e}"))?;
    serde_json::from_str(&json).map_err(|e| anyhow!("parsing rendered changelog JSON: {e}"))
}

// ---------------------------------------------------------------------------
// a72: tag-driven gap-fill
// ---------------------------------------------------------------------------

/// A git tag that parses as a `major.minor.patch` semantic version,
/// retaining its original tag string AND any pre-release suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedTag {
    major: u64,
    minor: u64,
    patch: u64,
    /// The semver pre-release component (the part after `-`), if any.
    /// `None` for stable releases.
    prerelease: Option<String>,
    /// The original tag string as it appears in the repo (e.g. `v1.2.0`).
    tag: String,
}

impl ParsedTag {
    /// Canonical, `v`-stripped version key used for set membership AND
    /// equality against the versions a `CHANGELOG.md` already documents.
    /// Retains the pre-release suffix so `1.0.0-rc.1` never matches the
    /// stable `1.0.0`.
    fn version_key(&self) -> String {
        match &self.prerelease {
            Some(pre) => format!("{}.{}.{}-{}", self.major, self.minor, self.patch, pre),
            None => format!("{}.{}.{}", self.major, self.minor, self.patch),
        }
    }

    fn is_stable(&self) -> bool {
        self.prerelease.is_none()
    }
}

/// Parse a tag string as a semver release version, tolerant of a leading
/// `v`/`V` AND of `+build` metadata. Returns `None` for tags that do not
/// parse as `major.minor.patch[-prerelease][+build]` (non-version tags are
/// simply ignored by gap-fill).
fn parse_version_tag(tag: &str) -> Option<ParsedTag> {
    let trimmed = tag.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Tolerate a single leading `v`/`V`.
    let core = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    // Strip `+build` metadata: per semver it carries no precedence AND has
    // no bearing on stable-vs-pre-release.
    let core = core.split('+').next().unwrap_or(core);
    // Split off the pre-release component at the first `-`.
    let (numbers, prerelease) = match core.split_once('-') {
        Some((nums, pre)) if !pre.is_empty() => (nums, Some(pre.to_string())),
        // A trailing `-` with an empty pre-release is not a valid version.
        Some((_nums, _empty)) => return None,
        None => (core, None),
    };
    let mut parts = numbers.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        // More than three numeric components — not a release version.
        return None;
    }
    Some(ParsedTag {
        major,
        minor,
        patch,
        prerelease,
        tag: trimmed.to_string(),
    })
}

/// Parse `tags`, keep only stable release versions (no pre-release
/// component), AND return them sorted ascending by version. Non-version
/// tags AND pre-release tags (`-dev`/`-rc`/`-alpha`/`-beta`/any semver
/// pre-release suffix) are dropped. Duplicate versions (e.g. `1.0.0` AND
/// `v1.0.0`) are collapsed to one.
fn stable_release_tags(tags: &[String]) -> Vec<ParsedTag> {
    let mut stable: Vec<ParsedTag> = tags
        .iter()
        .filter_map(|t| parse_version_tag(t))
        .filter(ParsedTag::is_stable)
        .collect();
    stable.sort_by_key(|a| (a.major, a.minor, a.patch));
    stable.dedup_by(|a, b| a.major == b.major && a.minor == b.minor && a.patch == b.patch);
    stable
}

/// Regex matching a `CHANGELOG.md` version heading AND capturing the
/// version string (`v`-stripped, pre-release suffix retained). Matches the
/// heading shapes the stylist produces — `## [1.0.0] - …`, `## v1.0.0 — …`,
/// `### 1.2.0-rc.1`, etc. — so daemon-side gap detection stays symmetric
/// with the stylist's own headings.
fn changelog_heading_version_regex() -> regex::Regex {
    regex::Regex::new(r"(?i)^#+\s*\[?v?(\d+\.\d+\.\d+(?:-[\w.]+)?)\]?")
        .expect("changelog heading version regex is a valid literal")
}

/// Extract the set of versions a `CHANGELOG.md` body already documents by
/// matching its version headings. The returned keys are `v`-stripped AND
/// retain any pre-release suffix so they line up with
/// `ParsedTag::version_key`.
fn documented_versions(changelog: &str) -> BTreeSet<String> {
    let re = changelog_heading_version_regex();
    let mut out = BTreeSet::new();
    for line in changelog.lines() {
        if let Some(caps) = re.captures(line) {
            out.insert(caps[1].to_string());
        }
    }
    out
}

/// A single gap-fill extraction range: `(since … to]`. `since` is the
/// previous stable release tag's name, or `None` meaning "ever" (from the
/// beginning of archive history).
#[derive(Debug, Clone, PartialEq, Eq)]
struct GapRange {
    since: Option<String>,
    to: String,
}

/// Compute the per-tag extraction ranges for the undocumented stable tags,
/// oldest-first. For each missing tag, the lower bound is the
/// immediately-preceding stable tag in the full ascending list (whether or
/// not it is itself documented), or `None` ("ever") when the missing tag is
/// the earliest stable release.
fn gap_fill_ranges(stable_sorted: &[ParsedTag], documented: &BTreeSet<String>) -> Vec<GapRange> {
    let mut ranges = Vec::new();
    for (i, tag) in stable_sorted.iter().enumerate() {
        if documented.contains(&tag.version_key()) {
            continue;
        }
        let since = if i == 0 {
            None
        } else {
            Some(stable_sorted[i - 1].tag.clone())
        };
        ranges.push(GapRange {
            since,
            to: tag.tag.clone(),
        });
    }
    ranges
}

/// List the workspace's tags (one per line, blank lines dropped).
fn list_repo_tags(workspace: &Path) -> Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .args(["tag", "--list"])
        .current_dir(workspace)
        .output()
        .with_context(|| format!("spawning `git tag --list` in {}", workspace.display()))?;
    if !output.status.success() {
        let stderr_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!("git tag --list failed: {stderr_text}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Read `<workspace>/CHANGELOG.md`, returning an empty string when the file
/// is absent (a fresh repo documents nothing yet).
fn read_changelog(workspace: &Path) -> Result<String> {
    let path = workspace.join("CHANGELOG.md");
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(anyhow!("reading {}: {e}", path.display())),
    }
}

/// Why a gap-fill run had nothing to do, for the friendly thread reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoOpReason {
    /// Every stable release tag is already documented.
    AlreadyCurrent,
    /// The repo has no stable release tags to document yet.
    NoStableTags,
}

impl NoOpReason {
    fn reply_body(self, repo_url: &str) -> String {
        match self {
            NoOpReason::AlreadyCurrent => format!(
                "ℹ️ Changelog for `{repo_url}` is already current — every stable release tag is documented. Nothing to do."
            ),
            NoOpReason::NoStableTags => format!(
                "ℹ️ No stable release tags to document yet for `{repo_url}`. Tag a release (e.g. `v1.0.0`), or pass `--since`/`--to` for an explicit range."
            ),
        }
    }
}

/// What `build_stylist_payload` resolved a request to.
enum StylistPayload {
    /// One or more version sections to hand to the stylist, already
    /// serialized as the `{ "sections": [ … ] }` JSON the prompt consumes.
    Sections(String),
    /// Nothing to document — the caller posts a friendly no-op reply AND
    /// advances the request to its terminal status without invoking the
    /// stylist or opening a PR.
    NoOp(NoOpReason),
}

/// Wrap one or more section objects into the `{ "sections": [ … ] }`
/// envelope the stylist prompt consumes. Pretty-printed for readability.
fn wrap_sections(sections: Vec<serde_json::Value>) -> Result<String> {
    let doc = serde_json::json!({ "sections": sections });
    let mut text = serde_json::to_string_pretty(&doc)
        .map_err(|e| anyhow!("serializing combined changelog sections: {e}"))?;
    text.push('\n');
    Ok(text)
}

/// Build the JSON payload handed to the stylist (a72).
///
/// With an explicit `--since` AND/OR `--to`, the single-range behavior is
/// preserved: one section for the operator-specified `(since … to]`.
/// Otherwise gap-fill runs: every undocumented stable release tag becomes
/// its own section, oldest-first — OR a `NoOp` when there is nothing to
/// document (all stable tags already documented, or no stable tags yet).
fn build_stylist_payload(
    workspace: &Path,
    parsed: &ParsedChangelogArgs,
) -> Result<StylistPayload> {
    // Explicit range overrides gap-fill: one section for the
    // operator-specified `(since … to]`.
    if parsed.since.is_some() || parsed.to.is_some() {
        let section = extract_section_json(
            workspace,
            parsed.since.as_deref(),
            parsed.to.as_deref().unwrap_or("HEAD"),
        )?;
        return Ok(StylistPayload::Sections(wrap_sections(vec![section])?));
    }

    // Flagless: tag-driven gap-fill.
    let tags = list_repo_tags(workspace)?;
    let stable = stable_release_tags(&tags);
    if stable.is_empty() {
        return Ok(StylistPayload::NoOp(NoOpReason::NoStableTags));
    }
    let changelog = read_changelog(workspace)?;
    let documented = documented_versions(&changelog);
    let ranges = gap_fill_ranges(&stable, &documented);
    if ranges.is_empty() {
        return Ok(StylistPayload::NoOp(NoOpReason::AlreadyCurrent));
    }
    let mut sections = Vec::with_capacity(ranges.len());
    for range in &ranges {
        // A `None` lower bound means "from the beginning of archive
        // history": pass the `ever` sentinel, NOT `None`. Passing `None`
        // would auto-detect the most-recent tag on `to`'s ancestry — which
        // for the earliest stable tag is the tag itself, yielding an empty
        // range.
        let since = range.since.as_deref().unwrap_or("ever");
        let section = extract_section_json(workspace, Some(since), &range.to)?;
        sections.push(section);
    }
    Ok(StylistPayload::Sections(wrap_sections(sections)?))
}

/// Drain handler for chat-driven changelog requests. The polling loop's
/// `run` calls this once per iteration with the per-iteration drained
/// queue snapshot. Each entry loads its `ChangelogRequestState`, runs
/// the deterministic extractor, invokes the stylist via the executor,
/// validates the diff's path scope, commits + pushes to a
/// `changelog-<short-hash>` branch, AND opens a single PR.
pub async fn process_changelog_requests(
    paths: &crate::paths::DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&crate::polling_loop::ChatOpsContext>,
    requests: &[crate::control_socket::ChangelogRequest],
) -> Result<()> {
    let fork_url = match github_cfg.fork_owner.as_deref() {
        Some(owner) => Some(crate::github::derive_fork_url(&repo.url, owner)?),
        None => None,
    };
    let fork_arg = fork_url.as_deref().map(|u| (u, repo.agent_branch.as_str()));
    crate::workspace::ensure_initialized(paths, workspace, &repo.url, fork_arg)
        .with_context(|| "changelog-stylist: workspace ensure_initialized".to_string())?;
    let _ = crate::queue::clear_stale_locks(workspace);
    let _ = git::reset_hard_head(workspace);
    let _ = git::clean_force(workspace);
    git::fetch(workspace).with_context(|| "changelog-stylist: git fetch".to_string())?;
    git::checkout(workspace, &repo.base_branch)
        .with_context(|| format!("changelog-stylist: checkout `{}`", repo.base_branch))?;
    git::pull_ff_only(workspace, &repo.base_branch).with_context(|| {
        format!("changelog-stylist: pull --ff-only `{}`", repo.base_branch)
    })?;

    let state_root = changelog_requests::default_state_root(paths);
    for request in requests {
        let mut state = match changelog_requests::read_state(
            &state_root,
            &repo.url,
            &request.request_id,
        ) {
            Ok(Some(s)) => s,
            Ok(None) => {
                tracing::warn!(
                    request_id = %request.request_id,
                    "changelog-stylist: no state file (entry pruned between enqueue and processing); skipping"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    request_id = %request.request_id,
                    "changelog-stylist: state read failed: {e:#}"
                );
                continue;
            }
        };

        state.status = ChangelogStatus::InFlight;
        let _ = changelog_requests::write_state(&state_root, &state);

        if let Err(e) = process_one_request(
            workspace, repo, executor, github_cfg, chatops_ctx, &state_root, &mut state,
        )
        .await
        {
            tracing::error!(
                url = %repo.url,
                request_id = %state.request_id,
                "changelog-stylist: processing failed: {e:#}"
            );
            mark_failed(&state_root, &mut state, format!("{e:#}"), chatops_ctx).await;
        }

        if let Err(e) = git::reset_hard_head(workspace) {
            tracing::warn!(
                url = %repo.url,
                "changelog-stylist: post-run reset_hard_head failed: {e:#}"
            );
        }
        let _ = git::clean_force(workspace);
        let _ = git::checkout(workspace, &repo.base_branch);
    }
    Ok(())
}

async fn process_one_request(
    workspace: &Path,
    repo: &RepositoryConfig,
    executor: &dyn Executor,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&crate::polling_loop::ChatOpsContext>,
    state_root: &Path,
    state: &mut ChangelogRequestState,
) -> Result<()> {
    let parsed = parse_changelog_args(&state.raw_args)
        .map_err(|e| anyhow!("parsing changelog args `{}`: {e}", state.raw_args))?;
    if parsed.workspace_override.is_some() {
        return Err(anyhow!(
            "refusing changelog: --workspace override arrived via chatops"
        ));
    }
    let changelog_json = match build_stylist_payload(workspace, &parsed)? {
        StylistPayload::Sections(json) => json,
        StylistPayload::NoOp(reason) => {
            // a72: nothing to document — no stylist, no PR. Post a short
            // thread reply AND advance to the terminal `Acted` status.
            tracing::info!(
                url = %repo.url,
                request_id = %state.request_id,
                "changelog-stylist: gap-fill no-op ({reason:?}); skipping stylist + PR"
            );
            if let Some(ctx) = chatops_ctx {
                let body = reason.reply_body(&state.repo_url);
                let _ = ctx
                    .chatops
                    .post_threaded_reply(&state.channel, &state.lifecycle_thread_ts, &body)
                    .await;
            }
            state.status = ChangelogStatus::Acted;
            let _ = changelog_requests::write_state(state_root, state);
            return Ok(());
        }
    };
    let ctx = ChangelogContext {
        changelog_json,
        repo_url: state.repo_url.clone(),
        revision_text: String::new(),
    };
    tracing::info!(
        url = %repo.url,
        request_id = %state.request_id,
        "changelog-stylist: invoking executor"
    );
    let outcome = executor.run_changelog(workspace, &ctx).await?;
    match outcome {
        ExecutorOutcome::Completed { .. } => {
            commit_and_open_pr(workspace, repo, github_cfg, chatops_ctx, state_root, state).await
        }
        ExecutorOutcome::Failed { reason } => Err(anyhow!("executor failed: {reason}")),
        // a74: surfaced only on the revise path today; the changelog flow is
        // out of scope. Treat it as a failure (defensive — never produced here
        // at runtime).
        ExecutorOutcome::PreconditionUnmet { reason } => {
            Err(anyhow!("executor reported PreconditionUnmet: {reason}"))
        }
        ExecutorOutcome::AskUser { .. } => Err(anyhow!(
            "executor returned AskUser; changelog flow does not support clarification"
        )),
        ExecutorOutcome::SpecNeedsRevision { .. } => Err(anyhow!(
            "executor flagged SpecNeedsRevision during changelog run"
        )),
        ExecutorOutcome::IterationRequested { .. } => Err(anyhow!(
            "executor returned IterationRequested during changelog run (iteration sequences not applicable)"
        )),
        ExecutorOutcome::Aborted { reason } => {
            // a39: subprocess killed by the daemon's own SIGTERM
            // cascade. Return Ok(()) so the changelog request is not
            // marked as a failure; the next iteration after restart
            // will retry from a clean state.
            tracing::info!(
                url = %repo.url,
                request_id = %state.request_id,
                "changelog-stylist: executor aborted by daemon shutdown: {reason}"
            );
            Ok(())
        }
    }
}

async fn commit_and_open_pr(
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    chatops_ctx: Option<&crate::polling_loop::ChatOpsContext>,
    state_root: &Path,
    state: &mut ChangelogRequestState,
) -> Result<()> {
    // Read the workspace's git status to discover what the stylist
    // changed. `status_entries` is the single NUL-delimited parser, so a
    // worktree-modified path arrives intact (no leading-char drop).
    let changed: Vec<String> = git::status_entries(workspace)
        .with_context(|| "changelog-stylist: reading post-Completed git status".to_string())?
        .into_iter()
        .map(|e| e.path)
        .collect();

    if changed.is_empty() {
        if let Some(ctx) = chatops_ctx {
            let body = format!(
                "ℹ️ Changelog run for `{repo_url}` completed with no changes.",
                repo_url = state.repo_url,
            );
            let _ = ctx
                .chatops
                .post_threaded_reply(&state.channel, &state.lifecycle_thread_ts, &body)
                .await;
        }
        state.status = ChangelogStatus::Acted;
        let _ = changelog_requests::write_state(state_root, state);
        return Ok(());
    }

    // Path-scope validation. Out-of-scope diffs are refused; the
    // workspace is reset clean below.
    let out_of_scope: Vec<String> = changed
        .iter()
        .filter(|p| !is_in_scope(p))
        .cloned()
        .collect();
    if !out_of_scope.is_empty() {
        let log_pointer = format!(
            "journalctl -u autocoder | grep request_id={}",
            state.request_id
        );
        let body = format!(
            "✗ changelog: LLM produced out-of-scope diff; refusing to commit. See {log_pointer}."
        );
        if let Some(ctx) = chatops_ctx {
            let _ = ctx
                .chatops
                .post_threaded_reply(&state.channel, &state.lifecycle_thread_ts, &body)
                .await;
        }
        tracing::warn!(
            request_id = %state.request_id,
            out_of_scope = ?out_of_scope,
            "changelog-stylist: rejecting out-of-scope diff"
        );
        state.status = ChangelogStatus::Failed;
        state.reason = Some(format!("out-of-scope diff: {out_of_scope:?}"));
        let _ = changelog_requests::write_state(state_root, state);
        return Ok(());
    }

    // Build the branch name from a short hash of the request_id. Stable
    // per request, unique across concurrent runs.
    let short_hash = short_id_hash(&state.request_id);
    let branch = format!("{CHANGELOG_BRANCH_PREFIX}{short_hash}");

    git::recreate_branch(workspace, &branch)
        .with_context(|| format!("changelog-stylist: recreate `{branch}`"))?;
    for p in &changed {
        let _ = std::process::Command::new("git")
            .args(["add", "--", p])
            .current_dir(workspace)
            .status();
    }
    let subject = format!("changelog: stylist draft (request {})", state.request_id);
    git::commit(workspace, &subject)
        .with_context(|| "changelog-stylist: commit changelog branch".to_string())?;
    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    git::push_force_with_lease(workspace, &branch, push_remote)
        .with_context(|| "changelog-stylist: pushing changelog branch".to_string())?;

    let pr_title = format!("changelog: stylist draft ({short_hash})");
    let pr_body = format!(
        "This PR carries the LLM-styled CHANGELOG.md draft for `{repo_url}` (request `{request_id}`).\n\n\
         Reviewers: read the diff on GitHub. To iterate, post `@<bot> revise <instruction>` on this PR and the stylist will re-run with your instruction applied.",
        repo_url = state.repo_url,
        request_id = state.request_id,
    );
    let pr_url = open_changelog_pull_request(
        repo,
        github_cfg,
        &branch,
        &repo.base_branch,
        &pr_title,
        &pr_body,
    )
    .await
    .with_context(|| "changelog-stylist: opening PR".to_string())?;

    if let Some(ctx) = chatops_ctx {
        let body = format!(
            "✓ Changelog draft ready at {pr_url}. Review on GitHub; revise via @<bot> revise <text>."
        );
        let _ = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.lifecycle_thread_ts, &body)
            .await;
    }

    state.status = ChangelogStatus::Acted;
    let _ = changelog_requests::write_state(state_root, state);
    Ok(())
}

async fn mark_failed(
    state_root: &Path,
    state: &mut ChangelogRequestState,
    reason: String,
    chatops_ctx: Option<&crate::polling_loop::ChatOpsContext>,
) {
    state.status = ChangelogStatus::Failed;
    state.reason = Some(reason.clone());
    if let Err(e) = changelog_requests::write_state(state_root, state) {
        tracing::warn!(
            request_id = %state.request_id,
            "changelog-stylist: recording Failed state failed: {e:#}"
        );
    }
    if let Some(ctx) = chatops_ctx {
        let body = format!(
            "✗ Changelog run for `{repo_url}` failed: {reason}",
            repo_url = state.repo_url,
        );
        let _ = ctx
            .chatops
            .post_threaded_reply(&state.channel, &state.lifecycle_thread_ts, &body)
            .await;
    }
}

/// 8-char hex hash of `request_id`. Stable + URL-safe.
fn short_id_hash(request_id: &str) -> String {
    let mut state: u64 = 0xcbf29ce484222325;
    for b in request_id.as_bytes() {
        state ^= *b as u64;
        state = state.wrapping_mul(0x100000001b3);
    }
    format!("{state:016x}")[..8].to_string()
}

/// Walk every open PR in the repo whose head branch starts with
/// `changelog-` AND drive the PR-comment revision loop against it.
/// Mirrors the shape of `revisions::process_revision_requests` but is
/// purpose-built for the changelog flow: on a revision trigger, the
/// stylist re-runs with the operator's revision text injected, the
/// diff scope is validated, AND the new commit is force-pushed to the
/// PR's existing branch (no PR close/re-open).
pub async fn process_changelog_revision_requests(
    paths: &crate::paths::DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    executor: &dyn Executor,
    chatops_ctx: Option<&crate::polling_loop::ChatOpsContext>,
) -> Result<()> {
    let (owner, repo_name) = github::parse_repo_url(&repo.url)?;
    let token = crate::github_credentials::resolve_token(github_cfg, &owner)?;
    let bot_username = github::self_bot_username(github::DEFAULT_API_BASE, &token)
        .await
        .with_context(|| "changelog-revision: resolving bot username")?;
    let open_prs = github::list_open_prs_all(
        github::DEFAULT_API_BASE,
        &token,
        &owner,
        &repo_name,
    )
    .await
    .with_context(|| {
        format!("changelog-revision: listing open PRs for {owner}/{repo_name}")
    })?;
    let changelog_prs: Vec<&github::PrSummary> = open_prs
        .iter()
        .filter(|p| p.head.ref_.starts_with(CHANGELOG_BRANCH_PREFIX))
        .collect();
    if changelog_prs.is_empty() {
        return Ok(());
    }
    for pr in &changelog_prs {
        if let Err(e) = process_one_changelog_pr_revision(
            paths,
            workspace,
            repo,
            github_cfg,
            executor,
            chatops_ctx,
            pr,
            &owner,
            &repo_name,
            &token,
            &bot_username,
        )
        .await
        {
            tracing::warn!(
                url = %repo.url,
                pr_number = pr.number,
                "changelog-revision processing for PR failed (iteration continues): {e:#}"
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_one_changelog_pr_revision(
    paths: &crate::paths::DaemonPaths,
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    executor: &dyn Executor,
    chatops_ctx: Option<&crate::polling_loop::ChatOpsContext>,
    pr: &github::PrSummary,
    owner: &str,
    repo_name: &str,
    token: &str,
    bot_username: &str,
) -> Result<()> {
    // Per-PR state lives under the existing `revisions/` directory so
    // both flows share the same prune-on-close machinery.
    let mut state = match crate::revisions::read_state(paths, workspace, pr.number)? {
        Some(s) => s,
        None => crate::revisions::RevisionState {
            pr_number: pr.number,
            agent_branch: pr.head.ref_.clone(),
            last_seen_comment_at: pr.created_at,
            auto_revisions_applied: 0,
            revision_cap: u32::MAX,
            cap_decline_posted: false,
            human_revise_count: 0,
            human_revise_cap_decline_posted: false,
            code_reviews_applied: 0,
            code_review_cap: Some(5),
            cap_decline_posted_for_code_review: false,
            last_suggested_rereview_at_revisions_count: None,
            original_review_head_sha: None,
        },
    };
    let comments = github::list_issue_comments_since(
        github::DEFAULT_API_BASE,
        token,
        owner,
        repo_name,
        pr.number,
        state.last_seen_comment_at,
    )
    .await?;
    if comments.is_empty() {
        return Ok(());
    }
    let mut latest_seen: Option<chrono::DateTime<chrono::Utc>> = None;
    for comment in comments {
        if comment.user_login().eq_ignore_ascii_case(bot_username)
            && !comment
                .body
                .trim_start()
                .starts_with(crate::revisions::REVIEWER_REVISION_MARKER)
        {
            advance_seen(&mut latest_seen, comment.created_at);
            continue;
        }
        let revision_text = match crate::revisions::parse_revision_trigger(&comment.body, bot_username)
        {
            Some(t) => t,
            None => {
                advance_seen(&mut latest_seen, comment.created_at);
                continue;
            }
        };
        // Re-run the stylist with the revision text injected; force-push
        // to the existing changelog branch.
        if let Err(e) = re_run_stylist_and_force_push(
            workspace,
            repo,
            github_cfg,
            executor,
            chatops_ctx,
            &pr.head.ref_,
            &revision_text,
            pr.number,
            owner,
            repo_name,
            token,
        )
        .await
        {
            tracing::warn!(
                url = %repo.url,
                pr_number = pr.number,
                "changelog-revision re-run failed: {e:#}"
            );
            let body = format!(
                "✗ Changelog revision failed: {e}. The PR is unchanged."
            );
            let _ = github::post_issue_comment(
                github::DEFAULT_API_BASE,
                token,
                owner,
                repo_name,
                pr.number,
                &body,
            )
            .await;
        } else {
            state.auto_revisions_applied = state.auto_revisions_applied.saturating_add(1);
            let body = format!(
                "✅ Changelog revision applied. Total revisions on this PR: {}.",
                state.auto_revisions_applied
            );
            let _ = github::post_issue_comment(
                github::DEFAULT_API_BASE,
                token,
                owner,
                repo_name,
                pr.number,
                &body,
            )
            .await;
        }
        advance_seen(&mut latest_seen, comment.created_at);
        crate::revisions::write_state(paths, workspace, &state)?;
    }
    if let Some(t) = latest_seen
        && t > state.last_seen_comment_at
    {
        state.last_seen_comment_at = t;
        crate::revisions::write_state(paths, workspace, &state)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn re_run_stylist_and_force_push(
    workspace: &Path,
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    executor: &dyn Executor,
    _chatops_ctx: Option<&crate::polling_loop::ChatOpsContext>,
    branch: &str,
    revision_text: &str,
    _pr_number: u64,
    _owner: &str,
    _repo_name: &str,
    _token: &str,
) -> Result<()> {
    git::fetch(workspace).with_context(|| "changelog-revision: git fetch")?;
    git::checkout(workspace, &repo.base_branch)
        .with_context(|| format!("changelog-revision: checkout `{}`", repo.base_branch))?;
    git::pull_ff_only(workspace, &repo.base_branch).with_context(|| {
        format!("changelog-revision: pull --ff-only `{}`", repo.base_branch)
    })?;
    let parsed = ParsedChangelogArgs::default();
    let changelog_json = match build_stylist_payload(workspace, &parsed)? {
        StylistPayload::Sections(json) => json,
        StylistPayload::NoOp(_) => {
            // A flagless revision found no undocumented stable tags — the
            // base changelog is already current, so there is nothing to
            // restyle. Surface as an error so the revise loop reports it.
            return Err(anyhow!(
                "changelog revision: nothing to document (gap-fill found no missing stable tags)"
            ));
        }
    };
    let ctx = ChangelogContext {
        changelog_json,
        repo_url: repo.url.clone(),
        revision_text: revision_text.to_string(),
    };
    let outcome = executor.run_changelog(workspace, &ctx).await?;
    match outcome {
        ExecutorOutcome::Completed { .. } => {}
        ExecutorOutcome::Failed { reason } => {
            return Err(anyhow!("executor failed: {reason}"));
        }
        // a74: surfaced only on the revise path today; out of scope here.
        // Treat it as a failure (defensive — never produced here at runtime).
        ExecutorOutcome::PreconditionUnmet { reason } => {
            return Err(anyhow!("executor reported PreconditionUnmet: {reason}"));
        }
        ExecutorOutcome::AskUser { .. } => {
            return Err(anyhow!("executor returned AskUser; not supported here"));
        }
        ExecutorOutcome::SpecNeedsRevision { .. } => {
            return Err(anyhow!("executor returned SpecNeedsRevision; not supported here"));
        }
        ExecutorOutcome::IterationRequested { .. } => {
            return Err(anyhow!(
                "executor returned IterationRequested; not supported here"
            ));
        }
        ExecutorOutcome::Aborted { reason } => {
            // a39: subprocess killed by the daemon's own SIGTERM
            // cascade. Return Ok(()) — the revise loop will retry on
            // the next iteration after restart.
            tracing::info!(
                url = %repo.url,
                "changelog-revision: executor aborted by daemon shutdown: {reason}"
            );
            return Ok(());
        }
    }
    let changed: Vec<String> = git::status_entries(workspace)
        .with_context(|| "changelog-revision: post-Completed git status")?
        .into_iter()
        .map(|e| e.path)
        .collect();
    let out_of_scope: Vec<String> = changed
        .iter()
        .filter(|p| !is_in_scope(p))
        .cloned()
        .collect();
    if !out_of_scope.is_empty() {
        let _ = git::reset_hard_head(workspace);
        let _ = git::clean_force(workspace);
        return Err(anyhow!(
            "out-of-scope diff: {out_of_scope:?}; refusing to commit"
        ));
    }
    if changed.is_empty() {
        return Err(anyhow!("revision produced no diff"));
    }
    // Force-recreate the changelog branch from base AND commit the new
    // stylist output. This preserves the branch name (so the PR's head
    // does not change) but rewrites the single commit on it.
    git::recreate_branch(workspace, branch)
        .with_context(|| format!("changelog-revision: recreate `{branch}`"))?;
    for p in &changed {
        let _ = std::process::Command::new("git")
            .args(["add", "--", p])
            .current_dir(workspace)
            .status();
    }
    git::commit(workspace, "changelog: stylist revision")
        .with_context(|| "changelog-revision: commit")?;
    let push_remote = if github_cfg.fork_owner.is_some() {
        "fork"
    } else {
        "origin"
    };
    git::push_force_with_lease(workspace, branch, push_remote)
        .with_context(|| "changelog-revision: pushing revised branch")?;
    Ok(())
}

fn advance_seen(
    latest: &mut Option<chrono::DateTime<chrono::Utc>>,
    candidate: chrono::DateTime<chrono::Utc>,
) {
    match latest {
        Some(curr) if *curr >= candidate => {}
        _ => *latest = Some(candidate),
    }
}

/// Open the changelog PR. Mirrors `open_triage_pull_request` from
/// `polling_loop` but lives here so the changelog flow can change PR
/// shape independently.
async fn open_changelog_pull_request(
    repo: &RepositoryConfig,
    github_cfg: &GithubConfig,
    head_branch: &str,
    base_branch: &str,
    title: &str,
    body: &str,
) -> Result<String> {
    let (owner, name) = github::parse_repo_url(&repo.url)
        .with_context(|| "changelog-stylist: parsing repo URL".to_string())?;
    let token = crate::github_credentials::resolve_token(github_cfg, &owner)?;
    let head = if let Some(fork_owner) = github_cfg.fork_owner.as_deref() {
        format!("{fork_owner}:{head_branch}")
    } else {
        head_branch.to_string()
    };
    let pr = github::create_pull_request(
        &owner,
        &name,
        &head,
        base_branch,
        title,
        body,
        &token,
        None,
        false,
    )
    .await?;
    Ok(pr.html_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_scope_accepts_root_changelog_and_proposal_files() {
        assert!(is_in_scope("CHANGELOG.md"));
        assert!(is_in_scope(
            "openspec/changes/archive/2026-05-22-foo/proposal.md"
        ));
        assert!(is_in_scope(
            "openspec/changes/archive/2026-05-22-foo-bar-baz/proposal.md"
        ));
    }

    #[test]
    fn in_scope_rejects_arbitrary_paths() {
        assert!(!is_in_scope("src/foo.rs"));
        assert!(!is_in_scope("README.md"));
        assert!(!is_in_scope("openspec/changes/active/foo/proposal.md"));
        assert!(!is_in_scope(
            "openspec/changes/archive/2026-05-22-foo/tasks.md"
        ));
        assert!(!is_in_scope("CHANGELOG.md.bak"));
    }

    #[test]
    fn short_id_hash_is_deterministic_and_8_chars() {
        let a = short_id_hash("req-1");
        let b = short_id_hash("req-1");
        let c = short_id_hash("req-2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 8);
    }

    /// 3.5 — end-to-end: a worktree-modified archive `proposal.md` (a
    /// legitimate `changelog:` frontmatter edit) reaches `is_in_scope`
    /// with its path intact via `status_entries`, so the out-of-scope
    /// check does NOT reject it. This pins the regression the change
    /// fixes: a whole-blob `.trim()` used to chop the leading `o` off
    /// `openspec/...`, making `is_in_scope` return false and aborting the
    /// changelog run.
    #[test]
    fn out_of_scope_check_accepts_modified_archive_proposal() {
        use std::process::Command;
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path();
        let run = |args: &[&str]| {
            let st = Command::new("git")
                .args(args)
                .current_dir(ws)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        let rel = "openspec/changes/archive/2026-05-22-foo/proposal.md";
        let abs = ws.join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, "# Proposal\n\nbody\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "seed archive"]);
        // The stylist stamps `changelog:` frontmatter — a worktree edit
        // with a BLANK staged column, so its record begins with a space.
        std::fs::write(&abs, "---\nchangelog: skip\n---\n# Proposal\n\nbody\n").unwrap();

        // Mirror the production out-of-scope check.
        let changed: Vec<String> = git::status_entries(ws)
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(changed, vec![rel.to_string()], "path must arrive intact");

        let out_of_scope: Vec<String> =
            changed.iter().filter(|p| !is_in_scope(p)).cloned().collect();
        assert!(
            out_of_scope.is_empty(),
            "the modified archive proposal must NOT be rejected; got {out_of_scope:?}"
        );
        assert!(
            is_in_scope(&changed[0]),
            "is_in_scope must accept the intact path"
        );
    }

    // -----------------------------------------------------------------
    // a72: tag-driven gap-fill — pure helpers
    // -----------------------------------------------------------------

    /// 1.1 / 1.2 — tolerant parse: `v` prefix accepted, build metadata
    /// stripped, non-version tags ignored.
    #[test]
    fn parse_version_tag_is_tolerant_and_ignores_non_versions() {
        assert_eq!(parse_version_tag("v1.2.0").unwrap().version_key(), "1.2.0");
        assert_eq!(parse_version_tag("1.2.0").unwrap().version_key(), "1.2.0");
        assert_eq!(parse_version_tag("V2.0.0").unwrap().version_key(), "2.0.0");
        // Pre-release suffix is retained AND marks the tag non-stable.
        let rc = parse_version_tag("v1.2.0-rc.1").unwrap();
        assert!(!rc.is_stable());
        assert_eq!(rc.version_key(), "1.2.0-rc.1");
        // Build metadata is tolerated AND does not make the tag a
        // pre-release.
        let bm = parse_version_tag("v1.2.0+build.5").unwrap();
        assert!(bm.is_stable());
        assert_eq!(bm.version_key(), "1.2.0");
        // Non-version tags are ignored.
        assert!(parse_version_tag("nightly").is_none());
        assert!(parse_version_tag("v1.2").is_none());
        assert!(parse_version_tag("1.2.0.3").is_none());
        assert!(parse_version_tag("release-2026").is_none());
        assert!(parse_version_tag("v1.2.0-").is_none());
    }

    /// 4.1 — stable filter: pre-release tags are skipped AND stable
    /// releases come back ascending by version.
    #[test]
    fn stable_filter_skips_prerelease_and_sorts_ascending() {
        let tags = vec![
            "v1.2.0".to_string(),
            "v1.2.0-dev-108".to_string(),
            "v1.2.0-rc.1".to_string(),
            "v1.1.0".to_string(),
        ];
        let stable = stable_release_tags(&tags);
        let names: Vec<&str> = stable.iter().map(|t| t.tag.as_str()).collect();
        assert_eq!(names, vec!["v1.1.0", "v1.2.0"]);
    }

    /// 4.1 (extra) — versions sort numerically, not lexically
    /// (`v1.10.0` is newer than `v1.9.0`), AND non-version tags drop out.
    #[test]
    fn stable_filter_sorts_numerically_and_drops_non_versions() {
        let tags = vec![
            "v1.10.0".to_string(),
            "v1.9.0".to_string(),
            "nightly".to_string(),
            "v2.0.0".to_string(),
        ];
        let stable = stable_release_tags(&tags);
        let names: Vec<&str> = stable.iter().map(|t| t.tag.as_str()).collect();
        assert_eq!(names, vec!["v1.9.0", "v1.10.0", "v2.0.0"]);
    }

    /// 4.2 — documented-version detection: a `## [1.0.0]` heading yields
    /// `{1.0.0}`, AND the missing set against tags `{v1.0.0, v1.1.0}` is
    /// `[v1.1.0]` (read off the gap-fill ranges' `to` labels).
    #[test]
    fn documented_versions_and_missing_set() {
        let changelog =
            "# Changelog\n\n## [Unreleased]\n\n## [1.0.0] - 2026-05-01\n- did things\n";
        let documented = documented_versions(changelog);
        assert!(documented.contains("1.0.0"));
        assert_eq!(documented.len(), 1, "only 1.0.0 is a version heading");

        let tags = vec!["v1.0.0".to_string(), "v1.1.0".to_string()];
        let stable = stable_release_tags(&tags);
        let ranges = gap_fill_ranges(&stable, &documented);
        let missing: Vec<&str> = ranges.iter().map(|r| r.to.as_str()).collect();
        assert_eq!(missing, vec!["v1.1.0"], "1.0.0 documented → only v1.1.0 missing");
    }

    /// 4.2 (extra) — the stylist's own `## v1.0.0 — <date>` heading shape
    /// is detected too, keeping daemon gap detection symmetric with the
    /// stylist's headings.
    #[test]
    fn documented_versions_detects_v_prefixed_emdash_headings() {
        let changelog = "# Changelog\n\n## v1.0.0 — 2026-05-01\n- thing\n\n### 1.2.0-rc.1\n- pre\n";
        let documented = documented_versions(changelog);
        assert!(documented.contains("1.0.0"));
        assert!(documented.contains("1.2.0-rc.1"));
    }

    /// 4.3 — gap-fill ranges, oldest-first, with an `ever` lower bound for
    /// the earliest stable release. Asserts the ranges/order, not message
    /// text.
    #[test]
    fn gap_fill_ranges_oldest_first_with_ever_lower_bound() {
        let tags = vec!["v1.1.0".to_string(), "v1.0.0".to_string()];
        let stable = stable_release_tags(&tags);
        let documented = BTreeSet::new();
        let ranges = gap_fill_ranges(&stable, &documented);
        assert_eq!(
            ranges,
            vec![
                GapRange {
                    since: None,
                    to: "v1.0.0".to_string(),
                },
                GapRange {
                    since: Some("v1.0.0".to_string()),
                    to: "v1.1.0".to_string(),
                },
            ]
        );
    }

    /// 4.3 (extra) — when the earliest stable tag is already documented,
    /// the next missing tag's lower bound is still the previous stable tag
    /// (documented or not), not `ever`.
    #[test]
    fn gap_fill_range_uses_documented_predecessor_as_lower_bound() {
        let tags = vec!["v1.0.0".to_string(), "v1.1.0".to_string()];
        let stable = stable_release_tags(&tags);
        let mut documented = BTreeSet::new();
        documented.insert("1.0.0".to_string());
        let ranges = gap_fill_ranges(&stable, &documented);
        assert_eq!(
            ranges,
            vec![GapRange {
                since: Some("v1.0.0".to_string()),
                to: "v1.1.0".to_string(),
            }]
        );
    }

    // -----------------------------------------------------------------
    // a72: build_stylist_payload — git-backed end-to-end
    // -----------------------------------------------------------------

    fn git_run(ws: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(ws)
            .output()
            .expect("git invocation");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_repo(ws: &std::path::Path) {
        git_run(ws, &["init", "-q", "--initial-branch=main"]);
        git_run(ws, &["config", "user.email", "test@example.com"]);
        git_run(ws, &["config", "user.name", "test"]);
        git_run(ws, &["config", "commit.gpgsign", "false"]);
    }

    fn write_file(ws: &std::path::Path, rel: &str, contents: &str) {
        let path = ws.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn commit_all(ws: &std::path::Path, msg: &str) -> String {
        git_run(ws, &["add", "-A"]);
        git_run(ws, &["commit", "-q", "-m", msg]);
        git_run(ws, &["rev-parse", "HEAD"])
    }

    fn write_archive(ws: &std::path::Path, date_prefix: &str, slug: &str, proposal_md: &str) {
        let dir_name = format!("{date_prefix}-{slug}");
        let archive_dir = ws.join("openspec/changes/archive").join(&dir_name);
        std::fs::create_dir_all(&archive_dir).unwrap();
        std::fs::write(archive_dir.join("proposal.md"), proposal_md).unwrap();
        let cap_dir = archive_dir.join("specs").join("cap");
        std::fs::create_dir_all(&cap_dir).unwrap();
        std::fs::write(
            cap_dir.join("spec.md"),
            "## ADDED Requirements\n\n### Requirement: stub\nstub.\n",
        )
        .unwrap();
    }

    /// Flagless run fills every missing stable release tag, oldest-first,
    /// each as its own section, each over its `(previous stable tag … this
    /// tag]` range.
    #[test]
    fn build_payload_gapfill_fills_two_missing_tags_oldest_first() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        write_archive(ws, "2026-05-01", "alpha", "## Why\n\nAlpha.\n");
        commit_all(ws, "ship alpha");
        git_run(ws, &["tag", "v1.0.0"]);
        write_archive(ws, "2026-05-02", "beta", "## Why\n\nBeta.\n");
        commit_all(ws, "ship beta");
        git_run(ws, &["tag", "v1.1.0"]);

        let parsed = ParsedChangelogArgs::default();
        let json = match build_stylist_payload(ws, &parsed).unwrap() {
            StylistPayload::Sections(j) => j,
            StylistPayload::NoOp(_) => panic!("expected sections, got no-op"),
        };
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let sections = v["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 2, "two undocumented stable tags → two sections");
        // Oldest-first; first section's lower bound is the `ever` sentinel.
        assert_eq!(sections[0]["version"], "v1.0.0");
        assert_eq!(sections[0]["since"], "ever");
        assert_eq!(sections[0]["to"], "v1.0.0");
        assert_eq!(sections[1]["version"], "v1.1.0");
        assert_eq!(sections[1]["since"], "v1.0.0");
        assert_eq!(sections[1]["to"], "v1.1.0");
        // Each section carries exactly the archive in its range.
        assert_eq!(sections[0]["entries"][0]["slug"], "alpha");
        assert_eq!(sections[0]["entries"].as_array().unwrap().len(), 1);
        assert_eq!(sections[1]["entries"][0]["slug"], "beta");
        assert_eq!(sections[1]["entries"].as_array().unwrap().len(), 1);
    }

    /// 4.4 — no-op: every stable tag already documented → `NoOp`, so the
    /// caller invokes no stylist AND opens no PR.
    #[test]
    fn build_payload_noop_when_all_stable_tags_documented() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        write_archive(ws, "2026-05-01", "alpha", "## Why\n\nAlpha.\n");
        commit_all(ws, "ship alpha");
        git_run(ws, &["tag", "v1.0.0"]);
        write_file(
            ws,
            "CHANGELOG.md",
            "# Changelog\n\n## [1.0.0] - 2026-05-01\n- alpha\n",
        );
        commit_all(ws, "document v1.0.0");

        let parsed = ParsedChangelogArgs::default();
        let payload = build_stylist_payload(ws, &parsed).unwrap();
        assert!(
            matches!(payload, StylistPayload::NoOp(NoOpReason::AlreadyCurrent)),
            "all stable tags documented → AlreadyCurrent no-op (no stylist, no PR)"
        );
    }

    /// 4.4 (extra) — no-op: a repo with only pre-release tags has no
    /// stable release to document.
    #[test]
    fn build_payload_noop_when_no_stable_tags() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        git_run(ws, &["tag", "v1.0.0-rc.1"]);

        let parsed = ParsedChangelogArgs::default();
        let payload = build_stylist_payload(ws, &parsed).unwrap();
        assert!(
            matches!(payload, StylistPayload::NoOp(NoOpReason::NoStableTags)),
            "only a pre-release tag exists → NoStableTags no-op (no PR)"
        );
    }

    /// 4.5 — override: an explicit `--since`/`--to` takes the single-range
    /// path AND does not enumerate the other missing stable tags.
    #[test]
    fn explicit_range_takes_single_section_and_skips_gapfill() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path();
        init_repo(ws);
        write_file(ws, "README.md", "seed\n");
        commit_all(ws, "seed");
        write_archive(ws, "2026-05-01", "alpha", "## Why\n\nAlpha.\n");
        commit_all(ws, "ship alpha");
        git_run(ws, &["tag", "v1.0.0"]);
        write_archive(ws, "2026-05-02", "beta", "## Why\n\nBeta.\n");
        commit_all(ws, "ship beta");
        git_run(ws, &["tag", "v1.1.0"]);
        write_archive(ws, "2026-05-03", "gamma", "## Why\n\nGamma.\n");
        commit_all(ws, "ship gamma");
        git_run(ws, &["tag", "v1.2.0"]);

        let parsed = ParsedChangelogArgs {
            since: Some("v1.0.0".to_string()),
            to: Some("v1.1.0".to_string()),
            workspace_override: None,
        };
        let json = match build_stylist_payload(ws, &parsed).unwrap() {
            StylistPayload::Sections(j) => j,
            StylistPayload::NoOp(_) => panic!("explicit range must produce a section"),
        };
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let sections = v["sections"].as_array().unwrap();
        assert_eq!(sections.len(), 1, "explicit range → exactly one section");
        assert_eq!(sections[0]["since"], "v1.0.0");
        assert_eq!(sections[0]["to"], "v1.1.0");
        // Only beta sits in `(v1.0.0 … v1.1.0]`; gamma / v1.2.0 are not
        // enumerated by the override path.
        let entries = sections[0]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["slug"], "beta");
    }
}
