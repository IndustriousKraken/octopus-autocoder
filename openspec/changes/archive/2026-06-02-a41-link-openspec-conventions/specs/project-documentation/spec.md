# project-documentation — delta for a41-link-openspec-conventions

## ADDED Requirements

### Requirement: OpenSpec upstream-docs pointer is regression-tested across the spec-drafting prompt set AND `docs/README.md`
The repository SHALL include a regression test asserting that nine files — eight agent-facing prompts AND `docs/README.md` — each contain a pointer to OpenSpec's upstream documentation. The pointer's purpose is to give both agents AND human contributors a canonical reference for scenario syntax (`GIVEN`/`WHEN`/`THEN`), delta format (`ADDED`/`MODIFIED`/`REMOVED`/`RENAMED`), AND requirement-header rules without authoring a parallel convention document.

The covered set is:

- `prompts/implementer.md`
- `prompts/implementer-revision.md`
- `prompts/chat-request-triage.md`
- `prompts/audit-triage.md`
- `prompts/missing-tests-audit.md`
- `prompts/security-bug-audit.md`
- `prompts/brownfield-draft.md`
- `prompts/scout.md`
- `docs/README.md`

The regression test reads each file via `std::fs::read_to_string` AND verifies the contents contain BOTH (a) the literal substring `https://github.com/Fission-AI/OpenSpec`, AND (b) at least one of the topical hints `GIVEN`, `WHEN`, `scenario`, `delta`, OR `Requirement`. The two-part check ensures the link is present AND surrounded by enough context to give the agent (or human) the format vocabulary that motivates the link.

The test SHALL produce a single combined failure listing (NOT first-failure-only). Each entry in the failure message SHALL name the file path AND which check failed (URL substring missing, OR topical hint missing). Combined reporting lets a contributor editing several files at once see every offender in one run.

The test SHALL be deterministic — no network, no clock, no environment mutation. File reads are the only I/O.

When a future change introduces a new spec-drafting prompt, OR removes one from the set, OR introduces a project-local convention document that consolidates project-specific deviations (e.g., `openspec/AGENTS.md`), the change SHALL update both this requirement's covered set AND the regression test's file list in lockstep.

#### Scenario: Regression test passes against the current repo state
- **GIVEN** the repository is in its post-merge state for `a41-link-openspec-conventions`
- **WHEN** the regression test runs
- **THEN** every file in the covered set contains the substring `https://github.com/Fission-AI/OpenSpec`
- **AND** every file in the covered set contains at least one of the topical hints (`GIVEN`, `WHEN`, `scenario`, `delta`, `Requirement`)
- **AND** the test passes with no diagnostic output

#### Scenario: Removing the URL from a covered file fails the test
- **GIVEN** a hypothetical future change removes the `https://github.com/Fission-AI/OpenSpec` substring from `prompts/implementer.md` without updating this requirement OR the regression test
- **WHEN** the regression test runs in CI for that change
- **THEN** the test fails with a diagnostic naming `prompts/implementer.md: missing required substring 'https://github.com/Fission-AI/OpenSpec'`
- **AND** the failure surfaces before the change can merge

#### Scenario: Removing the topical hint from a covered file fails the test
- **GIVEN** a hypothetical future change keeps the URL but strips out all five topical hints from `prompts/audit-triage.md`
- **WHEN** the regression test runs
- **THEN** the test fails with a diagnostic naming `prompts/audit-triage.md: missing topical hint (one of GIVEN, WHEN, scenario, delta, Requirement)`

#### Scenario: Multiple offenders are reported in one run
- **GIVEN** a hypothetical future change removes the URL from THREE covered files
- **WHEN** the regression test runs
- **THEN** the test fails with a single combined diagnostic naming ALL THREE files AND the failed check for each
- **AND** the contributor can fix all three without re-running the test repeatedly

#### Scenario: Path resolution works regardless of test-invocation directory
- **GIVEN** the test is invoked via `cargo test` from the repo root OR from the `autocoder/` crate directory
- **WHEN** the test resolves file paths via `CARGO_MANIFEST_DIR` AND its parent
- **THEN** the test reads the same nine files in both invocations
- **AND** the test passes identically in both
