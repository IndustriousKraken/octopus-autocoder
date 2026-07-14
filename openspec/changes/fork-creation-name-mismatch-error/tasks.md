## 1. Forge: return the created fork's identity

- [ ] 1.1 Change `create_fork_at` in `autocoder/src/forge/github.rs` to parse the 2xx response body and return the fork's `full_name` as `Option<String>` (`None` when the body yields no identity); propagate the new return type through `create_fork` and the test re-export.
- [ ] 1.2 Extend the mockito coverage for fork creation: a 2xx with a `full_name` body returns that identity; a 2xx with an empty or shapeless body returns `None`; non-2xx behavior is unchanged.

## 2. Startup: verify identity before polling

- [ ] 2.1 Update the `ForkOps` trait and `GitForkOps` in `autocoder/src/cli/run.rs` so `create_fork` surfaces the returned identity to the caller.
- [ ] 2.2 In `ensure_forks_exist_with`, after a successful creation call, derive the expected owner/name pair from the fork URL and compare it case-insensitively against the returned `full_name`. On mismatch, record a `ForkSetupFailure` immediately whose cause names the actual fork, the expected fork, and the rename remedy — and skip the reachability poll for that repository. On match or `None`, keep the existing poll behavior byte-for-byte.

## 3. Alert remedy text

- [ ] 3.1 Update `fork_setup_failure_alert_message` so the remedy hint reads "restart autocoder or run `autocoder reload` on the daemon host" instead of referring to a bare `reload` verb; adjust the existing message-shape unit test.

## 4. Tests for the new decision path

- [ ] 4.1 Scripted-`ForkOps` unit test: identity mismatch produces exactly one failure whose cause names both fork identities and the rename remedy, performs zero reachability probes, and leaves other repositories' setup untouched.
- [ ] 4.2 Scripted-`ForkOps` unit tests: matching identity proceeds to the poll as today; `None` identity proceeds to the poll; case-only difference in `full_name` counts as a match.
- [ ] 4.3 Run the full `cargo test` suite and confirm the existing fork-setup scenarios (POST failure, unreachable-within-timeout, all-repos-fail) still pass unchanged.
