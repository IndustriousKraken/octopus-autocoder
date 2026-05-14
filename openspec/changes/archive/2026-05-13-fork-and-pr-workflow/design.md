## Context

autocoder's current PR-creation pipeline assumes the agent branch lives
on the same repository as the base branch:

```
clone upstream → branch off base → commit → push to upstream/agent-q → PR (head=agent-q, base=main)
                                            ^^^^^^^^^^^^^^^^^^^^^^^^^   ^^^^^^^^^^^^^^^^^^^^^^^^^^^
                                            needs write on upstream     same-repo PR
```

This requires the GitHub identity behind autocoder to hold push access
on every upstream repository. For org repos with real downstream
consumers, "push access" is a powerful credential — even with branch
protection covering `main`/`dev`, the daemon can still force-push to or
delete any unprotected branch.

The standard open-source contribution flow already solves this:

```
clone upstream → branch off base → commit → push to fork/agent-q → cross-repo PR (head=fork-owner:agent-q, base=main, posted to upstream)
                                            ^^^^^^^^^^^^^^^^^^^^   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                                            needs write on fork    cross-repo PR with namespaced head
                                            (fork is owned by
                                            the machine user)
```

The machine user only needs **read** access to upstream — exactly what
an external contributor has. A compromised autocoder host can push to
its own forks and open PRs against upstream, but cannot touch the
upstream repository directly.

GitHub's REST API supports this natively. The PR-creation endpoint
accepts a namespaced `head` parameter: `<fork-owner>:<branch>` instead
of plain `<branch>`. The API call still goes to the upstream repo's
`/pulls` endpoint.

## Goals / Non-Goals

**Goals:**

- An operator can deploy autocoder with read-only access on upstream
  repositories. Configuring `fork_owner: <handle>` and pre-creating
  the forks under that handle is sufficient.
- The daemon never holds, in any of its credentials, the ability to
  write to an upstream branch. This includes SSH (via team
  membership), PATs (scoped to `Pull requests: read & write` only),
  and any future credential types.
- Backward compatibility: omitting `fork_owner` preserves the current
  direct-push behavior exactly. Existing deployments continue to work
  without a single config edit.
- The change is implemented inside the existing call sites
  (`workspace::ensure_initialized`, `git::push_force_with_lease`,
  `github::create_pull_request`); no new module or new pipeline.

**Non-Goals:**

- **Auto-creation of forks.** Operators are expected to fork each
  configured repository manually before pointing autocoder at it.
  Auto-creation via `POST /repos/{owner}/{repo}/forks` is async,
  needs polling, and adds error surface; deferred to a follow-on
  change if manual setup turns out painful.
- **Cross-fork PR pivoting.** A single upstream repository has
  exactly one fork (under `fork_owner`). Operators wanting per-repo
  fork-owner overrides are out of scope; one machine user per
  deployment is the design center.
- **Per-repo opt-out.** `fork_owner` is global: either all repos
  use fork-PR mode, or none do. Mixed-mode deployments are not
  supported.
- **Fork housekeeping.** Stale agent branches on the fork (from old
  iterations) are not pruned automatically. Force-push to
  `agent_branch` reuses the same branch name, so sprawl is bounded
  to one branch at a time. Old commit SHAs persist on the fork but
  are unreachable.
- **Fork-side branch protection.** The fork is owned by the machine
  user and is treated as a write-target. Operators may add branch
  protection on the fork if they wish; autocoder doesn't depend on
  or interact with it.

## Decisions

### Config schema

```yaml
github:
  fork_owner: rbeverly-autocoder       # NEW (optional, opt-in)
  owner_tokens:
    UpstreamOrg:
      value: "github_pat_xxx"
```

Single new field: `github.fork_owner: Option<String>`. When `Some`,
fork-PR mode is active for every configured repository. When `None`
(or absent), direct-push mode is preserved exactly.

### Fork URL derivation

The fork lives at `git@github.com:<fork-owner>/<repo>.git` (or HTTPS
equivalent based on the upstream URL's scheme). The repo name is
preserved by GitHub on fork; the only segment that changes is the
owner.

```rust
fn derive_fork_url(upstream_url: &str, fork_owner: &str) -> Result<String> {
    let (_upstream_owner, repo) = github::parse_repo_url(upstream_url)?;
    // Preserve the scheme of the upstream URL.
    if upstream_url.starts_with("git@github.com:") {
        Ok(format!("git@github.com:{fork_owner}/{repo}.git"))
    } else if upstream_url.starts_with("https://github.com/") {
        Ok(format!("https://github.com/{fork_owner}/{repo}.git"))
    } else {
        Err(anyhow!("unrecognized upstream URL scheme for fork derivation: {upstream_url}"))
    }
}
```

### Workspace initialization changes

`workspace::ensure_initialized(workspace, upstream_url, fork_url)` —
gains an optional `fork_url` parameter. The signature becomes:

```rust
pub fn ensure_initialized(
    workspace: &Path,
    upstream_url: &str,
    fork_url: Option<&str>,
) -> Result<()>
```

When `fork_url` is `Some`, after the existing clone/fetch logic,
the manager runs (idempotently):

```bash
# Pseudo-shell:
if ! git remote | grep -qx fork; then
    git remote add fork "$fork_url"
else
    # Update the URL if it has drifted (e.g. fork_owner changed in config).
    git remote set-url fork "$fork_url"
fi
```

The `origin` remote always points at upstream (unchanged).

### Branch push selection

`git::push_force_with_lease(workspace, branch, remote)` — gains a
third parameter naming the remote:

```rust
pub fn push_force_with_lease(workspace: &Path, branch: &str, remote: &str) -> Result<()>
```

The polling loop passes `"fork"` when `fork_owner` is configured,
`"origin"` otherwise.

### PR creation

`github::create_pull_request` already accepts `head: &str`. The
polling loop formats the head parameter:

```rust
let head = match github_cfg.fork_owner.as_deref() {
    Some(owner) => format!("{owner}:{agent_branch}"),
    None => agent_branch.to_string(),
};
```

The endpoint is unchanged: `POST /repos/{upstream-owner}/{upstream-repo}/pulls`.
GitHub interprets the `head: "fork-owner:branch"` form as a cross-repo
PR. The response shape is identical to a same-repo PR.

### Startup validation

When `fork_owner` is configured, `cli::run::execute` SHALL verify
that each configured upstream repository has a corresponding fork
at the expected URL **before spawning any polling task**. The check
is a `git ls-remote <fork-url> HEAD` invocation: succeeds → fork
exists and the machine user can read it; fails → fork is missing
or unreachable, error names both the upstream URL and the expected
fork URL.

This piggybacks on the existing `validate_github_token_routes`
pattern (added in the `multi-token-github-credentials` change) and
aggregates per-repo failures into a single startup error.

### Rewind

`rewind --hard` currently runs `git push origin --delete <agent-branch>`
to clean up the remote agent branch. In fork-PR mode, this targets
the `fork` remote instead. The branch deletion semantics are
unchanged — it's still a `--delete` push, just to a different remote.

## Risks / Trade-offs

- **Risk:** Operator misconfigures `fork_owner` (typo, wrong
  machine-user handle).
  - **Mitigation:** Startup validation catches this — every
    repository's fork URL is probed via `git ls-remote` before
    any polling task spawns. Typos surface at boot, naming both
    the upstream URL and the wrong fork URL.

- **Risk:** Fork drifts from upstream over time; `agent_branch`
  on the fork carries stale base commits.
  - **Mitigation:** This is prevented by the existing per-pass
    branch-init logic. Each pass runs `git fetch origin && git
    checkout <base> && git pull --ff-only origin <base> && git
    checkout -B <agent>`, so the agent branch always starts from
    upstream's latest base, regardless of the fork's state. The
    fork is purely a push-target.

- **Risk:** Auto-fork is missing for a newly-added repository in
  `config.yaml`; the operator forgets the manual fork step.
  - **Mitigation:** Startup validation catches this with a clear
    error pointing at the missing fork. A future change can add
    auto-creation via the GitHub API.

- **Risk:** Branch protection rules on the upstream that block
  the machine user from opening PRs (some orgs gate PR creation
  on team membership or CLA).
  - **Mitigation:** Out of scope. If an org's branch protection
    requires the PR author to be a specific kind of user, the
    operator's responsibility is to grant the machine user that
    status. Standard GitHub PR-creation permissions are what we
    rely on.

- **Risk:** The machine user's fork drifts from the upstream's
  default branch over time; new operators looking at the fork
  web UI see an outdated `main`.
  - **Mitigation:** Cosmetic only; autocoder never reads from the
    fork's default branch. Operators who care can run
    `gh repo sync` periodically, or ignore.

- **Risk:** Two `--repo` selectors on `rewind` resolve to the
  same fork (because two upstreams have the same repo name and
  the fork's repo name preserves upstream's).
  - **Mitigation:** Existing workspace collision detection
    already prevents two upstreams from sharing a workspace
    path. If they don't share a workspace, their forks under the
    same `fork_owner` would land at the same fork URL — which
    GitHub would reject (you can't fork two repos to the same
    name). So this is naturally prevented at GitHub's layer.
    Document the limitation in the README.
