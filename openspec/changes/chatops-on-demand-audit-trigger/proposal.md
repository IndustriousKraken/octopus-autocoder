## Why

The audit framework today only runs audits on their configured cadence (`daily`, `weekly`, `monthly`, etc.). An operator preparing a codebase for production typically wants the opposite shape: run an architecture brightline now, fix what it surfaces, run a security audit now, fix what that surfaces, iterate. The cadence-based scheduling is the wrong primitive for that workflow — waiting a day or a week between iterations defeats the point.

The operator's mental model is "I want this audit to run now, regardless of when it last ran or whether its cadence says it's due." An on-demand trigger via chatops AND CLI gives the operator exactly that, without disturbing the existing cadence machinery. A scheduled monthly audit still runs monthly; an on-demand triggered audit runs on the next polling iteration, bypassing the cadence check.

Same shape as the existing `rebuild-specs` chatops verb: an operator types a command, the daemon records a per-repo flag, the next polling iteration honors the flag and runs the work. Findings post via the existing audit-notification flow (which, once `chatops-audit-findings-in-threads` ships, will be threaded). No new transport, no new infrastructure — just a new way to fill the existing audit pipeline.

A secondary motivation: testing audit prompts during development. Operators iterating on a custom `prompt_path` for an audit need to run that audit against a real workspace and see the output. Today that requires editing the audit's cadence to fire imminently, waiting for the polling tick, observing the output, then editing the cadence back. On-demand triggering compresses that cycle to "trigger, observe, iterate" in minutes.

## What Changes

**New chatops verb: `audit`.** Syntax: `@<bot> audit <audit-substring> <repo-substring>`. Substring matching follows the established pattern used by every other chatops verb's repo argument — case-insensitive, single match proceeds, multiple matches return the candidate list, no matches returns the full list of valid audit types.

Examples:
- `@<bot> audit sec myrepo` → triggers `security_bug_audit` on `myrepo`
- `@<bot> audit consu sound-cab` → triggers `architecture_consultative` on a repo matching `sound-cab`
- `@<bot> audit arch myrepo` → ambiguous (matches `architecture_brightline` and `architecture_consultative`); reply lists the candidates
- `@<bot> audit gibberish myrepo` → no match; reply lists all registered audit types

**New CLI subcommand: `autocoder audit run`.** Syntax: `autocoder audit run --workspace <path> --audit <name>`. The CLI form runs against an explicit workspace path. When the daemon is running on the host AND the workspace matches a repo the daemon is managing, the CLI talks to the daemon via the control socket (same pattern as `autocoder reload`); the request is enqueued like the chatops verb. When the daemon is NOT running, the CLI runs the audit standalone — directly invokes the audit module against the workspace and prints the findings to stdout.

The standalone path is useful for prompt-template development: operators editing `prompts/security-bug-audit.md` can iterate without restarting the daemon.

**Per-repo flag for queued audits.** Each `RepoTaskHandle` gains a `pending_audit_runs: Arc<Mutex<Vec<String>>>` field. The on-demand trigger appends the resolved audit-type name to this list. At the start of each polling iteration, the audit scheduler checks the list BEFORE its normal cadence-driven scheduling: every audit type in the list is run unconditionally (cadence is ignored). After running, the list is cleared.

When the same audit type is queued multiple times before the next iteration fires (operator typo, double-click on a chatops command), the duplicate entries collapse to a single run. The audit fires once per iteration, not once per queue entry.

**Bot ack format.** The chatops verb's reply is a one-line ack:

```
✓ Queued security_bug_audit for git@github.com:acme/myrepo.git. Will run on the next polling iteration (~5m).
```

The ETA is computed from the repo's `poll_interval_sec`. If the daemon is between iterations and the next tick is soon (<30 seconds), the ETA reads `imminently` instead of a minute count.

**Existing audit-notification flow unchanged.** When the queued audit runs in the next iteration, its findings post via the same chatops notification path as a cadence-triggered audit. Once `chatops-audit-findings-in-threads` ships, the on-demand audit's findings also use the threaded form — identical UX.

**Audit-state interaction.** An on-demand triggered audit DOES update the audit-state file (`last_run` timestamp moves forward). This is intentional: the audit ran, so its cadence clock should reflect that. If the operator wants to bypass cadence entirely, they can keep triggering on-demand; if they want the audit to also continue on its cadence, the on-demand run shifts the next scheduled fire later by the cadence interval (e.g. on-demand fires today + monthly cadence → next scheduled fire is one month from today, not one month from the original schedule). The trade-off favors not double-running audits soon after an on-demand fire.

## Impact

- **Affected specs:** `orchestrator-cli` — one ADDED requirement covering the chatops verb, the CLI subcommand, the substring-matching rules for audit types, the queued-audit-runs flag, the cadence interaction, and the bot-ack format.
- **Affected code:**
  - `autocoder/src/chatops/operator_commands.rs` — extend the parser with `OperatorCommand::AuditNow { audit_substring, repo_substring }`. Extend the dispatcher to resolve both substrings, submit the `queue_audit` control-socket action, and format the bot ack.
  - `autocoder/src/control_socket.rs` — add `queue_audit` action: appends to the named repo's `RepoTaskHandle.pending_audit_runs`. Action also returns the resolved audit-type-name for the ack.
  - `autocoder/src/control_socket.rs` — extend `RepoTaskHandle` with `pending_audit_runs: Arc<Mutex<Vec<String>>>`.
  - `autocoder/src/cli/mod.rs` — new `Audit { Run { workspace, audit } }` subcommand. The run handler probes for the control socket; if present, sends the same action; if absent, invokes the audit module directly against the workspace path.
  - `autocoder/src/audits/scheduler.rs` — at the start of each iteration's audit-scheduling phase, drain `pending_audit_runs`; for each entry, look up the registered audit by name, run it unconditionally (skip the cadence check), update its audit-state. THEN proceed to the normal cadence-driven scheduling for any audit not already run this iteration via the queue.
  - Substring matcher helper: extract or reuse `match_repo`'s logic for matching against audit-type names. A new `fn match_audit_type<'a>(substring: &str, registered: &'a [&str]) -> AuditMatch<'a>` mirroring `RepoMatch`.
  - Tests:
    - Parser: `@<bot> audit sec myrepo` → `Some(AuditNow { audit_substring: "sec", repo_substring: "myrepo" })`. Verb is case-insensitive (`AUDIT`, `Audit` both work).
    - Substring matcher: `sec` matches `security_bug_audit` uniquely; `arch` returns Multiple; `gibberish` returns None.
    - Queue: dispatching `AuditNow` appends to the repo's `pending_audit_runs`; the next iteration's scheduler drains and runs.
    - De-duplication: queuing the same audit-type twice produces one run per iteration.
    - CLI standalone: with no daemon running, `autocoder audit run --workspace <tempdir> --audit <name>` invokes the audit module directly.
    - CLI via daemon: with a fixture daemon running, `autocoder audit run` sends the queue action via the control socket.

- **Operator-visible behavior:** new chatops verb + CLI subcommand for on-demand audit triggering. Iterative workflows ("refactor → architecture audit → fix → security audit → repeat") become feasible without re-configuring cadences mid-day. The cadence machinery is unchanged; on-demand and cadence-triggered audits coexist.
- **Breaking:** no. The new verb is additive. Audits configured with `cadence: disabled` can now be triggered on-demand even though their cadence still says disabled — the on-demand path is independent.
- **Acceptance:** `cargo test` passes (new + existing). An operator posts `@<bot> audit sec myrepo` in a configured chatops channel; the bot replies with a one-line ack within 1 second; on the next polling iteration for that repo, `security_bug_audit` runs regardless of its cadence; the audit's findings post via the existing notification flow within a few minutes.
