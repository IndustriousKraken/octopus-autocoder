# Tasks

OpenSpec: implements the deltas in `specs/executor/spec.md`,
`specs/orchestrator-cli/spec.md`, and `specs/chatops-manager/spec.md`.

## 1. Assembled, legible failure reason (provider-agnostic)

- [ ] 1.1 In `autocoder/src/executor/claude_cli.rs`, replace the failure-reason
  construction in the `if !status.success()` arm (~`claude_cli.rs:935-946`, today
  `stderr.trim().take(200)` else `format!("executor exited with {status}")`) with
  an assembled reason built from the captured `AgenticRunOutcome`
  (`agentic_run.rs:47`: `final_answer`, `stderr`, `exit_status`, `timed_out`).
- [ ] 1.2 Reuse / extend `agentic_run::failure_excerpt(outcome, max)`
  (`agentic_run.rs:76`) as the single assembler so there is ONE place that builds
  the reason. It SHALL combine, in priority order and each truncated to a bounded
  budget: the `final_answer` if non-empty, then `stderr` if non-empty (labeled),
  and ALWAYS the exit status / terminating signal — and when BOTH `final_answer`
  and `stderr` are empty, the exit status / signal is the reason (e.g. a process
  killed by a signal). Do NOT parse or pattern-match the content for any decision —
  it is surfaced raw.
- [ ] 1.3 Confirm the timeout path still yields its existing terminal reason
  (`timed_out` → a clear "timeout" reason) and is not regressed by the assembler.

## 2. Optional, strategy-local retry hint (`CliStrategy`)

- [ ] 2.1 In `autocoder/src/agentic_run.rs`, add to `trait CliStrategy`
  (`agentic_run.rs:195`) a DEFAULTED method
  `fn is_retryable(&self, _outcome: &AgenticRunOutcome) -> Option<bool> { None }`.
  The default returns `None` (no opinion). Do NOT implement a non-`None` body for
  the `claude` strategy in this change — `None` is correct (it defers to the
  bounded no-result rule); the hook exists so a future adapter can encode its own
  provider's signals without core changes.

## 3. Bounded retry on a no-committable-result failure (orchestrator)

- [ ] 3.1 In `autocoder/src/config.rs`, add `executor.session_retries: u32`
  (additional attempts beyond the first; small default e.g. `2`; `0` disables),
  modeled on `default_verifier_gate_retries` (`config.rs:1459`). Document that it
  bounds whole-session re-invocations, distinct from `timeout_secs` (a duration)
  and `verifier_gate_retries` (gate-scoped, excludes this case).
- [ ] 3.2 In the pass/outcome path (`autocoder/src/polling_loop/outcome.rs`), when
  an executor session returns a FAILED outcome (including the no-diff `Completed`
  already mapped to `Failed` at `outcome.rs:204-209`) AND produced no committable
  result — `git status --porcelain` empty / `has_executor_changes` false
  (`outcome.rs:544`) — re-invoke the session, up to `executor.session_retries`
  times, with backoff between attempts. Consult `CliStrategy::is_retryable`: a
  `Some(false)` short-circuits (do not retry), a `Some(true)` retries even with a
  committable result, and `None` applies the no-committable-result rule.
- [ ] 3.3 A session that produced a committable result is NOT retried (it is a
  normal success/failure with output). Exhausting the retries surfaces the failure
  with its assembled reason (section 1). A retry-in-progress is observably distinct
  from a terminal failure in the operator-facing surface (do not hardcode wording).
- [ ] 3.4 Keep the existing whole-pass scheduling intact: this is an in-pass
  bounded retry, not a change to the daemon's next-pass re-pickup behavior.

## 4. Notification carries the assembled reason

- [ ] 4.1 Ensure the operator-facing failure notifications render the assembled
  reason from section 1 verbatim (truncated at the source), not a bare exit code:
  the revise-failed PR comment / chatops shape (`autocoder/src/revisions.rs:1334`),
  and the pass-path failure/alert `last_reason` surfacing. These already render the
  `reason` string transitively, so enriching it at the source (section 1) flows
  through; verify no site re-summarizes or discards it.

## 5. Tests

- [ ] 5.1 Reason assembly (`agentic_run` / `claude_cli` tests): given an outcome
  with a non-empty `final_answer`, the assembled reason includes it (truncated);
  given non-empty `stderr`, the reason includes it (truncated, labeled); given
  BOTH empty with a non-zero exit / signal, the reason surfaces the exit status /
  signal. Assert the assembled-reason BEHAVIOR (which captured fields appear), NOT
  exact wording.
- [ ] 5.2 Bounded retry (`polling_loop` tests): a session that fails with no
  committable result is retried up to `session_retries` times then surfaces; a
  session that fails WITH a committable result is NOT retried; `session_retries: 0`
  disables retry; a `CliStrategy::is_retryable` returning `Some(false)`
  short-circuits. Drive via the existing test seams (no real subprocess); assert
  attempt counts / outcomes, not message wording.
- [ ] 5.3 `executor.session_retries` config: default applied when absent; explicit
  `0` disables; parsed from config.

## 6. Validation

- [ ] 6.1 `cd autocoder && cargo test --bin autocoder` (the suite is known-flaky
  under parallel load — re-run / isolate any failure before treating it as real).
- [ ] 6.2 `openspec validate executor-outcome-legibility-and-retry --strict` from
  the repo root.
