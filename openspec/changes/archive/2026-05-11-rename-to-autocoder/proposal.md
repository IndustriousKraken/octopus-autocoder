## Why

After shipping five OpenSpec changes that established the full daemon, three operator-experience issues are visible from a fresh-eyes read:

1. **The binary name "orchestrator" is generic and overlong.** It's overloaded by a hundred unrelated CI/Kubernetes/data-pipeline products and types as ~11 characters every time. It does not describe what this tool actually does, which is **autonomously write code**.
2. **The README has accreted by-change rather than by-operator.** Deployment information is scattered across "Configuration", "CLI Usage", "Deployment Guide", "Security", and inline notes. There is no single linear "clone → run" walkthrough.
3. **`config.example.yaml` baits operators with API-key fields for providers they don't have set up.** The realistic install on a deploy host is **Claude Code authenticated with `claude auth login`**, which needs no API key in the config at all. The example should privilege that path and treat alternative providers as advanced material.

This change addresses all three together so the operator experience improves coherently in one pass.

## What Changes

- **Rename the binary** from `orchestrator` to `autocoder`. This changes `Cargo.toml`, the clap `#[command(name = ...)]`, the GitHub User-Agent header, the MCP server's `serverInfo.name`, and every doc/example invocation.
- **Restructure the README** with a "Quick Start" section as the first thing an operator reads, followed by reference material. Deployment instructions consolidate into one section.
- **Rewrite `config.example.yaml`** to be a Claude-Code-first working example: single-repo by default, no API key fields visible, reviewer and Slack blocks commented out with pointers to the relevant README sections.
- **Update the deployment guide** to name the systemd unit `autocoder.service` and document the Claude Code authentication step on the deploy host.

## Capabilities

### Modified Capabilities
- `orchestrator-cli`: the user-facing command name changes from `orchestrator` to `autocoder`. The capability's directory keeps the name `orchestrator-cli` for now (see design.md non-goals); only the command-line invocation text in the spec changes.

### Unchanged
All other capabilities — only the externally-typed command name and the prose around it change.

## Impact

After this change lands, operators invoke `autocoder run --config <path>` instead of `orchestrator run ...`. The systemd unit installed by the deployment guide is named `autocoder.service`. The README's first section walks a new operator from `git clone` through a running daemon in linear order. The default `config.example.yaml` runs on the operator's machine without any API-key fiddling, assuming `claude` is on PATH and a GitHub PAT is exported.

Existing installations need to migrate by replacing their systemd unit and any PATH references — there is no in-binary backward-compatibility alias. Operators who want one can `ln -s` the new binary themselves.
