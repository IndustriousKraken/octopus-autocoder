# iteration-sequence-gates-once

## Why

A multi-iteration change re-runs all three pre-executor gates before every code iteration. The gates evaluate a change's spec deltas — but the executor iterates on code, not specs, so between iterations the gate inputs are almost always byte-identical. At the iteration cap of 5 that is up to 15 billed gate sessions per change where 3 would do (and up to 45 with the no-submission retry budget), re-judging inputs that have not changed. Gate verdicts are agentic judgments, so they must never be manufactured by code — but re-billing a fresh judgment of the very same bytes the same sequence already passed is pure spend with no added assurance.

## What Changes

- Pre-executor gate evaluation becomes sequence-scoped: when every enabled pre-executor gate passes for a change, the daemon records a content hash of the change directory's gate inputs in a state-dir record. A continuation iteration (iteration-pending marker present) whose recomputed hash matches does not re-spawn the gate sessions — the sequence's recorded verdicts stand, rendered in the ledger as carried forward.
- Any difference, a missing record, or any error computing or reading the hash runs the gates in full — the skip fails toward running, never toward passing.
- The record is dropped whenever the sequence terminates, so a fresh sequence always re-gates.

## Impact

- Affected specs: `orchestrator-cli` (Verifier-gate framework)
- Affected code: `autocoder/src/polling_loop/queue_walk.rs`, a small state-dir record module, `autocoder/src/paths.rs`, gate-ledger rendering
