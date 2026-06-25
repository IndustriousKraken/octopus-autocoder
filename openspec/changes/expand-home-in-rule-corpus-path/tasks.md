# Tasks

## 1. Home expansion in resolve_corpus

- [ ] 1.1 In `autocoder/src/preflight/global_rules.rs` `resolve_corpus`, add a leading-home expansion step in the LOCAL-PATH branch (the `else` arm), applied to the trimmed corpus value BEFORE constructing the `PathBuf` and BEFORE the `path.exists()` check. Expand a leading `~/` or `$HOME/` (and a bare `~` or `$HOME` that is the whole value) to the operator's home directory; concatenate the remainder onto the resolved home dir.
- [ ] 1.2 Determine the home directory from the environment (e.g. the `HOME` env var / a `home_dir`-style helper). When the home directory cannot be determined, leave the value unexpanded so the existing `path does not exist` error still surfaces (do not fail with a new, unrelated error).
- [ ] 1.3 Leave non-leading tildes, the `~user` form, and any value not starting with `~/` / `$HOME/` (nor equal to a bare `~` / `$HOME`) untouched. Run existence and is-directory checks against the EXPANDED path so the error messages report the expanded location.
- [ ] 1.4 Do NOT touch the git-URL branch: a value for which `is_git_url` is true skips expansion entirely. Callers `autocoder/src/cli/run.rs` and `autocoder/src/cli/verify.rs` need no change.

## 2. Tests

- [ ] 2.1 A corpus value of `~/<subdir>` resolves to `<home>/<subdir>`: with `HOME` pointed at a temp dir containing the subdir, `resolve_corpus` returns the expanded absolute path (and passes the exists/is-dir checks).
- [ ] 2.2 A corpus value of `$HOME/<subdir>` resolves the same way to `<home>/<subdir>`.
- [ ] 2.3 An absolute local path (e.g. the temp dir itself) is returned unchanged — expansion is a no-op for a value with no leading home token.
- [ ] 2.4 A git-URL corpus value is unaffected by expansion: it still takes the git-URL branch (clone/reuse), and a value that merely contains a tilde elsewhere is not mangled.
- [ ] 2.5 (Optional edge) When the home directory cannot be determined, a `~/...` value falls through to the existing `path does not exist` error rather than a new error kind.

## 3. Docs

- [ ] 3.1 Note in the relevant config/installer docs (where `executor.global_rules.corpus` is described, alongside `install-verify.sh`) that a LOCAL corpus path may begin with `~/` or `$HOME/` and is expanded to the operator's home directory; a git-URL value is used as-is.
