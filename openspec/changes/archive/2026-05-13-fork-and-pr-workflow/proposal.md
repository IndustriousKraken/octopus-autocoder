## Why

Today autocoder operates as a write-capable collaborator on each configured
upstream repository: it pushes the agent branch directly to upstream and
opens a same-repo PR. This model requires that the GitHub identity behind
autocoder hold push access to every managed repo. Even with branch
protection on `main`/`dev`, the daemon retains the ability to:

- Force-push to any unprotected branch (including feature branches that
  may trigger CI/CD or be watched by downstream pipelines)
- Delete branches it has push access to
- Push to any branch that branch-protection rules don't explicitly cover

For solo personal repositories this is acceptable. For organizations with
real downstream consumers — and operators who consider branch protection
a defense-in-depth measure rather than a primary failsafe — it is not.
The principle the operator wants to enforce is "autocoder cannot directly
modify the upstream repository under any circumstances; it can only
propose changes via the standard PR-from-fork mechanism that external
contributors use."

This change adds a **fork-and-PR workflow mode** modeled on the standard
open-source contribution flow. When enabled:

- autocoder's GitHub identity is granted **read-only** access to the
  upstream repository (collaborator with `Read` permission or
  equivalent team membership).
- A separate fork of each upstream repository exists under a
  machine-user account that autocoder owns. The machine user has full
  control over its own forks; no special access on upstream.
- autocoder pushes the agent branch to the **fork**, not upstream.
- The PR is opened as a cross-repository PR from `<fork-owner>:<branch>`
  to `<upstream-owner>:<base-branch>`.

The security delta is meaningful: a compromise of the autocoder host
collapses to "the attacker can push to the machine user's forks and
open PRs against upstream" — which is exactly the threat model the
existing human-review process is designed to handle. The attacker
cannot write to any branch on the real repositories.

## What Changes

- Add a new optional `github.fork_owner: String` config field naming
  the GitHub account that owns the forks (typically a dedicated machine
  user). Absent → existing direct-push behavior preserved.

- Workspace initialization (`workspace::ensure_initialized`):
  - If `fork_owner` is set, after the upstream `clone`/`fetch`,
    idempotently register a second remote named `fork` pointing at
    `git@github.com:<fork-owner>/<repo>.git` (or the HTTPS equivalent
    derived from the upstream URL's scheme).

- Branch push (`git::push_force_with_lease`):
  - Picks the remote based on `fork_owner` presence: `origin` when
    absent (current behavior), `fork` when present.

- PR creation (`github::create_pull_request`):
  - When `fork_owner` is set, the `head` parameter becomes
    `<fork-owner>:<agent-branch>`. The API endpoint remains
    `POST /repos/<upstream-owner>/<upstream-repo>/pulls` — cross-repo
    PRs are posted to the upstream's pulls endpoint.

- Rewind subcommand: deletes the agent branch from the `fork` remote
  when `fork_owner` is set (instead of from `origin`).

- Startup validation: when `fork_owner` is set, verify each
  configured repository has a corresponding fork at the expected URL.
  A missing fork produces a startup error naming both the upstream and
  expected fork URLs, before any polling task spawns.

- README updates: fork-and-PR mode becomes the documented recommended
  deployment pattern. Direct-push is kept as an alternative for solo
  personal-repo setups.

- Config example: a commented `fork_owner` line is added under the
  `github:` block with a brief explanation.

## Capabilities

### Modified Capabilities

- `workspace-manager`: idempotent workspace initialization grows
  optional second-remote setup when `fork_owner` is configured.
- `git-workflow-manager`: branch push selects the remote based on
  config; PR creation uses cross-repo `head` format when
  `fork_owner` is set.
- `orchestrator-cli`: new `github.fork_owner` config field; startup
  validation checks fork existence; `rewind` subcommand targets the
  fork remote for remote-branch deletion.

## Impact

Operators with org repos and real downstream consumers can deploy
autocoder under a read-only collaborator/team-member identity with
full confidence that the daemon cannot directly push to the upstream
repository. Branch protection ceases to be load-bearing for
autocoder's blast radius — it remains useful as defense against
other contributors but is no longer the only thing preventing
autocoder from writing to protected branches.

Existing direct-push deployments are unaffected: omitting the
`fork_owner` field preserves current behavior exactly. Migration is
opt-in and incremental — each operator decides when to set up a
machine user, fork their repos, and add `fork_owner: <handle>` to
their config.

The queued `chatops-progress-notifications` and
`experimental-chatops-providers` changes are unaffected: both
operate on the existing polling-loop hot path without caring about
which remote receives the push or how the PR `head` is formatted.
