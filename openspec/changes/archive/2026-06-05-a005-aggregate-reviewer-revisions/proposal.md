## Why

Reviewer-initiated revisions are dispatched one-at-a-time: `revisions.rs` loops `for comment in comments`, so each `<!-- reviewer-revision -->` comment is its own executor run and its own auto-revision-cap increment. PR #95 showed the cost vividly — the initial review produced three requests, two of which were the *same* `GateRegistry::standard()` concern worded twice, processed separately; the second came back "✅ Revision evaluated, no change made" — a full executor run and a cap slot spent on a no-op because the first already fixed it.

Concerns from one review are usually related and small ("change this function" + "update its two callers" is one fix, not two). Splitting them into N runs:
- burns N of the auto-revision cap instead of 1 (a 3-concern review eats most of the default cap);
- pays for N cold codebase re-reads instead of one warm pass — slower, and worse results since related fixes aren't reasoned about together;
- produces redundant no-ops like #95's duplicate.

Separately, a46 made auto-revise fire on actionable concerns **regardless of verdict**, which is the "it passed but still revised" surprise. With a004 making security findings `Block`, the safe-and-predictable default is to auto-revise on `Block` only — security still gets auto-fixed (it now Blocks), while style `Concerns` stay advisory.

## What Changes

**Aggregate reviewer-initiated revisions into one run per review.** All `<!-- reviewer-revision -->` requests produced by a single review SHALL be collected and dispatched as ONE revision run — one executor invocation carrying all the concerns together — counting as ONE increment against the auto-revision cap, with one operator-visible summary of what is being addressed. The aggregated run sees all concerns in one warm pass (no duplicate no-ops; related fixes reasoned together).

**`auto_revise` becomes a tri-state, default `block`.** `auto_revise: block | actionable | off` (legacy `true` → `actionable`, legacy `false` → `off`). Default `block`: auto-revise fires (aggregated) only on a `Block` verdict. `actionable` restores the a46 behavior (fire on actionable concerns regardless of verdict). `off` disables it. With a004 (security → `Block`), `block` still auto-fixes the security-critical findings while non-security `Concerns` stay advisory the operator can act on with `@<bot> revise`.

## Impact

- **Affected specs:** `orchestrator-cli` — ADD `Reviewer-initiated revisions from one review dispatch as a single run`. `code-reviewer` — ADD `auto_revise is a tri-state defaulting to block`.
- **Affected code:** `revisions.rs` — collect a review's reviewer-revision comments and dispatch one aggregated revision (one executor invocation, one cap increment, one summary) instead of looping per comment. `code_reviewer.rs` / config — `auto_revise` parses `block|actionable|off` (with the legacy bool mapping) and gates the dispatch accordingly.
- **Operator-visible behavior:** a multi-concern review opens one revision run addressing everything, not N (no more cap exhaustion or duplicate no-ops, one clear summary). By default auto-revise fires only on `Block` (so a `Concerns` review no longer silently rewrites); flip `auto_revise: actionable` for the old fire-on-everything behavior.
- **Dependencies:** complements a004 (so `block` default still auto-fixes security). Independent of a003.
- **Acceptance:** `cargo test` passes; `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a005-aggregate-reviewer-revisions --strict` passes. Tests: a review with N actionable concerns dispatches one revision run with one cap increment (not N); `auto_revise: block` does not fire on a `Concerns` verdict but does on `Block`; the legacy bool maps correctly.
