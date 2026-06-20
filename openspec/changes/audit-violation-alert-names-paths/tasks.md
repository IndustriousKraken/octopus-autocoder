# Tasks

## 1. Name the offending paths in the violation reason

- [ ] 1.1 In `audits/scheduler.rs::detect_write_policy_violation`, change the `WritePolicy::None` arm to build its reason from the offending paths (every dirty entry's `path`) rather than only `entries.len()`. Keep the total count in the reason.
- [ ] 1.2 Add a small shared helper that formats an offending-path list with a fixed display cap and a remaining-count summary ("+K more") when the set exceeds the cap, and conveys the total count. Apply it to all three naming arms (`None`, `OpenSpecOnly`, `PlanningLanes`) so a storm-dirty run cannot emit an unbounded reason.
- [ ] 1.3 Leave the revert, the `AuditWritePolicyViolation` alert category, and the audit-run log's full porcelain section unchanged — only the `reason` string content changes.

## 2. Tests

- [ ] 2.1 A `WritePolicy::None` violation reason contains the offending path (e.g. `opencode.json`) AND the total count. (Behavior on the reason string, mirroring the existing `OpenSpecOnly` path-naming test.)
- [ ] 2.2 The existing `OpenSpecOnly`/`PlanningLanes` path-naming tests still pass (out-of-lane paths are still named).
- [ ] 2.3 An offending set larger than the cap lists capped paths AND a remaining-count summary AND still conveys the total; the list is bounded (does not grow with the input size beyond the cap).
