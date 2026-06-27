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

#### Scenario: No marker present — clear is a no-op
- **WHEN** a dirty-tree `Completed` revision succeeds for a change with NO `.needs-spec-revision.json` marker present
- **THEN** the dispatcher performs no marker delete AND reports no error (the clear is conditional on the marker existing)

#### Scenario: Marker deletion failure does not fail the revision
- **GIVEN** a change with a `.needs-spec-revision.json` marker present AND an open PR
- **WHEN** the revision dispatcher processes a triggering comment AND the executor returns the dirty-tree `Completed` outcome AND the marker deletion fails
- **THEN** the deletion failure is logged AND the revision outcome is still reported as successful (the marker is non-authoritative runtime state)

## MODIFIED Requirements

### Requirement: Spec-needs-revision executor outcome + marker
The executor SHALL return a new `ExecutorOutcome::SpecNeedsRevision` variant when one or more tasks in a change's `tasks.md` require capabilities outside the executor's sandbox. The agent flags upfront — BEFORE making any changes to the workspace — by scanning `tasks.md` against an enumerated set of unimplementable-task patterns. When the outcome fires, autocoder SHALL write an operator-cleared `.needs-spec-revision.json` marker in the change's directory, post a chatops alert under a new `AlertCategory::SpecNeedsRevision` (24h-throttled per the existing per-category window), and halt the queue walk for the iteration (consistent with the existing halt-on-non-archive semantic). The marker SHALL exclude the change from `list_pending` until removed by the operator or by the revision dispatcher on a dirty-tree Completed outcome, mirroring the perma-stuck marker's pattern.

The agent SHALL NOT auto-edit `tasks.md` to make the spec implementable. The agent flags; the operator authors the edit. This preserves the project's invariant that no AI process modifies its own marching orders without human review.

#### Scenario: Agent flags unimplementable tasks before doing work
- **WHEN** the executor invokes the agent on a change whose
  `tasks.md` includes one or more tasks matching the
  unimplementable-task patterns documented in the implementer
  prompt template (e.g. `sudo` on real host, missing tools,
  real GitHub tag pushes, browser interactions, VM/container
  spin-up, manual smoke tests, manual external observation)
- **THEN** the agent emits the `SpecNeedsRevision` outcome
  with each flagged task's id + verbatim text + one-line
  reason AND a free-form `revision_suggestion` describing
  what to change in `tasks.md`
- **AND** the agent does NOT modify any files in the workspace
  before emitting the outcome (the flag-and-halt happens
  pre-implementation; no partial work is committed)

#### Scenario: autocoder writes the marker and alerts
- **WHEN** the executor returns `SpecNeedsRevision { ... }` for
  change `<slug>` in workspace `<workspace>`
- **THEN** autocoder writes
  `<workspace>/openspec/changes/<slug>/.needs-spec-revision.json`
  containing: `change` name, RFC-3339 `marked_at`, the full
  `unimplementable_tasks` list, the `revision_suggestion`, and
  a static `operator_action` field naming the file the
  operator needs to edit
- **AND** posts exactly one chatops notification under
  `AlertCategory::SpecNeedsRevision` (subject to the existing
  24h per-category throttle) whose body lists each flagged
  task's id + text, the agent's revision suggestion, the
  operator action checklist, AND the marker file path + the
  per-change run log path
- **AND** halts the queue walk for this iteration: no later
  pending changes are processed in this iteration (mirroring
  the `halt-queue-walk-on-non-archive` semantic)

#### Scenario: Marker excludes change from list_pending
- **WHEN** a subsequent iteration runs AND the marker
  `openspec/changes/<slug>/.needs-spec-revision.json` exists
- **THEN** `queue::list_pending` does NOT return `<slug>`
- **AND** the executor is never invoked for `<slug>` in this
  iteration
- **AND** the perma-stuck counter for `<slug>` is NOT
  incremented (the marker is operator-action territory, not
  repeat-failure territory)

#### Scenario: Marker is operator-cleared for the tasks.md workflow; auto-cleared only on a successful revision-dispatcher run
- **WHEN** an operator edits `tasks.md` to revise the flagged tasks AND commits + pushes the revision
- **THEN** the marker file `.needs-spec-revision.json` is NOT auto-removed by autocoder on the next polling iteration
- **AND** the operator must delete the marker file (typically by `rm` and a subsequent commit, OR by deleting it locally and relying on autocoder's iteration to surface the now-cleaned state on next pass — the marker is in `.git/info/exclude` so it's never committed)
- **AND** the next iteration after the marker is gone proceeds normally: the change re-enters `list_pending` and the executor is invoked against the revised tasks.md
- **EXCEPTION** the revision dispatcher clears the marker on a dirty-tree `Completed` outcome, per the "A successfully applied revision clears the change's needs-spec-revision marker" requirement above — this is the only auto-clear path; the prohibition above applies exclusively to the edit-tasks.md workflow and any other polling iteration

#### Scenario: Operator overrides an over-conservative flag
- **WHEN** an operator reviews the flagged tasks AND judges
  the agent was overly cautious (e.g. the agent flagged a
  task the operator believes IS implementable)
- **THEN** the operator deletes the marker file WITHOUT
  editing tasks.md
- **AND** the change re-enters `list_pending` on the next
  iteration
- **AND** if the agent flags the same tasks again, the
  operator may add a comment in tasks.md near the flagged
  task explaining why it's implementable (e.g. naming a
  tool path or workflow that resolves the concern), OR they
  may update the implementer prompt template via a separate
  change to relax the relevant pattern

#### Scenario: Marker file is gitignored at workspace root
- **WHEN** `workspace::ensure_initialized` runs
- **THEN** `.git/info/exclude` contains
  `.needs-spec-revision.json` (added alongside the existing
  `.failure-state.json`, `.audit-state.json`,
  `.perma-stuck.json` entries)
- **AND** the marker file does NOT trip the pre-pass
  dirty-workspace check AND is NOT removed by
  `git clean -fd` during the per-iteration recovery path

#### Scenario: Agent does NOT auto-edit tasks.md
- **WHEN** the agent identifies one or more unimplementable
  tasks
- **THEN** the agent emits the outcome with the list AND a
  suggestion text, but does NOT modify `tasks.md` itself
- **AND** does NOT create or modify any spec artifacts under
  `openspec/changes/<slug>/`
- **AND** does NOT submit a PR proposing the revision
- **AND** the operator remains the sole author of the tasks.md
  edit, preserving the contract that no AI process edits its
  own marching orders without human review

#### Scenario: Malformed outcome sentinel falls back to Failed
- **WHEN** the agent emits a `SpecNeedsRevision` sentinel
  that fails to deserialize (missing required fields, unknown
  type, empty `unimplementable_tasks` list, etc.)
- **THEN** the Claude CLI executor logs a WARN naming the
  parse failure with an excerpt of the offending payload
- **AND** the executor returns `Failed { reason: "agent
  emitted unparseable SpecNeedsRevision sentinel: <excerpt>"
  }` instead of the new variant
- **AND** the polling loop's existing Failed-outcome handling
  kicks in (perma-stuck counter increments, no marker
  written) — the unparseable-sentinel case must NOT silently
  succeed
