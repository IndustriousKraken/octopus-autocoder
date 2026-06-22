# Human `@<bot> revise` is uncapped by default (opt-in cap)

## Why

The per-PR human-revise cap (`executor.max_revise_triggers_per_pr`, default `10`,
added by a000) rate-limits deliberate operator `@<bot> revise` requests. But a
human revise is an explicit, authorized operator action — capping it by default
contradicts the standing invariant that an operator's deliberate revision always
processes, and it adds friction (a surprise "cap reached" decline mid-workflow)
with no benefit the operator asked for. This makes the cap OPT-IN: the config
value becomes optional and defaults to NONE (unlimited), mirroring the already
opt-in re-review cap (`reviewer.max_code_reviews_per_pr`). Human revises are never
capped by default; the cap remains available to any operator who wants it.

## What Changes

- `executor.max_revise_triggers_per_pr` becomes `Option<u32>`, defaulting to
  `None` (unlimited) instead of `10`.
- When it is `None` (the default), an authorized human `@<bot> revise` ALWAYS
  processes: it is not counted against a cap and is never declined for cap reasons.
- When it is set to a positive `N`, the existing per-PR cap behavior applies
  unchanged (count tracked, a trigger at the cap declined with one notice, no
  executor invocation).
- The auto-revision cap (`max_auto_revisions_per_pr`) and the re-review cap
  (`reviewer.max_code_reviews_per_pr`) are untouched.

## Impact

- Affected capability: `orchestrator-cli` — the human-revise-cap requirement is
  renamed (it is no longer "rate-capped" by default) and modified to opt-in.
- Affected code: `autocoder/src/config.rs` (`max_revise_triggers_per_pr` →
  `Option<u32>`, default `None`; drop `default_max_revise_triggers_per_pr` or
  repoint it), and the revise dispatcher in `autocoder/src/revisions.rs` (the
  `if !is_automatic && human_revise_count >= human_revise_cap` check becomes a
  no-op when the cap is `None`; thread `Option<u32>` instead of `u32`).
- Behavior-preserving for anyone who explicitly configured a cap; only the default
  changes (10 → unlimited).
