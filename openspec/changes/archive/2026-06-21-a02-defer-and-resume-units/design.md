# Design

OpenSpec: this change adds the `defer`/`undefer` operator commands. See
`proposal.md` for the why and scope.

## Auto-detecting change vs issue

The verb takes one `<slug>` and infers the lane by where the unit lives, so the
operator never has to spell out "this is a change" / "this is an issue":

- A CHANGE is `openspec/changes/<slug>/` (a directory containing `proposal.md`),
  mirroring the changes lane's enumeration root (`CHANGES_SUBDIR = "openspec/changes"`,
  `queue.rs:16`; `list_pending` reads only that dir, `queue.rs:52-138`).
- An ISSUE is `issues/<slug>.md` (single-file) OR `issues/<slug>/` (directory),
  mirroring the issues lane's two on-disk forms (`ISSUES_SUBDIR = "issues"`,
  `lanes/issues.rs:34`; `list_ready` accepts both shapes, `lanes/issues.rs:360-423`).

Detection rules for `defer`:
- Exactly one of the two lane locations exists → defer that unit.
- Neither exists → clear "not found" error.
- Both a `openspec/changes/<slug>/` AND an `issues/<slug>(.md|/)` exist for the same
  slug → ambiguous; the error names both candidate locations and asks the operator
  to resolve the collision (the same slug should not name two units).

For `undefer`, detection inverts: the unit is sought under `deferred-changes/<slug>/`
then `deferred-issues/<slug>(.md|/)`; the same not-found / ambiguous handling applies.

## Deferred locations (committed, outside both lanes)

A deferred unit is MOVED — not copied, not marked — to a sibling directory at the
repo root that neither lane enumerates:

- A change: `openspec/changes/<slug>/` → `deferred-changes/<slug>/`.
- An issue: `issues/<slug>.md` → `deferred-issues/<slug>.md`, OR `issues/<slug>/` →
  `deferred-issues/<slug>/` (the on-disk form is preserved exactly).

These roots are invisible to selection for free: the changes lane reads only
`openspec/changes/` and the issues lane reads only `issues/`. `deferred-changes/`
and `deferred-issues/` are neither — no lane code change is required. Undefer is the
exact inverse move, returning the unit to its original lane location.

Markers travel with the unit (they live inside a directory unit, or as siblings for
a single-file issue) but defer does NOT clear them: the unit is preserved as-is so a
later undefer resumes exactly where it left off. (Removing a marker is the separate
`clear-perma-stuck` / `clear-revision` path.) A deferred unit's perma-stuck marker
is simply dormant while deferred because no lane enumerates the deferred root.

## Move mechanism: agent-branch + PR, never a base commit

The move is performed by the daemon ON THE AGENT BRANCH and rides the established
push + PR-creation flow, exactly like the rollback-recovery handler and the
OCTOPUS.md provisioning:

1. The control-socket handler resolves the workspace (`workspace::resolve_path`),
   ensures it is initialized, checks out the base branch, and syncs it to the remote
   tip — the same preamble `handle_rollback_recovery` uses (`control_socket.rs`
   ~3246-3276).
2. It recreates the agent branch at the base tip (`git::recreate_branch`,
   `git.rs:274-277`), performs the directory move on the working tree, then stages
   and commits (`git::add_all` / `git::commit`, `git.rs:279-291,335-338`) — the same
   write-then-commit shape as `octopus_guide::provision_on_agent_branch`
   (`octopus_guide.rs:255-290`).
3. It pushes the agent branch (`git::push_force_with_lease`, `git.rs:428-435`) and
   goes through the PR-creation path, honoring per-repo `auto_submit_pr`: a PR when
   true/default, the `BranchPushedNoPr` outcome (`outcome": "branch_pushed_no_pr"`,
   `pr_open.rs:168-192`) when false.

A direct commit to the base branch is rejected by design: each pass syncs the base
with `git pull --ff-only` and recreates the agent branch from it (`pass.rs:473-475`),
and any dirty mid-iteration state is wiped by `attempt_dirty_workspace_recovery`
(`git reset --hard origin/<base>` + `git clean -fd`, `pass.rs:607-612`). A base
commit that diverges from `origin/<base>` would break the ff-only pull and be lost,
and it violates canon's prohibition on base-branch commits outside a PR. The PR is
the unit's audit trail; its body states what was deferred (or resumed) and from/to
which location.

## Acknowledgement: single ack, NOT two-step confirm

Defer and undefer are reversible and discard no code, so neither needs the heavy
two-step confirmation the destructive commands require. Contrast:

- `wipe-workspace` and `rollback` use a channel-keyed pending-confirmation store
  with a 60s TTL (`rollback_pending` / `take_valid`, `operator_commands.rs`
  ~3498-3513) because they destroy local state / discard code.
- `defer`/`undefer` reply with a single immediate acknowledgement, the same shape as
  `clear-perma-stuck` (`✓ ...` on success, `✗ ...` on a clear error). No pending
  store, no `defer-confirm` verb.

This mirrors the same-selector and same-control-socket conventions of the existing
operator commands (`match_repo`, `operator_commands.rs:1470`; actions dispatched via
the Unix-domain control socket).

## Idempotency and errors

- `defer <slug>` for a slug in neither lane → `✗ no change or issue '<slug>' on <repo>`.
- `defer <slug>` for a slug ALREADY at the deferred location (and absent from the
  lane) → reports already-deferred, a no-op success (`✓ '<slug>' is already deferred
  on <repo>`); no second PR.
- `defer <slug>` where the slug exists in BOTH lanes → ambiguous error naming both.
- `undefer <slug>` for a slug not under any deferred root → `✗ no deferred change or
  issue '<slug>' on <repo>`.
- `undefer <slug>` where the unit already exists back in its lane (and absent from
  the deferred root) → reports already-active, a no-op success.

## What this is NOT

- Not a marker: defer does not write a `.deferred.json` sibling and leave the unit in
  place — a marker inside the lane root would still be enumerated unless every lane
  walker learned to skip it. Moving the unit out of the lane root is the smaller,
  lane-change-free mechanism and is the explicit design choice here.
- Not a delete and not a revise: the unit's content is preserved byte-for-byte.
