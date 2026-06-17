## ADDED Requirements

### Requirement: `verify` subcommand runs the pre-executor gate checks locally on a working-tree change
autocoder SHALL provide a `verify <change-slug>` subcommand that runs the pre-executor verifier-gate checks — `[in]` (change-internal) AND `[canon]` (change-vs-canonical), AND any other realized spec-checking gate that is enabled — against a change in the LOCAL working tree, so an operator can learn whether a change would pass the server gates BEFORE pushing it. It is a new invocation surface for the existing checks, NOT a redefinition of the verifier-gate framework: it invokes the same check logic, the same prompts, the same model configuration (`executor.change_*_contradiction_check_llm`), AND the same submission schemas the server uses, so its verdict matches what the server will enforce.

The subcommand SHALL run in the repository's working directory, reading `openspec/changes/<change-slug>/specs/**` (the deltas) and the local `openspec/specs/**` (canon) — the working copy, before any push. It SHALL be read-only: it SHALL NOT run the executor, SHALL NOT write `.needs-spec-revision.json`, AND SHALL NOT modify the workspace. It reports findings to stdout, grouped by gate AND labeled with the gate identifier, each carrying the finding narrative the server marker's `revision_suggestion` would carry.

By default `verify` SHALL run the gates ENABLED in config (so its verdict matches server enforcement); a selector MAY override (`--all` for every realized spec-checking gate, `--gate <list>` for a named subset). Exit code SHALL be CI-usable, conforming to the `gatekeepers-fail-closed` standard: `0` ONLY when every gate that ran returned no findings; non-zero when any gate finds a contradiction; AND non-zero when an enabled gate CANNOT run (model unconfigured, transport error, unregistered strategy) — `verify` SHALL report "gate could not run" AND fail, never reporting clean for a gate that did not actually evaluate.

`verify` is a subcommand of the autocoder binary (so it ships the identical check logic the server runs). A check-only install SHALL be supported: it fetches a PREBUILT binary, places it on the interactive `PATH`, AND drops a minimal config carrying only what `verify` needs (the `executor.change_*_contradiction_check_llm` model blocks and corpus locations) — so it runs on a low-powered spec-authoring machine without building from source OR running the daemon.

#### Scenario: A clean change passes verify
- **WHEN** an operator runs `verify <slug>` in a repo against a change whose deltas contradict neither themselves nor canon AND the relevant gates are enabled and configured
- **THEN** each run gate reports clean AND the command exits `0`
- **AND** no marker is written, no executor runs, AND the workspace is unmodified

#### Scenario: A contradicting change is reported with a non-zero exit
- **WHEN** `verify <slug>` runs against a change whose deltas contradict canon (or each other)
- **THEN** the command prints the finding(s), each labeled with the gate that produced it (`[in]` / `[canon]` / …)
- **AND** it exits non-zero
- **AND** the finding narrative matches what the server's `.needs-spec-revision.json` would carry

#### Scenario: verify's verdict matches the server gate
- **WHEN** `verify` runs the same enabled gate against the same change the server would
- **THEN** it uses the same check logic, prompts, model config, and submission schema as the server
- **AND** a change `verify` reports clean is not subsequently kicked back by that same server gate (absent canon drift since the local run)

#### Scenario: A gate that cannot run fails closed, not clean
- **WHEN** an enabled gate cannot run during `verify` (its model is unconfigured, the agentic session errors, or its strategy is unregistered)
- **THEN** `verify` reports that the gate could not run AND exits non-zero
- **AND** it does NOT report the change as clean

#### Scenario: Default runs enabled gates; selector overrides
- **WHEN** `verify <slug>` is run with no gate selector
- **THEN** it runs exactly the spec-checking gates enabled in config
- **WHEN** `verify <slug> --all` or `verify <slug> --gate in,canon` is run
- **THEN** it runs the selected gates regardless of their enabled state (reporting any that cannot run as fail-closed)

#### Scenario: Check-only install runs without a daemon or a source build
- **WHEN** an operator runs the check-only install on a spec-authoring machine
- **THEN** a prebuilt `verify`-capable binary is placed on the interactive `PATH` AND a minimal config with the contradiction-check model blocks is written
- **AND** `verify` runs against a local repo with no daemon running and without compiling from source
