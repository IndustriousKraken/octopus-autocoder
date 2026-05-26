## 1. Parser + substring matcher

- [ ] 1.1 Extend `OperatorCommand` in `autocoder/src/chatops/operator_commands.rs` with `AuditNow { audit_substring: String, repo_substring: String }`. Parser recognizes `@<bot> audit <audit-substring> <repo-substring>` with case-insensitive verb matching.
- [ ] 1.2 Add `pub fn match_audit_type<'a>(substring: &str, registered: &'a [&str]) -> AuditMatch<'a>` mirroring the `match_repo` pattern. Case-insensitive substring search; returns `Unique(name)`, `Multiple(Vec<name>)`, or `None`.
- [ ] 1.3 Argument sanitization at parser entry: audit-substring matches `^[a-zA-Z0-9_-]{1,64}$`; reuse the repo-substring rules for the repo arg.
- [ ] 1.4 Tests:
  - `@<bot> audit sec myrepo` parses as `AuditNow`.
  - `@<bot> AUDIT sec myrepo` parses identically (case-insensitive verb).
  - `@<bot> audit` parses as error (missing args).
  - `match_audit_type("sec", &[..])` returns `Unique("security_bug_audit")`.
  - `match_audit_type("arch", &[..])` returns `Multiple([architecture_brightline, architecture_consultative])`.
  - `match_audit_type("zzz", &[..])` returns `None`.
  - Argument-sanitization rejects shell metachars / path traversal.

## 2. Per-repo queue for pending audit runs

- [ ] 2.1 Extend `RepoTaskHandle` in `autocoder/src/control_socket.rs` with `pub pending_audit_runs: Arc<Mutex<Vec<String>>>` defaulting to `Arc::new(Mutex::new(Vec::new()))` at handle creation.
- [ ] 2.2 New `queue_audit` control-socket action: takes `(url, audit_type)`, resolves the handle by URL, appends `audit_type` to its `pending_audit_runs`, returns the resolved audit-type name in the response so the dispatcher can build the ack with the canonical name.
- [ ] 2.3 De-duplication: appending an audit_type that's already in the queue is a no-op (still returns success). Queuing the same audit twice should produce one run per iteration.
- [ ] 2.4 Tests:
  - Action submits → handle's `pending_audit_runs` gets the new entry.
  - Action with duplicate entry → list unchanged.
  - Action against unknown URL → returns Err with a clear message.

## 3. Dispatcher: resolve + ack

- [ ] 3.1 In the dispatcher's `AuditNow` branch:
  1. Run `match_audit_type` against registered audit-type names. If Multiple/None, return the candidate-list reply (mirroring `match_repo`'s "be more specific" / "no repo matched" patterns).
  2. Run `match_repo` against the configured repos. Same Multiple/None handling.
  3. Submit `queue_audit` with both resolved names.
  4. Build the ack:
     ```
     ✓ Queued <audit_type> for <repo_url>. Will run on the next polling iteration (~Nm).
     ```
     where `<Nm>` is the per-repo `poll_interval_sec / 60` rounded to nearest, OR `imminently` when the daemon-reported "seconds until next iteration" is < 30.
- [ ] 3.2 Tests:
  - Happy path: both substrings unique → action submitted, ack returned.
  - Ambiguous audit substring → ack lists the candidate audit-type names.
  - Ambiguous repo substring → ack lists the candidate repo URLs.
  - No-match audit → ack lists all registered audit-type names.
  - No-match repo → existing "no repo matched" reply.

## 4. Audit-scheduler integration

- [ ] 4.1 In `autocoder/src/audits/scheduler.rs`, at the start of each iteration's audit phase:
  1. Drain the repo's `pending_audit_runs` into a local `HashSet<String>` (de-duplicated).
  2. For each queued audit-type, look it up in the registered audits.
  3. Run it unconditionally — skip the cadence check, run regardless of `last_run` timestamp.
  4. After it returns, update the audit's state file with the new `last_run` timestamp (this counts as a cadence-consuming run for future scheduling, per the proposal's stated cadence-interaction rule).
  5. Proceed to the normal cadence-driven scheduling for any audit type NOT already run via the queue this iteration.
- [ ] 4.2 The queued path emits the SAME notifications as the cadence-triggered path. Once `chatops-audit-findings-in-threads` lands, the queued path naturally gets threaded notifications too.
- [ ] 4.3 Tests:
  - Queue an audit, run an iteration → the audit runs (assert via outcome captured in a test stub).
  - Queue + iteration → after the iteration, the queue is empty (drained).
  - Queue an audit whose cadence says "not yet due" → the audit runs anyway.
  - Queue an audit, run iteration, run another iteration without queuing again → the audit does NOT run the second time (only ran because it was queued the first time; second iteration sees empty queue).

## 5. CLI subcommand

- [ ] 5.1 Add `Audit { Run { workspace: PathBuf, audit: String } }` to the `Command` enum in `autocoder/src/cli/mod.rs`.
- [ ] 5.2 Dispatch handler:
  1. Probe for the control socket at `<runtime_dir>/control.sock`.
  2. If present, connect AND check the daemon's repo list for a repo whose workspace matches the given `--workspace` path (deterministic-sanitization, or explicit `local_path` match). If found, submit `queue_audit`; print the ack to stdout; exit 0. If no matching repo, print an error explaining the workspace isn't managed by the running daemon and exit non-zero.
  3. If the socket is absent, run standalone: load the workspace, invoke the audit module directly, print findings to stdout, exit 0.
- [ ] 5.3 Tests:
  - With a fixture daemon running, `autocoder audit run --workspace <fixture-path> --audit security_bug_audit` submits the queue action via the control socket.
  - Without a daemon, the same command invokes the audit module directly against the workspace.
  - With a daemon running but the workspace isn't in its repo list, exit non-zero with a clear error.

## 6. README + docs updates

- [ ] 6.1 In `docs/CHATOPS.md`'s operator-commands section, add the `audit` verb to the table with example syntax, the substring-matching note, and the ack-format example.
- [ ] 6.2 In `docs/CLI.md`, add the `audit run` subcommand reference.
- [ ] 6.3 In `docs/OPERATIONS.md`'s audits section, add a paragraph describing the on-demand trigger as a complement to cadence-based scheduling, and the cadence-interaction rule (an on-demand run shifts the next scheduled fire forward).

## 7. Spec delta

- [ ] 7.1 The ADDED requirement in `openspec/changes/chatops-on-demand-audit-trigger/specs/orchestrator-cli/spec.md` codifies: the verb syntax + substring rules, the queue-based scheduling-bypass mechanism, the CLI subcommand's daemon-vs-standalone dispatch, the de-duplication rule, the cadence-state update on queued runs, and the ack format with ETA.

## 8. Verification

- [ ] 8.1 `cargo test` passes (new + existing).
- [ ] 8.2 `openspec validate chatops-on-demand-audit-trigger --strict` passes.
- [ ] 8.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
