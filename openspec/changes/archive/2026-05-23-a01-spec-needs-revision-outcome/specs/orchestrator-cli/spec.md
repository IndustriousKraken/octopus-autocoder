## ADDED Requirements

### Requirement: Spec-needs-revision executor outcome + marker
The executor SHALL return a new `ExecutorOutcome::SpecNeedsRevision` variant when one or more tasks in a change's `tasks.md` require capabilities outside the executor's sandbox. The agent flags upfront — BEFORE making any changes to the workspace — by scanning `tasks.md` against an enumerated set of unimplementable-task patterns. When the outcome fires, autocoder SHALL write an operator-cleared `.needs-spec-revision.json` marker in the change's directory, post a chatops alert under a new `AlertCategory::SpecNeedsRevision` (24h-throttled per the existing per-category window), and halt the queue walk for the iteration (consistent with the existing halt-on-non-archive semantic). The marker SHALL exclude the change from `list_pending` until removed by the operator, mirroring the perma-stuck marker's pattern.

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

#### Scenario: Marker is operator-cleared, not auto-cleared
- **WHEN** an operator edits `tasks.md` to revise the flagged
  tasks AND commits + pushes the revision
- **THEN** the marker file `.needs-spec-revision.json` is
  NOT auto-removed by autocoder on the next iteration
- **AND** the operator must delete the marker file
  (typically by `rm` and a subsequent commit, OR by deleting
  it locally and relying on autocoder's iteration to surface
  the now-cleaned state on next pass — the marker is in
  `.git/info/exclude` so it's never committed, but operators
  who want a literal git-tracked clear may use `git rm`)
- **AND** the next iteration after the marker is gone
  proceeds normally: the change re-enters `list_pending`
  and the executor is invoked against the revised tasks.md

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
