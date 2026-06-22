# Tasks — Unified confirm verb

All code sites are in `autocoder/src/chatops/operator_commands.rs`.

## 1. Consolidate the pending-confirmation stores

- [x] 1.1 Define a tagged `PendingDestructiveOp` enum (variants `Wipe { repo_url:
  String }` and `Rollback { repo_url: String, depth: RollbackChatDepth }`,
  extensible) to replace `PendingConfirmation` (`:2514`) and `PendingRollback`
  (`:2577`) as the value held per channel.
- [x] 1.2 Change `ConfirmationStore` (`:2523`) to hold
  `HashMap<String, (PendingDestructiveOp, Instant /* expires_at */)>`. Keep the
  `record_at` test seam (`:2541`) for past-expiry entries.
- [x] 1.3 Add `record_wipe(channel, repo_url, ttl)` and `record_rollback(channel,
  repo_url, depth, ttl)` helpers (or one `record(channel, op, ttl)`) that INSERT
  the tagged op, overwriting any existing entry for the channel (replace
  semantics). Update `take_valid` (`:2556`) to return `Option<PendingDestructiveOp>`,
  removing-and-returning only a non-expired entry (expired entries removed,
  returning `None`).
- [x] 1.4 Remove `RollbackConfirmationStore` (`:2588`) and its `record`/`take_valid`
  (`:2593`, `:2613`). Update the `OperatorCommandDispatcher` struct (`:2656`) to
  hold a single `pending: ConfirmationStore` field; drop `rollback_pending`
  (`:2660`). Update `OperatorCommandDispatcher::new` (`:2725-2726`) to initialize
  only `pending`.

## 2. Parser: one channel-keyed `confirm` intent + deprecated aliases

- [x] 2.1 Add a single `OperatorCommand::Confirm` variant (channel-keyed; carries
  no op identity, optional informational repo substring). The handler resolves
  the op from the channel's pending slot.
- [x] 2.2 Bare-`confirm` branch (`:640-647`): map a bare `confirm` token to
  `OperatorCommand::Confirm` (not `WipeWorkspaceConfirm`).
- [x] 2.3 Verb dispatcher: change the `"wipe-workspace-confirm" | "confirm"` arm
  (`:755`) and the `"rollback-confirm"` arm (`:1361`) so all three verbs
  (`confirm`, `wipe-workspace-confirm`, `rollback-confirm`) parse to
  `OperatorCommand::Confirm`. Keep `wipe-workspace-confirm` / `rollback-confirm`
  as DEPRECATED ALIASES (still accepted), documented in a code comment as such.

## 3. Dispatcher: single channel-keyed confirm handler

- [x] 3.1 Replace the `WipeWorkspaceConfirm` arm (`:3388`) and the `RollbackConfirm`
  arm (`:3602`) with one `OperatorCommand::Confirm` arm that calls
  `self.pending.take_valid(channel_id)` and matches the returned
  `PendingDestructiveOp`:
  - `Wipe { repo_url }` → submit the `wipe_workspace` action (existing wipe path
    from `:3397` onward, incl. drain-outcome reply).
  - `Rollback { repo_url, depth }` → submit the confirmed `rollback_recovery`
    action (existing rollback path from `:3611` onward, incl. preempt
    acknowledgement).
  - `None` → return ONE unified "no pending confirmation in this channel (or it
    expired — re-issue the original command)" reply (replacing the two distinct
    messages at `:3392` and `:3606`).
- [x] 3.2 `WipeWorkspace` record site (`:3326`): call the new `record_wipe`
  helper. `Rollback` record site (`:3594`): call the new `record_rollback`
  helper. Both still use `WIPE_CONFIRM_TTL_SECS` (`:52`). Confirm the
  replace-on-new-preview behavior is exercised (a new preview overwrites the
  channel's prior pending op).

## 4. Preview renderers name `@<bot> confirm`

- [x] 4.1 `format_rollback_confirmation` (`:2036`): change the trailing line
  (`:2058`) from `Reply \`@<bot> rollback-confirm\` …` to
  `Reply \`@<bot> confirm\` within 60s to perform this rollback.`
- [x] 4.2 `format_wipe_confirmation` (`:2084`): change the trailing line (`:2169`)
  from `Reply 'confirm' within {WIPE_CONFIRM_TTL_SECS} seconds to proceed.` to a
  form naming the canonical `@<bot> confirm` (e.g.
  `Reply \`@<bot> confirm\` within {WIPE_CONFIRM_TTL_SECS} seconds to proceed.`).

## 5. Help list

- [x] 5.1 `format_help_reply` (`:2345`): replace the wipe `confirm` line (`:2357`)
  with a single `confirm` line described as the second step for ALL two-step
  destructive commands (`wipe-workspace`, `rollback`). Remove the
  `rollback-confirm` line (`:2375`). Update the `wipe-workspace` (`:2356`) and
  `rollback` (`:2374`) verb descriptions so they say they await `@<bot> confirm`
  (60s TTL), not the op-specific verb. Do NOT list `wipe-workspace-confirm` or
  `rollback-confirm` (deprecated aliases are hidden).

## 6. Tests

- [x] 6.1 `confirm` after a `wipe-workspace` preview executes the wipe (bare AND
  `@<bot> confirm`).
- [x] 6.2 `confirm` after a `rollback` preview executes the rollback (bare AND
  `@<bot> confirm`).
- [x] 6.3 `confirm` with no pending (none recorded) returns the unified
  no-pending reply; same after the 60s TTL expires (seed a past-expiry entry via
  the `record_at` seam).
- [x] 6.4 A second destructive preview in the same channel REPLACES the first:
  e.g. `wipe-workspace` then `rollback`, then `confirm` executes the rollback
  (the wipe pending is gone); and the reverse ordering.
- [x] 6.5 The deprecated aliases `wipe-workspace-confirm` and `rollback-confirm`
  still execute the channel's pending op.
- [x] 6.6 Preview renderers' output names `@<bot> confirm` and does NOT name
  `rollback-confirm` / `wipe-workspace-confirm` (assert behavior/derivation, not
  exact prose beyond the load-bearing verb token).
- [x] 6.7 `help` output lists a single `confirm` verb and does NOT list
  `rollback-confirm` or `wipe-workspace-confirm`.
- [x] 6.8 Update existing tests that send `rollback-confirm` / assert the old
  preview verb (`:6164`, `:11959`, `:12089`, `:12095`, `:12132`, `:12169`,
  `:12188`, `:12192`, `:6700`) to the unified verb/messages where they assert the
  documented interface; keep alias-coverage tests on the deprecated path.

## 7. Validate

- [x] 7.1 Run `openspec validate unified-confirm-verb --strict` from the repo root.
- [x] 7.2 Run `cargo test -p autocoder` (chatops operator-commands tests) green.
