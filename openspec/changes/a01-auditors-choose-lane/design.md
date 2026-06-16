# Design

## D1 — Lane choice is the agent's, grounded in canon

Before writing, the audit reads the canonical spec(s) for the capability its
finding touches and decides the lane by one question: **does fixing this require
changing an observable contract?**

- **No** — the code is already correctly specified and the fix preserves observed
  behavior (a missing atomic claim, an unhandled error path, a leak, a race). →
  **issues lane.** `issues/<slug>/` with `issue.md` (diagnosis + acceptance stated
  against the EXISTING specification) and `tasks.md`, no `specs/` directory.
- **Yes** — the fix needs new or changed behavior at an observable boundary
  (public API, serialized/wire format, CLI surface, a state machine, a new
  invariant), OR canon itself permits/mandates the defective behavior and must be
  corrected. → **spec lane.** `openspec/changes/<slug>/` with the usual deltas; a
  contract-correcting fix uses a `MODIFIED` delta of the exact canonical
  requirement.

Defaulting to the spec lane is prohibited: the agent must make the call. This
mirrors the existing implement-time contract in `executor` ("Issue-flavored
implementer prompt verifies against existing canon"), which kicks an issue back
to the changes lane if its fix would require new behavior — a01 is the authoring
end of that same judgment.

## D2 — A lane-aware WritePolicy

Today the bug/gap audits run under `WritePolicy::OpenSpecOnly`, whose post-run
check reverts any write outside `openspec/changes/`. a01 introduces a policy that
permits writes under EITHER planning lane — `openspec/changes/<slug>/` OR
`issues/<slug>/` — and still reverts a write to source, docs, config, or anywhere
else (the existing `git reset --hard HEAD && git clean -fd` revert path is
unchanged; only the set of allowed prefixes widens). `workspace_writable()`
remains true (the mount is writable); only the post-run path-scope check changes.
`canon_consolidation_audit` keeps `OpenSpecOnly`.

The variant is named `WritePolicy::PlanningLanes` here as a concrete suggestion;
the binding contract is the two-prefix allowlist, not the identifier.

`PlanningLanes`' enforcement semantics are defined in this change's lane-choice
requirement (and its "write outside the two planning lanes is reverted"
scenario). The foundational `Periodic audit framework` requirement illustrates
per-policy enforcement by example (`None`, `OpenSpecOnly`) and makes no
exhaustiveness claim about the policy set, so it is deliberately not modified —
adding a third variant does not contradict it.

## D3 — Gated by `features.issues`

Lane choice follows the `features.issues` flag's runtime value (whatever its
default), so this change composes with a separate flip of the lane's default:

- **flag off** — the audit writes the spec lane only, exactly as today. No new
  behavior for repositories that have not opted into issues; an issue written into
  a disabled lane would never be worked, so it is never offered.
- **flag on** — the audit may choose either lane.

The audit resolves `features.issues` for the repository it runs against and passes
the result to the agent as part of its input, so the prompt only offers the issue
lane when it is actually enabled.

## D4 — Prompt content

Both prompts gain, ahead of the write step:

1. Read the canonical spec(s) for the area of the finding before proposing a fix.
2. Reuse canonical vocabulary — never coin a new term (a new state name, a new
   noun) for a concept canon already names; prefer a `MODIFIED` delta of an
   existing requirement over an `ADDED` requirement that introduces a parallel
   term.
3. Choose the lane per D1; when `features.issues` is off, the issue lane is not
   offered.
4. For the issue lane, produce `issue.md` (acceptance criteria stated against the
   existing specification) and `tasks.md`, no `specs/` directory.
5. Legibility: when a fix genuinely requires changing a canonical contract, write
   a spec and state the contract change plainly in the proposal's rationale — do
   not bury a contract change inside an issue. (This keeps the issue lane an
   honest "no contract change" claim, which a02 then verifies.)

Per the project's testing standard, none of this is pinned by asserting prompt
substrings; the behavior is verified through the lane the audit selects and the
artifacts it produces.

## D5 — Scope: which audits

Only the bug/gap audits choose a lane: `security_bug_audit` and
`missing_tests_audit`. `canon_consolidation_audit` exists to evolve canon and is
definitionally spec-lane; it is untouched. The advisory audits (`drift_audit`,
`architecture_advisor`, `documentation_audit`) write no units and are unaffected.

## D6 — What a01 deliberately does not do

a01 gives auditors the lane and the judgment to pick it. It does NOT add the
contradiction gates or self-heal (that is `a02`), so an a01 audit still relies on
the existing change-vs-canonical pre-flight gate at implement time as the backstop
for a mis-framed spec, and on the implementer kick-back for a mis-routed issue.
a02 moves both checks to authoring time.
