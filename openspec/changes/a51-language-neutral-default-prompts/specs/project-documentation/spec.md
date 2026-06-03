# project-documentation — delta for a51-language-neutral-default-prompts

## ADDED Requirements

### Requirement: Default prompts are language- and project-neutral
The default prompts shipped under `prompts/` run against any managed repository, which may be written in any language with any build toolchain. They SHALL NOT instruct the agent to run a specific language's build, lint, format, or test command (e.g. `cargo clippy`, `cargo test`, `npm test`, `pytest`, `go test`). Instead they SHALL direct the agent to run the project's own build / lint / format / test tooling, detected from the repository's build configuration (e.g. `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`).

`openspec validate --strict` is the one tool every managed repository shares — every managed repo uses OpenSpec — AND MAY be named directly. Worked examples in prompts SHALL use language-neutral phrasing (e.g. "the project's test command") rather than a concrete toolchain invocation. Multi-language enumerations (e.g. a list of source-file extensions across many languages) are already neutral AND are permitted.

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
