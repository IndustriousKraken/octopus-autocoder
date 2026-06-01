## Why

autocoder uses OpenSpec (https://github.com/Fission-AI/OpenSpec) for change management, but neither the agent-facing prompts nor the human-facing docs link to OpenSpec's upstream documentation. The result: agents drafting new spec deltas — `chat-request-triage`, `audit-triage`, `missing-tests-audit`, `security-bug-audit`, `brownfield-draft`, `scout` — operate without a canonical reference for scenario syntax (GIVEN/WHEN/THEN), delta format (ADDED / MODIFIED / REMOVED / RENAMED), or requirement-header rules. They derive conventions from the in-repo specs they happen to see, which is a slow learner AND drifts as the in-repo specs themselves drift. Operators drafting their first OpenSpec change face the same gap from the human side.

A concrete recent example: the existing in-repo specs use `WHEN`/`THEN`/`AND` exclusively across all eight canonical capabilities, with zero `GIVEN` usage. OpenSpec's upstream `concepts.md` and `getting-started.md` lead every example with `GIVEN` to separate pre-existing state from the triggering action — a distinction that improves readability when world state matters. Neither the agent nor the operator currently has any way to discover that distinction short of reading the upstream docs by name. A link in the right places closes the gap without inventing a parallel convention doc.

This change adds two pointers — one for agents (in the prompts that draft new spec content), one for humans (in `docs/README.md`). No new convention text is authored AND no behavior changes; the change is strictly an information-routing improvement.

## What Changes

**Agent-facing pointer in spec-drafting prompts.** A one-line pointer to OpenSpec's upstream docs SHALL appear in each prompt that drafts OR materially modifies OpenSpec change content. The target set is:

- `prompts/implementer.md` — implements existing changes; occasionally edits spec deltas; reads canonical specs heavily.
- `prompts/implementer-revision.md` — revises existing changes' specs in response to reviewer findings.
- `prompts/chat-request-triage.md` — converts chatops `propose` requests into new OpenSpec changes.
- `prompts/audit-triage.md` — converts audit `send it` directives into new OpenSpec changes.
- `prompts/missing-tests-audit.md` — drafts `tests-*` proposals.
- `prompts/security-bug-audit.md` — drafts `fix-*` / `secure-*` proposals.
- `prompts/brownfield-draft.md` — drafts canonical capability specs from existing code.
- `prompts/scout.md` — surveys for triage candidates; may produce draft proposals.

The pointer text is a single line near the top of each prompt's "you are drafting OpenSpec content" framing (whatever shape that takes per prompt):

> OpenSpec conventions reference: https://github.com/Fission-AI/OpenSpec/tree/main/docs — `concepts.md` covers scenario syntax (`GIVEN`/`WHEN`/`THEN`), delta format (`ADDED`/`MODIFIED`/`REMOVED`/`RENAMED`), AND requirement-header rules; `getting-started.md` shows worked examples. Consult these when in doubt about format.

The exact wording MAY vary per prompt (some prompts already have a docs-link section; the pointer SHOULD fit that section's voice). The load-bearing requirement is the link AND the topical hint of what the linked docs cover.

**Human-facing pointer in `docs/README.md`.** A new entry under the existing `## Internals` section (OR a new `## Contributing` section near the bottom) links to OpenSpec's upstream docs with the same topical hint as the prompt pointer. One line; no separate `docs/SPEC-CONVENTIONS.md` file (project-specific deviations from upstream conventions, when they accrue, are already captured in agent memory AND don't yet warrant a dedicated convention document).

The entry SHALL be phrased without kitsch (no exclamation marks, no "tip:" framing, no faux-friendly hooks) per the project's documentation tone rule.

**No code changes.** Documentation-and-prompts only. No spec convention is introduced or modified at the canonical-spec level beyond the new project-documentation requirement.

**No new agent-facing convention file** (e.g., `openspec/AGENTS.md` recognized by OpenSpec upstream). The convention reference is the upstream docs themselves; adding a local AGENTS.md without project-specific deviations would be ceremony, not signal. If project-specific deviations accumulate (the existing `aNN-` change-naming rule from memory, the MODIFIED-preserves-canonical rule, the no-kitsch tone rule), a future change can collect them into a local convention file AND cross-link from the upstream pointer. This change does NOT attempt that consolidation.

## Impact

- **Affected specs:**
  - `project-documentation` — ADDED a new requirement defining where the OpenSpec upstream-docs pointer lives (the spec-drafting prompt set AND `docs/README.md`), AND defining the pointer's content shape so future audits can verify presence.
- **Affected code:**
  - `prompts/implementer.md` — add the one-line pointer.
  - `prompts/implementer-revision.md` — same.
  - `prompts/chat-request-triage.md` — same.
  - `prompts/audit-triage.md` — same.
  - `prompts/missing-tests-audit.md` — same.
  - `prompts/security-bug-audit.md` — same.
  - `prompts/brownfield-draft.md` — same.
  - `prompts/scout.md` — same.
  - `docs/README.md` — add the human-facing entry under the appropriate section.
- **Operator-visible behavior:**
  - Operators reading `docs/README.md` see a link to OpenSpec upstream documentation.
  - The agent's behavior on existing change implementations is unchanged in code, but agents drafting new spec content gain a documented reference they can consult.
- **Backward compatibility:** none affected. No config, no schema, no runtime behavior.
- **Dependencies:** none. Independent of every other queued change. Can land in any order.
- **Acceptance:** `cargo test` passes; `openspec validate a41-link-openspec-conventions --strict` passes. Tests:
  - A repo-grep test asserts that each of the eight prompts named above contains the substring `https://github.com/Fission-AI/OpenSpec` AND a topical hint (one of `GIVEN`, `WHEN`, `scenario`, `delta`, OR `Requirement`).
  - A repo-grep test asserts `docs/README.md` contains the substring `https://github.com/Fission-AI/OpenSpec`.
  - The repo-grep tests fail if any of the nine pointers is removed in a future change without an explicit replacement; the failure surfaces in CI before merge.
