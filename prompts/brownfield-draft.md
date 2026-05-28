You are drafting a canonical OpenSpec capability spec for code that already exists in this workspace. The capability is named **{{capability_name}}**.

The operator MAY have provided guidance to scope your work. Follow it when present:

## Operator guidance

{{guidance}}

## Repository context

- Repo URL: `{{repo_url}}`
- Workspace `README.md`:

{{readme}}

- `docs/` files in the workspace:

{{docs_listing}}

- Code-symbol overview (top-level public items, build-system metadata where applicable):

{{symbols_overview}}

## Your job

Draft a new spec-only change at `openspec/changes/brownfield-{{capability_name}}/` that captures the **existing** behavior of the `{{capability_name}}` capability under canonical OpenSpec requirements. You SHALL NOT modify any source code; your sandbox is `WritePolicy::OpenSpecOnly` and a leak into source files will cause the run to fail. If any tool call appears to be modifying source, stop immediately and revert.

### Process

1. **Read the codebase to identify the capability's surface area.** Use Glob, Grep, AND Read to map the modules, public functions, configuration knobs, AND user-visible behaviors that constitute `{{capability_name}}`. Trace from entry points (CLI flags, HTTP routes, public APIs, chatops verbs, scheduled jobs — whatever applies) to the code that implements them.
2. **Read `README.md` AND any relevant `docs/*.md`** for existing user-facing description of `{{capability_name}}`. Where the docs disagree with the code, the **code wins** — the spec captures observable behavior, not aspirational behavior.
3. **Draft the change artifacts** at `openspec/changes/brownfield-{{capability_name}}/`:
   - `proposal.md` — `## Why` explains that this captures existing behavior under canonical specs (NO behavioral change). `## What Changes` enumerates the requirements being added. `## Impact` lists the single affected capability (`{{capability_name}}`) AND notes "no code changes."
   - `tasks.md` — review-oriented tasks: validate each requirement against the named code modules, confirm scenarios match observable behavior, run any existing test suite for the capability. Tasks are NOT implementation tasks; they are checklist items for the operator to verify the spec matches reality.
   - `specs/{{capability_name}}/spec.md` — an `## ADDED Requirements` block containing one `### Requirement: ...` per coherent slice of the capability's behavior, with `#### Scenario:` blocks grounded in what the code actually does.

### Output rules

- **One coherent slice of behavior per requirement.** Do NOT lump unrelated behaviors into a single requirement; do NOT split one logical behavior across requirements.
- **`SHALL` for normative statements.** Reserve commentary for the requirement body, not for the requirement title.
- **Scenarios describe observable behavior, not implementation detail.** A scenario reads as `**WHEN** X **THEN** Y` and the operator can verify it without reading the source.
- **No speculation.** Do NOT propose features that aren't in the code. Do NOT propose new behavior. If the code does it, the spec describes it; if it doesn't, the spec is silent.
- **No implementation prose in requirement bodies.** File paths, function signatures, AND module names belong in the proposal's `## Impact` section if they're useful for reviewers — NOT inside `### Requirement:` bodies.
- **Capability boundary unclear?** If you cannot reconcile `{{capability_name}}` with one cohesive slice of the codebase (the name is too broad, too narrow, OR doesn't match any single concern in the code), draft a best-effort spec covering what you DID identify AND surface the ambiguity in the proposal's `## Why` section. The operator iterates via `@<bot> revise` on the resulting PR.

### `tasks.md` shape

A review-oriented `tasks.md` looks like:

```
## 1. Validate the new spec against the code

- [ ] 1.1 For each requirement in `specs/{{capability_name}}/spec.md`, locate the corresponding code in <module/file>. Confirm the requirement's scenarios are observable today.
- [ ] 1.2 Run the existing test suite covering `{{capability_name}}` (e.g. `cargo test {{capability_name}}::` OR whatever the project's convention is). Confirm tests pass before AND after archiving the change.
- [ ] 1.3 If any scenario does NOT match observable behavior, revise the spec (NOT the code) — the goal is descriptive fidelity to what already exists.

## 2. Review the proposal's framing

- [ ] 2.1 Confirm `## Why` accurately reflects "captures existing behavior; no behavioral change."
- [ ] 2.2 Confirm `## Impact` lists exactly one affected capability AND notes "no code changes."
```

Tailor the wording to the specific capability, but keep the **review-oriented** spirit: tasks are checklist items for the human reviewer, NOT new code or new tests for you to write.

### Anti-noise rules (do NOT do these)

- Do NOT add `## REMOVED Requirements` OR `## MODIFIED Requirements` blocks — this is a brownfield draft; everything is `## ADDED`.
- Do NOT propose code changes anywhere in the change directory.
- Do NOT include design.md unless the capability's behavior is genuinely complex (most capabilities don't need one). If you include one, it documents how the existing code accomplishes the behavior, not how it should be redesigned.
- Do NOT cite line numbers or specific commits — those rot. Cite module names AND function names where useful, in the proposal only.

Once all three files are written, you are done. The polling iteration will verify the artifacts exist, run a sandbox-leak check, AND open a spec-only PR.
