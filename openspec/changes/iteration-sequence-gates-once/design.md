# Design: iteration-sequence-gates-once

## Why this does not violate gatekeepers-fail-closed

No verdict is manufactured. The gates ran, judged the exact bytes in question, and passed; the record only lets that judgment cover the rest of its own iteration sequence. Every doubt path — hash mismatch, missing record, unreadable record, hash-computation error — runs the gates in full. The skip is the optimization; running is the default. Nothing about a gate's disposition when it DOES run changes.

## Record shape and location

`<state_dir>/gate-pass/<workspace-basename>/<change>.json` containing `{ "inputs_hash": "<sha256>", "recorded_at": "<rfc3339>" }`. State-dir, not workspace: workspace-local bookkeeping is vulnerable to `git clean` and re-clones, and this record must never ride a commit. Written only when ALL enabled pre-executor gates pass in one pickup; a partial pass records nothing.

## Hash inputs

SHA-256 over the sorted list of (relative path, file bytes) for every regular file under the change's directory, excluding the daemon's marker files (`.in-progress`, `.question.json`, `.answer.json`, `.needs-spec-revision.json`, `.perma-stuck.json`, `.priority.json`, `.ignore-for-queue.json`). Hashing the whole change directory (not just the spec deltas) is deliberate: tasks.md and design edits can change what the gates would say, and over-hashing only costs a false re-gate, which is the safe direction.

## When the record is consulted

Only when the pickup is a continuation — the change has an iteration-pending marker. A fresh pickup (no marker) always gates and replaces any stale record on pass. This keeps the skip scoped to exactly the multi-iteration burn case and leaves every other pipeline path untouched (executor-failure retries across passes still re-gate, as today).

## When the record is dropped

Wherever the iteration-pending marker is dropped — `Completed`, `Failed`, `SpecNeedsRevision` — and additionally any time the gates run in full for the change (the new pass supersedes it). Removal is best-effort idempotent; a leftover record is harmless because it is only consulted behind the marker check and the hash comparison.

## Ledger rendering

Skipped-by-record gates render in the PR's gate-verdict ledger as the sequence's verdict annotated "carried forward (sequence)", so the PR record stays honest about when the judgment was made.
