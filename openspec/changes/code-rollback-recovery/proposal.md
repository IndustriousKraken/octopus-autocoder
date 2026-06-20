# Code-rollback recovery: roll back code, unarchive its specs and issues

## Why

When code merges without being gate-checked, the operator cannot trust it — but
the OpenSpec change or issue that drove it is sound and should simply be
re-implemented under the controls. There is no one-place way to do this across a
fleet (currently eleven repositories), and a plain `git reset`/`revert` gets it
wrong: the orchestrator commits the implementation, the archive move, AND the
canonical-spec fold together, so reverting the commits would discard the spec work
entirely — back to before it existed — leaving the operator to hand-rewrite the
very spec they wanted to re-run.

The precise operation is: discard the CODE, but return the archived changes/issues
to the active lanes so they flow back through the gates and get re-implemented. The
existing `rewind` subcommand only deletes the agent branch and unarchives NAMED
changes; it never rolls back base-branch code, so it cannot do this.

## What Changes

- A new `orchestrator-cli` read command — `log` (CLI subcommand AND
  `@<bot> log <repo> [N]` chatops verb) — lists a repo's recent commits so the
  operator picks a rollback depth by looking, from the same management surface.
- A new `orchestrator-cli` recovery operation that rolls a repo's code back by a
  commit count OR to a target SHA, expressed as a PULL REQUEST (not a direct base
  push — it rides the normal flow; an install that pushes directly to the real
  repo is in that posture by its own choice, with git history as the backstop).
  Within the rolled-back range it: restores the code to the target (discards the
  untrusted implementation); unarchives each OpenSpec change back to
  `openspec/changes/<slug>/` with its canon fold undone (pending, to be re-gated
  and re-implemented); unarchives each issue back to the active `issues/` lane;
  and leaves changes/issues archived outside the range untouched.
- The operation is fail-loud and reviewable (the PR body enumerates the rolled-back
  commits and the unarchived changes/issues), requires explicit confirmation
  because it discards code, and supports a dry-run/preview that changes nothing.

## Impact

- Affected specs: `orchestrator-cli` (ADD the `log` listing requirement AND the
  code-rollback recovery requirement). Adjacent to the existing "Rewind
  subcommand"; this is a distinct operation (code rollback vs agent-branch delete
  + named unarchive) and reuses `queue::unarchive` for the spec side.
- Affected code: a new rollback command (CLI + chatops verb + control-socket
  action), a commit-log lister, and the range→archived-units resolver that maps
  rolled-back commits to the changes/issues to unarchive; PR assembly reused from
  the normal flow.
- Destructive (discards code) but PR-gated and confirmation-gated; the operator
  reviews and merges the rollback PR.
