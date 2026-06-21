# Design

## Provision through the pull-request flow, not server provisioning

`autocoder install` (`autocoder/src/cli/install.rs`) provisions a HOST — it writes
`/etc/autocoder` config, a systemd unit, and a system user behind the
`SystemActions` trait. It never clones or touches a managed repository's working
tree. The managed repo is cloned and synced by the daemon in
`workspace::ensure_initialized` (`autocoder/src/workspace.rs`), and a unit's work is
turned into a branch, a commit, and a pull request by the polling loop. OCTOPUS.md
and the AGENTS.md reference are therefore provisioned by the daemon's normal
push + PR flow, not by host provisioning.

## Why a dedicated PR — and not a write-at-init base commit

The point of OCTOPUS.md and AGENTS.md is to serve readers who are NOT autocoder's
own agents: a coding agent or speccing agent run directly on the repo, and human
teammates. Those readers see the committed base-branch tree. So both files must
end up committed on the base branch — but the only mechanism the daemon has to put
content on the base branch is to merge a pull request. A direct base-branch commit
is impossible to keep:

- **Dirty-recovery wipes an untracked init write.** The per-pass dirty check
  (`autocoder/src/polling_loop/commits.rs`, the `git status --porcelain` gate near
  line 380) runs `attempt_dirty_workspace_recovery` (`commits.rs:578-582`:
  `git reset --hard origin/<base>` + `git clean -fd`) whenever the tree is dirty.
  A file written UNTRACKED in `ensure_initialized` (workspace.rs:82-266, before the
  base checkout) is swept away before any commit point.
- **A local base commit cannot survive base-sync.** `ensure_initialized` does not
  check out the base branch (the base checkout is later, `commits.rs:473`); even if
  it did, a local commit on `base_branch` diverges from `origin/<base>`, so the
  per-pass `git pull --ff-only` (`commits.rs:474`) fails, the canonical
  dirty-recovery `git reset --hard origin/<base>` discards it, and it violates the
  canon rule forbidding base-branch commits outside a pull request
  (`orchestrator-cli` spec).
- **It is never pushed.** A local-only commit is not remote-visible, defeating the
  whole purpose of a committed in-repo guide.

The mechanism that does work is the one every change already uses. The base sync at
`commits.rs:473-475` checks out `repo.base_branch`, runs `git pull --ff-only`, and
recreates the agent branch (`git::recreate_branch`). Pass work happens on that
recreated agent branch. So the daemon writes OCTOPUS.md and refreshes the AGENTS.md
region THERE — after base sync, on the agent branch — stages them with
`git::add_all`, commits with `git::commit`, and rides the established push +
PR-creation path (`git::push_force_with_lease` of the agent branch in
`autocoder/src/polling_loop/pass.rs:165`, then `open_pull_request`). Written on the
agent branch after base sync, the files are never wiped by dirty-recovery (which
resets to base, not the agent branch) and are never a base-branch commit; they reach
base only when the pull request merges.

### Honoring the per-repo PR toggle (auto_submit_pr)

The PR-open step already honors `RepositoryConfig::auto_submit_pr`
(`autocoder/src/polling_loop/pr_open.rs:168-193`): with `auto_submit_pr: true` (the
default) it opens a pull request; with `auto_submit_pr: false` it pushes the agent
branch and surfaces the branch (the `BranchPushedNoPr` outcome via
`maybe_post_branch_pushed_no_pr`) without opening a PR. Because the guide files ride
the agent branch as ordinary committed content, this toggle applies to them with no
extra wiring: PR when enabled, pushed-branch-no-PR when not.

## Per-repo guide-provisioning flag (default ENABLED)

Whether the daemon provisions the guide at all is a per-repository feature flag,
modeled on the existing per-repo feature flags in `autocoder/src/config.rs`
(`FeaturesConfig` near line 650; `IssuesFeatureConfig` near line 823 is the closest
analogue — an `enabled: bool` with a `default_*_enabled()` defaulter). A new
`features.octopus_guide` block carries `enabled: bool`, defaulting to `true` via a
`default_octopus_guide_enabled() -> true`.

Default-ON rationale: an in-repo agent guide benefits every managed repository the
operator owns, and the operator is by definition the decision-maker for repos they
manage. The override exists for the cases where a metafile is unwelcome: an operator
who simply does not want metafiles in a particular repo, or a contributor running
autocoder against a third-party open-source repo where adding metafiles is not their
call. When the flag is disabled the daemon writes nothing and opens no bootstrap PR
for the guide.

## Idempotency and staleness — no needless PR

OCTOPUS.md content is a single deterministic source in code; the write step compares
the base-branch file against the generated content and provisions only on absence or
mismatch. The AGENTS.md reference is a single marked region created or refreshed in
place, never clobbering surrounding content. When the base branch already carries a
current guide, the daemon writes nothing, commits nothing, and opens no pull request
— no empty PR, no churn. The bootstrap PR is opened only when the guide is missing or
out of date on base.

## No shared "single source" refactor

There is no shared format-definition source the prompts render from. The prompts
under `prompts/` are standalone hand-authored `.md` files loaded by
`autocoder/src/prompts/loader.rs` via `include_str!`; each restates the formats it
needs independently (e.g. `implementer.md` already states the no-archive rule and
links the OpenSpec docs). Unifying them is out of scope. This change authors
OCTOPUS.md's content directly and adds a one-line "read OCTOPUS.md when present"
directive to each default prompt. Prose duplication between the prompts and
OCTOPUS.md is accepted for now.
