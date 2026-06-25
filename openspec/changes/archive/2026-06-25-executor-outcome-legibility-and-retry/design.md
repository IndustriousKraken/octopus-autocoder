# Design

## Constraint that shapes everything: provider-agnosticism

The executor wraps a swappable agent CLI (`CliStrategy`), and behind that CLI is
any model provider. There is no single error vocabulary: one provider returns
`529 Overloaded` as the agent's final message, another `503`/`ServiceUnavailable`,
another a `rate_limit_exceeded` body, another a dropped socket. So the design may
NOT depend on recognizing or parsing provider-specific error text.

## Rejected alternative: classify the error by parsing it

The obvious approach — detect "transient" errors (529/429/overloaded/timeout) and
retry only those — was rejected. A central pattern-match over every CLI × every
provider's error format is incomplete the day it ships and breaks the first time
the executor points at a new CLI or model. It fights the model-diversity that is a
load-bearing property of this system rather than working with it.

## Chosen design

Two parse-free moves, plus an optional escape hatch:

1. **Surface, don't parse (legibility).** The executor already captures the
   evidence — the agent's final message, STDERR, and the exit status/signal
   (`AgenticRunOutcome` in `agentic_run.rs`; the per-run log already prints
   `=== FINAL ANSWER (N bytes) ===` / `=== STDERR (N bytes) ===`). On failure we
   assemble a `reason` from that evidence, RAW and truncated, and let a human read
   it. The system never has to understand the error, only show it. A priority-
   ordered assembler already exists — `agentic_run::failure_excerpt` (leads with
   STDERR) — and is the reuse target; the failure arm in `claude_cli.rs`
   (`stderr.take(200)` else `format!("executor exited with {status}")`) is the
   line that produced the bare incident message and is what changes.

2. **Retry on any no-result failure (resilience).** Rather than ask "was this
   transient?", retry a bounded number of times with backoff whenever a session
   FAILED and left no committable result. A transient clears; a deterministic
   failure recurs through all attempts and is surfaced. The guard is the existing
   success signal: a **dirty working tree** — `git status --porcelain` non-empty
   (`has_executor_changes` on the resume path, `polling_loop/outcome.rs`), the same
   signal that already maps a no-diff `Completed` to `Failed`. So only sessions
   that produced NOTHING usable are re-run; sessions that produced committable work
   are never blindly repeated.

3. **Optional, strategy-local classification.** `CliStrategy` gains a defaulted
   `is_retryable(&AgenticRunOutcome) -> Option<bool>` returning `None`. An adapter
   that DOES know its provider's signals MAY override it (e.g. to stop retrying a
   clearly-fatal auth error, or to retry past the no-result guard); `None` falls
   back to the bounded no-result rule. This keeps provider knowledge in the strategy
   that owns it and the core path provider-agnostic.

## Why the assembled reason lives in the backend-agnostic contract

The `reason` is mandated on the architecture-level "Backend-agnostic execution
contract" (the `Backend failure` scenario), phrased in generic terms — the agent's
final message, captured STDERR, exit status/signal — that any backend possesses.
This is the abstraction-first home: every backend honors it, and the concrete
`claude_cli` assembly is one realization.

## Config

A new `executor.session_retries` (additional attempts; small default e.g. `2`;
`0` disables) bounds the retry. No existing knob fits: `verifier_gate_retries` is
gate-scoped and explicitly excludes timeouts and CLI-unavailability;
`timeout_secs` / `agentic_session_timeout_secs` are durations, not attempt counts.
Modeled on `default_verifier_gate_retries` semantics (`config.rs`).

## Truncation

Each captured stream is truncated to a bounded budget so a large stack trace can
neither flood the chat channel nor bloat the stored outcome. The budget is a
constant in the assembler; the exact value is an implementation detail, not a
spec'd number.

## Scope

This is the general abstraction across ALL executor sessions — implementation
passes, PR revisions, audits, agentic gates — because they all route through the
same agentic-run primitive and the same outcome handling. It is not a revision-only
fix; the motivating incident was a revision, but the behavior is uniform.
