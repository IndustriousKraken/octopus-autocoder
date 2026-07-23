## 1. Classified failures

- [ ] 1.1 In `autocoder/src/cli/run.rs`, extend `ForkSetupFailure` with a transient/permanent class: identity-mismatch, underivable-URL, and unroutable-PAT failures are pre-classified permanent at their creation sites; creation-POST and reachability failures are classified via the shared `classify_recovery_failure` pattern set (unknown → transient); the 60s reachability timeout is classified transient.
- [ ] 1.2 Route permanent-classified failures through today's exact skip-plus-alert path; collect transient-classified ones separately for fork-pending task spawning. Remove the resolved half of the `TODO(a14)` comment.

## 2. Fork-pending task state

- [ ] 2.1 Spawn a polling task in a fork-pending state for each transient-classified repository: its iteration body re-attempts the full fork setup (probe → create-if-missing → identity check → reachability), does nothing else, logs a WARN per failed attempt, and posts a throttled chatops alert naming the repo as fork-pending.
- [ ] 2.2 On a successful attempt, log an INFO recovery notice and continue that same task as a normal polling task from the next step onward. A re-attempt that fails with a permanent-classified cause (e.g. the upstream got renamed while pending) flips to the permanent path: alert with the remedy hint and stop re-attempting (task exits the polling set as if skipped at startup).

## 3. Tests

- [ ] 3.1 Unit tests on classification: DNS/transport error → transient; 5xx → transient; reachability timeout → transient; identity mismatch / underivable URL / unroutable PAT → permanent; unknown pattern → transient.
- [ ] 3.2 Scripted-`ForkOps` tests: a transient failure spawns a fork-pending task whose next attempt succeeds and proceeds to normal work with an INFO notice; a permanent failure spawns no task and alerts with the reload remedy; a pending task hitting a permanent cause flips to the permanent path.
- [ ] 3.3 Alert behavior: fork-pending alerts throttle (no per-iteration spam); permanent alerts carry the restart-or-reload remedy unchanged.
- [ ] 3.4 Run the full `cargo test` suite; confirm the existing fork-setup scenarios (all-exist, create-success, identity mismatch, unparseable body, every-repo-fails) pass with their new classifications.
