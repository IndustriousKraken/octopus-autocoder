# Tasks

OpenSpec: implements the renamed + modified requirement in
`specs/orchestrator-cli/spec.md`.

## 1. Config: the cap becomes optional, default unlimited

- [x] 1.1 In `autocoder/src/config.rs`, change `max_revise_triggers_per_pr` from
  `u32` (`#[serde(default = "default_max_revise_triggers_per_pr")]`, currently
  `10`) to `Option<u32>` defaulting to `None` (unlimited). Mirror the existing
  opt-in `reviewer.max_code_reviews_per_pr` (`Option<u32>`, `None` = unlimited).
  Remove `default_max_revise_triggers_per_pr` (or repoint it) so the absent-config
  default is `None`. A legacy config that sets an explicit integer SHALL still
  parse and apply (serde deserializes `Some(n)`).

## 2. Dispatcher: skip the cap when unset

- [x] 2.1 In the revise dispatcher (`autocoder/src/revisions.rs`), the cap check
  (`if !is_automatic && state.human_revise_count >= human_revise_cap { decline }`)
  becomes a no-op when the cap is `None`: thread `Option<u32>` through
  `human_revise_cap`, and only count/decline when `Some(n)`. When `None`, an
  authorized human `@<bot> revise` always invokes the executor and is never
  declined for cap reasons. Keep `human_revise_count` tracking harmless when
  uncapped (it may still increment, or be skipped — but it MUST never gate).
- [x] 2.2 Leave the auto-revision cap (`max_auto_revisions_per_pr`) and the
  re-review cap (`reviewer.max_code_reviews_per_pr`) paths unchanged.

## 3. Tests

- [x] 3.1 With `max_revise_triggers_per_pr = None` (the default), N authorized
  `@<bot> revise` triggers (N large, e.g. 25) ALL invoke the executor and NONE is
  declined for cap reasons. Assert the invocation count / no-decline behavior, not
  message wording.
- [x] 3.2 With `max_revise_triggers_per_pr = Some(n)`, the existing behavior holds:
  triggers under `n` proceed, a trigger at `n` is declined with one notice and does
  not invoke the executor. (Preserve the existing cap tests, retargeted to `Some(n)`.)
- [x] 3.3 Config: absent `max_revise_triggers_per_pr` resolves to `None`; an
  explicit integer in config resolves to `Some(n)`.

## 4. Validation

- [x] 4.1 `cd autocoder && cargo test --bin autocoder` (the suite is known-flaky
  under parallel load — re-run / isolate any failure before treating it as real).
- [x] 4.2 `openspec validate human-revise-cap-opt-in --strict` from the repo root.
