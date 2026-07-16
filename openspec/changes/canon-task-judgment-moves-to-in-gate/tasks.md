## 1. Remove the mechanical scan

- [ ] 1.1 Remove `autocoder/src/preflight/canon_editing_tasks.rs`, its module declaration, and its call site (`handle_canon_editing_tasks_preflight` and the suggestion builder in `autocoder/src/polling_loop/preflight_checks.rs`), along with the scanner's own unit tests.
- [ ] 1.2 Confirm the alert/marker plumbing it shared with other pre-flights (`SpecNeedsRevision` category, marker struct populations) still compiles and behaves for the remaining producers.

## 2. Gate session takes over the check

- [ ] 2.1 Extend `prompts/change-contradiction-check.md`: the session also reads the change's `tasks.md` and reports any task whose EDIT TARGET is the spec corpus, per the modified `[in]`-gate requirement — a task that merely cites the spec corpus as context, or references the change's own delta files, is not a finding.
- [ ] 2.2 Extend the `submit_contradictions` handling in `autocoder/src/mcp_askuser_server.rs` and `autocoder/src/preflight/change_contradiction.rs` with the optional findings array named in the delta (schema-validated, defaulting to empty), and render such findings into the marker's `revision_suggestion` with the folded-at-archive explanation.
- [ ] 2.3 Ensure the empty case is unchanged: a submission with no contradictions and none of the new findings proceeds to the executor with no marker and no alert.

## 3. Tests

- [ ] 3.1 Unit tests over the plumbing (not prompt wording, per the testing standard): a submission carrying the new findings array yields a marker whose `revision_suggestion` names the task and the folded-at-archive rationale; an empty array is a clean pass; a schema-invalid submission stays a correctable tool error.
- [ ] 3.2 Run the full `cargo test` suite; confirm the remaining pre-flight checks (`a17` archivability, corpus check) and the gate's fail-closed paths are unaffected by the module removal.
