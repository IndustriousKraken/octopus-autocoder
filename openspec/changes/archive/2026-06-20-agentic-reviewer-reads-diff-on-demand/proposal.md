# Agentic reviewer reads the diff on demand instead of inlining it

## Why

The agentic reviewer reads changed-file *contents* on demand (via `Read`) and
deliberately does not pre-dump them — `reviewer.prompt_budget_chars` does not
apply in agentic mode. But it still inlines the **entire unified diff** into the
prompt, unbounded. On a large pass the inlined diff, plus the files the agent
then reads on demand, can exceed the reviewer model's context window. When that
happens the session ends without a valid `submit_review` submission and the
review fails closed — no silent `Approve`, and the PR carries a visible
`## Code Review: FAILED TO RUN` section so the failure is surfaced rather than
hidden. Even surfaced, a failed review is one the operator must re-run or merge
without. The failure is reproducible on large PRs while smaller PRs review
cleanly, which points at the one unbounded input the agentic path still forces
into every prompt: the diff.

Inlining the whole diff also contradicts the agentic reviewer's own premise —
that the agent pulls context on demand rather than receiving one giant prompt.
File contents already follow that model; the diff is the lone exception.

## What Changes

- The agentic reviewer SHALL extend its on-demand model to the unified diff: the
  diff is provided as a **readable artifact** (a path the agent `Read`s on
  demand), NOT inlined into the prompt body. The prompt carries the change
  briefs, the changed-file path list, AND a reference to the diff artifact —
  so its size is bounded by the brief and path list regardless of how large the
  diff is.
- Nothing is truncated or dropped: the full diff remains available via `Read`,
  and the agent decides how much diff and file context to pull (it can read the
  whole diff, focus on specific hunks, or read the changed files directly). This
  preserves the agentic path's "no budget truncation" property while removing the
  forced-inline that overflows large-PR sessions.
- The diff artifact lives where the read-only sandbox can reach it and is cleaned
  up after the session (it is not committed and does not dirty the worktree).

This change touches only the agentic transport's prompt construction. The
oneshot transport, the `submit_review` contract, verdict handling, `reviewer.mode`
dispatch, the caps, and the fail-closed no-submission behavior are unchanged.

## Impact

- Affected specs: `code-reviewer` (MODIFY **Agentic reviewer mode** — the diff
  is referenced as a readable artifact, not inlined).
- Affected code: `code_reviewer.rs` (`render_agentic_review_prompt` no longer
  inlines `ctx.diff`; it writes the diff to a sandbox-readable artifact and lists
  its path), the agentic-reviewer session setup (write + clean up the artifact),
  and `polling_loop/review_context.rs` (the diff is still gathered; how it is
  surfaced to the prompt changes).
- No change to the oneshot path, which keeps its `prompt_budget_chars`-bounded
  inlining.
