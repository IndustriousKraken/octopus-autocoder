## Context

`render_agentic_review_prompt` (`code_reviewer/agentic.rs:204`) builds the agentic reviewer's prompt entirely from code: a hardcoded quality-scope instruction, change briefs, path lists, the diff-artifact reference, and hardcoded `should_request_revision` guidance. The template loader that resolves `reviewer.code_review.prompt_path` → `reviewer.prompt_template_path` → embedded default (`code_reviewer.rs:514-523`) is consulted only by the oneshot path. Since `reviewer.kind` defaults to `agentic`, the documented CodeReview prompt override is a silent no-op for the default transport. Production incident (2026-07-23): an operator overrode the flag-calibration stanza to make `auto_revise: actionable` forward non-blocking concerns, confirmed config + reload, and reviews behaved identically — with no signal anywhere that the override never reached the reviewer. Diagnosis required reading the prompt-assembly source.

## Goals / Non-Goals

**Goals:**
- The documented override key works in the default transport, for its main use: calibrating reviewer judgment.
- A configured-but-broken override is loud, never silent — the incident's failure mode was silence.

**Non-Goals:**
- Making the agentic prompt fully template-driven. Its structure (briefs, artifact reference, reads-on-demand, `submit_review` contract) is load-bearing machinery spec'd elsewhere in the same requirement; letting operator text replace it would break the transport. Guidance-preamble is the safe, useful subset.
- A new config key. The existing, documented key gains meaning in agentic mode; per-transport semantics are documented rather than multiplied.
- Changing the `submit_review` tool description (it stays the neutral contract; calibration belongs to the operator preamble).

## Decisions

- **Preamble, not template substitution.** Operator text is prepended ahead of all code-built sections (and ahead of the per-change cross-change preamble, which is positional context, not guidance). Prompt-priming order puts house rules first, and the code-built mechanics remain intact regardless of what the operator writes.
- **Same resolution chain as oneshot.** Nested key wins, legacy key falls back — one loader, reused. No drift between transports about *which* file is honored, only about what role its content plays.
- **Fail loud on unreadable/empty, at review time.** Mirrors the `[in]` gate's empty-prompt-override precedent and the reviewer's own no-submission posture (discard + alert, never implicit Approve). The alternative — WARN and proceed with defaults — reproduces the silent no-op this change exists to kill, just with a log line.
- **Read the file per review, not at startup.** Operators iterate on calibration text; a per-review read makes edits take effect on the next review with no reload, and the fail-loud path catches a file deleted mid-flight. File size is trivially small; IO cost is nil.
- **Docs carry the per-transport semantics.** The CONFIG.md prompt-overrides row for CodeReview states: oneshot = full template; agentic = guidance preamble (write judgment guidance, not output-format mechanics — the oneshot template's `revision-requests` YAML block does not apply to agentic sessions).

## Risks / Trade-offs

- [An operator points agentic mode at the full oneshot template] → The wrong-mechanics text (YAML block instructions) rides along as noise; the `submit_review` schema still governs the actual submission, so the review works — degraded prompt hygiene, not breakage. The docs tell operators to keep a guidance-only file; the incident operator's fix is trimming their copy to the calibration stanza.
- [Prompt-injection surface: operator-controlled text enters the reviewer prompt] → The override file is operator-owned config on the daemon host, same trust level as config.yaml and every other prompt override the daemon already honors; no new boundary is crossed.
- [Fail-loud pauses reviews if the file goes missing] → That is the correct outcome for a configured-but-broken control (gatekeepers-fail-closed posture); the alert names the file, and removing the config key is the explicit opt-out.
