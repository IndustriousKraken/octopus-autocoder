# Implementation tasks

## 1. Neutralize the known toolchain-command leaks

- [x] 1.1 `prompts/implementer.md` — replace `cargo clippy --all-targets -- -D warnings` (acceptance checklist, ~line 40) with language-neutral guidance: run the project's linter / formatter / test suite, detected from the repository's build configuration (`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, etc.). Keep `openspec validate --strict` as-is.
- [x] 1.2 `prompts/implementer.md` — in the `final_answer` worked example (~line 54), replace the `cargo clippy --all-targets -- -D warnings: clean` line with a neutral placeholder (e.g. "project linter: clean") so the example does not model a Rust-only command.
- [x] 1.3 `prompts/brownfield-draft.md` (~line 97) — lead the test-command guidance with the neutral form ("the project's test command, detected from the build config") rather than `cargo test {{capability_name}}::`; keep a parenthetical example only if it is clearly marked as one language among many.

## 2. Sweep the remaining prompts

- [x] 2.1 Review every file under `prompts/` for instructions to run a specific language's build/lint/format/test command. Replace each with language-neutral guidance per the requirement. If a45 has merged, this sweep includes neutralizing a45's revision-prompt worked example (`cargo clippy` line); a52's revision-prompt additions are already neutral.
- [x] 2.2 Leave multi-language file-extension lists as-is (e.g. `missing-tests-audit.md` / `security-bug-audit.md`'s `.rs`, `.py`, `.cs`, `.go`, … enumerations) — they are already inclusive. Illustrative `path/to/file.rs` example paths are cosmetic; neutralize where it is a trivial change, otherwise leave them for the drift audit.
- [x] 2.3 Keep `openspec validate --strict` everywhere it appears — every managed repository uses OpenSpec.

## 3. Spec delta

- [x] 3.1 `specs/project-documentation/spec.md` — ADD `Default prompts are language- and project-neutral`.

## 4. Acceptance gate

- [x] 4.1 `cargo test` passes for the autocoder crate (no Rust code changed; confirm no regression).
- [x] 4.2 `cargo clippy --all-targets -- -D warnings` is clean (the project's own dev gate, unaffected).
- [x] 4.3 `openspec validate a51-language-neutral-default-prompts --strict` passes.
- [x] 4.4 No prompt-content test is added; prompt language-neutrality is governed by review AND the drift audit (per `Tests assert behavior or derivation, never message wording`).
