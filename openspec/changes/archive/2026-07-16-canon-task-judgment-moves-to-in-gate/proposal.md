## Why

The mechanical canon-editing task scan flags any task pairing a mutation-verb token with a canon-target token — it cannot bind a verb to its object, so "Update `docs/CHATOPS.md`'s documentation (the project-documentation canon requires…)" flags as a canon edit. Its own requirement promises precision ("a read-only mention of canon for context is NOT flagged") that token matching structurally cannot deliver: every real task contains a mutation verb, so any mention of canon anywhere in the sentence trips it. Three confirmed false positives in three weeks, each costing an operator round-trip (edit prose to dodge keywords → push → `clear-revision`), and one was misattributed to gate-model quality because nothing surfaces that this path is a keyword matcher, not an agent. The pipeline already pays for an intelligent reader at exactly this point — the `[in]` gate — whose whole job is reading the change and judging it; a keyword heuristic doing semantic judgment in front of it makes the system dumber than its parts.

## What Changes

- The mechanical canon-editing task scan is removed (requirement and implementation).
- The `[in]` gate's session gains the check instead: it also reads the change's `tasks.md` and reports any task that directs the implementer to edit the canonical specs (the implementer implements code and tests only; the delta is folded by `openspec archive`). Findings ride the gate's existing submission → `.needs-spec-revision.json` → chatops alert → halt flow, so the operator experience on a *true* positive is unchanged.
- Where the `[in]` gate is disabled, the archive-time duplicate-requirement abort remains the mechanical backstop — that check is a fact (a duplicate header at fold time), not a heuristic about prose.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: REMOVES "Pre-flight rejects a change whose tasks direct edits to the canonical specs"; MODIFIES "Change-internal contradiction pre-flight check (opt-in)" to include the canon-directing-task check in the gate session's scope.

## Impact

- `autocoder/src/preflight/canon_editing_tasks.rs` deleted; its call site in `autocoder/src/polling_loop/preflight_checks.rs` removed.
- `prompts/change-contradiction-check.md`: instructs the agent to also read `tasks.md` and report canon-directing tasks; the `submit_contradictions` schema gains an optional `canon_editing_tasks` array.
- `autocoder/src/preflight/change_contradiction.rs` + marker population: canon-editing-task findings render into `revision_suggestion` alongside contradiction findings.
- Operator-visible change: prose that merely mentions canon no longer blocks a change; a genuinely canon-directing task is still caught pre-executor wherever the `[in]` gate runs.
