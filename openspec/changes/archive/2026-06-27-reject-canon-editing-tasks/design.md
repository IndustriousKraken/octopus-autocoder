# Design

## D1 — Where it runs

The check is a sibling of the existing `Spec-delta archivability pre-flight check`:
it runs before the executor, on every change, every iteration, and reuses the same
failure plumbing — write `.needs-spec-revision.json`, post the
`AlertCategory::SpecNeedsRevision` alert (24h-throttled), AND halt the queue walk
under the same-repo blocking policy. The difference is what it inspects: the
archivability check parses delta HEADERS; this check scans `tasks.md` CONTENT.
Catching the defect here means no executor run and no `[in]`/`[canon]` gate run is
spent on a change that is doomed at archive time.

## D2 — Detection: mutation verb + canonical-specs target

The heuristic is mechanical and precision-biased. A task is flagged when it pairs:

- a mutation verb — apply, add, copy, write, edit, update, insert, append, paste,
  create, populate (case-insensitive); AND
- a canonical-specs target — the path segment `openspec/specs/`, OR the words
  `canon` / `canonical spec(s)`.

A reference to the change's OWN delta is NOT a canonical-specs target: a path under
`openspec/changes/<slug>/specs/`, or a bare `specs/<capability>/spec.md` qualified
as "in this change", is the legitimate delta and is ignored. A read-only mention of
canon for context ("consistent with the existing scheduled-payments spec") carries
no mutation verb and is not flagged.

This is a high-precision tripwire, not an exhaustive semantic check: it reliably
catches the observed literal-path case ("Apply the … block to
openspec/specs/…/spec.md") and the common phrasings, and accepts that a sufficiently
creative phrasing could slip past — which the prompt prevention (D4) and the noted
revert-before-archive backstop (D5) cover. Keeping it mechanical preserves the
existing pre-flight's "cheap, no-LLM" property.

## D3 — Marker reason

The marker's `revision_suggestion` names the offending task(s) AND states the rule:
the implementer implements code and tests only; the change's spec delta lives in
`specs/<capability>/spec.md` and is folded into canon by `openspec archive` — a task
must not apply it to `openspec/specs/`. The operator's fix is to delete the
offending task and clear the marker (the deltas themselves are untouched).

## D4 — Prompt prevention

The spec-writing audit prompts (`security-bug-audit.md`, `missing-tests-audit.md`)
and the change implementer prompt are updated so agents do not generate or perform
canon edits. The implementer prompt already says "openspec archive is denied in this
sandbox — leave the working tree dirty"; this adds the missing half: "do NOT edit
`openspec/specs/` directly — your change's spec delta is folded automatically on
archive." Prevention reduces how often the pre-flight must reject; the pre-flight is
the backstop for agents that ignore or never saw the instruction.

Per the project's testing rule, the prompts are verified by behavior (an audit's
produced `tasks.md` carries no canon-editing task; the pre-flight rejects one that
does), not by asserting prompt substrings.

## D5 — Out of scope: revert-before-archive backstop

The most bulletproof catch is mechanical and at archive time: revert any
working-tree modification under `openspec/specs/` before `openspec archive`, since
the implementer must never pre-fold the delta regardless of whether a task directed
it. That catches the context-lost case where an agent edits canon with NO
corresponding task (which a `tasks.md` scan cannot see). It is a natural follow-on
but is left out of this change to keep it focused on the pre-flight reject the
operator asked for; this change's pre-flight + prompt prevention resolve the
observed (task-directed) failure mode.
