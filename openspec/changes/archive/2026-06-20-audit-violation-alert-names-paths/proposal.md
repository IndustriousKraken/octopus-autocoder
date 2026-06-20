# An audit write-policy violation alert names the offending paths

## Why

When a periodic audit trips its write-policy post-check, the chatops alert is
the operator's first (often only) signal. For `WritePolicy::None` audits the
alert reason is a bare count — "workspace dirty after audit (expected clean):
1 entry(ies)" — naming nothing. The operator cannot tell whether the dirty
entry was harmless tooling ephemera or a real escape without opening the
audit-run log and reading its porcelain section.

This already bit us: an advisory audit reported a violation whose actual cause
(an `opencode.json` the wrapped CLI auto-generates) was only identifiable after
a log dive. The prefix-allowlist policies (`OpenSpecOnly`, `PlanningLanes`)
already name their offending paths in the reason; only the clean-workspace
policy (`None`) reports a count. The fix is to make path-naming uniform across
all three.

## What Changes

- A new `orchestrator-cli` requirement: an audit write-policy violation's
  operator-facing reason names the offending path(s), regardless of policy.
  This generalizes the existing `OpenSpecOnly`/`PlanningLanes` behavior to
  `None` (where every dirty entry is offending). To keep the alert bounded
  when a run dirties many files, the reason lists paths up to a fixed cap and
  appends a remaining-count summary; the total count is still conveyed, and
  the full uncapped set stays in the audit-run log.
- `detect_write_policy_violation` (`audits/scheduler.rs`) builds the `None`
  reason from the dirty entries' paths (capped) instead of only their count.
  The `OpenSpecOnly`/`PlanningLanes` arms already name paths; they gain the
  same cap so one storm-dirty run cannot produce an unbounded alert.

## Impact

- Affected specs: `orchestrator-cli` (ADD the violation-names-paths
  requirement). The existing "Periodic audit framework" scenario
  ("WritePolicy::None audit cannot modify the workspace") is unchanged — it
  says a chatops alert is posted; this refines what that alert contains.
- Affected code: `audits/scheduler.rs` (`detect_write_policy_violation` — the
  `None` arm names paths; a shared cap helper bounds all arms). No change to
  the revert, the alert category, or the audit-run log (which already records
  the full porcelain). Purely a legibility improvement to the reason string.
- The reason flows verbatim into both the chatops alert and the log's
  violation section, so both gain the path(s) at once.
