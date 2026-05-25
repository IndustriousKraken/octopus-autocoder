# autocoder

**autocoder** is an autonomous daemon that reads OpenSpec implementation proposals from one or more configured repositories, drives an AI coding agent (the Claude CLI by default) through each change in serial order, and opens monolithic Pull Requests for human review. It's "OpenSpec change at the top, working code in a PR at the bottom" wired into a single long-running process.

---

## Quick install

```bash
curl -fsSL https://raw.githubusercontent.com/IndustriousKraken/openspec-autocoder/main/install.sh | bash
```

The one-liner downloads a pre-built binary, verifies its SHA-256, places it at `/usr/local/bin/autocoder` (or `~/.local/bin/autocoder` if `sudo` is unavailable or `--user` is passed), then execs `autocoder install`. **The bootstrap script is intentionally tiny (~75 lines, no operator prompts).** Everything else — the configuration wizard, `useradd`/`systemctl`/`apt-get`, optional Claude CLI bootstrap — lives in the `autocoder install` Rust subcommand which ships with `cargo test` coverage.

By default `autocoder install` picks **server mode** on Linux when systemd is detected (`/run/systemd/system` present): it creates an `autocoder` system user, writes `/etc/autocoder/config.yaml` + `/etc/autocoder/secrets.env`, renders `/etc/systemd/system/autocoder.service`, and offers to start it. Otherwise it picks **dev mode** and writes to `~/.config/autocoder/` instead, with no system-user / systemd work. Either mode can be forced with `--mode server` or `--mode dev`.

For automation (Ansible, Terraform, cloud-init), pass `--non-interactive` along with `--repo-url`, `--token-env-var`, `--chatops-backend`, and `--reviewer-provider`. Anything after `--` on the `install.sh` command line is forwarded to the subcommand:

```bash
curl -fsSL .../install.sh | bash -s -- --non-interactive \
  --repo-url git@github.com:acme/widgets.git \
  --token-env-var GITHUB_TOKEN \
  --chatops-backend none \
  --reviewer-provider none
```

Prefer to build from source instead? See [docs/INSTALL.md](docs/INSTALL.md).

### Periodic audits during install

The wizard asks about periodic audits before writing `config.yaml`. The five LLM-driven audits — `architecture_brightline`, `architecture_consultative`, `drift_audit`, `missing_tests_audit`, `security_bug_audit` — are gated behind a single `[y/N]` question so operators who want to defer can answer "n" and move on. Operators who accept the gate get a fast-path prompt that enables all five at recommended cadences, falling back to an individual cadence walk-through if they decline the fast path.

For non-interactive installs, the same configuration is available via `--audits-spec-sync <disabled|daily|weekly|monthly>` (defaults to `daily`), `--audits-llm-driven <none|recommended|all-disabled>` (defaults to `none`), and per-audit `--audit-<slug> <cadence>` overrides. A `--non-interactive` invocation that passes none of the `--audits-*` flags inherits the conservative default (spec-sync daily; everything else disabled), so IaC scripts that pre-date this wizard step keep working without surprise behavior changes. See [docs/CONFIG.md#audits-optional](docs/CONFIG.md#audits-optional) for cadence syntax and the `extra` knobs each audit reads.

### Reinstalling / upgrading

Re-running `install.sh` downloads the latest binary (or a specific tag — pass `--version vX.Y.Z` to the script or set `AUTOCODER_VERSION=vX.Y.Z` in the environment), verifies the checksum, and replaces `/usr/local/bin/autocoder`. The subsequent `autocoder install` detects the existing `config.yaml` and exits 0 without re-prompting: the operator's choices made on first run are preserved across binary upgrades. To force the wizard back on (e.g. to relocate the config), pass `--upgrade` after `--`.

---

## Architecture

autocoder is a single tokio-based daemon with one polling task per configured repository. Each iteration follows a fixed workflow: fetch + branch init → process waiting (escalated) changes → process pending changes → push + PR if any commits were produced. The serial-per-repo invariant guarantees that change B does not run while change A is mid-flight or waiting on human input.

Built capabilities (each is a baseline spec under `openspec/specs/`):

1. **orchestrator-cli** — the `run` daemon entry point and the `rewind` recovery subcommand. Multi-repo dispatch with a shared cancellation token; per-repo polling tasks; SIGINT/SIGTERM drain.
2. **workspace-manager** — deterministic per-repo workspace paths under `/tmp/workspaces/`, idempotent clone-or-fetch, startup-time cross-repo collision detection, and a startup dirty-workspace check that skips a dirty repo for the process lifetime.
3. **openspec-queue-engine** — enumerate (pending + waiting), lock/unlock via `.in-progress` markers, stale-lock cleanup at startup, archive on completion with `YYYY-MM-DD-<change>` date prefix, unarchive on rewind.
4. **executor** — backend-agnostic `Executor` trait with `Completed` / `AskUser` / `Failed` outcomes plus a `resume()` entry point. First concrete backend is `ClaudeCliExecutor`, which wraps the `claude` CLI as a subprocess with a configurable timeout and two-layer `AskUser` detection (an MCP-tool marker file plus a stdout-regex backstop).
5. **git-workflow-manager** — branch init (`fetch → checkout base → pull --ff-only → checkout -B agent`), per-change commits with `<change>: <first line of ## Why>` subject truncated to 72 chars, monolithic PR creation via the GitHub REST API with `--force-with-lease` push.
6. **chatops-manager** — chat-platform escalation behind a `ChatOpsBackend` trait. Slack is the officially-supported provider; Discord, Teams, Mattermost, and Matrix are [experimental backends](docs/CHATOPS.md#experimental-chatops-backends) with no API-stability guarantees. On `AskUser`, the daemon posts a question to a configured channel and persists `.question.json` to disk. On the next iteration it polls the thread; when the first non-bot reply arrives it writes `.answer.json` and resumes the executor. Same-repo serial-queue invariant is preserved: any waiting change in a repository blocks all pending-change processing for that repo until resolved.
7. **code-reviewer** — opt-in AI code-quality review of the diff between base and agent branches. Configurable LLM provider (Anthropic or any OpenAI-compatible endpoint, including Grok, OpenRouter, local Ollama). A `Block` verdict creates the PR as a draft (with a `do-not-merge` label fallback on hosts that reject drafts).

The default executor backend wraps `claude` as a subprocess. The daemon writes a per-workspace `.mcp.json` pointing at itself as an MCP server exposing the `ask_user` tool; when the agent calls it, a marker file is written and the daemon picks it up after the child exits. The MCP server is hosted as a hidden subcommand of the autocoder binary, so deployment is a single-binary install.

---

## Documentation

Everything beyond the quick install lives under [`docs/`](docs/). The index there has one-line summaries for each file. Direct links:

- [Manual install from source](docs/INSTALL.md)
- [Configuration reference](docs/CONFIG.md) (full `config.yaml` schema, multi-token routing)
- [ChatOps escalation](docs/CHATOPS.md) (Slack setup, operator commands, inbound listener, experimental backends)
- [Code review](docs/CODE-REVIEW.md) (the optional AI reviewer's scope and prompt template)
- [Operating notes](docs/OPERATIONS.md) (workspace layout, queue order, recovery flows, audits, rebuilding canonical specs)
- [Deployment](docs/DEPLOYMENT.md) (binary install, systemd unit, SSH keys, upgrades)
- [Security & guardrails](docs/SECURITY.md) (credentials, branch protection, self-modification, sandbox)
- [CLI reference](docs/CLI.md) (`run`, `reload`, `rewind`)
- [Troubleshooting](docs/TROUBLESHOOTING.md) (rebuild failures and other diagnostic flows)

---

## Status & Roadmap

The seven capabilities listed under [Architecture](#architecture) are all **implemented and tested**. autocoder runs end-to-end against real GitHub repositories with the Claude CLI as executor and (optionally) Slack as the officially-supported escalation channel. The four experimental ChatOps backends (Discord, Teams, Mattermost, Matrix) compile and have unit-test coverage against recorded fixtures but no live-service validation; operators who deliberately select one are the ones surfacing bugs.

The following capabilities are **explicitly aspirational** — referenced in design documents but not built:

- **Verifier** *(planned; not in any active change)*: a spec-audit step that runs alongside the code reviewer and asks "did the diff actually implement the spec?" The reviewer agent currently focuses on code quality and explicitly does not assess spec compliance. Until the verifier ships, spec correctness is a human-review concern.
- **Drift audit** *(planned; not in any active change)*: a periodic whole-repo verification that catches gradual divergence between the baseline `openspec/specs/` and the code. Until this ships, the per-change architecture cross-reference (run once at change-archive time) is the closest equivalent.

Other items deferred without a current owner:

- **Multi-instance distributed deployment.** autocoder assumes single-instance ownership of each configured workspace; running two daemons against the same `local_path` would race. Out of scope for the current architecture.
- **Per-repo executor configuration overrides.** The `executor:` block is global; mixing Claude on one repo and a different backend on another in the same config is not supported.
- **Streaming or incremental code review.** The reviewer sends the full diff in one LLM call; truncation at 100k chars is documented in `prompts/code-review-default.md`.

To request an aspirational item, file an issue or open an OpenSpec change proposal in this repository. Self-modification guardrails apply when autocoder works on its own codebase; see [docs/SECURITY.md](docs/SECURITY.md).

---

## License

Licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or https://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

---

*Documentation maintained per the `project-documentation` OpenSpec rule. Any new capabilities or operational shifts must be updated here in the same change that introduces them.*
