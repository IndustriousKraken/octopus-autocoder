//! In-repo agent guide provisioning (`octopus-md-agent-guide`).
//!
//! A managed repository carries a committed `OCTOPUS.md` at its root that
//! states the in-repo workflow protocols — the issues format, the OpenSpec
//! change format, the canon/archive ownership rules, AND the gate model —
//! for any agent OR human working in the repo who is NOT one of autocoder's
//! own gated agents. `AGENTS.md` (the conventional agent-guide spot) carries
//! a managed marker region that points at `OCTOPUS.md` without clobbering any
//! maintainer-authored content.
//!
//! The content is a SINGLE deterministic source (the `OCTOPUS_MD` const and
//! the `agents_md_region` builder) so the write step AND the stale-comparison
//! use the exact same bytes.
//!
//! Provisioning happens through the daemon's established push + pull-request
//! flow, NOT at init and NOT as a base commit. [`provision_on_agent_branch`]
//! runs in the pass path AFTER the base sync recreates the agent branch: it
//! writes the two files ON THE AGENT BRANCH, stages them, and commits them so
//! they ride the pass's existing push (`git::push_force_with_lease`) + PR-open
//! path (honoring `auto_submit_pr`). Written on the agent branch, the files
//! survive the pre-pass dirty-recovery (which resets to base) AND are never a
//! base-branch commit; they reach base only when the resulting PR merges.

use std::future::Future;
use std::path::Path;

use anyhow::{Context, Result};

use crate::git;

tokio::task_local! {
    /// Per-task guide-provisioning gate, carrying the resolved per-repo
    /// `features.octopus_guide.enabled` value. The daemon binds it once per
    /// polling task via [`scope`] (the production default for the flag is
    /// ENABLED). Mirrors the issues-lane gate ([`crate::lanes::gate`]): a task
    /// that never called [`scope`] reads the unscoped default `false`, so the
    /// many polling-loop tests that drive a pass without opting in are
    /// unaffected by this feature.
    static ENABLED: bool;
}

/// Run `fut` with the guide-provisioning gate bound to `enabled` for the
/// duration of the future. Mirrors the issues-lane gate
/// ([`crate::lanes::gate::scope`]): set once at the top of each polling task
/// from the resolved per-repo flag.
pub fn scope<F>(enabled: bool, fut: F) -> impl Future<Output = F::Output>
where
    F: Future,
{
    ENABLED.scope(enabled, fut)
}

/// The current task's guide-provisioning gate. `false` when the surrounding
/// task did not call [`scope`] (the daemon always scopes from config; an
/// unscoped reader — e.g. a test that does not opt in — is OFF).
pub fn enabled() -> bool {
    ENABLED.try_with(|e| *e).unwrap_or(false)
}

/// The committed in-repo agent guide. A single deterministic source: the
/// write step AND the stale-comparison both use these exact bytes, so the
/// provisioning step is idempotent.
pub const OCTOPUS_MD: &str = r#"# OCTOPUS.md — in-repo agent & contributor guide

This repository is managed by [Octopus Autocoder](https://github.com/rab-autocode/octopus-autocoder).
It carries two workflow protocols any agent or human working in the repo must
follow: an **issues** lane for corrections, and an **OpenSpec change** lane for
behavior changes. This file states both, plus the ownership rules for canon and
the gate model. It is the in-repo reference for readers who are not autocoder's
own gated agents — a coding assistant or speccing agent run directly on the
repo, and human teammates. Autocoder's own agents have these same rules enforced
by the verifier gates and the session sandbox; this file documents them but does
not replace that enforcement.

## Issues protocol (corrections, no spec delta)

An **issue** is a correction: a fix to code that is already correctly specified.
An issue carries **no spec delta** and never contains a `specs/` directory.

An issue takes ONE of two on-disk forms under `issues/`:

- **Single file** — `issues/<slug>.md` (the default): a description, plus an
  optional `## Tasks` checklist.
- **Directory** — `issues/<slug>/` containing `issue.md` AND `tasks.md`:
  required only when the unit must carry a separate artifact (for example a
  quarantined public report body).

Use an issue when the spec is right and the code is wrong. Use a change (below)
when the desired behavior is not yet specified.

## OpenSpec change protocol (behavior changes)

A **change** lives in `openspec/changes/<slug>/` and contains:

- `proposal.md` — why and what changes,
- `tasks.md` — the implementation checklist,
- `design.md` — optional, for non-trivial design decisions,
- spec deltas at `specs/<capability>/spec.md`.

Spec deltas use `## ADDED Requirements`, `## MODIFIED Requirements`,
`## REMOVED Requirements`, and `## RENAMED Requirements` blocks.

A `## MODIFIED Requirements` block **reproduces the canonical requirement's
title EXACTLY** and **retains every existing scenario**. A scenario dropped from
a MODIFIED block silently deletes that scenario from canon when the change is
archived — so a MODIFIED block must restate the full requirement, not just the
part being changed.

Every change MUST pass `openspec validate --strict` before it is considered
ready.

## Canon and archive are autocoder-owned

The canonical specifications under `openspec/specs/` and the archive under
`openspec/changes/archive/` are **autocoder-owned**. A working session writes
ONLY its own change/issue planning artifacts and code. It:

- never edits `openspec/specs/` (canon) directly, and
- never runs `openspec archive`.

Autocoder folds a change's deltas into canon at archive time, after the change
is merged. A change must not pre-apply its own ADDED delta into
`openspec/specs/` — doing so makes the archive abort on a duplicate
requirement.

## Binding contract vs. implementation guidance

The binding contract is the canonical requirements and scenarios under
`openspec/specs/` — and nothing else. `proposal.md` and `design.md` record the
why and the how (including which libraries, versions, or approach); they are
implementation GUIDANCE, not contract.

"Autocoder-owned / immutable" means do NOT EDIT canon or an archived change's
artifacts — it does NOT freeze the implementation choices those files record. A
library, version, or approach named in a `proposal.md` or `design.md` (even an
archived one) MAY be changed in the current code — swap a library, replace a
deprecated dependency, adjust an approach — to address review feedback or
maintenance, with NO new change required, as long as no canonical requirement
changes. Changing a dependency in the current `Cargo.toml` is editing code, not
rewriting the archive. A reviewer asking to replace a deprecated library is in
scope for a revision; do it, rather than declining because a design doc named
the old one.

If a library or approach is genuinely binding (a project-internal component, a
mandated SDK, a required wire format), that constraint lives in a spec
requirement, where it IS the contract — it is never inferred from a passing
mention in a design doc.

## The gate model (gatekeepers fail closed)

A change passes through gatekeepers before its work lands. Each fails closed —
an inability to run a gate is a non-passing outcome, never a pass:

- `[in]` — the change does not contradict itself.
- `[canon]` — the change does not contradict canon, unless it explicitly
  modifies the contradicted requirement.
- `[rules]` — the change conforms to the global engineering rules.
- `[out]` — the merged code implements the spec.

## Further reading

For the fuller OpenSpec documentation, see https://github.com/Fission-AI/OpenSpec.
"#;

/// Stable start marker for the managed `AGENTS.md` region.
pub const AGENTS_REGION_START: &str = "<!-- octopus:guide start -->";
/// Stable end marker for the managed `AGENTS.md` region.
pub const AGENTS_REGION_END: &str = "<!-- octopus:guide end -->";

/// The managed `AGENTS.md` region body (between the markers, exclusive of the
/// marker lines). Deterministic — the write step and the stale-comparison use
/// these exact bytes.
fn agents_region_body() -> &'static str {
    "This repository follows the workflow protocols documented in [OCTOPUS.md](./OCTOPUS.md):\nthe issues format, the OpenSpec change format, the canon/archive ownership rules,\nand the gate model. Read OCTOPUS.md before planning or implementing work here."
}

/// The full managed region, marker lines included, as it appears in
/// `AGENTS.md`. Always terminated by a trailing newline after the end marker.
fn agents_managed_region() -> String {
    format!(
        "{AGENTS_REGION_START}\n{}\n{AGENTS_REGION_END}\n",
        agents_region_body()
    )
}

/// Compose the full desired `AGENTS.md` content given the existing content
/// (`None` when the file is absent). Replaces the managed marker region in
/// place when present; otherwise appends it, leaving all content outside the
/// markers intact. An absent file is created carrying only the managed region.
pub fn compose_agents_md(existing: Option<&str>) -> String {
    let region = agents_managed_region();
    let Some(existing) = existing else {
        return region;
    };
    if let (Some(start), Some(end_marker_pos)) = (
        existing.find(AGENTS_REGION_START),
        existing.find(AGENTS_REGION_END),
    ) {
        // Replace from the start marker through the end marker (and the
        // newline immediately after it, if any), preserving everything
        // outside the markers byte-for-byte.
        let after_end = end_marker_pos + AGENTS_REGION_END.len();
        let mut tail_start = after_end;
        if existing[after_end..].starts_with('\n') {
            tail_start = after_end + 1;
        }
        let mut out = String::with_capacity(existing.len() + region.len());
        out.push_str(&existing[..start]);
        out.push_str(&region);
        out.push_str(&existing[tail_start..]);
        return out;
    }
    // No managed region yet: append it, ensuring a blank line of separation
    // from any preceding content.
    let mut out = String::with_capacity(existing.len() + region.len() + 2);
    out.push_str(existing);
    if !existing.is_empty() {
        if !existing.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&region);
    out
}

/// Outcome of a provisioning attempt on the agent branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionOutcome {
    /// Provisioning is disabled for this repository (`features.octopus_guide
    /// .enabled == false`): nothing written, nothing committed.
    Disabled,
    /// The guide was already present AND current on the base-synced tree:
    /// nothing written, nothing committed (no diff, no PR).
    AlreadyCurrent,
    /// `OCTOPUS.md` and/or the managed `AGENTS.md` region were written or
    /// refreshed AND committed on the agent branch. The commit rides the
    /// pass's existing push + PR path.
    Committed,
}

/// Whether the on-disk `OCTOPUS.md` matches the canonical content.
fn octopus_md_current(workspace: &Path) -> bool {
    match std::fs::read_to_string(workspace.join("OCTOPUS.md")) {
        Ok(content) => content == OCTOPUS_MD,
        Err(_) => false,
    }
}

/// Whether the on-disk `AGENTS.md` already carries the current managed region
/// (i.e. recomposing it would be a no-op).
fn agents_md_current(workspace: &Path) -> bool {
    let existing = std::fs::read_to_string(workspace.join("AGENTS.md")).ok();
    match existing.as_deref() {
        Some(content) => compose_agents_md(Some(content)) == content,
        None => false,
    }
}

/// Provision `OCTOPUS.md` and the `AGENTS.md` reference on the recreated agent
/// branch, committing them so they ride the pass's push + PR path.
///
/// Idempotent and gated:
///
/// - When `enabled` is `false`, returns [`ProvisionOutcome::Disabled`] before
///   any write — no file touched, no commit, no PR.
/// - When `OCTOPUS.md` is already current AND `AGENTS.md` carries the current
///   managed region, returns [`ProvisionOutcome::AlreadyCurrent`] — no write,
///   no commit, no PR (so no empty PR, no churn).
/// - Otherwise writes `OCTOPUS.md` (when absent or stale) and refreshes the
///   managed `AGENTS.md` region (creating `AGENTS.md` if absent, preserving any
///   content outside the markers), stages BOTH with `git::add_all`, commits on
///   the agent branch, and returns [`ProvisionOutcome::Committed`].
///
/// MUST be called on the recreated agent branch, after the base sync — never on
/// `base_branch`. The caller treats a write/commit failure as non-fatal
/// (logs WARN, the pass proceeds).
pub fn provision_on_agent_branch(workspace: &Path, enabled: bool) -> Result<ProvisionOutcome> {
    if !enabled {
        return Ok(ProvisionOutcome::Disabled);
    }

    let octopus_ok = octopus_md_current(workspace);
    let agents_ok = agents_md_current(workspace);
    if octopus_ok && agents_ok {
        return Ok(ProvisionOutcome::AlreadyCurrent);
    }

    if !octopus_ok {
        let path = workspace.join("OCTOPUS.md");
        std::fs::write(&path, OCTOPUS_MD)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    if !agents_ok {
        let path = workspace.join("AGENTS.md");
        let existing = std::fs::read_to_string(&path).ok();
        let composed = compose_agents_md(existing.as_deref());
        std::fs::write(&path, composed)
            .with_context(|| format!("writing {}", path.display()))?;
    }

    // Stage and commit on the agent branch so the two files ride the pass's
    // existing push + PR path. `git::add_all`'s CLI-artifact excludes only
    // affect untracked per-run artifacts, not these committable files.
    git::add_all(workspace).context("staging OCTOPUS.md / AGENTS.md")?;
    git::commit(
        workspace,
        "docs: provision OCTOPUS.md in-repo agent guide + AGENTS.md reference",
    )
    .context("committing OCTOPUS.md / AGENTS.md")?;

    Ok(ProvisionOutcome::Committed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::TempDir;

    fn run_git(path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    /// A fixture mimicking the base-synced + agent-branch-recreated tree: a
    /// repo with one base commit, then a recreated agent branch checked out.
    fn fixture_agent_branch() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        run_git(&path, &["init", "-q", "-b", "main"]);
        run_git(&path, &["config", "user.email", "test@example.com"]);
        run_git(&path, &["config", "user.name", "test"]);
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        run_git(&path, &["add", "README.md"]);
        run_git(&path, &["commit", "-q", "-m", "initial"]);
        // Recreate the agent branch from base, as the pass's base sync does.
        run_git(&path, &["checkout", "-q", "-B", "agent-q"]);
        (dir, path)
    }

    fn commit_count_main_to_agent(path: &Path) -> usize {
        crate::git::rev_list_count(path, "main..agent-q").unwrap()
    }

    fn info_exclude_contents(path: &Path) -> String {
        std::fs::read_to_string(path.join(".git/info/exclude")).unwrap_or_default()
    }

    #[test]
    fn missing_guide_enabled_writes_both_files_and_commits() {
        let (_dir, path) = fixture_agent_branch();
        let outcome = provision_on_agent_branch(&path, true).unwrap();
        assert_eq!(outcome, ProvisionOutcome::Committed);
        // Both files exist on the agent-branch tree with the canonical bytes.
        assert_eq!(
            std::fs::read_to_string(path.join("OCTOPUS.md")).unwrap(),
            OCTOPUS_MD
        );
        let agents = std::fs::read_to_string(path.join("AGENTS.md")).unwrap();
        assert!(agents.contains(AGENTS_REGION_START));
        assert!(agents.contains(AGENTS_REGION_END));
        assert!(agents.contains("OCTOPUS.md"));
        // A commit was produced on the agent branch (rides push + PR).
        assert_eq!(commit_count_main_to_agent(&path), 1);
        // Both files are tracked (committed), not merely present.
        let tracked = Command::new("git")
            .args(["ls-files", "OCTOPUS.md", "AGENTS.md"])
            .current_dir(&path)
            .output()
            .unwrap();
        let listed = String::from_utf8_lossy(&tracked.stdout);
        assert!(listed.contains("OCTOPUS.md"), "OCTOPUS.md must be tracked");
        assert!(listed.contains("AGENTS.md"), "AGENTS.md must be tracked");
        // The files MUST NOT be registered in .git/info/exclude.
        let excludes = info_exclude_contents(&path);
        assert!(
            !excludes.contains("OCTOPUS.md"),
            "OCTOPUS.md must not be excluded: {excludes}"
        );
        assert!(
            !excludes.contains("AGENTS.md"),
            "AGENTS.md must not be excluded: {excludes}"
        );
    }

    #[test]
    fn disabled_writes_nothing_and_makes_no_commit() {
        let (_dir, path) = fixture_agent_branch();
        let outcome = provision_on_agent_branch(&path, false).unwrap();
        assert_eq!(outcome, ProvisionOutcome::Disabled);
        assert!(!path.join("OCTOPUS.md").exists(), "OCTOPUS.md must not be written");
        assert!(!path.join("AGENTS.md").exists(), "AGENTS.md must not be written");
        assert_eq!(
            commit_count_main_to_agent(&path),
            0,
            "disabled feature must contribute no commit"
        );
    }

    #[test]
    fn already_current_writes_nothing_and_makes_no_commit() {
        let (_dir, path) = fixture_agent_branch();
        // Pre-seed a current guide and commit it onto the agent branch so the
        // tree is clean and the next provision call is a no-op.
        provision_on_agent_branch(&path, true).unwrap();
        let baseline = commit_count_main_to_agent(&path);
        let outcome = provision_on_agent_branch(&path, true).unwrap();
        assert_eq!(outcome, ProvisionOutcome::AlreadyCurrent);
        assert_eq!(
            commit_count_main_to_agent(&path),
            baseline,
            "already-current must produce no additional commit"
        );
    }

    #[test]
    fn stale_octopus_md_is_rewritten_and_committed() {
        let (_dir, path) = fixture_agent_branch();
        std::fs::write(path.join("OCTOPUS.md"), "stale content\n").unwrap();
        run_git(&path, &["add", "OCTOPUS.md"]);
        run_git(&path, &["commit", "-q", "-m", "stale guide"]);
        let baseline = commit_count_main_to_agent(&path);
        let outcome = provision_on_agent_branch(&path, true).unwrap();
        assert_eq!(outcome, ProvisionOutcome::Committed);
        assert_eq!(
            std::fs::read_to_string(path.join("OCTOPUS.md")).unwrap(),
            OCTOPUS_MD,
            "stale OCTOPUS.md must be rewritten to canonical content"
        );
        assert_eq!(
            commit_count_main_to_agent(&path),
            baseline + 1,
            "rewriting a stale guide must produce a commit"
        );
    }

    #[test]
    fn existing_agents_md_content_is_not_clobbered() {
        let (_dir, path) = fixture_agent_branch();
        let unrelated = "# Project agent notes\n\nUse two-space indentation.\n";
        std::fs::write(path.join("AGENTS.md"), unrelated).unwrap();
        run_git(&path, &["add", "AGENTS.md"]);
        run_git(&path, &["commit", "-q", "-m", "maintainer AGENTS.md"]);
        let outcome = provision_on_agent_branch(&path, true).unwrap();
        assert_eq!(outcome, ProvisionOutcome::Committed);
        let agents = std::fs::read_to_string(path.join("AGENTS.md")).unwrap();
        assert!(
            agents.contains("Use two-space indentation."),
            "pre-existing content must survive: {agents}"
        );
        assert!(
            agents.contains(AGENTS_REGION_START) && agents.contains("OCTOPUS.md"),
            "the managed region must be added: {agents}"
        );
    }

    #[test]
    fn compose_agents_md_replaces_region_preserving_surroundings() {
        let existing = format!(
            "# Top\n\n{AGENTS_REGION_START}\nOLD BODY\n{AGENTS_REGION_END}\n\n## Footer\n"
        );
        let out = compose_agents_md(Some(&existing));
        assert!(out.starts_with("# Top\n"), "leading content preserved: {out}");
        assert!(out.contains("## Footer"), "trailing content preserved: {out}");
        assert!(!out.contains("OLD BODY"), "stale region body replaced: {out}");
        assert!(out.contains("OCTOPUS.md"), "current region present: {out}");
        // Idempotent: recomposing the result is a no-op.
        assert_eq!(compose_agents_md(Some(&out)), out);
    }

    #[test]
    fn compose_agents_md_creates_region_only_when_absent() {
        let out = compose_agents_md(None);
        assert!(out.starts_with(AGENTS_REGION_START));
        assert!(out.trim_end().ends_with(AGENTS_REGION_END));
    }
}
