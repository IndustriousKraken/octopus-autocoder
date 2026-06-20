## ADDED Requirements

### Requirement: An audit write-policy violation names the offending paths
When a periodic audit trips its write-policy post-check (the workspace carried an unexpected working-tree change after the audit ran), the operator-facing violation reason — the text rendered into the chatops alert AND the audit-run log's violation section — SHALL name the offending path(s), regardless of which `WritePolicy` was violated. A bare count ("N entry(ies)") is insufficient: the operator cannot tell whether the change was harmless tooling ephemera or a real escape without re-deriving it from the workspace.

This generalizes the behavior the prefix-allowlist policies already have (`WritePolicy::OpenSpecOnly` and `WritePolicy::PlanningLanes` reasons already list the paths outside their allowed prefix) to the clean-workspace policy (`WritePolicy::None`), so all three violation reasons name what was written. For `WritePolicy::None` every dirty entry is offending (nothing is allowed); for the prefix-allowlist policies the offending set is the entries outside the allowed prefix(es), as today.

To keep the alert bounded when a single run dirties many files (e.g. a build or cache directory), the reason SHALL list the offending paths up to a fixed cap AND, when more remain, append a remaining-count summary (e.g. "+K more") rather than emitting an unbounded list. The count SHALL still be present so the operator knows the total magnitude. The full, uncapped set remains available in the audit-run log's existing porcelain section.

#### Scenario: A WritePolicy::None violation names the dirty path
- **WHEN** an audit declaring `WritePolicy::None` leaves the workspace with an unexpected entry (e.g. `opencode.json`)
- **THEN** the violation reason names that path
- **AND** the reason still conveys the total number of offending entries

#### Scenario: The prefix-allowlist policies still name their out-of-lane paths
- **WHEN** an audit declaring `WritePolicy::OpenSpecOnly` or `WritePolicy::PlanningLanes` writes a path outside its allowed prefix(es)
- **THEN** the violation reason names the out-of-lane path(s), as before

#### Scenario: A large offending set is capped with a remaining-count summary
- **WHEN** a violation's offending set exceeds the display cap
- **THEN** the reason lists paths up to the cap AND appends a summary of how many more were omitted
- **AND** the total count is still conveyed
- **AND** the audit-run log still records the full, uncapped working-tree status
