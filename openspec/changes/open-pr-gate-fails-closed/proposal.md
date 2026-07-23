## Why

The open-PR skip gate treats a failed GitHub query as "no open PR exists" — its canonical scenario mandates proceeding with the iteration, calling the check "best-effort." But the gate's own rationale enumerates what proceeding costs when the answer was actually "yes": a redundant executor run re-implementing in-flight work, a force-push thrashing a PR under review, and a 422 at PR creation. Observed in production (Abyssum, 2026-07-16): one transient query failure while issue-fix PR #30 was open let the pass proceed, branch-init reset the agent branch from base where the issue still sat, the lane re-implemented it end to end, and duplicate PR #31 was opened twenty seconds after #30 merged. Proceeding on an unconfirmed query is a fail-open control guarding exactly the failures it then permits.

## What Changes

- A failed open-PR query skips the iteration (fail closed for one pass): no branch init, no lane walks, no executor. The daemon logs a WARN naming the failure; the next iteration re-runs the query normally, so a transient blip costs one polling interval instead of a duplicate agentic run.
- Sustained failure is operator-visible: after three consecutive query-failure skips for a repository, a throttled chatops alert names the gate and the last error — a repo silently idling behind a broken query is not acceptable either.
- A successful query keeps today's behavior exactly (open PR → skip; empty → proceed). The revision dispatcher still runs before the gate, so revisions keep reaching open PRs even during query-failure passes... only if its own queries succeed, as today.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: the "Skip iteration when an open PR exists for the agent branch" requirement's failure semantics flip from proceed-on-failure to skip-on-failure with escalating visibility.

## Impact

- `autocoder/src/polling_loop/pr_open.rs`: `open_pr_exists_for_agent_branch_at` returns a three-way outcome (open / none / unknown) instead of collapsing errors to `false`; the URL-parse and token-resolution failure arms follow the same fail-closed treatment.
- `autocoder/src/polling_loop/pass.rs`: the call site skips on unknown; a per-repo consecutive-failure counter (in-memory is sufficient — a restart resetting it merely delays the alert) drives the throttled alert via the existing alert machinery.
- Operator-visible change: a GitHub outage now pauses new work per repo (with WARNs and an eventual alert) rather than risking duplicate runs; revisions and chatops continue.
