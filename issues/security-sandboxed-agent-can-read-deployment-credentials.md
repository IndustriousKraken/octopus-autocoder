# Sandboxed agent can read the deployment's credentials (secrets.env, inherited env, ANTHROPIC_ passthrough)

## Symptom

The project invariant is that a sandboxed executor/gate/reviewer agent — which
processes UNTRUSTED repository content and is therefore prompt-injectable — must
NOT be able to read the deployment's credentials (GitHub PAT, LLM API keys,
chatops bot token). Three independent paths currently break that invariant. All
three run at the same uid as the daemon and are exploitable by any agent that
can run a shell command (i.e. any prompt-injected executor).

## Why

**1. `secrets.env` is not masked (mechanism-independent, highest priority).**
`cli/run.rs:262-263` builds `own_secret_paths = vec![canonicalize(config_path)]`
— it masks ONLY `config.yaml`. The installer writes a *sibling* file
`<config_dir>/secrets.env` (mode 0600, owner = daemon user) containing the raw
values (`cli/install.rs:1557-1572` `assemble_secrets_env`:
`GITHUB_TOKEN=<PAT>`, `<chatops>=<token>`, `<reviewer>=<api key>`), and server
mode loads it into the daemon env via `EnvironmentFile=/etc/autocoder/secrets.env`.
Under the executor denylist policy, `$HOME` is bound read-write and `/` is bound
read-only-but-readable (`sandbox.rs:617-653`), so the agent can
`cat ~/.config/autocoder/secrets.env` (dev) or `cat /etc/autocoder/secrets.env`
(server) and print every token. Note the irony: the masked `config.yaml` holds
only env-var *names* by default, while the unmasked `secrets.env` holds the
*values*. The sandbox already supports masking arbitrary own-secret paths (the
code comments at `sandbox.rs:192/1022/1121/1255` even claim "config + secrets
paths, always masked") — the capability exists, it just isn't wired to
`secrets.env`.

**2. bwrap / sandbox-exec inherit the daemon's full environment.**
`bwrap_argv` (`sandbox.rs:602-720`) emits no `--clearenv`; its own doc comment
states "bwrap inherits the caller's environment". `wrap_command` /
`sandbox_exec_argv` (`sandbox.rs:868-895, 852-862`) never call `env_clear()`.
tokio `Command` inherits the parent env by default. So under bwrap or
sandbox-exec, every secret in the daemon process env — anything sourced via
`SecretSource::EnvVar` (which requires the value in the daemon env), anything the
operator exported, and the `EnvironmentFile` values from (1) — is inherited by
the agent. `printenv` prints `GITHUB_TOKEN`, `ANTHROPIC_API_KEY`, etc. This also
bypasses the `agent_env.rs` `CredentialFilter`, which only scrubs the captured
login-shell env, not the daemon's own inherited process env.

**3. systemd-run `ANTHROPIC_` passthrough prefix also matches `ANTHROPIC_API_KEY`.**
`SYSTEMD_ENV_PASSTHROUGH_PREFIXES = ["ORCH_", "XDG_", "ANTHROPIC_"]`
(`sandbox.rs:462`); `should_passthrough` forwards any daemon-env var starting
with those prefixes (`sandbox.rs:464-469, 578-589`). It is intended to forward
`ANTHROPIC_BASE_URL`/`ANTHROPIC_MODEL`, but the prefix also matches
`ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN`. So even under systemd-run (the
preferred Linux mechanism, which otherwise gives a clean env), an Anthropic key
in the daemon env leaks to the agent.

Impact: a prompt-injected agent (malicious repo content driving the executor)
exfiltrates the GitHub PAT and provider keys over the open egress the sandbox
allows by design. The PAT grants push → repo/pipeline compromise. Backend
credential-exposure severity.

## Tasks

- [ ] In `cli/run.rs:262`, add the deployment's secret/state paths to
  `own_secret_paths`: at minimum `config_dir/secrets.env` (canonicalized), and
  ideally mask the entire config directory. Confirm the mask applies across
  systemd-run, bwrap, and sandbox-exec (the masking path at `sandbox.rs:1121` is
  mechanism-independent).
- [ ] Give bwrap and sandbox-exec the same curated env systemd-run already gets:
  add `--clearenv` to `bwrap_argv` then re-inject only the passthrough allowlist;
  `cmd.env_clear()` in `wrap_command` before setting the allowlist for the
  sandbox-exec path. Run every forwarded var through `CredentialFilter::is_credential`.
- [ ] Replace the `ANTHROPIC_` passthrough prefix with an explicit non-secret
  name allowlist (`ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL`), or filter each
  forwarded name through `CredentialFilter::is_credential` before `--setenv`.
- [ ] Also mask the daemon's resolved state/cache/runtime/logs dirs (they sit
  under the RW `$HOME` in the XDG/dev deployment; see the companion
  control-socket issue for why writable daemon state matters).

## Tests

- [ ] A sandbox plan built for an executor role masks `secrets.env` (assert the
  resolved mask set contains the secrets path) — the `/dev/null` shadow renders
  it unreadable.
- [ ] The composed bwrap argv contains `--clearenv`; the composed child env for
  bwrap/sandbox-exec contains none of a set of seeded fake secrets
  (`GITHUB_TOKEN`, `ANTHROPIC_API_KEY`) present in the parent env.
- [ ] `should_passthrough("ANTHROPIC_API_KEY")` is `false`;
  `should_passthrough("ANTHROPIC_BASE_URL")` is `true`.
