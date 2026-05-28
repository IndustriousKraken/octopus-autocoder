## ADDED Requirements

### Requirement: `brownfield` chatops verb queues a brownfield-draft executor request
The chatops listener SHALL submit a `BrownfieldAction` (per the chatops-manager requirement) which the daemon's control-socket handler converts into an entry on the resolved repo's `pending_brownfield_requests: VecDeque<RequestId>` queue. The daemon SHALL persist a per-request state file `<workspace>/.state/brownfield_requests/<request_id>.json` containing the request's `repo_url`, `capability_name`, `guidance: Option<String>`, `channel`, `thread_ts`, AND `status` (`Pending` | `InProgress` | `Acted` | `Failed` | `Aborted`).

Each polling iteration SHALL, after processing pending proposal requests AND before the standard change-processing pass, drain at most one brownfield request from the queue.

#### Scenario: Queue stores requests in submission order
- **WHEN** the operator posts two brownfield requests in sequence (`brownfield repo a`, `brownfield repo b`)
- **THEN** `pending_brownfield_requests` contains both request_ids in submission order
- **AND** the polling iteration drains them one per iteration

#### Scenario: State file persists across daemon restart
- **WHEN** a `BrownfieldRequestState` file exists with `status: Pending` AND the daemon restarts
- **THEN** the daemon's startup reads the file AND re-queues the request
- **AND** processing resumes on the next iteration

#### Scenario: Late conflict aborts the request
- **WHEN** a brownfield request reaches the polling iteration AND `openspec/specs/<capability-name>/spec.md` exists at the current workspace HEAD (created by a merge between dispatch AND processing)
- **THEN** the iteration posts a thread reply `✗ brownfield: openspec/specs/<capability-name>/spec.md now exists (created since the request was queued). Aborting.`
- **AND** the state's `status` becomes `Aborted`
- **AND** no executor invocation occurs

### Requirement: Brownfield-draft executor mode produces a spec-only change PR
When the polling iteration processes a brownfield request, it SHALL invoke the executor with `WritePolicy::OpenSpecOnly` AND a sandbox profile permitting `Read`, `Glob`, `Grep`, AND `Bash` (read-only). The executor's prompt SHALL be assembled from:

1. The embedded default template at `prompts/brownfield-draft.md` (via `include_str!`), OR the template at `features.brownfield.prompt_path` when configured AND the file exists.
2. The operator's guidance (when non-empty), interpolated into a `## Operator guidance` section of the prompt.
3. The capability name, the workspace's `README.md` contents, the list of `docs/*.md` filenames, AND a code-symbol overview built via `cargo metadata` (for Rust workspaces) OR a ripgrep pass for top-level public items (other languages).

On executor `Completed`, the iteration SHALL verify the change directory `openspec/changes/brownfield-<capability-name>/` contains `proposal.md`, `tasks.md`, AND `specs/<capability-name>/spec.md`. The iteration SHALL ALSO verify `git status --porcelain` shows no modifications outside `openspec/`; any such modification triggers `git reset --hard HEAD; git clean -fd`, a WARN log naming the leaked paths, AND a state transition to `Failed`.

On verification success, the iteration SHALL create a spec branch (NOT a fixes branch — brownfield never modifies source code), push, AND open a PR. The PR body SHALL include the proposal's "Why" section. The iteration SHALL post `✅ Brownfield draft PR opened: <pr_url>` to the request's thread AND set the state's `status` to `Acted` with the PR URL recorded.

On executor `Err` OR missing artifacts, the iteration SHALL post `✗ Brownfield draft failed: <reason>` to the request's thread, log the full error to the daemon log, revert the workspace, AND set the state's `status` to `Failed`.

#### Scenario: Successful run produces a spec-only PR
- **WHEN** the executor returns `Completed` AND `openspec/changes/brownfield-<cap>/` contains all required artifacts AND no source-file modifications leaked
- **THEN** the daemon creates a spec branch `<configured-prefix>brownfield/<cap>`, pushes, AND opens a PR
- **AND** the PR body contains the proposal's "Why" section
- **AND** the state's `status` is `Acted` with the PR URL
- **AND** the thread receives `✅ Brownfield draft PR opened: <pr_url>`
- **AND** NO fixes branch OR fixes PR is created

#### Scenario: Sandbox leak triggers cleanup
- **WHEN** the executor returns `Completed` AND `git status --porcelain` shows modifications under `src/` (in addition to `openspec/`)
- **THEN** the iteration reverts the workspace via `git reset --hard HEAD; git clean -fd`
- **AND** a WARN log fires naming the leaked paths
- **AND** the state's `status` is `Failed`
- **AND** the thread reply names the sandbox violation

#### Scenario: Missing change-directory artifacts produce a clear failure
- **WHEN** the executor returns `Completed` BUT `openspec/changes/brownfield-<cap>/specs/<cap>/spec.md` is absent
- **THEN** the state's `status` is `Failed`
- **AND** the thread reply names the missing artifact
- **AND** the workspace is reverted

#### Scenario: Operator guidance reaches the prompt
- **WHEN** the operator's request includes guidance `focus on the cron-trigger lifecycle; skip telemetry hooks`
- **THEN** the executor invocation's prompt contains a `## Operator guidance` section with the verbatim guidance text
- **AND** the LLM's draft scopes its requirements accordingly

#### Scenario: Per-workspace prompt override applies
- **WHEN** `features.brownfield.prompt_path: ./prompts/brownfield-custom.md` AND the file exists in the workspace
- **THEN** the iteration loads the custom template AND uses it instead of the embedded default
- **AND** the loaded template combines with the operator's guidance + the gathered inputs into the executor's prompt

#### Scenario: Missing override file falls back to embedded
- **WHEN** `features.brownfield.prompt_path: ./prompts/brownfield-custom.md` is configured BUT the file does not exist
- **THEN** the iteration logs a WARN naming the missing path
- **AND** the iteration falls back to the embedded default template
- **AND** the request proceeds successfully

#### Scenario: PR participates in standard revision loop
- **WHEN** the brownfield PR is open AND an operator comments `@<bot> revise add a requirement covering retry semantics` on it
- **THEN** the existing PR-comment revision-loop mechanism handles the comment
- **AND** the next polling iteration revises the spec PR per the operator's text

### Requirement: `features.brownfield` config schema
The daemon's per-repo config schema SHALL accept an optional top-level `features` block containing a `brownfield` sub-block with:

- `enabled: bool` (default `true`) — when `false`, the dispatcher refuses the verb at parse time.
- `prompt_path: Option<String>` (default `None`) — operator-supplied path (relative to workspace root) to a custom brownfield-draft prompt template.

Both fields are optional; absent fields take their defaults. Invalid values (non-boolean `enabled`, non-string `prompt_path`) cause config-load to fail-fast with a clear error naming the offending field.

#### Scenario: Default config enables brownfield
- **WHEN** a workspace's config omits the `features.brownfield` block
- **THEN** `features.brownfield.enabled` resolves to `true`
- **AND** `features.brownfield.prompt_path` resolves to `None`

#### Scenario: Explicit disable refuses the verb
- **WHEN** a workspace's config sets `features.brownfield.enabled: false`
- **THEN** the dispatcher refuses `@<bot> brownfield ...` requests for that workspace per the chatops-manager requirement

#### Scenario: Explicit override path resolves
- **WHEN** a workspace's config sets `features.brownfield.prompt_path: "./prompts/brownfield-custom.md"`
- **THEN** the polling iteration loads the file at that workspace-relative path
- **AND** uses its contents as the brownfield-draft prompt template

#### Scenario: Invalid field type fails config load
- **WHEN** a workspace's config sets `features.brownfield.enabled: "yes"` (string instead of bool)
- **THEN** config-load fails with an error naming `features.brownfield.enabled` AND the expected type
