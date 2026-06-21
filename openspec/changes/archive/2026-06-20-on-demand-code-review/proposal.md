# On-demand code review of a PR, commit, or target from chatops/CLI

## Why

The reviewer only runs inside the per-pass flow, over that pass's diff. An
operator investigating a problem cannot point it at a specific PR, a past commit,
a file they suspect, or an area they can only describe. That is exactly the
moment review is most useful — e.g. discovering the `[out]` gate was broken and
wanting to review that file immediately, or vetting a recent PR before trusting
it. With a commit-log surface already planned, on-demand review closes the loop:
look at the history, review a suspect target, then decide (roll back if recent,
spec/issue a fix if not).

## What Changes

- A new `orchestrator-cli` requirement adds an on-demand review command (CLI +
  `@<bot> review <repo> <target>`) where `<target>` is `pr <N>`, `commit <sha>`,
  `files <path...>`, or a free-text description (the reviewer locates the files
  itself). It reuses the existing agentic reviewer and reports the verdict back
  to chat (and, for a PR target, optionally as a PR comment). It is advisory and
  read-only — no revision, no code change, no marker change.
- A large target (broad area / whole codebase) is scoped by chunking into bounded
  reviewer sessions and aggregating, rather than overflowing one prompt.
- The `code-reviewer` "Agentic reviewer mode" requirement is extended so the
  reviewer's input is a review SURFACE that is either a diff (the pass, or a
  PR/commit) OR a target file-set/area with no diff; the diff-based behavior is
  unchanged and a new scenario covers the no-diff target.

## Impact

- Affected specs: `orchestrator-cli` (ADD the on-demand review command),
  `code-reviewer` (MODIFY "Agentic reviewer mode" to accept a diff-or-target
  surface; all existing clauses and scenarios preserved, one scenario added).
- Affected code: a `review` verb + control-socket action + CLI subcommand; a
  target resolver (PR/commit → local-clone diff via `git show`/`git diff`;
  files → target manifest; description → agent-located files); a target-oriented
  `ReviewContext` (diff optional); the chunk-and-aggregate path for large targets.
  Reuses the agentic reviewer session, `submit_review`, and the existing repo
  selector.
- Pairs with the planned commit-log (`log`) and code-rollback changes to form one
  disaster-investigation loop: list → review → decide (rollback or spec/issue).
