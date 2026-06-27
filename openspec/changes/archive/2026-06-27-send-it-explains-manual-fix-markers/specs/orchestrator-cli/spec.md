## MODIFIED Requirements

### Requirement: Spec-revision contradiction alert is a tracked, discussable thread
When autocoder posts a `SpecNeedsRevision` chatops alert for a CONTRADICTION marker — a `.needs-spec-revision.json` whose `unimplementable_tasks` array is empty AND whose `gate_error` is empty AND whose `unarchivable_deltas` array is empty (a `[in]` / `[canon]` / `[rules]` semantic finding, NOT the executor's unimplementable-tasks flag, NOR a gate-error hold, NOR an unarchivable-deltas archivability hold) — autocoder SHALL capture the posted message's `channel` AND `thread_ts` in a `RevisionThreadState` keyed to the repository AND change slug, so a later reply can be matched to the change. When the post is reply-matchable (a `thread_ts` is returned AND a `RevisionThreadState` is recorded), the alert body SHALL advertise that the operator may reply in the thread to discuss the revision OR post `@<bot> send it` to have the change revised and a PR opened. A degraded contradiction post (no `thread_ts`) is not reply-matchable, so its body SHALL NOT advertise `@<bot> send it` as an actionable path.

A MANUAL-FIX marker — one whose `unarchivable_deltas` array is non-empty (the `Spec-delta archivability pre-flight check` hold) OR whose `gate_error` is populated (a verifier gate that could not run) — is NOT a contradiction marker: `send it`'s revision executor cannot fix a delta-header/canon mismatch or a broken gate. For such a marker autocoder SHALL NOT record a `RevisionThreadState`, AND the alert body SHALL state that the change is held for a MANUAL spec fix (naming the cause — unarchivable spec deltas, or a verifier gate that could not run), that `@<bot> send it` cannot revise it, AND that the operator should fix it manually and then post `@<bot> clear-revision` to clear the hold. Because no `RevisionThreadState` is recorded, a later `@<bot> send it` in that thread falls through to the existing generic untracked-thread refusal (per `chatops-manager`'s `send it` routing) — this requirement does NOT add a new `send it` thread context and does NOT change that generic refusal.

A degraded post that returns no `thread_ts` SHALL still write the marker AND alert but SHALL NOT record a `RevisionThreadState` (the alert is simply not reply-matchable — graceful degradation, never an error). The `clear-revision` verb remains the unchanged manual escape for all marker causes.

#### Scenario: A contradiction alert is tracked and advertises the thread
- **WHEN** autocoder posts the `SpecNeedsRevision` alert for a marker with empty `unimplementable_tasks` AND empty `gate_error` AND empty `unarchivable_deltas`
- **THEN** it records a `RevisionThreadState` carrying the alert's `channel`, `thread_ts`, repository, AND change slug
- **AND** the alert body states that a reply discusses the revision AND that `@<bot> send it` revises the change and opens a PR

#### Scenario: An unimplementable-tasks alert is not tracked as a revision thread
- **WHEN** the marker's `unimplementable_tasks` is non-empty (the executor's flag-and-halt case)
- **THEN** no `RevisionThreadState` is recorded for it AND the alert does not advertise the revision thread
- **AND** that marker keeps its existing operator-authored flow (the agent flags; the operator edits `tasks.md`)

#### Scenario: A degraded post is not reply-matchable
- **WHEN** the alert post returns no `thread_ts`
- **THEN** the marker AND alert are still produced
- **AND** no `RevisionThreadState` is recorded (the thread is simply not reply-matchable)
- **AND** the alert body does not advertise `@<bot> send it` as an actionable path (it cannot be matched)

#### Scenario: An unarchivable-deltas alert explains the manual fix and is not tracked
- **WHEN** autocoder posts the `SpecNeedsRevision` alert for a marker whose `unarchivable_deltas` is non-empty — even though its `unimplementable_tasks` AND `gate_error` are both empty
- **THEN** no `RevisionThreadState` is recorded for it (it is NOT a contradiction marker AND is NOT `send it`-able)
- **AND** the alert body states that the change is held for unarchivable spec deltas, that `@<bot> send it` cannot revise it, AND that the operator should fix the delta header(s) to match canonical and then post `@<bot> clear-revision`

#### Scenario: A gate-error alert explains the manual fix and is not tracked
- **WHEN** autocoder posts the `SpecNeedsRevision` alert for a marker whose `gate_error` is populated
- **THEN** no `RevisionThreadState` is recorded for it (it is NOT a contradiction marker AND is NOT `send it`-able)
- **AND** the alert body states that the change is held because a verifier gate could not run, that `@<bot> send it` cannot revise it, AND that the operator should fix the gate and then post `@<bot> clear-revision`
