## Why

The default prompts shipped under `prompts/` run against every managed repository, but several instruct the agent to run Rust- and this-project-specific tooling. An agent reviewing a Python project reported "there's no Clippy in Python" because `prompts/implementer.md` directs `cargo clippy --all-targets -- -D warnings` and `cargo test` unconditionally. Concrete offenders today:

- `prompts/implementer.md` — `cargo clippy --all-targets -- -D warnings` in the acceptance checklist AND in the `final_answer` worked example.
- `prompts/brownfield-draft.md` — `cargo test {{capability_name}}::` as the primary form (hedged with "OR whatever the project's test command is," but it leads with cargo).

`openspec validate --strict` is fine to keep — every managed repository uses OpenSpec. The leak is language-specific build/lint/test/format commands, which simply do not exist in non-Rust repos.

## What Changes

**New requirement: default prompts are language- and project-neutral (project-documentation).** Default prompts SHALL NOT name a specific language's build/lint/format/test command; they direct the agent to run the project's own tooling, detected from the repository's build configuration (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, etc.). `openspec validate --strict` MAY be named directly. This is design intent for prompt content, verified by review AND the drift audit's semantic judgment — NOT a wording test (per `Tests assert behavior or derivation, never message wording`); a negative "no prompt contains `cargo`" scanner is both unenumerable across languages AND the prohibited wording-assertion category. The drift audit flags a language-specific command in any prompt as a finding against this requirement.

**The offending prompts are rewritten language-neutrally.** `prompts/implementer.md`'s `cargo clippy`/`cargo test` references become "the project's linter / test suite (detected from the build config)"; `prompts/brownfield-draft.md` leads with the neutral form. The full `prompts/` set is swept for the same pattern.

**Future scope (noted, not built here):** a per-repository tooling-config block that injects the exact lint/test/format commands into prompts via placeholders. This change establishes the language-neutral default; the per-repo override is a separate change if detect-and-run proves insufficient.

## Impact

- **Affected specs:**
  - `project-documentation` — ADDED `Default prompts are language- and project-neutral` (design intent; drift-audited; no content test).
- **Affected code (prompt content only; no Rust changes):**
  - `prompts/implementer.md` — replace the two `cargo clippy --all-targets -- -D warnings` references (acceptance checklist + worked example) and any `cargo test` with language-neutral guidance; keep `openspec validate --strict`.
  - `prompts/brownfield-draft.md` — lead the test-command guidance with the neutral "the project's test command (detected from the build config)" form rather than `cargo test`.
  - Sweep the remaining `prompts/` files for language-specific build/lint/format/test commands and neutralize them. Multi-language file-extension lists (e.g. `missing-tests-audit.md`'s `.rs`, `.py`, `.go`, …) are already inclusive AND are left as-is. Illustrative `path/to/file.rs` example paths are cosmetic; neutralize where it is a one-word change, otherwise leave for the drift audit.
- **Operator-visible behavior:** PR comments and audit findings against non-Rust repos stop instructing or referencing tools that do not exist for that language.
- **Acceptance:** `cargo test` and `cargo clippy --all-targets -- -D warnings` pass for the autocoder crate (no Rust changed; the gate is the project's own, unaffected); `openspec validate a51-language-neutral-default-prompts --strict` passes. No prompt-content test is added.
- **Dependencies:** none hard. SHOULD land after **a45** so the sweep also neutralizes a45's revision-prompt worked example (which adds a `cargo clippy` line); if a51 lands first, that line becomes a drift-audit finding against this requirement and is cleaned later. a52 also edits `prompts/implementer-revision.md` but its additions are already language-neutral.
