## 1. Resolver ownership guard

- [x] 1.1 In `autocoder/src/paths.rs`, add an ownership probe (stat the directory named by `XDG_RUNTIME_DIR`, compare its owner uid to the process's effective uid) and have `xdg_runtime_default` treat a foreign-owned or uninspectable directory as "variable unset", falling through to the `runtime/`-under-state-default branch. Keep `runtime_default_from` pure by injecting the probe result so existing env-free unit tests keep working.
- [x] 1.2 Log the ignored variable with the directory, its owner, and the reason (ownership mismatch vs. inspection error) — WARN when resolving for the daemon, debug for short-lived CLI invocations if the existing logging setup distinguishes them; otherwise a single WARN is acceptable.

## 2. Reload hint

- [x] 2.1 In `autocoder/src/cli/reload.rs`, extend the connection-failure hint to name the third likely cause — a stale `XDG_RUNTIME_DIR` inherited from an env-preserving user switch pointing at another user's runtime directory — while keeping the `sudo -u autocoder autocoder reload` suggestion.

## 3. Tests

- [x] 3.1 Unit tests for the guarded resolution: owned directory → `$XDG_RUNTIME_DIR/autocoder`; foreign-owned directory → state-default `runtime/` fallback; missing/uninspectable path → same fallback. Use a tempdir for the owned case; simulate the foreign/uninspectable cases through the injected probe (chown is unavailable in unprivileged test runs).
- [x] 3.2 Update the reload-CLI error-message test to assert the hint names the stale `XDG_RUNTIME_DIR` cause alongside the existing not-running / different-user causes.
- [x] 3.3 Run the full `cargo test` suite and confirm the existing path-precedence scenarios (config > env > systemd > XDG > hard fallback, relative-path rejection, same-path rejection) are unchanged.
