# Tasks

## 1. Plan carries extra read-only binds

- [x] 1.1 Add `pub extra_ro_paths: Vec<PathBuf>` to `SandboxPlan` (sandbox.rs) — additional host paths to bind read-only into the child namespace. Initialize it empty in `build_plan` / `build_plan_with_home`.

## 2. Each mechanism binds them after masking

- [x] 2.1 `systemd_run_argv`: after the policy match, emit `BindReadOnlyPaths=<p>` for each `extra_ro_paths` entry (systemd applies binds after `PrivateTmp` / `ProtectHome`).
- [x] 2.2 `bwrap_argv`: after the `--tmpfs /tmp` line (and thus after the policy's home `--tmpfs`), emit `--ro-bind-try <p> <p>` for each entry, so a `/tmp`- or home-resident socket is re-exposed over the masking tmpfs.
- [x] 2.3 `seatbelt_profile`: for each entry, allow read AND `network-outbound` to the literal path; in the allowlist (deny-default) arm emit the allows AFTER the home deny so they win (last-match-wins).

## 3. Thread the control socket into the plan

- [x] 3.1 In `agentic_run`, where the `SandboxPlan` is built for the wrapped spawn, push the resolved control-socket path into `plan.extra_ro_paths` when `ORCH_DAEMON_CONTROL_SOCKET` is set AND non-empty; add nothing when unset.

## 4. Tests (sandbox.rs)

- [x] 4.1 `systemd_run_argv` with a populated `extra_ro_paths` emits a `BindReadOnlyPaths=<sock>` — under both denylist and allowlist plans.
- [x] 4.2 `bwrap_argv` emits `--ro-bind-try <sock> <sock>`, positioned AFTER `--tmpfs /tmp` (assert index ordering) — under both policies.
- [x] 4.3 `seatbelt_profile` (allowlist) contains an allow for the socket path placed after the home deny.
- [x] 4.4 An empty `extra_ro_paths` adds no extra bind in any mechanism.

## 5. Tests (agentic_run.rs)

- [x] 5.1 With `ORCH_DAEMON_CONTROL_SOCKET` set, the plan handed to the wrapper carries the socket in `extra_ro_paths`; with it unset, `extra_ro_paths` is empty. (Extract the threading into a small testable helper if the spawn path is not directly assertable.)

## 6. Acceptance

- [x] 6.1 `cargo test` passes.
- [x] 6.2 `openspec validate sandbox-binds-control-socket --strict` passes.
