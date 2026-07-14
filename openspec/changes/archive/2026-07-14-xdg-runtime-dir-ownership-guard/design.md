## Context

`runtime_default_from` in `autocoder/src/paths.rs` uses `XDG_RUNTIME_DIR` verbatim: if set, the runtime path is `$XDG_RUNTIME_DIR/autocoder`, with no check that the directory belongs to the current user. Environment-preserving user switches (`su autocoder` from an operator's session) leak the operator's `XDG_RUNTIME_DIR=/run/user/<operator-uid>` into the new shell, so every path-resolving CLI subcommand (notably `reload`) targets a directory the autocoder user cannot traverse (0700, foreign-owned) and dies with "Permission denied" — while `sudo -u autocoder`, which scrubs the environment, works. Observed in production on 2026-07-13 while remediating a fork-setup failure. systemd and tmux both refuse a runtime dir not owned by the invoking user, for exactly this reason; the XDG Base Directory spec requires the runtime dir to be user-owned.

## Goals / Non-Goals

**Goals:**
- Make the implicit `XDG_RUNTIME_DIR`-derived runtime default self-correct when the variable is a leaked foreign value, so CLI and daemon converge on the same socket path regardless of how the operator reached the daemon's user.
- Teach the reload connection-failure hint about the stale-variable cause.

**Non-Goals:**
- Ownership checks on explicit overrides (`config.yaml` paths, `AUTOCODER_RUNTIME_DIR`, systemd's `$RUNTIME_DIRECTORY`) — an operator who names a path explicitly gets that path; explicit configuration is the escape hatch.
- Multi-candidate socket discovery (trying several likely paths) — one deterministic resolution, made correct, is simpler than a search.
- Non-Unix support; the daemon is Linux-only.

## Decisions

- **Guard the implicit inference only.** The ownership check lives where `XDG_RUNTIME_DIR` is read for the runtime default (`xdg_runtime_default` feeding `runtime_default_from`), not in the generic precedence walk. Steps (1)–(3) are deliberate operator statements; step (4) is an inference from ambient environment, which is the only place a leaked value can enter.
- **Check: `stat` the directory, compare `st_uid` to the process's effective uid.** One `rustix`/`std::os::unix` metadata call. Any failure to inspect (ENOENT, EACCES on a parent) is treated the same as foreign ownership: ignore the variable, fall back. Both outcomes log the reason at WARN in the daemon and quietly at debug for short-lived CLI invocations — the CLI's actionable surface is the connect-error hint, not resolver noise. Keep `runtime_default_from` pure by passing the ownership verdict (or a probe closure) in from `xdg_runtime_default`, preserving the existing env-free unit-test pattern.
- **Fallback is the existing no-variable branch** (`runtime/` under the XDG state default) — no new path shapes are introduced.
- **Hint text gains one cause, one suggestion.** The reload error already suggests `sudo -u autocoder autocoder reload`; it now also names the stale-`XDG_RUNTIME_DIR` cause so the operator understands *why* the clean-environment retry works instead of finding it mysterious.

## Risks / Trade-offs

- [A deployment that intentionally points `XDG_RUNTIME_DIR` at a foreign-owned directory changes behavior] → Such a setup violates the XDG spec and could not have worked for socket creation (0700 foreign dir); the explicit `AUTOCODER_RUNTIME_DIR` override remains available and unguarded.
- [Root invocations: root can traverse foreign dirs, so today "works" accidentally when root inherits a user's variable] → The guard makes root fall back too (root's euid ≠ dir owner), which is the correct, deterministic choice — root should reach the daemon via `sudo -u <daemon-user>` as documented, and the hint says so.
- [Behavior difference between daemon and CLI if only one is rebuilt] → Both resolve through the same `paths.rs` helper; the change ships atomically in one binary.
