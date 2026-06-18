# Tasks

## 1. Sandbox: deny canon writes and `openspec archive`

- [ ] 1.1 For the spec-writing/implementing/revision session roles (changes-lane implementer, spec-revision `send it` executor, spec-writing audits, audit triage, proposer), add `openspec/specs/` to the sandbox's denied write paths AND add an `openspec archive` invocation to the disallowed-bash patterns. Leave the change's own `openspec/changes/<slug>/` (delta subdir included), the issue unit, and (for the implementer) code paths writable.
- [ ] 1.2 Confirm advisory/read-only sessions (the `[in]`/`[canon]`/`[rules]`/`[out]` gates, the reviewer) are unaffected — they are already read-only.

## 2. Prompt: point at OCTOPUS.md when present

- [ ] 2.1 In the spec-writing/implementing/revision prompts, add a directive: when `OCTOPUS.md` exists at the repository root, read it for the workflow conventions AND the canon/archive constraints. When absent, omit the directive (graceful no-op). Reference the file rather than re-inlining the rules, so the prompt and OCTOPUS.md share one source.

## 3. Catch: post-session revert (fail-closed)

- [ ] 3.1 After a spec-writing/implementing/revision session and BEFORE its commit, run a diff check (modeled on the audit `PlanningLanes` post-hoc revert): revert any modification to `openspec/specs/` AND any created/modified entry under `openspec/changes/archive/` or `issues/archive/` that the session produced. Preserve the session's legitimate writes (the change/issue dir, code).
- [ ] 3.2 On a reverted violation, surface an operator-visible alert naming the session role AND what was reverted (a canon edit and/or an archive entry). The revert proceeds regardless of whether the prompt or sandbox layer held — it is the fail-closed backstop.

## 4. Tests

- [ ] 4.1 A session that writes a file under `openspec/specs/` has the edit reverted before commit AND an alert is surfaced; the rest of the session's work is preserved.
- [ ] 4.2 A session that creates an entry under `openspec/changes/archive/` (simulating `openspec archive`) has it reverted AND surfaced.
- [ ] 4.3 A session that writes only its own `openspec/changes/<slug>/specs/` delta dir (and, for the implementer, code) has those writes preserved — the guard does not touch the change's own deltas.
- [ ] 4.4 The prompt includes the OCTOPUS.md directive when the file is present AND omits it when absent (assert on derivation/presence, not exact prose).
