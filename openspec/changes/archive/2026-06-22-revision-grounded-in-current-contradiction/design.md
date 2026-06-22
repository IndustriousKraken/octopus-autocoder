# Design

## The durable marker, not the chat thread, is the source of truth

The standing project principle is that the gate/preflight that flags a change is
the durable record of "this spec needs revision"; the chat thread is ephemeral
operator UX layered on top. The loop this change fixes happened because the
implementation inverted that: the most-accurate, most-current statement of the
contradiction lived ONLY in the thread (each re-gate posted its finding there and
nowhere else), while the durable marker stayed frozen at the first pre-flight
finding. When the thread failed to load, the only authoritative artifact left was
stale.

The fix restores the principle: every re-gate failure refreshes the marker with
the current findings, so the marker always states what currently contradicts. The
thread carries the operator's chosen DIRECTION (align-to-canon vs modify-canon);
the marker carries WHAT is wrong. Both are inputs to a revision; the marker is the
durable one.

## Fail closed on an unreadable thread, rather than proceed with a warning

Two options were considered for a transcript fetch that fails after a bounded
retry:

1. Proceed from the (now-current) marker alone, loudly warning that the thread
   could not be read.
2. Open no PR and report that the discussion could not be read; the operator
   retries.

We chose (2). The marker tells the executor WHAT to fix, but not the operator's
chosen direction; revising without the direction means the session guesses, and a
wrong guess produces a PR the operator did not want — paying tokens to land the
wrong revision, the exact failure mode the broader hardening effort is removing.
Refusing to revise blind is the gatekeepers-fail-closed posture applied to the
write path: an inability to read the inputs is a distinct, surfaced, non-acting
state, never a silent best-effort. Transient fetch failures are absorbed by the
bounded retry; a persistent failure is made legible instead of silently
producing a blind revision. The read-only advisor is exempt — it writes nothing,
so a degraded single-turn answer is acceptable as long as it says it is degraded.

## Converge within one `send it`, bounded and escalating

A failed `send it` today discards its edits (`restore_base`) and the next one
recreates the branch from base, so there is no accumulation across rounds — each
round must resolve EVERY contradiction at once or it is thrown away. Rather than
push that burden onto the operator (send it, read, send it, read…), the executor
loops edit→re-gate internally up to a small bound, accumulating fixes on the
revision branch, and only discards + reports when the bound is exhausted. With
`contradiction-gates-report-all-findings` surfacing the full set up front, this
loop should rarely exceed one iteration; it is a safety net for the residual case
where a re-gate surfaces something new (model non-determinism).

Escalation prevents the silent loop from simply moving inside the executor: the
re-gate returns structured findings (the conflicting requirement identity, and
for `[canon]` its capability), and when the SAME finding identity survives the
bounded attempts, the report names that specific requirement and that the
revision is not clearing it — turning "still fails" repeated N times into a
specific, actionable escalation.

## No provider-specific parsing

Consistent with the executor-legibility work, nothing here parses provider-
specific error text. The transcript-retry decision keys on fetch success/failure;
the converge/escalation decision keys on the gates' own structured findings; the
marker stores those findings raw. No model-output string is pattern-matched for a
control-flow decision.
