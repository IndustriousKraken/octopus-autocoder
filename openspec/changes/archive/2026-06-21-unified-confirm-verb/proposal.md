# Unified confirm verb for two-step destructive operator commands

## Why

The two-step destructive-confirm interface is inconsistent. `wipe-workspace` is
confirmed by replying `confirm` (accepted bare OR mentioned, channel-keyed), but
`rollback` requires the distinct verb `@<bot> rollback-confirm`. Behind that are
two separate pending-confirmation stores (`ConfirmationStore` for wipe,
`RollbackConfirmationStore` for rollback) that can hold a pending wipe AND a
pending rollback on the same channel at once, plus two different reply syntaxes
and inconsistent mention rules. An operator cannot answer "do I reply with
`@bot` or not, and which word" the same way everywhere. The confirmation step
for a destructive command should be ONE verb with ONE syntax, regardless of
which destructive op is pending.

## What Changes

- ONE canonical confirmation verb `confirm` for every two-step destructive
  operator command (currently `wipe-workspace` and `rollback`; future two-step
  destructive ops inherit it). `confirm` is channel-keyed: it resolves to
  whatever destructive op is pending in that channel.
- Both `@<bot> confirm` AND bare `confirm` are accepted uniformly. `@<bot> confirm`
  is the documented canonical form. No per-op confirm verb appears in the
  documented interface.
- One effective pending destructive op per channel (at most one). A new
  destructive op's preview REPLACES any prior pending for that channel.
- `confirm` with no live pending (none recorded, or expired past the 60s TTL)
  returns a clear "no pending confirmation in this channel — re-issue the
  original command" reply.
- The preview renderers (`format_wipe_confirmation`, `format_rollback_confirmation`)
  instruct the operator to reply `@<bot> confirm`. Help lists `confirm` as the
  single two-step confirmation verb.
- `rollback-confirm` and `wipe-workspace-confirm` remain DEPRECATED ALIASES that
  still execute (so an in-flight operator is not broken) but are hidden from help
  and no longer named by the preview messages.

## Non-goals

- `defer`/`undefer` are single-ack commands (no two-step confirm). They do NOT
  use `confirm` and are unchanged. Only the two-step DESTRUCTIVE confirms
  (rollback, wipe-workspace) are unified.
- Removing the deprecated aliases. That MAY follow in a later change.

## Impact

- Affected capabilities: `chatops-manager` (the unified verb, single pending
  store, help) AND `orchestrator-cli` — its "Chatops operator commands"
  requirement also canonizes the wipe-workspace two-step confirm with the old
  op-specific strings; this change reconciles that scenario to the unified verb +
  op-agnostic no-pending message so the two capabilities do not contradict.
- Affected code: `autocoder/src/chatops/operator_commands.rs` (parser arms,
  pending stores, dispatcher confirm handlers, preview renderers, help list).
