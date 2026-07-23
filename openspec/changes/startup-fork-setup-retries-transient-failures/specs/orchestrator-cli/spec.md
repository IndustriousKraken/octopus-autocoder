## MODIFIED Requirements

### Requirement: Startup verification of fork existence
When `github.fork_owner` is set, autocoder SHALL ensure each configured repository has a reachable fork at the derived URL before that repository's polling task begins normal work. Forks that are missing or unreachable SHALL be created automatically via `POST /repos/{upstream-owner}/{upstream-repo}/forks` using the PAT resolved for the upstream owner. On a 2xx creation response, autocoder SHALL parse the response body and compare the returned fork's identity (`full_name`) against the owner/name expected from the derived fork URL (case-insensitive). When the identities match — or the response body cannot be parsed for an identity — the daemon polls the fork URL via `git ls-remote` until it becomes reachable or until a 60-second timeout elapses. When the identities differ (the idempotent existing-fork case where the fork carries a different name — e.g. the upstream was renamed after forking, or fork creation collided with an existing repository name), autocoder SHALL record the failure immediately with a cause that names the actual fork returned, the expected fork, AND the remedy (rename the existing fork to the expected name), WITHOUT entering the reachability poll — the poll cannot succeed for a fork that exists under a different name.

A fork-setup failure SHALL be classified with the same transient/permanent classification the mid-iteration recovery path uses (per "Mid-iteration recovery failures classify transient vs. permanent; transient retries on next iteration", including its default-to-transient posture for unrecognized errors), and the handling SHALL branch on the class:

- **PERMANENT** — the fork-identity mismatch, an underivable fork URL, an unroutable PAT, and permanent-classified creation responses: the daemon SHALL record the failure (the upstream URL, the expected fork URL, AND the cause); **skip that repository** (no polling task is spawned for it) — the daemon never retries a permanent-classified failure on its own, so absent operator action the skip lasts the remainder of the process lifetime, and the repository rejoins the polling set only when the operator remediates and either restarts the daemon or runs `autocoder reload` on the daemon host (reload's repository reconciliation spawns a polling task for any configured repository that lacks one); AND emit a chatops alert through the standard outbound notification path that identifies the repository AND carries a brief remedy hint, where the remedy hint names the concrete recovery commands — restart the daemon or run `autocoder reload` on the daemon host (NOT a bare verb like `reload`).
- **TRANSIENT** — transport/DNS errors, transient-classified HTTP responses, AND the fork-created-but-not-reachable-within-60s timeout (GitHub populates forks asynchronously): the daemon SHALL spawn the repository's polling task in a **fork-pending** state instead of skipping it. A fork-pending task re-attempts the full fork setup at the start of each iteration AND does no other work for the repository until setup succeeds; each failed attempt logs a WARN naming the cause, AND a throttled chatops alert names the repository as fork-pending while attempts continue. On a successful attempt the task SHALL log an INFO recovery notice AND proceed as a normal polling task from that iteration on — no operator action required.

A fork-setup failure for one repository SHALL NEVER prevent the daemon from starting, from serving other repositories, OR from serving chatops. The daemon exits non-zero at startup only for non-per-repo fatal conditions (e.g. config-load failure) — NEVER for a per-repo fork-setup failure, even when every configured repository fails fork setup (it stays up so transient failures can self-heal AND an operator can remediate permanent ones).

#### Scenario: All forks already exist
- **WHEN** autocoder starts with `github.fork_owner` set AND every
  configured repository's derived fork URL resolves via
  `git ls-remote <fork-url> HEAD` on the first probe
- **THEN** no fork-creation API calls are issued
- **AND** all polling tasks are spawned and the daemon enters its
  normal polling state

#### Scenario: A fork is missing and creation succeeds
- **WHEN** autocoder starts with `github.fork_owner` set AND at least
  one configured repository's derived fork URL fails the initial
  `git ls-remote` probe
- **THEN** autocoder issues `POST /repos/<upstream-owner>/<upstream-repo>/forks`
  with header `Authorization: Bearer <token>` (token resolved by the
  existing per-owner routing) for each missing fork
- **AND** on a 2xx response whose returned fork identity matches the
  derived fork URL, autocoder polls the fork URL via `git ls-remote`
  every 2 seconds for up to 60 seconds
- **AND** when polling succeeds, the daemon proceeds to spawn polling
  tasks normally
- **AND** the daemon emits one info-level log line per created fork
  of the form `created fork <fork-url> from upstream <upstream-url>`

#### Scenario: A permanent-classified creation failure skips for the process lifetime
- **WHEN** autocoder attempts to create a missing fork AND the
  `POST /repos/{upstream-owner}/{upstream-repo}/forks` call returns a
  permanent-classified non-2xx status (e.g. a 403 missing-scope or 404)
- **THEN** that repository's failure is recorded with the upstream
  URL, the expected fork URL, and the HTTP status (plus a body snippet
  truncated to 200 chars)
- **AND** autocoder skips that repository (no polling task is spawned
  for it, and the daemon does not retry on its own — recovery is by
  operator restart or reload) AND emits a chatops alert that
  identifies the repository AND carries a remedy hint
- **AND** autocoder continues setting up the remaining repositories AND
  the daemon does NOT exit

#### Scenario: A transient-classified failure spawns a fork-pending task that self-heals
- **WHEN** a repository's fork setup fails with a transient-classified
  cause (a transport/DNS error or a transient-classified HTTP status)
- **THEN** the repository's polling task is spawned in the fork-pending
  state (it is NOT skipped for the process lifetime)
- **AND** each iteration re-attempts the full fork setup, logging a
  WARN per failed attempt, AND a throttled chatops alert names the
  repository as fork-pending
- **AND** when an attempt succeeds, the task logs an INFO recovery
  notice AND proceeds as a normal polling task with no operator action

#### Scenario: The reachability timeout is transient, not a lifetime skip
- **WHEN** the POST returns 2xx with a matching fork identity AND
  `git ls-remote <fork-url> HEAD` fails for 60 seconds of polling at
  2-second intervals
- **THEN** the failure is classified transient (GitHub populates forks
  asynchronously)
- **AND** the repository's polling task spawns fork-pending AND the
  next iteration's re-attempt probes the fork again, succeeding as
  soon as the fork is populated

#### Scenario: An unrecognized failure defaults to transient
- **WHEN** a fork-setup failure's cause matches neither the documented
  transient nor permanent patterns
- **THEN** it is treated as transient (the classification's
  default-to-transient posture), so an unfamiliar error retries
  rather than permanently sidelining the repository
- **AND** the per-attempt WARN makes the unfamiliar pattern visible in
  the journal

#### Scenario: A fork-pending repository does no other work until setup succeeds
- **WHEN** a repository's polling task is in the fork-pending state
- **THEN** its iterations perform ONLY the fork-setup re-attempt — no
  branch init, no lane walks, no audits, no executor — until an
  attempt succeeds

#### Scenario: An existing fork carries a different name than expected
- **WHEN** autocoder issues the fork-creation POST AND the 2xx
  response's fork identity (`full_name`) differs from the owner/name
  expected from the derived fork URL (e.g. the upstream was renamed
  after the fork was created, so the fork still carries the old name)
- **THEN** that repository's failure is recorded immediately with a
  cause that names the actual fork returned, the expected fork, AND
  the rename remedy (e.g. "GitHub returned existing fork
  `<actual-full-name>` but `<expected-full-name>` was expected; rename
  the fork to match, then restart or run `autocoder reload`")
- **AND** no `git ls-remote` reachability polling is performed for
  that repository (no 60-second wait)
- **AND** the failure is PERMANENT: the repository is skipped (no retry
  until an operator restart or reload) AND a chatops alert identifying
  it is emitted, AND the daemon proceeds to serve the other
  repositories without exiting

#### Scenario: A fork already exists when creation is attempted
- **WHEN** autocoder issues the fork-creation POST AND the upstream
  has already been forked to the destination user under the expected
  name
- **THEN** the GitHub API returns 2xx with the existing fork's
  metadata (idempotent behavior)
- **AND** the returned fork identity matches the derived fork URL, so
  autocoder treats this as success and proceeds with the reachability
  probe normally

#### Scenario: Creation response body cannot be parsed for a fork identity
- **WHEN** the fork-creation POST returns 2xx AND the response body
  does not yield a parseable fork identity
- **THEN** autocoder proceeds with the reachability probe exactly as
  for a matching identity — the `git ls-remote` poll remains the
  ground truth AND a malformed or unexpected response shape never
  fails a fork setup that would otherwise succeed

#### Scenario: Fork-setup alert names the concrete reload command
- **WHEN** a permanent-classified fork-setup failure alert is emitted
- **THEN** its remedy hint instructs the operator to restart the
  daemon or run `autocoder reload` on the daemon host (the alert never
  refers to a bare `reload` verb)

#### Scenario: One repository's fork failure does not take down the others
- **WHEN** autocoder starts with multiple repositories AND one
  repository's fork cannot be set up AND the other repositories' forks
  are reachable
- **THEN** the daemon spawns polling tasks for the reachable
  repositories AND enters normal polling
- **AND** chatops is served (the daemon does not exit)
- **AND** a permanent-classified failure leaves that repository absent
  from the active polling set until the operator remediates AND
  restarts or reloads, while a transient-classified failure leaves it
  fork-pending and self-healing

#### Scenario: Every repository's fork fails
- **WHEN** every configured repository fails fork setup
- **THEN** the daemon still starts AND stays up serving chatops, having
  emitted one chatops alert per failed repository
- **AND** it does NOT exit non-zero

### Requirement: Per-repository asynchronous polling loop — fork-pending exception

Canonical requirement: "autocoder SHALL implement the per-repository polling task referenced in `orchestrator-architecture/specs/orchestrator-cli/spec.md` as a sleep-then-iterate cycle that runs the architecture's single-pass workflow on every iteration."

A polling task in the fork-pending state SHALL run only the fork-setup re-attempt on each iteration, skipping the full single-pass workflow, until fork setup succeeds. The fork-setup re-attempt on each iteration SHALL re-run the full fork-setup sequence: probe the fork URL via `git ls-remote`, create via POST if missing, identity check, reachability poll. On a successful attempt the task SHALL log INFO and proceed as a normal polling task from that iteration on.

#### Scenario: Normal iteration when fork-pending
- **WHEN** a polling task is in the fork-pending state
- **THEN** its iteration runs only the fork-setup re-attempt (probe → create-if-missing → identity check → reachability) — no workspace init, no stale-lock cleanup, no dirty-workspace refusal, no branch recreation, no queue walk, no push, no PR creation
- **AND** on success, the task logs an INFO recovery notice and proceeds as a normal polling task from the next step onward
- **AND** on transient failure, it logs a WARN and sleeps until the next iteration
- **AND** on permanent failure (e.g. the upstream was renamed while pending), the task exits the polling set with a chatops alert naming the remedy hint, matching permanent-classified behavior for startup fork setup
