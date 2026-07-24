## Why

The reviewer prompt override (`reviewer.code_review.prompt_path`, legacy `reviewer.prompt_template_path`) is honored only by the oneshot HTTP path — the agentic reviewer builds its prompt entirely in code with hardcoded guidance, and agentic is the default transport ("the only mode anyone uses"). An operator who configures the documented override gets a silent no-op: the config loads, `reload` reports `reviewer` applied, and every review still runs on the embedded guidance. Observed in production (2026-07-23): an operator tuned the concern-flagging criteria to make `auto_revise: actionable` forward non-blocking concerns, verified config and reload, and watched reviews behave exactly as before — with nothing anywhere indicating the override never reached the reviewer.

## What Changes

- In agentic mode, a configured reviewer prompt override is included as an **operator guidance preamble** at the top of the rendered session prompt — ahead of the code-built sections (change briefs, changed-path list, diff-artifact reference, `submit_review` contract), which are retained unchanged. Operators use it to calibrate concern-flagging (e.g. when to set `should_request_revision`), review emphasis, and house rules.
- The override file's role differs by transport and is documented as such: the oneshot path uses it as the full template (unchanged); the agentic path uses its content as guidance — output-format mechanics (the oneshot template's `revision-requests` YAML block) don't apply there, since the agentic contract is the `submit_review` tool.
- A configured-but-unreadable (or empty) override fails the review loudly at use time — reviewer-failure alert, no silent fallback to embedded guidance — matching the `[in]` gate's empty-override precedent. A silently ignored calibration is precisely the defect this change fixes.
- Unset override: behavior is byte-for-byte today's.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `code-reviewer`: the "Agentic reviewer mode" requirement gains the operator-preamble contract for the configured prompt override, including the fail-loud unreadable-override posture.

## Impact

- `autocoder/src/code_reviewer/agentic.rs`: `render_agentic_review_prompt` gains the operator-preamble slot (ahead of the existing cross-change preamble); the session assembly resolves the override via the same nested→legacy loader the oneshot path uses (`code_reviewer.rs:514-523`).
- `docs/CONFIG.md` prompt-overrides table: the CodeReview row documents the per-transport semantics (full template for oneshot; guidance preamble for agentic).
- Operators with an existing override file written for oneshot should trim it to guidance-only content for agentic use; the docs row says so.
