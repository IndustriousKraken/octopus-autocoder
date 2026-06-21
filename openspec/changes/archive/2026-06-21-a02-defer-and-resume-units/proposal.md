# Defer and resume a change or issue

## Why

A change that goes perma-stuck — repeatedly kicked back, blocking its lane — has no
resting state today. The operator can clear markers (which re-feeds the loop) or
roll code back (destructive), but cannot set the unit aside intact and come back to
it later. Real work gets stuck on a hard problem that is not worth solving right
now; the choices are "keep failing" or "lose it." There needs to be a way to defer
a unit — change OR issue — out of both lanes without deleting or revising it, and
to undefer (resume) it when ready. Because defer discards no code and is fully
reversible, it needs only a normal acknowledgement, not the two-step confirmation
the destructive rollback command requires.

## What Changes

- Two new chatops verbs (`chatops-manager`): `@<bot> defer <repo-substring> <slug>`
  and `@<bot> undefer <repo-substring> <slug>`, resolving the repo by the same
  selector rule the other operator commands use. The verb auto-detects whether
  `<slug>` is a change or an issue by where it lives. Both reply with a single
  acknowledgement (no confirmation dance) or a clear error.
- Two new control-socket actions and their operation semantics (`orchestrator-cli`):
  defer moves the unit to a committed location OUTSIDE both lanes —
  `deferred-changes/<slug>/` for a change, `deferred-issues/<slug>` for an issue
  (preserving single-file `.md` OR directory form) — and undefer moves it back to
  its original lane location.
- The move rides the established agent-branch + push + PR flow (honoring per-repo
  `auto_submit_pr`), NOT a direct base-branch commit.
- A lanes-ignore-deferred guarantee: the changes lane enumerates only
  `openspec/changes/` and the issues lane only `issues/`, so a deferred unit at the
  repo root is invisible to selection with no lane change required.

## Impact

- Affected specs: `chatops-manager` (ADD: the `defer`/`undefer` verbs, their
  acknowledgements and error messages) and `orchestrator-cli` (ADD: the
  defer/undefer operation semantics, deferred locations, the agent-branch + PR
  mechanism, and the lanes-ignore-deferred guarantee).
- Affected code: the chatops verb parser + dispatcher (`chatops/operator_commands.rs`),
  two new control-socket handlers (`control_socket.rs`), and a daemon-side move
  helper that rides the pass's push + PR path (mirroring `octopus_guide.rs` and the
  rollback-recovery handler). No change to `queue.rs` or `lanes/issues.rs`
  enumeration — they already ignore non-lane directories.
- Depends on `a01-workspace-ops-preempt-and-serialize` (implement first): defer and
  undefer are workspace-mutating control-socket ops, so they use its
  `preempt_and_acquire_busy_marker` helper and conform to its **Workspace-mutating
  control-socket operations preempt and serialize against the pass** invariant —
  preempting any in-flight pass and holding the per-repo busy marker for the move,
  exactly as the rollback handler does. Without this the defer handler would race a
  concurrent agentic session and corrupt the workspace (the bug a01 fixes).
