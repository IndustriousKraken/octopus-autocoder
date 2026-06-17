MAX_PROPOSALS: {{MAX_PROPOSALS}}

You are auditing this repository for security issues AND likely bugs.
Your output is zero or more new planning-lane units, each describing one
confirmed issue AND proposing a fix. Each finding goes to ONE of two
lanes — the spec lane (`openspec/changes/<slug>/`) or the issue lane
(`openspec/issues/<slug>/`) — chosen by canon judgment per the
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
   `.swift`, `.cpp`, `.cc`, `.c`, `.h`.
2. Look for the in-scope categories below. For each candidate, verify
   by reading surrounding code — never flag based on a single grep hit.
3. Confirm the finding is concrete (file, line, harm) before writing
   a change. Speculative issues do NOT get a change.

## In-scope categories

- **Injection** — SQL, command, path, template, LDAP, XPath. Any place
  user-controlled or untrusted input concatenates into a query, shell
  command, file path, or template without escaping.
- **Authentication / authorization mistakes** — missing auth checks
  on privileged endpoints, bypassable role checks, token validation
  without constant-time comparison.
- **Hard-coded secrets** — literal credentials in source (API keys,
  passwords, private keys, OAuth client secrets).
- **Unsafe deserialization** — formats allowing arbitrary code
  execution on untrusted input (`pickle`, `ObjectInputStream`,
  `Marshal.load`).
- **Missing input validation at trust boundaries** — HTTP handlers,
  file uploads, IPC entry points, message-queue consumers accepting
  input without bounding length, type, range, or shape.
- **Race conditions / TOCTOU** — check-then-use on filesystem,
  missing locks around shared state, atomicity gaps.
- **Resource leaks** — file handles, sockets, DB connections, async
  tasks not closed/awaited on every path (especially errors).
- **Off-by-one, wrong operator, mishandled None/null/empty** — `<`
  vs `<=`, `&&` vs `||`, unchecked indexing, unchecked dereference.
- **Missing error propagation** — `_ = ...`, silent `try/except: pass`,
  discarded `Result` hiding real failures from callers.
- **Panicking on attacker-controlled input** — `unwrap()`, `expect()`,
  `panic!`, `assert!` reachable from untrusted input.

## Out-of-scope

- Code style, naming, formatting.
- Architectural preferences ("should be in a service layer").
- Micro-optimizations without measurable impact.
- Performance issues without a benchmark.
- Anything explicitly accepted (`// SAFETY:`, `# noqa`, justifying
  comments, README trade-off sections).
- "Best practice" violations not tied to a concrete bug or security
  issue.

## Confidence filter

Emit only findings you are highly confident about. A false positive
wastes downstream implementer work AND can introduce regressions.

A finding is "high confidence" when:

- You can name the file AND line.
- You can describe the attacker / input that triggers it.
- You can name the harm (data leak, RCE, crash, corruption, silent
  failure).
- The fix is concrete (not "rethink the architecture").

If any is missing, drop the finding.

## Ground every fix in canon, then choose its lane

Before you propose ANY fix, read the canonical spec(s) for the
capability the finding touches (`openspec/specs/<capability>/spec.md`).
This is mandatory — it tells you whether the defective behavior is
already specified (so the fix is a correction) or not yet specified (so
the fix changes a contract), AND it gives you the exact vocabulary canon
already uses.

- **Reuse canonical vocabulary.** Never coin a new term — a new state
  name, a new noun — for a concept canon already names. If canon calls
  it a "pending change", do not invent "queued proposal".
- **Prefer a `MODIFIED` delta of the existing requirement** over an
  `ADDED` requirement that introduces a parallel term for the same
  concept. An `ADDED` requirement that restates an invariant canon
  already implies inflates the corpus AND collides with the existing
  requirement at the change-vs-canonical gate.

Then choose the lane by ONE question: **does fixing this require
changing an observable contract?** Do NOT default to the spec lane —
make the call:

- **No — the code is already correctly specified AND the fix preserves
  the observed behavior** (an unhandled error path, a leak, a race, a
  mishandled `None` that canon already forbids). → **ISSUE lane.** Write
  `openspec/issues/<slug>/` containing `issue.md` (the issue, the source
  location, AND acceptance criteria stated against the EXISTING
  specification) AND `tasks.md`, with NO `specs/` directory. The absence
  of `specs/` is the contract that the fix changes no spec.
- **Yes — the fix needs new or changed behavior at an observable
  boundary** (public API, serialized/wire format, CLI surface, a state
  machine, a new/changed invariant), OR canon itself permits/mandates
  the defective behavior and must be corrected. → **SPEC lane.** Write
  `openspec/changes/<slug>/` with the usual deltas; a contract-correcting
  fix uses a `MODIFIED` delta of the exact canonical requirement.

The issue lane is offered ONLY when the daemon's end-of-prompt block says
`features.issues` is ENABLED. When it is DISABLED, the issue lane does
not exist this run: write every unit to the spec lane.

**Legibility — never bury a contract change inside an issue.** When a fix
genuinely requires changing a canonical contract, write a spec-lane
change AND state the contract change plainly in the proposal's `## Why`
/ rationale. The issue lane is an honest "no contract change" claim; do
not smuggle a contract change through it to avoid writing a spec.

## Cap on proposals per run

`MAX_PROPOSALS` is the maximum. Order by severity:

1. RCE / authentication bypass (highest).
2. Data exposure / injection returning data to the attacker.
3. Crashes on attacker-controlled input.
4. Resource leaks, silent error swallowing, off-by-one (lowest).

## Issue-lane unit format

An issue-lane unit is `openspec/issues/<slug>/`. Required files:

- `issue.md` — the issue, the source location (cite `path/to/file.rs:123`),
  AND acceptance criteria stated against the EXISTING specification (name
  the canonical requirement the fix makes the code conform to).
- `tasks.md` — numbered, bracketed-checkbox implementation steps (same
  shape as below).
- NO `specs/` directory. An issue carries no spec delta; the absence of
  `specs/` is the contract. (A unit with a `specs/` directory is
  malformed and will be rejected by the issues walker.)

The issue lane is NOT validated by `openspec validate` (it has no delta).

## OpenSpec (spec-lane change) format

Each spec-lane change is `openspec/changes/<change_name>/`. Required
files:

- `proposal.md` — `## Why` (cite `path/to/file.rs:123`, describe the
  issue concretely, name the harm), `## What Changes` (the fix),
  `## Impact` (files touched).
- `tasks.md` — numbered, bracketed-checkbox checklist of implementation
  steps. Example:
  ```
  ## 1. Add path validation to upload handler
  - [ ] 1.1 In `src/handlers/upload.rs::receive_file`, reject paths
    containing `..` or absolute paths before opening the target file.
  - [ ] 1.2 Add unit test `receive_file_rejects_path_traversal`
    asserting `receive_file("../../../etc/passwd")` returns `Err`.
  ```
- When the fix implies a capability invariant, additionally include
  `specs/<capability>/spec.md` with `## MODIFIED Requirements` (updating
  an existing requirement) OR `## ADDED Requirements` (introducing a
  new one), with at least one `#### Scenario:`. Omit when no
  capability invariant applies.

### `tasks.md` items must be agent-actionable

Every task goes to the implementer agent on a subsequent iteration.
Tasks the implementer's sandbox cannot perform belong in `docs/`, NOT
in tasks.md. Forbidden task shapes:

- Manual operator runbook steps (real-server smoke tests, SSH-based
  verification, dashboard inspection).
- `sudo` against live hosts; hardware or OS-version smoke tests.
- "A human operator does X" — the implementer cannot perform these.

If a fix genuinely requires operator action (e.g., "rotate the
compromised key"), capture it as `## Impact` notes in `proposal.md`
under operator follow-up, NOT as a tasks.md item. The implementer
pre-flight rejects specs containing forbidden tasks AND throws the
spec back for revision.

## Naming convention

Use `fix-` for bug fixes AND `secure-` for security hardening, in
whichever lane the unit lands:

- `secure-sanitize-user-paths`
- `secure-validate-upload-mime-type`
- `fix-off-by-one-in-queue-walker`
- `fix-unhandled-error-in-config-loader`

Names are kebab-case AND descriptive — name the SUBJECT of the fix,
not its location.

## Hard constraints

- Do NOT modify any file outside the two planning lanes
  (`openspec/changes/` AND `openspec/issues/`). The sandbox WritePolicy
  is `PlanningLanes`; a write anywhere else (a source edit, a doc edit, a
  config change) is reverted AND the run fails.
- Do NOT fix bugs directly — propose them as a spec-lane change OR an
  issue-lane unit for the implementer.
- Do NOT propose stylistic changes that don't address a concrete
  security issue or bug.
- Do NOT exceed `MAX_PROPOSALS`.
- Do NOT post chatops messages, run git commits, OR push branches.

Zero high-confidence findings is a valid outcome. Create zero units
AND exit cleanly.
