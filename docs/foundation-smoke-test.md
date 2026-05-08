# Foundation Smoke Test

This document describes the manual end-to-end verification procedure for the
`orchestrator-foundation` change. Section 11 of `tasks.md` references this
file; tasks 11.2 and 11.3 are completed by running the procedure below
against real GitHub sandbox repositories.

The smoke tests confirm that a built `orchestrator` binary can:

1. Clone or fetch a sandbox repository into its workspace.
2. Detect a ready OpenSpec change in `openspec/changes/`.
3. Drive the configured executor against that change.
4. Commit the executor's diff on the agent branch.
5. Push the agent branch and open a Pull Request via the GitHub API.
6. Archive the change and clean up its `.in-progress` lock.
7. Honor SIGINT/SIGTERM with a timely shutdown.

The procedure assumes the operator has:

- A GitHub account with permission to create personal sandbox repositories.
- A fine-grained PAT with `Contents: read/write` and `Pull requests: write`
  scoped to the sandbox repos, exported as `GITHUB_TOKEN`.
- The `claude` CLI (or another supported executor) installed on `$PATH`.
- A clean checkout of this repository with `cargo build --release` already run.

## 11.1 Single-repo smoke test

### Setup

1. Create a fresh GitHub sandbox repository, e.g. `your-handle/orchestrator-smoke-1`.
   Initialize with a single `main` branch and at least one commit so the branch
   exists.

2. In a local clone of that sandbox, create one OpenSpec change directory:

   ```
   openspec/changes/add-greetings-file/proposal.md
   ```

   With contents like:

   ```markdown
   ## Why
   Smoke-test fixture: confirm the orchestrator can apply a trivial change.

   ## What Changes
   - Create a file named `GREETINGS` containing the text `hello world`.
   ```

   Add `tasks.md` in the same directory listing the work:

   ```markdown
   - [ ] Create a file named `GREETINGS` at the repo root containing the
     text `hello world` (no trailing newline).
   ```

   Commit and push these to `main`.

3. Write a `config.yaml` (do NOT commit this — it contains repository pointers
   and is local to your operator workspace):

   ```yaml
   repositories:
     - url: "git@github.com:your-handle/orchestrator-smoke-1.git"
       base_branch: main
       agent_branch: agent-q
       poll_interval_sec: 60

   executor:
     kind: claude_cli
     command: claude
     timeout_secs: 1800

   github:
     token_env: GITHUB_TOKEN
   ```

### Run

```bash
export GITHUB_TOKEN=ghp_yourpathere
RUST_LOG=info ./target/release/orchestrator run --config config.yaml
```

Wait until the orchestrator logs `opened PR` for the change, then send
`SIGINT` (Ctrl-C). The process should log `received SIGINT; shutting down`
followed by `shutdown complete` and exit within ~30 seconds.

### Pass criteria

- `gh pr list --repo your-handle/orchestrator-smoke-1 --head agent-q` shows
  exactly one open PR.
- `git log origin/agent-q -1 --pretty=full` (in a fresh clone of the sandbox)
  shows one commit ahead of `main` whose subject matches
  `add-greetings-file: <first non-empty line of the proposal's ## Why>`,
  truncated to 72 characters.
- The PR's diff contains exactly the new `GREETINGS` file.
- Inside `/tmp/workspaces/<derived-name>`, `openspec/changes/add-greetings-file`
  has been moved to `openspec/changes/archive/<UTC-date>-add-greetings-file/`.
- No `.in-progress` files remain anywhere under `openspec/changes/`.

## 11.2 Multi-repo smoke test

Repeat 11.1 with TWO sandbox repos in `config.yaml`, with deliberately
different polling intervals:

```yaml
repositories:
  - url: "git@github.com:your-handle/orchestrator-smoke-1.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 60

  - url: "git@github.com:your-handle/orchestrator-smoke-2.git"
    base_branch: main
    agent_branch: agent-q
    poll_interval_sec: 180
```

Run for ~5 minutes. Both sandboxes must produce a PR.

Send `SIGTERM` (`kill <pid>`). The orchestrator must:

- Log `received SIGTERM; shutting down`.
- Drain both polling tasks within 30 seconds.
- Exit zero.

### Pass criteria

- Both sandboxes contain exactly one PR each on `agent-q`.
- Neither workspace contains `.in-progress` files.
- The faster repo (60s interval) iterated more times than the slower repo
  (180s interval); confirm by counting `starting polling loop` /
  `polling pass produced no changes` log lines per repo URL.

## 11.3 Cleanup verification

After both smoke tests, confirm:

- Each sandbox's local workspace at `/tmp/workspaces/<derived-name>` shows the
  implemented change in `openspec/changes/archive/<UTC-date>-<change>/`, with
  the original directory gone from `openspec/changes/`.
- `find /tmp/workspaces -name .in-progress` returns nothing.
- `git status` inside each workspace is clean (modulo the agent branch's
  unmerged commits, which is expected — the orchestrator doesn't merge its
  own PRs).
