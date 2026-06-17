# Auditors choose their output lane by canon judgment

## Why

The two bug/gap spec-writing audits (`security_bug_audit`, `missing_tests_audit`)
can only emit OpenSpec changes. Most of what they find is a defect in code that
is already correctly specified — a missing atomic claim, an unhandled error path,
a race — i.e. behavior-preserving corrections that belong in the issues lane, not
new canonical requirements. Forcing every finding into a spec has two costs:

- **Canon grows without benefit.** Each impl-bug fix mints requirements that
  restate behavior canon already implies, inflating the corpus an operator and
  every future audit must reason over.
- **Avoidable contradiction kickbacks.** An audit that invents vocabulary to
  describe a fix (a new state, a new term) collides with the existing canonical
  requirement for that area, and the change is bounced by the change-vs-canonical
  pre-flight gate after the authoring context is gone — the operator then rewrites
  the spec by hand.

The issues lane already exists for exactly this class of work (behavior-preserving
corrections, no spec delta), and the implementer already kicks an issue back to
the changes lane if its fix would require new behavior. What is missing is the
authoring end: an audit cannot write an issue, and nothing directs it to choose
the right lane. The audits run on high-capability models that can make this call.

## What Changes

- `security_bug_audit` AND `missing_tests_audit` SHALL read the canonical specs
  for the area they touch AND choose their output lane by judgment: a change that
  alters an observable contract goes to the spec lane (`openspec/changes/<slug>/`);
  a behavior-preserving fix to already-correctly-specified code goes to the issues
  lane (`issues/<slug>/`). They SHALL NOT default to the spec lane.
- Their `WritePolicy` is broadened so the audit may write under EITHER
  `openspec/changes/` OR `issues/` (still reverting any write outside those two
  planning lanes — no code edits).
- Lane choice is gated by `features.issues`: when the flag is off for a repository
  the audit uses the spec lane only, preserving today's behavior; when on, it may
  choose either lane.
- The prompts direct the agent to reuse canonical vocabulary (never coin a new
  term for a concept canon already names) AND, when a fix genuinely requires
  changing a canonical contract, to write a spec and say so plainly in the
  proposal rather than bury a contract change inside an issue.
- `canon_consolidation_audit` is unchanged: its purpose is to evolve canon, so it
  stays spec-only.

## Stacked context

This is `a01` of a three-change stack. `a02-audit-output-gate-checked` adds the
contradiction-check-and-self-heal over both lanes (spec lane: `--strict` + the
internal and canonical contradiction gates with bounded self-heal; issue lane:
the authoring-time canon check that verifies an issue truly carries no contract
change). `a03-spec-revision-thread` adds the interactive revision thread for the
residue. This change only gives auditors the lane and the judgment to pick it.

## Impact

- Affected specs: `orchestrator-cli` (the two audit requirements + a new
  lane-choice requirement).
- Affected code: the spec-writing audit harness `WritePolicy` (a lane-aware
  policy), the post-run write-scope check, the commit step, and the
  `security-bug-audit` / `missing-tests-audit` prompts.
- Backwards-compatible: with `features.issues` off, behavior is unchanged. This
  change keys off the flag's runtime value, not its default, so it composes with
  a separate change to the lane's default state.
