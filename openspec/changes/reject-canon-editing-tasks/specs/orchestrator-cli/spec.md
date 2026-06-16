## ADDED Requirements

### Requirement: Pre-flight rejects a change whose tasks direct edits to the canonical specs
Before invoking the executor against any change, autocoder SHALL scan the change's `tasks.md` for any task that directs a direct edit to the canonical specs, AND reject the change for revision when one is found. This runs alongside the `Spec-delta archivability pre-flight check` — the same point in the pipeline, the same marker, the same halt semantics — but it inspects task CONTENT rather than delta headers. The rationale: the implementer implements code and tests only; a change's spec delta lives in its own `specs/<capability>/spec.md` and is folded into the canonical specs by `openspec archive`. A task that instead applies the delta to `openspec/specs/` makes the implementer pre-fold canon, after which `openspec archive` aborts on a duplicate requirement and the change goes perma-stuck — so the defect SHALL be caught before any executor or verifier-gate run is spent on it.

The detection is mechanical AND precision-biased: a task is flagged when it pairs a mutation verb (apply, add, copy, write, edit, update, insert, append, paste, create, populate — case-insensitive) with a canonical-specs target (the path segment `openspec/specs/`, OR the words `canon` / `canonical spec`). A reference to the change's OWN delta — a path under `openspec/changes/<slug>/specs/`, or a `specs/<capability>/spec.md` qualified as belonging to this change — is NOT a canonical-specs target. A read-only mention of canon for context (no mutation verb) is NOT flagged.

On a flagged task, autocoder SHALL write `.needs-spec-revision.json` whose `revision_suggestion` names the offending task(s) AND states that the implementer implements code and tests only — the spec delta is folded by `openspec archive`, so no task may apply it to `openspec/specs/`. It SHALL post the `AlertCategory::SpecNeedsRevision` chatops alert (subject to the existing 24h throttle) AND halt the queue walk for this iteration per the same-repo blocking policy. The executor SHALL NOT be invoked for this change, NOR SHALL the `[in]` / `[canon]` verifier gates run for it.

#### Scenario: A task applying the delta to canon is flagged before the executor
- **WHEN** a change's `tasks.md` contains a task such as `Apply the ADDED Requirements block from specs/<cap>/spec.md to openspec/specs/<cap>/spec.md`
- **THEN** the pre-flight flags it (mutation verb `Apply` + canonical-specs target `openspec/specs/`)
- **AND** autocoder writes `.needs-spec-revision.json` naming the offending task, posts the `SpecNeedsRevision` alert, AND halts
- **AND** the executor is NOT invoked AND the `[in]` / `[canon]` gates do NOT run for this change

#### Scenario: A read-only reference to canon is not flagged
- **WHEN** a task references the canonical specs for context only (e.g. "ensure the change matches the existing `<cap>` contract") with no mutation verb directing a write to `openspec/specs/`
- **THEN** the pre-flight does NOT flag it
- **AND** the change proceeds normally to the executor

#### Scenario: A reference to the change's own delta is not flagged
- **WHEN** a task references the change's own delta — a path under `openspec/changes/<slug>/specs/`, or `specs/<cap>/spec.md` qualified as belonging to this change
- **THEN** the pre-flight does NOT treat it as a canonical-specs target
- **AND** the change proceeds normally

#### Scenario: A clean change is unaffected
- **WHEN** a change's `tasks.md` directs only code and test work
- **THEN** the pre-flight finds nothing to flag
- **AND** the change proceeds to the executor exactly as before
