## Why

autocoder's GitHub comment triggers are **ungated**. Any GitHub user who comments on an autocoder-opened PR can fire billed/LLM work:

- `@<bot> revise <text>` re-invokes the executor and force-pushes a revision to the agent branch;
- `@<bot> code-review` runs a fresh reviewer pass.

The revision dispatcher (`autocoder/src/revisions.rs`) filters only the bot's **own** comments — there is no check on *who* the commenter is (no `author_association`, no maintainer allowlist). Human-initiated `@<bot> revise` is also **uncapped** (only reviewer-initiated auto-revisions are bounded). On a public repository this means an arbitrary member of the public can drive revisions ("revise this to add a server that floods example.com") or spam reviews/revisions to burn cost — the exact abuse the project's trust model must prevent. The Slack chatops surface is already gated (private-channel allowlist — `chatops-manager` "Drop-before-dispatch inbound filters"); the GitHub surface is the open door.

This closes that door before the app is shared more widely.

## What Changes

**Authorize comment-sourced verbs by author association.** Before dispatching any verb parsed from a GitHub PR/issue comment (`revise`, `code-review`, AND any future comment verb), the daemon authorizes the commenter: their GitHub `author_association` must be in an allowlist (default `OWNER` / `MEMBER` / `COLLABORATOR` — exactly those with repo write/triage) OR their `login` must be in a configured `allowed_users` list. Unauthorized comments are dropped before any work, the seen-marker advanced so they do not re-fire, and the drop is logged; an optional, default-off one-time decline reply. Default-deny.

**Cap comment-triggered work per PR.** Human-initiated `@<bot> revise` gains a per-PR cap, closing the currently-uncapped path and complementing the existing auto-revision cap (`executor.max_auto_revisions_per_pr`) and re-review cap (`reviewer.max_code_reviews_per_pr`). Past the cap, further `revise` triggers on that PR are declined without invoking the executor.

**ADD-only by design.** a000 introduces two new requirements rather than modifying the `revise` (orchestrator-cli) or `code-review` (code-reviewer) verb requirements, so it does NOT collide with in-flight changes (a53, a57) that touch those requirements, AND it sorts first in the queue for immediate release. The gate is wired into the dispatcher as a precondition on every comment-sourced verb.

## Impact

- **Affected specs:** `orchestrator-cli` — ADD `GitHub comment-sourced verbs require an authorized commenter`; ADD `Human-initiated PR revisions are rate-capped per PR`.
- **Affected code:** the revision dispatcher (`autocoder/src/revisions.rs`) gains an authorization check before dispatching any comment verb, reading `author_association` (and author `login`) from the GitHub comments API; config gains `github.command_authorization.{allowed_associations, allowed_users, decline_comment}` AND `executor.max_revise_triggers_per_pr`.
- **Operator-visible behavior:** only repo owners/members/collaborators (plus any configured `allowed_users`) can trigger `revise` / `code-review` on PRs; unauthorized comments are silently ignored (or politely declined once, if configured). Heavy operators can raise the per-PR revise cap.
- **Security posture:** closes the public-trigger hole. Defense-in-depth: a companion change (**a001**) should add untrusted-content **quarantine** for the revise text / scout-read issue bodies fed to the LLM (the injection framing — delimit as data, scope from the trigger type, never obey content), behind this authorization gate. a000 is the gate; a001 is the quarantine.
- **Dependencies:** none — ADD-only, independent of the fleet stream and of a53/a57. Sorts and processes first.
- **Acceptance:** `cargo test` passes; `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a000-harden-external-triggers --strict` passes.
