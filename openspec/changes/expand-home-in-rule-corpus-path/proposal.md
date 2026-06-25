# Expand a leading ~ or $HOME in a local rule-corpus path

## Why

`executor.global_rules.corpus` accepts either a git URL or a local directory
path. When the value is a local path, `resolve_corpus`
(`autocoder/src/preflight/global_rules.rs:194`) takes the non-git-URL branch and
does `PathBuf::from(corpus)` with NO tilde or home expansion, then immediately
checks `path.exists()`. A value like `corpus: ~/.config/autocoder/global-rules`
is therefore treated as a literal directory named `~` under the current working
directory; it does not exist, and the check fails with
`executor.global_rules.corpus path does not exist: ~/.config/autocoder/global-rules`.

This is not a hypothetical typo: the check-only install's `install-verify.sh`
writes exactly `corpus: ~/.config/autocoder/global-rules` into the minimal config
it drops on a spec-authoring machine. So the default configuration produced by the
official installer fails the corpus-resolvability requirement the moment the
`[rules]` gate is enabled — `verify` cannot run and (server-side) the daemon's
fail-fast startup validation rejects the config. Operators must hand-edit the path
to an absolute one, defeating the point of a drop-in config.

The fix is small and local to the path branch: before the directory-exists check,
expand a leading `~/` or `$HOME/` to the operator's home directory. A git-URL
corpus value is unaffected, and an already-absolute path is unchanged.

## What Changes

- In `resolve_corpus`, the LOCAL-PATH branch (the `else` arm, before the
  `path.exists()` check) SHALL expand a leading `~/` or `$HOME/` in the corpus
  value to the operator's home directory, then resolve and existence-check that
  expanded path. The git-URL branch is unchanged: a value detected as a git URL
  skips expansion entirely.
- A bare `~` or `$HOME` (the whole value) expands to the home directory; a `~user`
  form or a tilde not at the start of the value is left untouched (only a leading
  `~/` or `$HOME/`, or the bare home token, is expanded). When the home directory
  cannot be determined, the value is left as-is so the existing
  path-does-not-exist error still surfaces.

## Impact

- Affected specs: `orchestrator-cli` (the `[rules]` gate / corpus config — one
  ADDED requirement for local-path home expansion; it adds a guarantee without
  restating the corpus-resolvability contract).
- Affected code: `autocoder/src/preflight/global_rules.rs` (`resolve_corpus`, the
  local-path branch only). Callers `autocoder/src/cli/run.rs:449` and
  `autocoder/src/cli/verify.rs:342` are unchanged — they pass the configured value
  through as before.
- Makes the check-only install's default `corpus: ~/.config/autocoder/global-rules`
  resolve as written, on both the `verify` accelerator path and the server-side
  daemon-startup validation.
- A docs note records that a local corpus path may use `~/` or `$HOME/`.
