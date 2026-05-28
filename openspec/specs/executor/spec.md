# executor Specification

## Purpose
TBD - created by archiving change orchestrator-architecture. Update Purpose after archive.
## Requirements
### Requirement: Backend-agnostic execution contract
The orchestrator SHALL invoke implementations through a trait-shaped abstraction that takes a workspace path and an OpenSpec change name and returns an outcome enum. The architecture-level spec does NOT name a concrete backend; concrete implementations (CLI wrappers, MCP-connected agents, future native loops) are introduced by separate implementation changes.

#### Scenario: Successful implementation
- **WHEN** the orchestrator calls `Executor::run(workspace, change_name)` with a valid workspace path and an unarchived change name
- **AND** the underlying backend reports successful completion of the implementation
- **THEN** the call returns `Ok(ExecutorOutcome::Completed)`
- **AND** the workspace working tree contains modifications attributable to the executor, verifiable via `git status --porcelain` returning a non-empty result inside the workspace

#### Scenario: Agent requires clarification
- **WHEN** the underlying backend signals ambiguity through any backend-specific mechanism (tool call, exit code, structured output, etc.)
- **THEN** the call returns `Ok(ExecutorOutcome::AskUser { question, resume_handle })` where `question` is a non-empty human-readable string and `resume_handle` is a value implementing `serde::Serialize` and `serde::Deserialize` so it can be persisted to `.question.json` and restored after a daemon restart
- **AND** no commits are produced on the agent branch as a side effect of the halted implementation
- **AND** the orchestrator (NOT the executor) is responsible for writing `.question.json` and posting the question to ChatOps

#### Scenario: Backend failure
- **WHEN** the underlying backend terminates abnormally (non-zero exit, crash, malformed output, network error, or an enclosing timeout fires)
- **THEN** the call returns `Ok(ExecutorOutcome::Failed { reason })` with a non-empty `reason` string OR `Err(_)` for unrecoverable infrastructure errors that prevent the executor from determining outcome
- **AND** the orchestrator unlocks the change (removes `.in-progress`) and does NOT archive it

### Requirement: Resume after ask-user
The executor SHALL support resuming a previously halted implementation when a human answer becomes available.

#### Scenario: Resuming with an answer
- **WHEN** the orchestrator calls `Executor::resume(resume_handle, answer)` with a `resume_handle` previously returned from `run` and a non-empty `answer` string
- **THEN** the call returns one of `Ok(ExecutorOutcome::Completed)`, `Ok(ExecutorOutcome::AskUser { ... })`, `Ok(ExecutorOutcome::Failed { ... })`, or `Err(_)`, with the same observable side-effect contracts as `run`
- **AND** the orchestrator MUST consume (delete or mark answered) the prior `.question.json` before invoking `resume`, so the executor cannot observe a stale escalation

#### Scenario: Resume after daemon restart
- **WHEN** the orchestrator restarts and finds a `.question.json` file alongside a corresponding `.answer.json` in `<workspace>/openspec/changes/<change>/`
- **THEN** the orchestrator deserializes the stored `resume_handle` from `.question.json` and calls `Executor::resume(handle, answer)` to continue execution
- **AND** the executor backend MUST tolerate a `resume_handle` that was serialized by a prior process invocation

### Requirement: CLI-wrapping executor backend (`claude_cli`)
autocoder SHALL provide a concrete `Executor` implementation that wraps
an external command-line agent tool as a child process. The backend is
selected via `executor.kind: claude_cli` in the configuration. Every
spawn SHALL include the sandbox flags described under "Tool-use
sandbox is applied at every spawn".

#### Scenario: ClaudeCliExecutor instantiation
- **WHEN** autocoder initializes AND `executor.kind` is `claude_cli`
- **THEN** autocoder instantiates a `ClaudeCliExecutor` configured
  from `executor.command` (default `claude`), `executor.timeout_secs`
  (default 1800), and a resolved `ExecutorSandboxConfig` (operator
  value or per-field default)
- **AND** the executor is wrapped in `Arc<dyn Executor>` and shared
  across all spawned polling tasks

#### Scenario: Outcome mapping from CLI exit code
- **WHEN** `Executor::run(workspace, change)` is called
- **THEN** the executor generates the per-iteration sandbox settings
  file in a temp dir, then spawns the configured command as a tokio
  child process inside the workspace with the sandbox flags and
  the prompt on stdin
- **AND** on child exit code 0, the call returns
  `Ok(ExecutorOutcome::Completed)` (the executor does NOT inspect
  the workspace for diff)
- **AND** on non-zero child exit, the call returns
  `Ok(ExecutorOutcome::Failed { reason })` where `reason` contains
  the first 200 characters of captured stderr
- **AND** if the configured `executor.timeout_secs` elapses, the
  child process is killed and the call returns
  `Ok(ExecutorOutcome::Failed { reason: "timeout" })`
- **AND** the temp settings file is deleted after the child exits

#### Scenario: Resume not supported in this phase
- **WHEN** `Executor::resume(handle, answer)` is called on the
  foundation `ClaudeCliExecutor` (prior to the
  `chatops-escalation` change)
- **THEN** the call returns `Err(_)` whose text indicates resume
  is not supported until the `chatops-escalation` change retrofits
  real resume semantics
- **AND** no child process is spawned and no workspace state is
  modified

(Note: in the in-tree implementation today, `resume` is wired
through `chatops-escalation` already. This scenario reflects the
historical foundation-phase contract preserved for spec
continuity. The active `resume` path uses the same sandbox
generation as `run`, per the "Resume applies the same sandbox"
scenario above.)

### Requirement: Executor output persistence and visibility
The `ClaudeCliExecutor` SHALL persist every subprocess invocation's prompt, captured stdout, and captured stderr to a per-change log file outside the workspace, and SHALL emit a WARN-level diagnostic tail when an exit-0 run produced no working-tree changes. Additionally, `build_prompt` SHALL log a WARN naming the reason whenever it falls back to raw-markdown concatenation. The executor SHALL record the spawned child's PID to a sidecar file alongside the busy marker so stuck-state recovery can target the right process group.

#### Scenario: Persistent log file written on every run
- **WHEN** `ClaudeCliExecutor::run` completes a subprocess invocation
  (any outcome: success, non-zero, or timeout)
- **THEN** the prompt sent to the subprocess, the captured stdout, and
  the captured stderr are written to
  `<system-temp>/autocoder/logs/<workspace-basename>/<change>.log`
  where `<workspace-basename>` is the last path component of the
  workspace and `<change>` is the change name
- **AND** the file format is plain text consisting of a
  `=== PROMPT (<p> bytes) ===` header followed by the verbatim
  prompt, a `=== STDOUT (<n> bytes) ===` header followed by the
  verbatim stdout, and a `=== STDERR (<m> bytes) ===` header
  followed by the verbatim stderr
- **AND** any prior contents of that file are overwritten (the file
  represents the most recent run for that change)
- **AND** the parent directories are created on demand
- **AND** errors writing the log file are logged at WARN but do NOT
  fail the executor outcome (logging is best-effort)

#### Scenario: Inline tail logged on suspicious empty-workspace exit
- **WHEN** the subprocess exits 0 AND `git status --porcelain` is
  empty AND no AskUser marker (layer-1) was written AND no
  layer-2 clarification phrase was matched
- **THEN** the executor logs a single WARN-level message naming the
  change and including the trailing ~2KB of stdout and trailing
  ~2KB of stderr (whichever is shorter), so the operator can read
  the agent's apparent reasoning directly from `journalctl` without
  opening the per-change log file
- **AND** the message also includes the per-change log-file path so
  the operator can find the full output if the inline tail is
  truncated mid-thought

#### Scenario: build_prompt logs WARN on each fallback path
- **WHEN** `build_prompt` cannot use `openspec instructions apply`
  output for any reason
- **THEN** the executor logs a WARN naming the change and a
  structured `reason` field whose value is exactly one of:
  `openspec_not_found` (the `openspec` binary could not be spawned,
  typically because it is not on autocoder's PATH),
  `openspec_exited_nonzero` (the binary spawned but returned a
  non-zero exit status), or `openspec_empty_stdout` (the binary
  exited 0 but produced no stdout)
- **AND** in the `openspec_exited_nonzero` case the log also
  includes the exit code and a tail of stderr (up to 200 chars) to
  speed diagnosis
- **AND** `build_prompt` then proceeds with raw-markdown
  concatenation as before, returning a non-empty prompt or an Err
  if no change material exists

#### Scenario: Spawned child runs in its own process group
- **WHEN** `run_subprocess` spawns the wrapped CLI as a child
  process
- **THEN** the child is launched as the leader of a new process
  group via `pre_exec` calling `setsid()` (Unix), so the per-repo
  busy marker can record the child's PGID and the daemon can use
  `killpg(pgid, signal)` to terminate the entire subprocess tree
  (including any MCP servers spawned by the agent) if a stuck
  state is detected
- **AND** this has no effect on the executor's normal
  exit-mapping behavior; it only enables process-group signaling
  during stuck-state recovery

#### Scenario: Subprocess sidecar file tracks Claude's PID
- **WHEN** `run_subprocess` successfully spawns the wrapped CLI
- **THEN** the executor writes the child's PID (which equals its
  PGID because of `process_group(0)`) to
  `<system-temp>/autocoder/busy/<workspace-basename>.subprocess`
  as plain decimal text followed by a newline
- **AND** the file is removed when the child exits (RAII guard
  scoped to `run_subprocess`)
- **AND** a daemon crash that bypasses the guard leaves the
  sidecar file in place, so the next pass's busy-marker stuck-
  state recovery can read it and `killpg` the orphaned subprocess
  tree (the original busy marker's `pgid` field records autocoder's
  group, which is not the kill target an orphaned subprocess
  requires)
- **AND** errors writing the sidecar file are logged at WARN but
  do NOT fail the executor outcome

### Requirement: Implementer prompt template loading
The executor SHALL load an implementer prompt template at construction. The template wraps the openspec change content with a role-establishing imperative so the wrapped CLI knows it is acting as an autonomous implementer and not a chat assistant. The default template is compiled into the binary; deployments may override it by setting `executor.implementer_prompt_path` in `config.yaml` to a readable file path.

#### Scenario: Default template used when override is absent
- **WHEN** `executor.implementer_prompt_path` is unset in `config.yaml`
- **THEN** the executor uses the template compiled into the binary
  (sourced from `prompts/implementer.md` at build time)
- **AND** no filesystem access for the template occurs at runtime

#### Scenario: Override path is loaded at construction
- **WHEN** `executor.implementer_prompt_path` is set to a file path
- **THEN** the executor reads the file at construction (before the
  polling loop starts) and uses its contents as the template
- **AND** if the file is missing, unreadable, or empty, daemon
  startup fails with an error message naming the path

#### Scenario: Template substitution
- **WHEN** the executor renders the prompt for a change
- **THEN** every literal occurrence of `{{change_body}}` in the
  template is replaced with the output of
  `openspec instructions apply --change <change>`
- **AND** the rendered prompt is sent to the wrapped CLI on stdin

### Requirement: Tool-use sandbox is applied at every spawn
The CLI-wrapping executor backend SHALL apply tool-use restrictions to
every spawned child process via a per-iteration Claude Code settings
file derived from `executor.sandbox` config. The settings file is
generated in the OS temp directory (not the workspace), passed to
the spawned CLI via `--settings <path>`, and deleted after the child
exits.

#### Scenario: Default sandbox applies when block is absent
- **WHEN** `config.yaml` has no `executor.sandbox` block
- **THEN** at each `run` and `resume` invocation, the executor
  generates a temp Claude Code settings file containing the
  default-deny patterns for network commands and credential paths,
  AND spawns `claude` with `--settings <temp-path>
  --allowedTools <default-list> --permission-mode acceptEdits` as
  additional flags
- **AND** the default-deny list contains at minimum
  `Bash(curl:*)`, `Bash(wget:*)`, `Bash(ssh:*)`,
  `Bash(scp:*)`, `Bash(nc:*)`, `Bash(git push:*)`,
  `Bash(git remote *)`, `Read(/home/*/.ssh/**)`,
  `Read(/home/*/.claude/**)`

#### Scenario: Operator-customized sandbox is honored
- **WHEN** `config.yaml`'s `executor.sandbox` block explicitly lists
  `allowed_tools`, `disallowed_bash_patterns`, AND
  `disallowed_read_paths`
- **THEN** the generated settings file's `permissions.deny` contains
  exactly the operator's `Bash(...)` and `Read(...)` patterns
- **AND** the `--allowedTools` flag value is exactly the operator's
  `allowed_tools` list joined by commas
- **AND** no default values are merged in (operators express the
  full intended list)

#### Scenario: Partially-specified sandbox falls back to defaults per-field
- **WHEN** `executor.sandbox` is present but omits one of the three
  fields (e.g. specifies `allowed_tools` but not
  `disallowed_bash_patterns`)
- **THEN** the omitted field defaults to its safe baseline
- **AND** the specified field uses the operator's value verbatim

#### Scenario: Settings file is per-iteration and cleaned up
- **WHEN** the executor spawns the child
- **THEN** the settings file path is in the OS temp directory
  (`std::env::temp_dir()`), not inside the workspace
- **AND** the file is deleted after the child exits, regardless of
  exit status
- **AND** failure to delete the temp file is logged at warn level
  but does NOT propagate as an error

#### Scenario: Resume applies the same sandbox
- **WHEN** `Executor::resume(handle, answer)` spawns the child
- **THEN** the same sandbox-flag-and-settings-file generation runs,
  with the same defaults / operator config as the original `run`
  call

### Requirement: Sandbox config schema
autocoder SHALL accept an optional `executor.sandbox` block with three
optional sub-fields, each with a documented safe default applied when
absent. The default `disallowed_bash_patterns` SHALL include patterns
blocking openspec state-mutation operations so the executor cannot
short-circuit a change by archiving it.

#### Scenario: `allowed_tools` field
- **WHEN** `executor.sandbox.allowed_tools` is set
- **THEN** the value is a YAML list of Claude Code tool names (e.g.
  `["Read", "Write", "Edit", "Glob", "Grep", "Bash"]`)
- **AND** the value is passed verbatim to the `--allowedTools` flag
  joined by commas

#### Scenario: `disallowed_bash_patterns` field
- **WHEN** `executor.sandbox.disallowed_bash_patterns` is set
- **THEN** each entry becomes `Bash(<pattern>)` in the generated
  settings file's `permissions.deny` array

#### Scenario: `disallowed_read_paths` field
- **WHEN** `executor.sandbox.disallowed_read_paths` is set
- **THEN** each entry becomes `Read(<pattern>)` in the generated
  settings file's `permissions.deny` array

#### Scenario: Default `allowed_tools`
- **WHEN** `executor.sandbox.allowed_tools` is absent
- **THEN** the default is `["Read", "Write", "Edit", "Glob", "Grep", "Bash"]`
- **AND** notable exclusions are `WebFetch` and `WebSearch`

#### Scenario: Default `disallowed_bash_patterns` includes network egress
- **WHEN** `executor.sandbox.disallowed_bash_patterns` is absent
- **THEN** the default includes at minimum: `curl:*`, `wget:*`,
  `nc:*`, `ncat:*`, `netcat:*`, `ssh:*`, `scp:*`, `sftp:*`,
  `rsync:*`, `git push:*`, `git remote *`, `git fetch *://*`

#### Scenario: Default `disallowed_bash_patterns` blocks openspec state mutation
- **WHEN** `executor.sandbox.disallowed_bash_patterns` is absent
- **THEN** the default also includes `openspec archive:*` AND
  `openspec unarchive:*`
- **AND** read-only `openspec` operations (validate, list, status,
  show, instructions) are NOT in the denylist; the executor needs
  them to inspect change state

#### Scenario: Default `disallowed_read_paths`
- **WHEN** `executor.sandbox.disallowed_read_paths` is absent
- **THEN** the default includes at minimum: `/home/*/.ssh/**`,
  `/home/*/.claude/**`, `/etc/shadow`, `/etc/ssl/private/**`

### Requirement: Sandbox does not bind the code-reviewer
The tool-use sandbox SHALL apply only to the executor's spawned
agent CLI subprocess, NOT to the code-reviewer's LLM API calls. The
code-reviewer operates via direct HTTP requests under operator
configuration (provider, api_key, api_base_url, model) and is a
separate data flow.

#### Scenario: Reviewer call is unaffected by sandbox
- **WHEN** the code-reviewer is enabled AND
  `code_reviewer::review(diff, summary)` is called
- **THEN** the HTTP call to the configured LLM provider proceeds
  per the reviewer's config without consulting
  `executor.sandbox`
- **AND** the diff content (which the operator's reviewer config
  authorized for upload) is sent as configured

### Requirement: Executor invokes Claude CLI in JSON event streaming mode and captures events to a structured log
When `executor.output_format` is `"json"` (the default), the executor SHALL invoke the wrapped Claude CLI with the `--output-format stream-json` argument (or whatever flag name Claude CLI's current release uses for line-delimited JSON event output). The executor SHALL spawn a streaming reader task that reads stdout line-by-line, parses each line as a JSON event, AND dispatches the parsed event to a `StructuredLogWriter` that builds TWO sibling files per change:

- **Summary log** at `<logs_dir>/runs/<basename>/<change>.log` containing `PROMPT`, `ACTIONS` (replaced with a single pointer line, NOT the action stream), `FINAL ANSWER`, AND `STDERR` sections in that order. The ACTIONS slot SHALL contain exactly one line: `=== ACTIONS (see <change>.stream.log) ===`. Operators reading the summary log see a short, signal-dense file with the agent's prompt input AND the agent's deliberate end-of-run emission, plus a pointer to where the verbose action stream lives.
- **Stream log** at `<logs_dir>/runs/<basename>/<change>.stream.log` containing the verbose action stream — `[tool_use] ...`, `[tool_result] (N bytes returned)`, `[assistant] ...`, `[raw] ...`, `[unknown:<type>] ...` lines as today's single-file ACTIONS section. No section headers. One continuous stream.

Dispatch routing happens at event-classification time inside the writer; no buffering of the full stream in memory is required. The streaming approach guarantees that on timeout-kill, both files already contain every event the child emitted before the kill — the summary log is structurally complete (all four section headers present) AND the stream log contains whatever action events arrived.

Daemon-internal consumers of per-change log content SHALL NOT read the stream log for daemon-meaningful markers. The PR-comment composer reads the summary log's FINAL ANSWER section (per the canonical "PR-comment Agent implementation notes body uses the FINAL ANSWER" requirement). The sentinel scanner reads `outcome.final_answer` directly from the executor's structured outcome (per the `a20a1`-narrowed scoping). The stream log is operator-diagnostic only.

#### Scenario: Successful JSON run produces structured log
- **WHEN** Claude CLI is invoked with JSON streaming mode AND the run completes successfully
- **THEN** the summary log file contains four section markers in order: `=== PROMPT (<n> bytes) ===`, `=== ACTIONS (see <change>.stream.log) ===`, `=== FINAL ANSWER (<n> bytes) ===`, `=== STDERR (<n> bytes) ===`
- **AND** the stream log file contains formatted lines for each tool_use, tool_result, and intermediate assistant text block in the run
- **AND** the FINAL ANSWER section in the summary log contains the text from the `result` event that closes the run
- **AND** the summary log's ACTIONS slot contains ONLY the pointer line — no `[tool_*]` or `[assistant]` content

#### Scenario: Timeout-killed run preserves the ACTIONS up to the kill
- **WHEN** Claude CLI emits N events on stdout AND autocoder's timeout enforcement kills the child before the `result` event arrives
- **THEN** the stream log file contains the N events that arrived
- **AND** the summary log's FINAL ANSWER section is empty (the `result` event never arrived to populate it)
- **AND** both files are structurally complete: the summary log has all four section headers with size annotations updated; the stream log contains whatever lines arrived before the kill

#### Scenario: Malformed JSON line lands in the stream log as raw
- **WHEN** the stdout reader receives a line that fails JSON parsing
- **THEN** the line is appended to the stream log as `[raw] <line content>`
- **AND** a WARN log is emitted naming the malformed line
- **AND** subsequent lines continue to be parsed normally
- **AND** the summary log is unaffected (the line does not appear in any of its sections)

#### Scenario: Unknown event type lands in the stream log as unknown
- **WHEN** the stdout reader receives a JSON event whose `type` field doesn't match a known variant
- **THEN** the event is appended to the stream log as `[unknown:<type>] <raw json>`
- **AND** subsequent events continue to be processed normally
- **AND** the summary log is unaffected

#### Scenario: Zero-action run still creates both files
- **WHEN** a run completes with zero `tool_use` / `tool_result` events AND no intermediate assistant text (e.g. the agent processed the prompt purely via internal reasoning AND emitted only a `result` event)
- **THEN** the summary log is created with all four section markers
- **AND** the stream log is created AS AN EMPTY FILE (no `[tool_*]` lines) so the operator's `<change>.stream.log` path resolves AND the diagnostic-consistency invariant holds
- **AND** the summary log's ACTIONS pointer line still reads `=== ACTIONS (see <change>.stream.log) ===`

#### Scenario: Stream log path is sibling to summary log
- **WHEN** the writer creates the per-change log files for change `<slug>` in workspace `<basename>`
- **THEN** the summary log path is `<logs_dir>/runs/<basename>/<slug>.log`
- **AND** the stream log path is `<logs_dir>/runs/<basename>/<slug>.stream.log`
- **AND** the two paths share the same parent directory

### Requirement: PR-comment "Agent implementation notes" body uses the FINAL ANSWER, not the action stream
The polling-loop code that constructs the `## Agent implementation notes` PR comment SHALL read the FINAL ANSWER section's content from the per-change log file AND use it as the comment body. The ACTIONS section's content (tool calls, intermediate assistant text) SHALL NOT appear in the PR comment under any circumstance — it is operator-diagnostic content only. When the FINAL ANSWER section is empty (timeout case OR any other reason the run didn't reach the `result` event), the comment body uses the fallback string `(executor timed out before final summary; see daemon log for action stream)`.

#### Scenario: Successful run's PR comment matches FINAL ANSWER exactly
- **WHEN** a successful change's log file has a FINAL ANSWER section with text `<X>`
- **THEN** the PR's "Agent implementation notes" comment body for that change is `<X>` (verbatim, modulo Markdown formatting around it)
- **AND** the comment body does NOT contain any tool_use, tool_result, or intermediate assistant text from the ACTIONS section

#### Scenario: Empty FINAL ANSWER uses the fallback string
- **WHEN** a change's log file's FINAL ANSWER section is empty (timeout-kill before the run completed)
- **THEN** the comment body is `(executor timed out before final summary; see daemon log for action stream)`
- **AND** the PR is created normally if any commits landed; the comment just notes the missing summary

### Requirement: Per-change log files are pruned after `executor.log_retention_days` days, preserving active-change logs
At daemon startup AND once every 24 hours during operation, the daemon SHALL run a retention pass over the per-change log directory. A summary log file `<change>.log` SHALL be eligible for deletion when its modification time is older than `now - log_retention_days * 86400` seconds AND its corresponding change directory at `<workspace>/openspec/changes/<change>/` does NOT exist (the change has been archived OR removed). Logs for changes that are STILL active SHALL be preserved regardless of age. The default `log_retention_days` value is `30`; operator-configurable; clamped at `365`.

The retention pass operates on log-file PAIRS: when a summary log is eligible for deletion, the sibling `<change>.stream.log` file (if present) SHALL be deleted in the same retention pass. The order is summary-first, then stream; partial-success cases (summary deleted, stream-delete failed due to filesystem error) log WARN naming the orphan AND the retention pass continues processing remaining changes. Active-change preservation extends to the pair: when `<change>.log` is preserved, its sibling stream log is also preserved.

An orphan stream log (a `<change>.stream.log` file present WITHOUT its summary log — e.g. from a partial pre-spec migration OR manual operator action) SHALL be eligible for deletion when its OWN mtime exceeds the retention window AND no `openspec/changes/<change>/` directory exists. Orphan cleanup logs WARN naming the file so operators see the cleanup happen.

#### Scenario: Stale log for archived change is deleted
- **WHEN** the retention pass runs AND a summary log file `<change>.log` has mtime more than `log_retention_days` days ago AND no `openspec/changes/<change>/` directory exists for it
- **THEN** the summary log file is deleted
- **AND** the sibling `<change>.stream.log` is also deleted in the same pass (if present)
- **AND** the retention report's `files_deleted` count includes both files (counted separately)

#### Scenario: Old log for active change is preserved
- **WHEN** a summary log file is older than the retention window AND its change directory still exists in the active path
- **THEN** the summary log file is NOT deleted
- **AND** the sibling stream log file is also NOT deleted
- **AND** the retention report's `files_preserved` count includes both files

#### Scenario: Recent log is preserved regardless of change state
- **WHEN** a summary log file's mtime is within the retention window
- **THEN** the summary log file is NOT deleted regardless of whether the change is active or archived
- **AND** the sibling stream log file is also NOT deleted

#### Scenario: Orphan stream log cleanup
- **WHEN** the retention pass encounters a `<change>.stream.log` file whose corresponding summary log `<change>.log` does NOT exist AND whose mtime exceeds the retention window AND whose change directory does NOT exist
- **THEN** the orphan stream log file is deleted
- **AND** a WARN log fires naming the orphan path AND noting the cleanup
- **AND** the retention report's `files_deleted` count includes the orphan

#### Scenario: Partial-success on stream deletion logs WARN
- **WHEN** the summary log is deleted successfully BUT the sibling stream log deletion fails (e.g. permission denied, transient filesystem error)
- **THEN** a WARN log fires naming the orphan stream log path
- **AND** the retention pass continues processing remaining changes (no abort)
- **AND** the next retention pass picks up the orphan via the orphan-cleanup scenario above

### Requirement: `executor.output_format: "text"` preserves the legacy at-exit capture behavior
When `executor.output_format` is `"text"`, the executor SHALL omit the `--output-format stream-json` flag from the spawn command AND fall back to today's at-exit-capture pattern. The log file shape uses the legacy `=== STDOUT ===` / `=== STDERR ===` section names instead of the new `=== ACTIONS ===` / `=== FINAL ANSWER ===` shape. The PR-comment construction path detects the legacy section names AND reads raw stdout as the comment body (today's behavior).

#### Scenario: Text-mode opt-out uses legacy log shape
- **WHEN** the config has `executor.output_format: "text"`
- **THEN** the spawn command lacks `--output-format stream-json`
- **AND** the log file uses `=== STDOUT (<n> bytes) ===` and `=== STDERR (<n> bytes) ===` section names
- **AND** the PR-comment construction path reads raw stdout from the STDOUT section as the comment body

#### Scenario: Text-mode opt-out on timeout produces today's zero-bytes outcome
- **WHEN** the config has `executor.output_format: "text"` AND a run times out
- **THEN** the log file's STDOUT section reads `=== STDOUT (0 bytes) ===` (the legacy behavior of losing the buffer on kill is preserved verbatim)

### Requirement: Sentinel emission instructions in the implementer prompt include a concrete worked example AND a self-check hint
Every outcome-sentinel format documented in `prompts/implementer.md` (currently the `SpecNeedsRevision` sentinel; future formats SHALL follow the same pattern) SHALL be presented with three structural elements:

1. **A substitution instruction** appearing IMMEDIATELY BEFORE the example, naming the rule that the example is a pattern AND that emitting it verbatim is a parse failure.
2. **A worked example with no angle-bracket placeholders** showing what a complete, parseable sentinel looks like. The example SHALL deserialize cleanly into the corresponding Rust type via `serde_json::from_str` AND SHALL contain realistic task ids, prose, AND reasoning that the agent can model.
3. **A self-check hint** appearing AFTER the example, instructing the agent to scan its emitted sentinel for `<...>` patterns inside string values before emitting AND describing the daemon's placeholder-detection diagnostic.

The implementer prompt SHALL NOT use angle-bracket placeholders (`<id-from-tasks-md>`, `<verbatim quote>`, etc.) inside string values in any sentinel example. Earlier versions of the prompt used this pattern AND triggered literal-emission failures; the lesson is preserved as a hard rule.

Operator-customizable override prompts (loaded via the uniform `PromptLoader` per `a24`'s spec) MAY use any structure the operator prefers — the canonical rule binds the bundled default only. Operators whose customized templates regress to placeholder-style examples will hit the same failure mode the bundled prompt previously hit; the placeholder-detection requirement in `orchestrator-cli` surfaces the diagnostic AND points the operator at the bundled default for reference.

#### Scenario: Bundled prompt's sentinel example is parseable
- **WHEN** an automated test deserializes the worked-example JSON from `prompts/implementer.md`'s sentinel section into `SpecNeedsRevisionDetail`
- **THEN** the deserialization succeeds without error
- **AND** every field's value is a concrete string (no angle-bracket markers, no template variables)

#### Scenario: Bundled prompt contains the three structural elements
- **WHEN** a maintainer reads `prompts/implementer.md`'s sentinel section
- **THEN** the section contains a substitution instruction paragraph IMMEDIATELY BEFORE the example
- **AND** the example itself contains no angle-bracket placeholders inside string values
- **AND** a self-check hint paragraph appears AFTER the example naming the daemon's placeholder-detection diagnostic

#### Scenario: Future sentinel formats follow the same pattern
- **WHEN** a future change introduces a new sentinel format in `prompts/implementer.md` (OR a new operator-aimed prompt template added by the daemon)
- **THEN** the new format's documentation in the prompt follows the substitution-instruction + worked-example + self-check-hint structure
- **AND** the new format's example deserializes cleanly into its corresponding Rust type

### Requirement: Timeout classification takes precedence over sentinel extraction; sentinel scan is scoped to deliberate-emission content
The executor's outcome-dispatch path SHALL check `outcome.timed_out` BEFORE attempting any sentinel extraction OR sentinel-parse fallback. When `outcome.timed_out` is `true`, the executor SHALL return `Failed { reason: "timeout" }` (OR the canonical timeout-reason format) WITHOUT scanning for, extracting, OR attempting to parse any sentinel-shaped substring in the captured event stream. The sentinel is by definition a deliberate end-of-run emission; a timed-out run did not reach end-of-run, so no sentinel-shaped scrollback content is semantically the agent's emission.

When the run did NOT time out AND a sentinel scan is performed, the scan's input scope depends on the configured output format:

- **JSON streaming mode** (`executor.output_format: json`, the default): the scanner reads ONLY `outcome.final_answer`. When `final_answer` is `None` (the agent never reached the `result` event for any reason — crash, protocol error, etc.), the sentinel scan returns `None` AND the normal exit-status path handles the outcome. The scanner SHALL NOT fall back to `outcome.stdout`. Rationale: the `result` event's text is the agent's deliberate end-of-run emission; tool-result echoes, prompt-context echoes, AND other event-stream content are NOT deliberate emissions AND must not be matched against the sentinel.
- **Text mode** (`executor.output_format: text`, the legacy opt-out): the scanner reads `outcome.stdout`. This mode has no separate `result`-event channel, so stdout IS the agent's emission stream. Timeout precedence still applies — a timed-out text-mode run is classified as timeout BEFORE the sentinel scan runs.

This requirement narrows the canonical "Malformed outcome sentinel falls back to Failed" scenario WITHOUT changing it: a malformed sentinel that genuinely appears in the agent's deliberate emission still triggers the canonical fallback. The change is what counts as "the agent's deliberate emission" — sentinel-shaped substrings in tool-result echoes OR prompt-context echoes are no longer in scope.

#### Scenario: Timed-out run with sentinel-shaped scrollback returns timeout
- **WHEN** the executor invocation completes with `outcome.timed_out: true` AND `outcome.stdout` contains a well-formed `=== AUTOCODER-OUTCOME ===` block followed by valid JSON (the worst-case false-match: sentinel content present, would-be-parseable)
- **THEN** the executor returns `Failed { reason: "timeout" }`
- **AND** no sentinel-extraction attempt is made
- **AND** no `agent emitted unparseable SpecNeedsRevision sentinel` log line fires
- **AND** the perma-stuck counter increments against a transient-infrastructure category (the canonical "predictable failure" set) if the operator has configured that classification, NOT against a genuine agent failure

#### Scenario: Timed-out run with prompt-template echo in stdout returns timeout
- **WHEN** the executor invocation completes with `outcome.timed_out: true`, `outcome.final_answer: None`, AND `outcome.stdout` contains a tool-result echo of `prompts/implementer.md` (including the sentinel example block with `\n31\t`-style line-number prefixes)
- **THEN** the executor returns `Failed { reason: "timeout" }`
- **AND** the line-number-prefixed pseudo-sentinel content is NOT parsed
- **AND** no misleading `unparseable sentinel` reason is surfaced to the operator

#### Scenario: JSON streaming mode scans only final_answer
- **WHEN** the executor invocation completes with `output_format: Json`, `outcome.timed_out: false`, `outcome.final_answer: Some("Implementation complete; all tests pass.")` (no sentinel), AND `outcome.stdout` contains a sentinel-shaped block from a tool-result echo
- **THEN** the sentinel scanner reads ONLY `final_answer`
- **AND** the scan returns `None`
- **AND** the executor proceeds to the normal exit-status path
- **AND** the stdout echo's sentinel-shaped content is ignored

#### Scenario: JSON streaming mode with sentinel in final_answer parses correctly
- **WHEN** `output_format: Json`, `outcome.timed_out: false`, AND `outcome.final_answer: Some("=== AUTOCODER-OUTCOME ===\n{\"type\":\"spec_needs_revision\",\"unimplementable_tasks\":[...],...}")`
- **THEN** the sentinel scanner extracts the payload from `final_answer` AND parses it
- **AND** a well-formed payload returns `SpecNeedsRevision { ... }` per the canonical outcome
- **AND** a malformed payload triggers the canonical "Malformed outcome sentinel falls back to Failed" path

#### Scenario: Text mode preserves stdout scan for non-timeout runs
- **WHEN** `output_format: Text`, `outcome.timed_out: false`, AND `outcome.stdout` contains a sentinel block
- **THEN** the sentinel scanner reads `outcome.stdout` AND extracts the block
- **AND** the existing parse + dispatch behaviour is unchanged from pre-spec text-mode behaviour
- **AND** text mode's stdout-as-emission semantic is preserved

#### Scenario: JSON streaming mode with final_answer absent skips the sentinel scan
- **WHEN** `output_format: Json`, `outcome.timed_out: false` (run completed normally per exit status), AND `outcome.final_answer: None` (no `result` event was captured for some non-timeout reason — protocol error, missing event type, etc.)
- **THEN** the sentinel scan returns `None` without consulting `outcome.stdout`
- **AND** the executor proceeds to the normal exit-status path (which may classify as Failed for other reasons)
- **AND** stdout echo content is not considered for sentinel matching even when final_answer is unexpectedly empty

