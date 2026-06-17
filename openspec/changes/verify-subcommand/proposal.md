# Local `verify` subcommand: run the pre-executor gates before pushing

## Why

The `[in]` (change-internal) and `[canon]` (change-vs-canonical) gates only run
server-side, inside the polling loop, at implement time. So the only way to learn
whether a change passes them is to push it and wait for the daemon to run it — and
when it fails, the daemon writes `.needs-spec-revision.json`, the operator relays
the marker, fixes the spec, pushes again, and clears the marker. That round-trip
has eaten most of several sessions: every kickback (a duplicate-add audit task, a
shared-label conflation, a top-level-vs-thread mismatch) was a multi-minute remote
loop for a check that is a single agentic session.

The check logic already exists and is exercised every iteration; what is missing is
a way to run it locally, on the working tree, before pushing. A `verify` subcommand
that invokes the SAME checks and reports findings turns the remote round-trip into a
local, seconds-long loop — and because it reuses the server's exact logic, model
config, and prompts, its verdict matches what the server will enforce (closing the
gap that local adversarial reviewers leave open: they approximate the gate, this IS
the gate).

## What Changes

- A `verify <change-slug>` subcommand runs the enabled pre-executor verifier-gate
  checks (`[in]`, `[canon]`, and any other enabled spec-checking gate) against a
  change in the LOCAL working tree, reusing the same check logic, prompts, model
  config (`executor.change_*_contradiction_check_llm`), and submission schemas the
  server uses.
- It runs in the repository's working directory, operating on
  `openspec/changes/<change-slug>/` and the local `openspec/specs/`. It is
  read-only: it does NOT run the executor, write `.needs-spec-revision.json`, or
  modify the workspace — it reports findings to stdout, grouped and labeled by gate.
- Exit code is CI-usable: 0 when every run gate is clean; non-zero when any gate
  finds a contradiction, AND (fail-closed) non-zero when an enabled gate cannot run
  — it never reports clean for a gate that did not actually run.
- By default it runs the gates ENABLED in config (matching server enforcement); a
  selector overrides (`--all`, or `--gate in,canon`).
- A check-only install (a prebuilt binary plus a minimal config carrying only the
  contradiction-check model blocks and corpus locations) lets it run on a
  low-powered spec-authoring machine without building from source or running the
  daemon.

## Impact

- Affected specs: `orchestrator-cli` (the new subcommand; CLI subcommands live here
  alongside `rewind`, `audit run`, `check-config`).
- Affected code: a new `Command::Verify` in `cli/`; a thin driver that invokes the
  existing gate-check functions (`verifier_gate.rs` / `llm.rs`) against working-tree
  paths and renders findings; the check-only install script (fetch prebuilt binary,
  place on PATH, drop a minimal config).
- Reuses, does not redefine, the verifier-gate framework — `verify` is a new
  invocation surface for the existing `[in]`/`[canon]` checks, outside the executor
  lifecycle.
- This is the local accelerator; the server gates remain the fail-closed
  enforcement (they run against fresher canon and cover all contributors).
