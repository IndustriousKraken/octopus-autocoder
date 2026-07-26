# Tasks: review-discard-names-rejection

## 1. Remember rejections

- [x] 1.1 In the submission relay path (the shared submission store that both the control-socket `record_submission` handler and the in-process listener record through), when a `submit_review` payload fails daemon-side validation, retain the rejection — truncated reason and timestamp — keyed like the submission itself, accumulating per session; keep returning the existing correctable tool error to the agent unchanged.
- [x] 1.2 Clear retained rejections when a valid submission is recorded for the same key, and drain them with the session's submission consume, so nothing leaks across sessions.
- [x] 1.3 Expose the retained rejections to the agentic review runner alongside the existing no-submission diagnostic (the `consume_submission` response carries them in the same round trip).

## 2. Surface them on discard

- [x] 2.1 In `autocoder/src/code_reviewer/agentic.rs`, when a session ends with no valid submission AND rejections were retained, lead the discard diagnostic with a line naming the last rejection and the count, e.g. `submission rejected 2x — last: verdict "Concerns" not in [Approve, Block]`, ahead of the raw captured-output diagnostic.
- [x] 2.2 When a session ends with no valid submission AND no submission was ever attempted, lead with `no submission attempted` instead.
- [x] 2.3 Record each rejection, with its timestamp, in the per-session review log, so the full sequence is recoverable from disk.

## 3. Tests

- [x] 3.1 Test: a rejected-never-corrected session's discard evidence names the last rejection reason and the count, ahead of the captured output.
- [x] 3.2 Test: a never-attempted session's discard evidence contains the no-submission-attempted line.
- [x] 3.3 Test: a rejection followed by a corrected valid submission leaves no rejection residue, and drained rejections do not leak into a later session's consume.

## 4. Validation

- [x] 4.1 Run `openspec validate review-discard-names-rejection --strict` and fix any findings.
