## Why

The agentic CLI strategies write per-run, server-specific config files into the workspace **root** that the wrapped CLI auto-discovers from the project directory: claude's `.mcp.json`, opencode's `opencode.json` (provider/base-URL/model) plus its `.opencode/` project scratch, and agy's `mcp_config.json`. Unlike the claude settings file (which the daemon hides under `.git/`), these cannot move out of the working tree — the CLI only finds them in the cwd. The executor's `git add -A` then stages whatever is present, so these per-run configs get committed into the PR. A code reviewer correctly flagged a committed `opencode.json`: it's not a secret (keys are written as `{env:}` references, never raw), but a server-specific, per-run config has no business in the repo. The source already acknowledged the gap (`opencode.json` "is not git-excluded").

## What Changes

- `git::add_all` adds the known per-run CLI artifacts to the workspace's `.git/info/exclude` before every `git add -A`, so they are never staged. The artifact list (`agentic_run::WORKSPACE_CLI_ARTIFACT_EXCLUDES`) is built from the strategies' own filename constants, so it can't drift: `.mcp.json`, `opencode.json`, `.opencode/`, `mcp_config.json`.
- `.git/info/exclude` is **local** (never committed — no `.gitignore` change appears in the repo) AND affects only **UNTRACKED** files, so a repository that legitimately tracks one of these is unaffected; only autocoder's generated copy is skipped. Applying it in `add_all` covers every staging path (executor pass, revisions, chatops commits) at one chokepoint.

## Impact

- **Affected specs:** `workspace-manager` — ADD `Per-run CLI config artifacts are excluded from commits` (extends the a16 principle — daemon bookkeeping stays out of the managed tree — to the CLIs' auto-discovered configs).
- **Affected code:** `agentic_run.rs` (`WORKSPACE_CLI_ARTIFACT_EXCLUDES`); `git.rs` (`ensure_local_excludes`, called from `add_all`). Three tests.
- **Operator-visible:** per-run CLI configs stop appearing in PRs. No behavior change for a repo that already tracks such a file (a tracked file stays committed).
- **Non-goals:** does NOT remove the files from the workspace (the CLI needs them at run time); does NOT touch the repo's `.gitignore`; does NOT change the `{env:}`-reference key handling (that already prevents secret leakage).
- **Acceptance:** `cargo test` (the exclude tests + full suite) + `openspec validate exclude-cli-artifacts-from-commits --strict`.
