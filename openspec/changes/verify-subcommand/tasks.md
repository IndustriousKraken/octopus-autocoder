# Tasks

## 1. The subcommand

- [ ] 1.1 Add `Command::Verify { change_slug, gate selector, config path }` to `cli/mod.rs` (alongside `Doctor`, `CheckConfig`, `SyncSpecs`). Runs in the cwd repo; no `--repo` selector.
- [ ] 1.2 Add a `cli/verify.rs` driver that resolves the change directory (`openspec/changes/<slug>/`) and the local canon (`openspec/specs/`) from the cwd repo, loads the (minimal) config, and invokes the enabled pre-executor gate checks against those working-tree paths.

## 2. Reuse the gate checks (no reimplementation)

- [ ] 2.1 Invoke the SAME `[in]` and `[canon]` check functions the verifier-gate framework uses (`verifier_gate.rs` / `llm.rs`), passing the working-tree change + local canon, the same prompts, the same `executor.change_*_contradiction_check_llm` model config, and the same submission schemas. Do NOT fork the logic.
- [ ] 2.2 Default to the gates ENABLED in config; honor `--all` (every realized spec-checking gate) and `--gate <list>` (named subset). Run the gates generically so a later corpus-parameterized gate (global-rules) is picked up without changing `verify`.

## 3. Output + exit semantics (fail-closed)

- [ ] 3.1 Render findings to stdout grouped by gate, each labeled with the gate identifier and carrying the same narrative the server marker's `revision_suggestion` would.
- [ ] 3.2 Exit `0` only when every gate that ran is clean; non-zero on any finding; non-zero (fail-closed) when an enabled gate cannot run (model unconfigured / transport error / unregistered strategy), reporting "gate could not run" — never report clean for a gate that did not evaluate.
- [ ] 3.3 Read-only: assert no `.needs-spec-revision.json` is written, no executor is invoked, and the workspace is unmodified by `verify`.

## 4. Check-only install

- [ ] 4.1 Add a check-only install path (script) that fetches the PREBUILT binary (built in CI / on the server — never compiled on the spec-box), places it on the interactive `PATH`, and writes a minimal config containing only the `executor.change_*_contradiction_check_llm` model blocks (and corpus locations) — no repos/chatops/reviewer/daemon config.
- [ ] 4.2 Ensure CI publishes the prebuilt binary artifact the install script consumes.

## 5. Tests

- [ ] 5.1 A clean change → exit 0, no marker, no executor, workspace unmodified (assert behavior/state).
- [ ] 5.2 A change with a seeded contradiction → the finding is printed gate-labeled AND exit is non-zero.
- [ ] 5.3 An enabled gate that cannot run (e.g. model unconfigured) → fail-closed: "could not run" + non-zero, NOT clean.
- [ ] 5.4 Default runs only enabled gates; `--all` / `--gate` override; an unknown gate name is an error, not a silent skip.
- [ ] 5.5 The verify driver invokes the same check entry points as the server path (assert via a shared function call / no duplicated logic), so verdicts cannot drift from the server gate.

## 6. Docs

- [ ] 6.1 Document `verify` in `docs/` (and the spec-box setup): run it in a repo before pushing; check-only install on the spec-authoring machine; it is the local accelerator, the server gates remain the enforcement.
