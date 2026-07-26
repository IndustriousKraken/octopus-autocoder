# review-discard-names-rejection

## Why

When an agentic reviewer session attempts `submit_review` and the daemon-side validator rejects the payload (schema violation, bad enum value, cross-field rule), the rejection reason goes back to the agent as a correctable tool error — and nowhere else. If the agent never corrects it, the session ends as a generic "recorded no valid submit_review submission" discard. The daemon KNEW exactly why the submission was invalid and threw that knowledge away; the operator is left inferring the cause from raw session output. The 2026-07-26 incident took a code-reading session to diagnose (an operator prompt override teaching the model a `Concerns` verdict the tool schema does not accept); one line naming the rejection would have made it self-diagnosing from the Slack notification. It also cleanly separates the two no-submission modes — "attempted and rejected N times" versus "never attempted" — which point at different culprits (payload/content problems versus prompt-contract problems).

## What Changes

- Submission rejections are remembered per session: when a `submit_review` attempt is rejected by the daemon-side validator, the rejection reason (truncated) and count are retained for that session.
- A no-submission discard reason names the LAST rejection and the rejection count ahead of the raw captured output; when no submission was ever attempted, the reason states that explicitly.
- The per-session review log records each rejection as it happens.

## Impact

- Affected specs: `code-reviewer` (Reviewer session output is persisted and surfaced on a no-submission discard)
- Affected code: submission relay/store (retain rejections per role), `autocoder/src/code_reviewer/agentic.rs` (discard reason assembly), review session log
