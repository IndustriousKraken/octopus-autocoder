## ADDED Requirements

### Requirement: A successfully applied revision clears the change's needs-spec-revision marker
When the revision dispatcher applies a revision to an open PR with the dirty-tree `Completed` outcome — a real change committed and force-pushed to the agent branch, per "Revision execution updates the agent branch and posts a reply comment" — the daemon SHALL clear that change's local `.needs-spec-revision.json` marker if it is present, AFTER the commit and `--force-with-lease` push succeed. This eliminates the operator toil of remembering `clear-revision` once a flagged spec has been revised: the open PR already parks the repository, so the marker's hold is redundant, and the marker is transient runtime state (gitignored, lost on re-clone) rather than the authoritative record — the gate or preflight that wrote it remains the source of truth.

The clear SHALL fire ONLY for the dirty-tree `Completed` branch (a revision was actually applied). It SHALL NOT clear the marker on a clean-tree declination (`Completed` with no code change), a substantive `Failed`, a precondition-unmet failure, or `AskUser` — no revision landed in those cases, so a flagged concern may still stand. The clear SHALL be best-effort: a failure to delete the marker is logged but does NOT fail the revision, which has already succeeded.

This clear is a daemon-side filesystem delete performed after the push. It does NOT change the existing revision behavior in which the agent is instructed not to delete the marker and the daemon unstages it so it is never committed. The operator `clear-revision` verb is unchanged and remains the path for markers that never reach a revision (e.g. an operator-must-edit `SpecNeedsRevision` flag) and as a manual override. Clearing on a successful revision is safe under a later close-without-merge: the gate or preflight re-flags the still-un-revised spec on a subsequent pass and re-writes the marker, so no un-revised change is stranded.

#### Scenario: A successfully applied revision clears a present marker
- **GIVEN** a change with a `.needs-spec-revision.json` marker present AND an open PR
- **WHEN** the revision dispatcher processes a triggering comment AND the executor returns the dirty-tree `Completed` outcome (the commit and `--force-with-lease` push to the agent branch succeed)
- **THEN** the daemon deletes that change's `.needs-spec-revision.json` marker
- **AND** the revision's existing behavior (the success reply comment, the cap increment, the seen-marker advance) is unchanged

#### Scenario: A declination or failed revision retains the marker
- **GIVEN** a change with a `.needs-spec-revision.json` marker present AND an open PR
- **WHEN** the revision outcome is a clean-tree `Completed` declination (no code change), OR a substantive `Failed`, OR a precondition-unmet failure, OR `AskUser`
- **THEN** the daemon does NOT delete the `.needs-spec-revision.json` marker (no revision was applied, so the flagged concern may still stand)

#### Scenario: Clearing is conditional and best-effort
- **WHEN** a dirty-tree `Completed` revision succeeds for a change with NO `.needs-spec-revision.json` marker present
- **THEN** the dispatcher performs no marker delete AND reports no error (the clear is conditional on the marker existing)
- **AND** if a marker is present but its deletion fails, the failure is logged AND the revision outcome is still reported as successful (the marker is non-authoritative runtime state)
