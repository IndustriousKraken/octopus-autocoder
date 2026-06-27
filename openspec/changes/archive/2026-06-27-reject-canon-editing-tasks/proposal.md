# Reject changes whose tasks direct edits to the canonical specs

## Why

A change's `tasks.md` is the implementer's marching orders, and the implementer
implements CODE and TESTS only — the change's spec delta lives in its own
`specs/<capability>/spec.md` and is folded into the canonical specs by
`openspec archive` automatically. When a task instead directs the implementer to
apply the delta to `openspec/specs/` directly (e.g. "Apply the ADDED Requirements
block … to openspec/specs/scheduled-payments/spec.md"), the implementer dutifully
pre-folds the requirement into canon; `openspec archive` then tries to fold the
same delta, finds it already present, and aborts on a duplicate-ADD. The change
fails every iteration and goes perma-stuck — after burning a full executor run
(and the `[in]`/`[canon]` gate agents) each time.

This was observed on a security-bug-audit-authored change whose `tasks.md`
contained exactly such a "Spec update: apply the block to openspec/specs/" task.
The change's deltas were otherwise valid (it validates `--strict`, passes the
header-only archivability pre-flight, and archives cleanly against clean canon) —
the only defect was the self-defeating task. The defect is agent-agnostic: any
spec-writing audit, brownfield session, or a context-lost agent that "forgets how
OpenSpec works" can emit such a task.

The cheapest place to catch it is BEFORE the executor: a mechanical pre-flight that
rejects the change for revision, so no LLM run and no gate run is wasted, and the
operator gets a clear reason instead of a perma-stuck two iterations later.

## What Changes

- A pre-flight check (a sibling of the archivability pre-flight) scans each
  change's `tasks.md` before the executor runs. When a task directs a write to the
  canonical specs — a mutation verb (apply, add, copy, write, edit, update, insert,
  append, paste, create, populate) paired with a canonical-specs target
  (`openspec/specs/…`, or "canon" / "the canonical spec") — the daemon writes
  `.needs-spec-revision.json` (reason: a task directs a canon edit), posts the
  `SpecNeedsRevision` alert, AND halts, WITHOUT invoking the executor or the
  verifier gates. A read-only reference to canon for context is NOT flagged.
- The spec-writing audit prompts AND the change implementer prompt are updated to
  state that `tasks.md` is code-and-tests only — never a task that edits
  `openspec/specs/` or applies a delta to canon; the archive folds the delta. This
  is prevention; the pre-flight is the agent-agnostic backstop.

## Impact

- Affected specs: `orchestrator-cli` (the new pre-flight requirement).
- Affected code: the pre-flight pipeline that already runs the archivability check
  (same marker, same halt + same-repo-blocking semantics); the spec-writing audit
  prompts (`security-bug-audit.md`, `missing-tests-audit.md`) and the change
  implementer prompt template.
- Independent change; it touches no requirement another in-flight change modifies.
- A complementary backstop — reverting any implementer edit to `openspec/specs/`
  before `openspec archive`, which also catches a canon edit made with no
  corresponding task — is noted as a follow-on, not included here.
