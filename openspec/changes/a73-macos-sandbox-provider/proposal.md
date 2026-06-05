## Why

`a006` made the OS-level sandbox load-bearing and fail-closed, but its mechanisms — `systemd-run` and `bwrap` — are Linux-only. On macOS neither exists, so the mechanism gate fails closed and the daemon refuses to run any agentic subprocess. A Mac (e.g. a Mac mini) is genuinely useful as an autocoder host for compile-heavy projects, so macOS should be a supported platform rather than a forced choice between Linux-only and `allow_unsandboxed`.

macOS has a native analog: **`sandbox-exec`** (the Seatbelt sandbox), which applies a generated profile enforcing a filesystem allowlist. This change adds it as the macOS mechanism so the same credential/filesystem isolation holds there. It is low priority — end of the queue.

## What Changes

**`sandbox-exec` becomes the macOS sandbox mechanism.** The sandbox is now selected by a platform-appropriate mechanism: Linux keeps `systemd-run` (primary) / `bwrap` (fallback); macOS uses `sandbox-exec` with a generated Seatbelt profile that enforces the same **filesystem allowlist** (workspace read-write for the executor / read-only for read-only roles; the running role's own CLI config store read-only; everything else denied) and the platform-equivalent operation restrictions (deny process inspection of other processes, raw/packet networking, and privilege elevation, where macOS exposes them). `sandbox-exec` ships with macOS, so no install is required.

**The mechanism gate generalizes.** The fail-closed gate that named "neither `systemd-run` nor `bwrap`" becomes "no platform-appropriate mechanism" — on Linux that is still systemd-run/bwrap; on macOS it is `sandbox-exec`. A macOS host is therefore normally satisfied and runs sandboxed instead of failing closed. The `os_hide` / `engine_deny` toggles, the egress-out-of-scope posture, and the `allow_unsandboxed` opt-in all carry over unchanged.

`sandbox-exec` is deprecated-but-functional on current macOS and is the v1 mechanism; a future change may move to a newer macOS sandboxing surface if Apple removes it.

## Impact

- **Affected specs:** `executor` — MODIFY `Every agentic subprocess runs inside an OS-level sandbox` (platform-appropriate mechanism + macOS scenarios). `orchestrator-cli` — MODIFY `Sandbox credential-protection config — toggles, precedence, and relaxed-posture logging` (generalize the mechanism gate).
- **Affected code:** a platform-mechanism abstraction in `agentic_run.rs` that selects Linux (systemd-run/bwrap) vs macOS (sandbox-exec); a macOS provider that generates a Seatbelt profile from the per-role filesystem allowlist (`os_hide`-derived store set included) and runs `sandbox-exec -f <profile> -- <cli>`, preserving the existing stdout/stderr capture, process-group, and timeout handling; the mechanism-availability gate generalized to platform-appropriate.
- **Operator-visible behavior:** a macOS host runs agentic subprocesses sandboxed via `sandbox-exec` instead of failing closed; no extra install on macOS. Linux behavior is unchanged (still needs `bwrap` when the daemon is unprivileged).
- **Dependencies:** extends the archived `a006` (canonical). No new external dependency on macOS (`sandbox-exec` is part of the OS). Independent of other queued work; low priority (end of queue).
- **Acceptance:** `cargo test` passes; `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a73-macos-sandbox-provider --strict` passes. Tests: the generated Seatbelt profile is derived from the per-role allowlist (workspace rw/ro, own store ro, rest denied) — a derivation test, cross-platform; platform selection picks `sandbox-exec` on macOS and systemd-run/bwrap on Linux; the mechanism gate is satisfied on macOS (no fail-closed); on-host enforcement (an out-of-allowlist read denied, a read-only role's workspace write denied) is validated on a macOS host.
