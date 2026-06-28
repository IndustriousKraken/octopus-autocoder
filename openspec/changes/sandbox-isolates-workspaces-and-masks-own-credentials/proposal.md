# Sandbox isolates sibling workspaces and masks the daemon's own credentials

## Why

The OS-level sandbox's default policy is the exposed-home denylist: `$HOME` is
present and (for the executor) read-write, except a mask-list of sensitive paths.
Two gaps in that policy are exploitable:

1. **The daemon's own credentials are not masked.** The mask-list
   (`DEFAULT_MASK_RELATIVE`) covers third-party credentials (`~/.ssh`, `~/.aws`,
   `~/.config/gh`, `~/.npmrc`, …) but NOT autocoder's own `config.yaml` or
   `secrets.env`, which hold the deployment's most sensitive credentials — the
   Anthropic key, the GitHub PAT, reviewer/LLM provider keys, chatops tokens. The
   module doc even claims "autocoder's own config" is masked; it isn't. So a
   spawned agent can read the daemon's credentials with a plain `cat`, and —
   because home is read-write — edit or delete the config (including to disable
   its own sandbox). Operator config locations vary (`~/.config/autocoder/`,
   `~/autocoder/config.yaml`, `/etc/autocoder/`), so a hardcoded relative path
   cannot cover them.

2. **Sibling workspaces are exposed read-write.** Under the denylist the entire
   home is writable, including every other managed repository's workspace under
   `~/.cache/autocoder/workspaces/`. An agent's `bash` (e.g. a stray or
   mis-targeted `rm -rf`) can therefore delete or corrupt OTHER repos' in-flight
   work — potentially destroying hours of implementation across many repos at
   once. Reading sibling repos is useful (referring to a related project); writing
   or deleting them is never intended.

## What Changes

Both gaps are closed by tightening the denylist policy in the OS-level sandbox
requirement:

- **Mask the daemon's own config + secrets.** The daemon's RESOLVED config file
  path AND secrets file path (wherever the operator placed them, in or out of
  `$HOME`) SHALL be in the mask-list — read- AND write-protected — for every
  sandboxed role. Resolved at runtime from the loaded config, not a hardcoded
  location.
- **Sibling workspaces read-only, own workspace read-write.** Under the denylist,
  the executor's read-write home exposure SHALL NOT extend write access to OTHER
  managed repositories' workspaces: the workspaces-parent directory SHALL be bound
  read-only with ONLY the role's own workspace read-write. An agent may READ
  sibling repos but cannot modify or delete them.

This narrows the default denylist; it does not touch strict mode, egress, the
capability drops, or read-only-role workspace handling.

## Impact

- Closes the credential-exfiltration path (#4) and the cross-workspace data-loss
  blast radius (#5).
- Modifies `executor`: "Every agentic subprocess runs inside an OS-level sandbox".
- Cross-repo reads stay allowed (the convenient behavior); only cross-workspace
  writes/deletes and own-credential reads are removed.
- `#4` is intentionally promoted to a canon scenario (pinning "the daemon's own
  config + secrets are masked by default") rather than left as an implementation
  issue, so the guarantee is enforced by the verifier going forward.
