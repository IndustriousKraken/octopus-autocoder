## ADDED Requirements

### Requirement: Local rule-corpus path expands a leading ~ or $HOME
When `executor.global_rules.corpus` is configured as a LOCAL DIRECTORY PATH (a value that is NOT detected as a git URL), autocoder SHALL expand a leading `~/` or `$HOME/` in that value to the operator's home directory BEFORE the directory is resolved and existence-checked. A bare `~` or `$HOME` that is the entire value SHALL expand to the home directory itself. This makes a configured corpus such as `~/.config/autocoder/global-rules` — the value the check-only installer's `install-verify.sh` writes — resolve to `<home>/.config/autocoder/global-rules` rather than a literal `~` directory, so the default check-only config resolves as written.

The expansion is leading-only AND conservative: only a value beginning with `~/` or `$HOME/`, or equal to a bare `~` / `$HOME`, is expanded; a tilde elsewhere in the value, the `~user` form, and any already-absolute path are left untouched. When the home directory cannot be determined, the value SHALL be left unexpanded so the existing `path does not exist` error still surfaces (no new error kind is introduced). The exists AND is-directory checks SHALL run against the expanded path, so their diagnostics report the expanded location.

A git-URL corpus value SHALL be UNAFFECTED: a value detected as a git URL takes the clone/reuse branch and is NOT subject to home expansion. This requirement adds the expansion guarantee for local paths only; it does not change which values are treated as git URLs, nor the corpus-resolvability contract that an enabled `[rules]` gate requires a resolvable corpus.

#### Scenario: A ~/ corpus path expands to the home directory
- **WHEN** `executor.global_rules.corpus` is the local path `~/.config/autocoder/global-rules` AND the operator's home directory contains that subdirectory
- **THEN** autocoder expands the leading `~/` to the home directory and resolves the corpus to `<home>/.config/autocoder/global-rules`
- **AND** the directory-exists check passes against the expanded path
- **AND** no `path does not exist` error is raised for a literal `~` directory

#### Scenario: A $HOME/ corpus path expands the same way
- **WHEN** `executor.global_rules.corpus` is the local path `$HOME/.config/autocoder/global-rules` AND that subdirectory exists under the operator's home directory
- **THEN** autocoder expands the leading `$HOME/` to the home directory and resolves to the same `<home>/.config/autocoder/global-rules` path
- **AND** the resolved corpus passes the exists AND is-directory checks

#### Scenario: An absolute local path is unchanged
- **WHEN** `executor.global_rules.corpus` is an already-absolute local directory path with no leading `~/` or `$HOME/`
- **THEN** expansion is a no-op AND the value is resolved and existence-checked exactly as before

#### Scenario: A git-URL corpus value is unaffected
- **WHEN** `executor.global_rules.corpus` is a value detected as a git URL
- **THEN** autocoder takes the clone/reuse branch and does NOT apply home expansion
- **AND** the resolved corpus is the local clone, exactly as before this change
