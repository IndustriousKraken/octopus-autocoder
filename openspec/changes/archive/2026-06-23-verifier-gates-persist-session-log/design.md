# Design

## Persist on every outcome, not just failures

The log is written for clean, findings, no-submission, timeout, and error
outcomes alike. The marginal cost is one file write of output the daemon already
holds, and the upside is symmetric: a held change is diagnosable, AND a surprising
clean pass or a questionable finding is auditable after the fact. Gating
persistence on "only when it failed" would re-create the current blind spot for
the false-positive and false-negative cases.

## Two sides of the no-submission story

A no-submission hold has a model side and a daemon side, and the bug can live on
either:

- **Model side** — captured in the session log (requirement 1): the wrapped CLI's
  streamed actions, the tool list it was given, whether it called the submission
  tool, its final message, stderr, exit status, and the timed-out flag. For an
  opencode/Kimi run this stream already shows the available tools and the model's
  tool calls, so it answers "was the tool there / did the model call it / did the
  CLI error."
- **Daemon side** — recorded by the MCP submission server and the submission
  listener (requirement 2): which submission tool the MCP server advertised for
  the session's role, and whether a submission was actually relayed to the daemon
  before the consume returned none. This nails the two cases the model's own
  stdout may not show reliably: the tool was never advertised, and the model
  called it but the relay/control-socket dropped it.

Together they let an operator distinguish: tool-not-advertised vs.
advertised-but-not-called vs. called-but-not-relayed vs. session-errored/timed-out
— each of which has a different fix.

## One session-log writer, not three

The audits already persist run logs (`audits/<type>-<timestamp>.log`); the planned
reviewer-legibility change adds a reviewer session log; this adds a gate session
log. These should converge on a single small helper that takes a captured
`AgenticRunOutcome` plus a (category, name) and writes the discoverable file,
rather than a third bespoke writer. The gate change either reuses that helper (if
the reviewer change has landed) or mirrors the audit writer and leaves a clear
seam for consolidation. This follows the project's extract-before-proliferate
preference.

## Explicitly out of scope

- No gate prompt changes. The investigation that motivated this found the prompt
  is not the suspect; making the prompt more pedantic without evidence is
  precisely the move this change exists to avoid. This change only makes the
  evidence visible.
- No new log-retention policy — gate logs follow the existing run-log convention.
- No change to gate dispositions, the fail-closed posture, or what counts as a
  submission.

## Provider-agnostic, no parsing for decisions

The session log stores raw captured output; the daemon records facts
(advertised-tool, received-submission) it already knows from its own control flow.
Nothing pattern-matches model output to make a control-flow decision — consistent
with the rest of the legibility work.
