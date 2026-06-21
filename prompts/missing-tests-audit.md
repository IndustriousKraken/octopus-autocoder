If `OCTOPUS.md` exists at the repository root, read it before you start: it
states this repo's in-repo workflow protocols (the issues format, the OpenSpec
change format, the canon/archive ownership rules, and the gate model). When
`OCTOPUS.md` is absent, skip this with no further action.

MAX_PROPOSALS: {{MAX_PROPOSALS}}

You are auditing test coverage for this repository. Your output is
zero or more new planning-lane units, each describing a meaningful
coverage gap AND proposing tests to fill it. Each gap goes to ONE of two
lanes — the spec lane (`openspec/changes/<slug>/`) or the issue lane
(`issues/<slug>/`) — chosen by canon judgment per the
"Choosing the output lane" section below. The daemon appends a block to
the END of this prompt naming which lanes are available this run AND the
exact paths; obey it.

OpenSpec format reference: https://github.com/Fission-AI/OpenSpec/tree/main/docs
(`concepts.md` for scenario syntax `GIVEN`/`WHEN`/`THEN`, delta blocks
`ADDED`/`MODIFIED`/`REMOVED`/`RENAMED`, AND requirement-header rules).
Consult on `openspec validate --strict` failures.

## What to do

1. Survey the source tree. Identify source files via extensions:
   `.rs`, `.py`, `.cs`, `.go`, `.js`, `.ts`, `.rb`, `.java`, `.kt`,
   `.swift`, `.cpp`, `.cc`, `.c`, `.h`. Use `Glob` to enumerate; use
   `Grep` AND `Read` to inspect.
2. For each meaningful function, identify whether it has tests AND
   whether those tests exercise its error/edge paths.
3. Focus on gaps with behavioral consequences:
   - `Error`/`Result` paths with no test (happy path covered, failure
     branches not).
   - Branches without assertions (test runs the code but never
     verifies output).
   - Obvious edge cases from the signature: boundary values,
     `None`/`null`/empty inputs, off-by-one conditions, zero-length
     collections, integer overflow.

## What NOT to flag

Suppress trivial gaps:
- Getters and setters with no logic.
- Single-line constructors.
- `Default` impls.
- `From`/`Into` conversions with no behavior beyond field copying.
- Code in clearly experimental modules (`// EXPERIMENTAL`, files
  under `experimental/`).

Do NOT propose changes to test code that already works:
- Do NOT propose deleting existing tests.
- Do NOT propose modifying existing tests unless factually broken
  (does not compile, or runs but never asserts).
- When in doubt, leave the existing test alone AND propose a NEW test.

## Ground every gap in canon, then choose its lane

Before you propose tests for ANY gap, read the canonical spec(s) for the
capability the gap touches (`openspec/specs/<capability>/spec.md`). This
is mandatory — it tells you whether the behavior the missing test would
cover is already specified (so the test just asserts existing canon) or
implies a new/changed invariant (a contract change), AND it gives you the
exact vocabulary canon already uses.

- **Reuse canonical vocabulary.** Never coin a new term for a concept
  canon already names.
- **Prefer a `MODIFIED` delta of the existing requirement** (adding a
  `#### Scenario:` to it) over an `ADDED` requirement that introduces a
  parallel term for the same concept.

Then choose the lane by ONE question: **does closing this gap require
asserting a NEW or CHANGED capability invariant (a contract change)?** Do
NOT default to the spec lane — make the call:

- **No — the code is already correctly specified AND the missing test
  just pins observed behavior canon already implies.** → **ISSUE lane.**
  Write `issues/<slug>/` containing `issue.md` (the gap, the
  source location, AND acceptance stated against the EXISTING
  specification) AND `tasks.md` (the test functions to add), with NO
  `specs/` directory.
- **Yes — closing the gap asserts a new or changed invariant** (a
  contract the canon does not yet state). → **SPEC lane.** Write
  `openspec/changes/<slug>/` with the `specs/<capability>/` delta plus
  `proposal.md` AND `tasks.md`.

The issue lane is offered ONLY when the daemon's end-of-prompt block says
`features.issues` is ENABLED. When it is DISABLED, the issue lane does
not exist this run: write every unit to the spec lane.

**Legibility — never bury a contract change inside an issue.** When
closing a gap genuinely requires a new or changed invariant, write a
spec-lane change AND state the contract change plainly in the proposal's
`## Why` / rationale. The issue lane is an honest "no contract change"
claim; do not smuggle a contract change through it.

## Cap on proposals per run

`MAX_PROPOSALS` is the maximum number of change directories per
invocation. Order by priority:

1. Missing tests on error paths (highest).
2. Untested branches.
3. Obvious edge cases (lowest).

## Issue-lane unit format

An issue-lane unit is `issues/<slug>/`. Required files:

- `issue.md` — the coverage gap, the source location, AND acceptance
  criteria stated against the EXISTING specification (name the canonical
  requirement the new tests assert).
- `tasks.md` — numbered, bracketed-checkbox steps, each a specific test
  function to add (same shape as below).
- NO `specs/` directory. An issue carries no spec delta; the absence of
  `specs/` is the contract. (A unit with a `specs/` directory is
  malformed and will be rejected by the issues walker.)

The issue lane is NOT validated by `openspec validate` (it has no delta).

## OpenSpec (spec-lane change) format

Each spec-lane change is `openspec/changes/<change_name>/`. Required
files:

- `proposal.md` — `## Why` (names the coverage gap concretely:
  functions, paths), `## What Changes` (names the new tests),
  `## Impact` (names the files the tests will land in).
- `tasks.md` — numbered, bracketed-checkbox checklist. Each item is
  a specific test function to add. Example:
  ```
  ## 1. Add error-path tests for parse_config
  - [ ] 1.1 `parse_config_errors_on_missing_required_field` —
    asserts `parse_config(input_with_missing_field)` returns
    `Err(ConfigError::MissingField("name"))`.
  - [ ] 1.2 `parse_config_errors_on_negative_port` —
    asserts `parse_config(toml_with_port_eq_minus_one)` returns
    `Err(ConfigError::InvalidPort)`.
  ```
- When the gap implies a capability invariant (maps to an existing
  requirement under `openspec/specs/<capability>/spec.md`),
  additionally include `specs/<capability>/spec.md` with a
  `## MODIFIED Requirements` block adding new `#### Scenario:`
  entries. Otherwise this file is optional.

### `tasks.md` items must be agent-actionable

Every task goes to the implementer agent on a subsequent iteration.
Tasks the implementer's sandbox cannot perform belong in `docs/`, NOT
in tasks.md. Forbidden task shapes:

- Manual operator runbook steps (real-server smoke tests, SSH-based
  verification, dashboard inspection).
- `sudo` against live hosts; hardware or OS-version smoke tests.
- "A human operator runs X" — the implementer cannot perform these.

If a coverage gap can only be filled by a manual procedure (e.g.,
"verify the load balancer rotates correctly under live traffic"), it
is OUT OF SCOPE for this audit. Skip it. The implementer pre-flight
rejects specs containing forbidden tasks AND throws the spec back for
revision.

## Naming convention

Prefix every unit with `tests-`, in whichever lane it lands. Names are
kebab-case AND descriptive — name the SUBJECT of the missing tests, not
their location.

- `tests-error-paths-in-queue-engine`
- `tests-edge-cases-in-busy-marker-recovery`
- `tests-boundary-values-in-rate-limiter`

## Hard constraints

- Do NOT modify any file outside the two planning lanes
  (`openspec/changes/` AND `issues/`). The sandbox WritePolicy
  is `PlanningLanes`; a write anywhere else fails the run.
- Do NOT propose deleting tests.
- Do NOT propose modifying existing tests unless factually broken.
- Do NOT exceed `MAX_PROPOSALS` units.
- Do NOT post chatops messages, run git commits, OR push branches.
  The audit framework commits validated changes after your run
  finishes.

Zero meaningful gaps after a good-faith inspection is a valid
outcome. Create zero units AND exit cleanly.
