# Implementation tasks

## 1. Platform-mechanism selection

- [ ] 1.1 Add a sandbox-mechanism abstraction in `agentic_run.rs` that selects by platform: Linux → `systemd-run` (primary) / `bwrap` (fallback) as today; macOS → `sandbox-exec`.
- [ ] 1.2 Probe `sandbox-exec` availability on macOS at startup (it ships with the OS); feed the result into the mechanism-availability gate.

## 2. macOS provider (`sandbox-exec` / Seatbelt)

- [ ] 2.1 Generate a Seatbelt profile from the per-role filesystem allowlist: `(deny default)`; allow read on the runtime + the running role's own CLI config store; allow read on the workspace, and read-write on the workspace only for the executor (read-only roles get read-only). Derive the allowed CLI-store set from `os_hide` exactly as the Linux path does.
- [ ] 2.2 In the profile, deny the platform equivalents of the Linux capability drops where macOS exposes them: process-information access to other processes, raw/packet networking, and privilege elevation. Do NOT restrict outbound egress (out of scope).
- [ ] 2.3 Run the child as `sandbox-exec -f <profile> -- <cli> …`, preserving the existing stdout/stderr capture, process-group, and timeout/kill handling.
- [ ] 2.4 Write the generated profile to a per-invocation temp path outside the workspace and remove it after the child exits.

## 3. Mechanism gate

- [ ] 3.1 Generalize the fail-closed gate from "neither systemd-run nor bwrap" to "no platform-appropriate mechanism": Linux uses systemd-run/bwrap; macOS uses `sandbox-exec`. A macOS host with `sandbox-exec` is satisfied and runs sandboxed; the `allow_unsandboxed` opt-in is unchanged.

## 4. Docs

- [ ] 4.1 DEPLOYMENT.md: document macOS as a supported host (sandboxed via `sandbox-exec`, no install required; deprecated-but-functional, v1 mechanism), alongside the Linux note that an unprivileged daemon needs `bwrap`.

## 5. Tests

- [ ] 5.1 Derivation: the generated Seatbelt profile is derived from the per-role allowlist (workspace rw for executor / ro for read-only roles; own CLI store ro; everything else denied) — assert the profile content against the allowlist input, cross-platform.
- [ ] 5.2 Platform selection picks `sandbox-exec` on macOS and systemd-run/bwrap on Linux.
- [ ] 5.3 The mechanism gate is satisfied on macOS when `sandbox-exec` is available (no fail-closed); the Linux fail-closed path is unchanged.
- [ ] 5.4 On-host enforcement (macOS): an out-of-allowlist read is denied and a read-only role's workspace write is denied — validated on a macOS host.

## 6. Acceptance gate

- [ ] 6.1 `cargo test` passes for the autocoder crate.
- [ ] 6.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 6.3 `openspec validate a73-macos-sandbox-provider --strict` passes.
