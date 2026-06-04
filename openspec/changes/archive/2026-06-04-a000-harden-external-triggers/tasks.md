# Implementation tasks

## 1. Config

- [x] 1.1 Add a `github.command_authorization` block: `allowed_associations: Vec<String>` (default `["OWNER", "MEMBER", "COLLABORATOR"]`), `allowed_users: Vec<String>` (default `[]`), `decline_comment: bool` (default `false`). Validate `allowed_associations` entries against the known GitHub association set at config load; reject unknown values with a clear error.
- [x] 1.2 Add `executor.max_revise_triggers_per_pr: u32` (default `10`).
- [x] 1.3 Document both in `docs/CONFIG.md` (and the example config), and the authorization model + default-deny behavior in `docs/OPERATIONS.md` / `docs/CHATOPS.md` where the `revise` / `code-review` verbs are described.

## 2. Authorization gate (`autocoder/src/revisions.rs`)

- [x] 2.1 When fetching PR/issue comments, capture each comment's `author_association` AND author `login` (extend the comment-fetch deserialization if it currently drops these fields).
- [x] 2.2 In the dispatcher, after a comment parses as a verb (`revise`, `code-review`, and any other comment-sourced verb) and after the existing bot-self-author filter, evaluate authorization: pass if `author_association ∈ allowed_associations` OR `login ∈ allowed_users`. Treat an absent/unrecognized association as unauthorized.
- [x] 2.3 On authorization failure: do not dispatch; advance the seen-marker past the comment so it does not re-fire; log at INFO with `login` + association. If `decline_comment` is `true`, post exactly one decline reply (track via the seen-marker / state so it is not reposted).
- [x] 2.4 On authorization success: proceed to the existing verb handling unchanged.

## 3. Per-PR human-revise cap (`autocoder/src/revisions.rs` + per-PR state)

- [x] 3.1 Add a human-revise counter to the per-PR state file (distinct from the auto-revision and code-review counters).
- [x] 3.2 Before invoking the executor for an authorized `@<bot> revise`, check the counter against `executor.max_revise_triggers_per_pr`. If at the cap, post exactly one decline notice, advance the seen-marker, and do not invoke the executor. Otherwise invoke and increment.
- [x] 3.3 Confirm the auto-revision and re-review caps are untouched and independent.

## 4. Tests

- [x] 4.1 Authorization: a synthetic comment payload with `author_association: COLLABORATOR` parsing as `revise` is authorized → dispatch proceeds; the same payload with `author_association: NONE` is dropped → no executor invocation, seen-marker advanced. (Assert the dispatch/no-dispatch behavior and the marker state, not message text.)
- [x] 4.2 Authorization: a `login` in `allowed_users` with `author_association: NONE` is authorized. An absent/unknown association is denied.
- [x] 4.3 Decline reply: with `decline_comment: true`, a dropped trigger posts exactly one reply; with `false`, none (assert reply count, not wording).
- [x] 4.4 Rate cap: the Nth authorized human `revise` (N = cap) proceeds; the (N+1)th is declined without invoking the executor; the auto-revision counter is unaffected.
- [x] 4.5 Slack path: a Slack verb in an allowlisted channel dispatches without an author_association check (the GitHub gate does not regress the Slack path).

## 5. Acceptance gate

- [x] 5.1 `cargo test` passes for the autocoder crate.
- [x] 5.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 5.3 `openspec validate a000-harden-external-triggers --strict` passes.
