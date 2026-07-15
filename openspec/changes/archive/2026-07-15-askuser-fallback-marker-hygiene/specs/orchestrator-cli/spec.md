## MODIFIED Requirements

### Requirement: `verify` subcommand runs the pre-executor gate checks locally on a working-tree change

autocoder SHALL provide a `verify <change-slug>` subcommand that runs the pre-executor verifier-gate checks — `[in]` (change-internal), `[canon]` (change-vs-canonical), `[rules]` (global-rules), AND any other realized spec-checking gate that is enabled — against a change in the LOCAL working tree, so an operator can learn whether a change would pass the server gates BEFORE pushing it. It is a new invocation surface for the existing checks, NOT a redefinition of the verifier-gate framework: it invokes the same check entry points (`preflight::change_contradiction::run_agentic_contradiction_check`, `preflight::canon_contradiction::run_agentic_canon_contradiction_check`, `preflight::global_rules::run_agentic_global_rules_check`; shared core in `preflight::corpus_check`), the same prompts, the same per-gate model configuration (`executor.change_internal_contradiction_check_llm`, `executor.change_canonical_contradiction_check_llm`, `executor.global_rules_check_llm`), AND the same submission schemas the server uses, so its verdict matches what the server will enforce.

`verify` SHALL stand up the submission transport in-process via `control_socket::spawn_submission_listener(paths)` as a hard precondition for the duration of the run; without it the gates fail closed and `verify` cannot pass. `verify` SHALL resolve its agentic-session timeout from `ExecutorConfig::agentic_session_timeout()` (reading `executor.agentic_session_timeout_secs`, default `3600` when omitted) — NOT a verify-local literal.

The subcommand SHALL run in the repository's working directory, reading `openspec/changes/<change-slug>/specs/**` (the deltas) and the local `openspec/specs/**` (canon) — the working copy, before any push. It SHALL NOT run the executor, SHALL NOT write `.needs-spec-revision.json`, AND SHALL NOT make spec or source edits. It MAY create transient run artifacts (`.mcp.json`, the control socket, AND any ask-user fallback markers its gate sessions leave when the in-process socket becomes unreachable) AND SHALL clean them up on exit. Because `verify` runs in an operator's own clone — not a daemon-managed workspace with its own exclude registration — `verify` SHALL, at run start, idempotently register the per-run artifact patterns (including `.askuser-pending*`) in the target repository's `.git/info/exclude`, so an artifact that survives an interrupted or crashed run cannot be swept into a commit by a later broad `git add`. `.git/info/exclude` is local-only and never itself committed. It reports findings to stdout, grouped by gate AND labeled with the gate identifier, each carrying the finding narrative the server marker's `revision_suggestion` would carry.

By default `verify` SHALL run the gates ENABLED in config (so its verdict matches server enforcement); a selector MAY override (`--all` for every realized spec-checking gate, `--gate <list>` for a named subset). Exit code SHALL be CI-usable, conforming to the `gatekeepers-fail-closed` standard: `0` ONLY when every gate that ran returned no findings; non-zero when any gate finds a contradiction; AND non-zero when an enabled gate CANNOT run (model unconfigured, transport error, unregistered strategy, no submission captured) — `verify` SHALL report "gate could not run" AND fail, never reporting clean for a gate that did not actually evaluate. When the resolved gate set is EMPTY (no spec-checking gate enabled AND no selector forcing one), `verify` SHALL NOT exit `0` silently: it SHALL report that no gate evaluated the change AND exit non-zero, conforming to the `gatekeepers-contain-no-judgment` standard (code never manufactures a clean pass when nothing was evaluated).

`verify` is a subcommand of the autocoder binary (so it ships the identical check logic the server runs). A check-only install SHALL be supported: it fetches a PREBUILT binary, places it on the interactive `PATH`, AND drops a minimal config carrying only what `verify` needs (the `executor.change_internal_contradiction_check_llm`, `executor.change_canonical_contradiction_check_llm`, AND `executor.global_rules_check_llm` model blocks with their `enabled` flags, plus corpus locations) — so it runs on a low-powered spec-authoring machine without building from source OR running the daemon.

#### Scenario: A clean change passes verify
- **WHEN** an operator runs `verify <slug>` in a repo against a change whose deltas contradict neither themselves nor canon AND the relevant gates are enabled and configured
- **THEN** each run gate reports clean AND the command exits `0`
- **AND** no marker is written, no executor runs, AND no spec or source files are edited; transient run artifacts are cleaned up

#### Scenario: A contradicting change is reported with a non-zero exit
- **WHEN** `verify <slug>` runs against a change whose deltas contradict canon (or each other)
- **THEN** the command prints the finding(s), each labeled with the gate that produced it (`[in]` / `[canon]` / `[rules]`)
- **AND** it exits non-zero
- **AND** the finding narrative matches what the server's `.needs-spec-revision.json` would carry

#### Scenario: verify's verdict matches the server gate
- **WHEN** `verify` runs the same enabled gate against the same change the server would
- **THEN** it uses the same check entry point, prompts, model config, and submission schema as the server
- **AND** a change `verify` reports clean is not subsequently kicked back by that same server gate (absent canon drift since the local run)

#### Scenario: A gate that cannot run fails closed, not clean
- **WHEN** an enabled gate cannot run during `verify` (its model is unconfigured, the agentic session errors, its strategy is unregistered, or no submission is captured)
- **THEN** `verify` reports that the gate could not run AND exits non-zero
- **AND** it does NOT report the change as clean

#### Scenario: Without the submission listener every gate fails closed
- **WHEN** `verify` runs but the in-process submission listener was not stood up (the control-socket env var is unset)
- **THEN** every gate drains no submission AND is reported as unable to run (fail-closed) with a non-zero exit
- **AND** no gate reports clean — confirming the listener is a hard precondition

#### Scenario: An empty enabled-gate set is loud, not a silent pass
- **WHEN** `verify <slug>` runs with a config in which NO spec-checking gate is enabled AND no selector forces one
- **THEN** `verify` reports that no gate evaluated the change AND exits non-zero
- **AND** it does NOT exit `0` — code never manufactures a clean pass for a change nothing checked

#### Scenario: verify honors the unified agentic-session timeout
- **WHEN** `verify` runs with `executor.agentic_session_timeout_secs` configured (or omitted)
- **THEN** the gate sessions use the value resolved from `ExecutorConfig::agentic_session_timeout()` (the configured value, or `3600` when omitted)
- **AND** `verify` does NOT use a verify-local timeout literal

#### Scenario: Default runs enabled gates; selector overrides
- **WHEN** `verify <slug>` is run with no gate selector
- **THEN** it runs exactly the spec-checking gates enabled in config
- **WHEN** `verify <slug> --all` or `verify <slug> --gate in,canon` is run
- **THEN** it runs the selected gates regardless of their enabled state (reporting any that cannot run as fail-closed)
- **AND** an unknown gate name in `--gate` is an error, not a silent skip

#### Scenario: Gate-session ask-user fallback markers are cleaned up on exit
- **WHEN** a gate session during `verify` hits the ask-user fallback and writes a pending-question marker
- **THEN** `verify` deletes the marker during its exit cleanup, alongside its other transient run artifacts
- **AND** the repository's working tree is left as the operator had it

#### Scenario: An interrupted verify run cannot leak artifacts into a commit
- **WHEN** a `verify` run is interrupted (crash, kill) after a gate session wrote an ask-user fallback marker but before cleanup ran
- **AND** the operator later stages broadly (e.g. `git add openspec/` or `git add -A`)
- **THEN** the surviving marker is not staged (the `.askuser-pending*` pattern was registered in `.git/info/exclude` at run start)
- **AND** no `.gitignore` change appears in the repository (the exclude file is local-only)

#### Scenario: Exclude registration is idempotent across verify runs
- **WHEN** `verify` runs several times against the same repository
- **THEN** each artifact pattern appears at most once in `.git/info/exclude`

#### Scenario: Check-only install runs without a daemon or a source build
- **WHEN** an operator runs the check-only install on a spec-authoring machine
- **THEN** a prebuilt `verify`-capable binary is placed on the interactive `PATH` AND a minimal config with the `executor.change_internal_contradiction_check_llm`, `executor.change_canonical_contradiction_check_llm`, and `executor.global_rules_check_llm` model blocks is written
- **AND** `verify` runs against a local repo with no daemon running and without compiling from source
