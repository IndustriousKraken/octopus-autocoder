## ADDED Requirements

### Requirement: Managed repos carry a committed OCTOPUS.md agent guide
A repository under management SHALL carry an `OCTOPUS.md` at the repository root — the conventional spot an agent guide occupies — that states the in-repo workflow protocols for any agent OR human working in the repo. Its audience is everyone who is NOT autocoder's own gated agents: a coding assistant or speccing agent run directly on the repo, AND human teammates. For autocoder's own agents the same rules are enforced by the verifier gates AND the session sandbox; OCTOPUS.md documents them but does not replace that enforcement.

OCTOPUS.md SHALL be **committed into the managed repository**, not merely present in the local workspace, so it is visible to readers who never check out autocoder's agent branch. This is the inverse of autocoder's per-workspace bookkeeping files, which are kept OUT of version control via `.git/info/exclude`.

OCTOPUS.md SHALL state, at minimum, the following protocols, reflecting current canon (it MAY link to the fuller OpenSpec documentation for readers with web access):

- **Issues protocol.** An issue is a correction (a fix to code that is already correctly specified) that carries NO spec delta. It takes ONE of two on-disk forms: a single file `issues/<slug>.md` (the default — a description and an optional `## Tasks` checklist) OR a directory `issues/<slug>/` (containing `issue.md` AND `tasks.md`, required when the unit must carry a separate artifact such as a quarantined public report body). Neither form contains a `specs/` directory.
- **OpenSpec change protocol.** A change lives in `openspec/changes/<slug>/` with a `proposal.md`, a `tasks.md`, an optional `design.md`, AND spec deltas at `specs/<capability>/spec.md` using `## ADDED`/`## MODIFIED`/`## REMOVED`/`## RENAMED Requirements` blocks. A `MODIFIED` block reproduces the canonical requirement's title EXACTLY AND retains every existing scenario (a dropped scenario silently deletes canon at archive). A change MUST pass `openspec validate --strict`.
- **Canon and archive ownership.** The canonical specs under `openspec/specs/` AND the archive under `openspec/changes/archive/` are autocoder-owned: a session writes only its own change/issue planning artifacts AND code; it never edits canon directly NOR runs `openspec archive`. Autocoder folds a change's deltas into canon at archive, after the change is merged.
- **The gate model.** A change passes through gatekeepers that fail closed: `[in]` (the change does not contradict itself), `[canon]` (the change does not contradict canon unless it explicitly modifies the contradicted requirement), `[rules]` (the change conforms to the global rules), AND `[out]` (the merged code implements the spec). An inability to run a gate is a non-passing outcome, never a pass.

OCTOPUS.md SHALL be discoverable via the `AGENTS.md` convention: the repository's `AGENTS.md` SHALL reference OCTOPUS.md, without clobbering any existing `AGENTS.md` content the repository already carries.

#### Scenario: A managed repo carries a committed OCTOPUS.md
- **WHEN** a repository is under management
- **THEN** an `OCTOPUS.md` exists at the repository root AND is committed into the repository (not merely present in the local workspace)
- **AND** it states the issues protocol, the OpenSpec change protocol, the canon/archive ownership rules, AND the gate model

#### Scenario: OCTOPUS.md states the canon/archive ownership rule
- **WHEN** an agent reads OCTOPUS.md before planning work
- **THEN** it is told that `openspec/specs/` AND the archive are autocoder-owned — a session writes only its own change/issue artifacts AND code, never edits canon directly, AND never runs `openspec archive` (autocoder folds the delta at archive, after merge)

#### Scenario: OCTOPUS.md describes the two issue forms and the no-spec-delta rule
- **WHEN** an agent reads OCTOPUS.md's issues section
- **THEN** it learns that an issue is either a single file `issues/<slug>.md` OR a directory `issues/<slug>/` (with `issue.md` AND `tasks.md`), carries no spec delta, AND never contains a `specs/` directory

#### Scenario: Discoverable via AGENTS.md without clobbering it
- **WHEN** a repository already has its own `AGENTS.md`
- **THEN** the `AGENTS.md` reference to OCTOPUS.md is present
- **AND** the repository's pre-existing `AGENTS.md` content is left intact

## MODIFIED Requirements

### Requirement: Default prompts are language- and project-neutral
The default prompts shipped under `prompts/` run against any managed repository, which may be written in any language with any build toolchain. They SHALL NOT instruct the agent to run a specific language's build, lint, format, or test command (e.g. `cargo clippy`, `cargo test`, `npm test`, `pytest`, `go test`). Instead they SHALL direct the agent to run the project's own build / lint / format / test tooling, detected from the repository's build configuration (e.g. `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`).

`openspec validate --strict` is the one tool every managed repository shares — every managed repo uses OpenSpec — AND MAY be named directly. Worked examples in prompts SHALL use language-neutral phrasing (e.g. "the project's test command") rather than a concrete toolchain invocation. Multi-language enumerations (e.g. a list of source-file extensions across many languages) are already neutral AND are permitted.

The default prompts SHALL each direct the agent to read `OCTOPUS.md` at the repository root when it is present, for the in-repo workflow protocols (the issues format, the OpenSpec change format, the canon/archive ownership rules, AND the gate model). The directive SHALL be a graceful no-op when `OCTOPUS.md` is absent, AND SHALL NOT depend on any particular default-prompt set: a prompt added later carries the same directive. This requirement does NOT mandate that the prompts stop restating the protocols they need inline; OCTOPUS.md is an additional in-repo reference, not a single source the prompts render from.

This is design intent for prompt content, verified by review AND the drift audit's semantic judgment — NOT by a unit test asserting prompt wording (per the requirement `Tests assert behavior or derivation, never message wording`). A negative "no prompt contains `cargo`" scanner would be both unenumerable across all languages AND the prohibited wording-assertion category; the drift audit makes the judgment instead.

A future change MAY add a per-repository tooling-config block that injects the exact lint/test/format commands into prompts via placeholders. This requirement establishes the language-neutral default in the meantime.

#### Scenario: Default prompts name no language-specific build tooling
- **WHEN** the default prompts under `prompts/` are reviewed against this requirement (by a human reviewer OR the drift audit)
- **THEN** none instructs the agent to run a specific language's build, lint, format, or test command (e.g. `cargo`, `npm`, `pytest`, `go test`)
- **AND** each prompt that asks the agent to lint or test directs it to the project's own tooling, detected from the repository's build configuration
- **AND** `openspec validate --strict` MAY still be named directly

#### Scenario: A language-specific command in a default prompt is a drift-audit finding
- **GIVEN** a default prompt instructs `cargo clippy --all-targets -- -D warnings` (a Rust-specific command)
- **WHEN** the drift audit reads this requirement against the prompt
- **THEN** the command is reported as a finding: the prompt assumes a specific toolchain that does not apply to all managed repositories
- **AND** the disposition is to replace it with language-neutral guidance, NOT to special-case the prompt per language

#### Scenario: Each default prompt points the agent to OCTOPUS.md when present
- **WHEN** the default prompts under `prompts/` are reviewed against this requirement (by a human reviewer OR the drift audit)
- **THEN** each directs the agent to read `OCTOPUS.md` at the repository root when present, for the in-repo workflow protocols
- **AND** the directive is a graceful no-op when `OCTOPUS.md` is absent, so a prompt run against a repo without one is unaffected
