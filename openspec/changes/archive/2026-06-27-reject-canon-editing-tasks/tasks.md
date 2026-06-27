# Tasks

## 1. The pre-flight check

- [x] 1.1 Add a `tasks.md`-content pre-flight that runs at the same point as the spec-delta archivability pre-flight (before the executor, every change, every iteration). Parse the change's `tasks.md` and flag any task pairing a mutation verb (apply, add, copy, write, edit, update, insert, append, paste, create, populate — case-insensitive) with a canonical-specs target (the path segment `openspec/specs/`, OR the words `canon` / `canonical spec`). Exclude references to the change's own delta (`openspec/changes/<slug>/specs/`, or `specs/<cap>/spec.md` qualified as "in this change") and read-only mentions (no mutation verb).
- [x] 1.2 On a flag, write `.needs-spec-revision.json` (extend the existing schema/reason set — a `canon_editing_tasks` field naming the offending task line(s) is acceptable) with a `revision_suggestion` stating the rule (implementer does code+tests only; the delta is folded by `openspec archive`; no task may apply it to `openspec/specs/`). Post the `AlertCategory::SpecNeedsRevision` alert (24h throttle) AND halt the queue walk per the same-repo blocking policy.
- [x] 1.3 Ensure the executor is NOT invoked AND the `[in]` / `[canon]` verifier gates do NOT run for a flagged change (reject precedes both, like the archivability pre-flight).

## 2. Prompt prevention

- [x] 2.1 Update `prompts/security-bug-audit.md` and `prompts/missing-tests-audit.md`: `tasks.md` is code-and-tests only — never a task that edits `openspec/specs/` or applies the change's delta to canon; the archive folds the delta automatically.
- [x] 2.2 Update the change implementer prompt template: alongside the existing "openspec archive is denied in this sandbox — leave the working tree dirty" instruction, add "do NOT edit `openspec/specs/` directly — your change's spec delta is folded into canon automatically on archive."

## 3. Tests

- [x] 3.1 A `tasks.md` with an "apply … to openspec/specs/…" task is flagged: the pre-flight writes the marker, the executor is not invoked, and the gates do not run (assert behavior/state, not message wording).
- [x] 3.2 A `tasks.md` referencing the change's own `specs/<cap>/spec.md` delta, or mentioning canon read-only, is NOT flagged and proceeds.
- [x] 3.3 A clean code-and-tests `tasks.md` is NOT flagged.
- [x] 3.4 Behavior check on the audit prompts: an audit-produced change's `tasks.md` carries no canon-editing task (derive from produced artifacts, not prompt substrings).

## 4. Docs

- [x] 4.1 Document the new pre-flight reason in `docs/OPERATIONS.md` / `docs/TROUBLESHOOTING.md`'s `.needs-spec-revision.json` section: a change can be held because a task directs a canon edit; the fix is to remove that task and clear the marker.
