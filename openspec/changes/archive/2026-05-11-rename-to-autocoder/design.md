## Context

Five OpenSpec changes have accumulated user-facing surface area (CLI commands, config files, environment variables, README sections, deployment scripts). Each change added its own documentation and config examples in isolation. The accretion left UX issues that were not visible inside any single change's scope but are obvious to a fresh-eyes operator: scattered deployment info, misleading provider examples, and an awkward binary name. This change does the cleanup pass in a single coherent edit.

## Goals / Non-Goals

**Goals:**
- An operator can clone the repo, follow the README's first section linearly, and have a running daemon in ~10 minutes without scrolling.
- The default `config.example.yaml` runs on the operator's machine without API-key fiddling, assuming Claude Code is installed and authenticated and a GitHub PAT is exported.
- `autocoder` is the only binary name appearing in default config, deployment guidance, and CLI examples. The string "orchestrator" disappears from operator-facing surfaces.
- Existing automated tests continue to pass — this is a rename, not a behavior change.

**Non-Goals:**
- **Renaming capability directories** under `openspec/specs/`. The capability names (`orchestrator-cli`, etc.) are internal OpenSpec graph identifiers; operators never see or type them. Renaming directories would require either a "move capability" OpenSpec primitive that does not exist, or a verbose REMOVED-from-A + ADDED-to-B dance for every Requirement. Out of scope for this change; can be done in a follow-on if it ever causes confusion.
- **Backward-compatibility alias from `orchestrator` to `autocoder`.** Users running an old version migrate by replacing their systemd unit. The orchestrator has not been deployed to production yet, so there is no installed-base concern.
- **Splitting the README into multiple files** (e.g. a separate `docs/quickstart.md`). A single well-structured README is fine.
- **Changing any behavior.** This is a surface-level rename + documentation restructure. Tests should not change beyond updating string-matching assertions for the User-Agent header and MCP `serverInfo.name`.

## Decisions

- **Binary name: `autocoder`.** Descriptive (it autonomously writes code), short (9 chars vs 12), unlikely to conflict in this niche, and the user explicitly endorsed it. The Cargo.toml `[package].name` field changes from `orchestrator` to `autocoder`; cargo derives the binary name from that.
- **User-Agent header: `openspec-autocoder`.** The `openspec-` prefix is retained so GitHub-side log filtering operators may have set up continues to pick up these requests. The User-Agent identifies the requesting application, not just the binary name.
- **MCP `serverInfo.name`: `autocoder-ask-user`.** Parallels the binary name; the existing format (`<binary>-<role>`) is kept so Claude Code logs are readable when the MCP server appears in them.
- **systemd unit name: `autocoder.service`.** Simpler than `openspec-autocoder.service`; the OpenSpec context is clear from the install path (`/etc/systemd/system/autocoder.service`) and from the unit's `WorkingDirectory`. The deployment guide should document this naming convention.
- **README structure (top → bottom):**

  ```
  1. Title + one-paragraph intro (what it is)
  2. Quick Start  ← NEW. Linear walkthrough.
     - Prerequisites
     - Configure GitHub PAT and Claude Code
     - Write config.yaml
     - Build + run
     - Verify with a sandbox
  3. Configuration reference
  4. Architecture (capability enumeration, post-implementation)
  5. Optional capabilities
     - ChatOps Escalation
     - Code Review
  6. Operating notes
     - Workspace path derivation
     - Multi-repo setup
  7. Deployment guide (systemd, env vars, log paths)
  8. AI Security & Guardrails
  9. CLI reference (subcommand summaries)
  10. Status & Roadmap
  ```

  Quick Start is the only addition; the rest is consolidation of existing material into a fixed order. Quick Start references the other sections by anchor, so an operator who wants details can navigate; one who just wants the daemon running can stop at the end of Quick Start.

- **Config example simplification.** The Claude-Code-first canonical example:

  ```yaml
  repositories:
    - url: "git@github.com:your-org/your-repo.git"
      base_branch: main
      agent_branch: agent-q
      poll_interval_sec: 300

  executor:
    kind: claude_cli
    # `command: claude` is the default; uncomment only to point at a wrapper script.
    # command: claude
    timeout_secs: 1800

  github:
    token_env: GITHUB_TOKEN

  # Optional: AI code-quality reviewer. See README's "Code Review" section.
  # reviewer:
  #   enabled: true
  #   provider: anthropic
  #   model: claude-sonnet-4-6
  #   api_key_env: ANTHROPIC_API_KEY

  # Optional: Slack ChatOps escalation. See README's "ChatOps Escalation" section.
  # slack:
  #   bot_token_env: SLACK_BOT_TOKEN
  #   default_channel_id: C0123456789
  ```

  Key changes versus the current file:
  - Single repo by default. Multi-repo is shown later in the "Operating notes" section, not in the example file.
  - No visible `api_key_env` fields. Operators who haven't enabled reviewer or Slack never see the API-key bait.
  - The executor block doesn't show `command:` by default — it's documented as a comment so operators can find the override when they need it.

- **Claude Code authentication on the deploy host.** The Quick Start documents `claude auth login` as a prerequisite alongside Rust toolchain install and the GitHub PAT. The Claude Code creds live in `~/.claude/` and survive a systemd restart as long as the unit's `User=` is the same account that ran `claude auth login`.

## Risks / Trade-offs

- **Risk: Operators on an old version hit "command not found" after upgrading.**
  - **Mitigation:** No installed base yet — this is a pre-deploy rename. The deployment guide documents the rename explicitly for any future upgrade.
- **Risk: Capability directory name (`orchestrator-cli`) diverges from binary name (`autocoder`).**
  - **Mitigation:** Capability names are internal to OpenSpec's graph and never user-visible. Operators read `autocoder --help`, never `cat openspec/specs/orchestrator-cli/spec.md`. The divergence is documented in this change's design.md (non-goals). A follow-on change can rename the capability directory if it ever causes confusion.
- **Risk: `autocoder` clashes with another tool on the deploy host.**
  - **Mitigation:** Document the install path (`/usr/local/bin/autocoder`) and note that operators with a conflicting binary should install with a different filename via the `mv` step in the deployment guide.
- **Risk: Existing tests fail due to hardcoded "orchestrator" strings.**
  - **Mitigation:** Two specific test assertions reference the User-Agent and MCP `serverInfo.name`. Both get updated in lockstep with the production code change. All other tests are behavior-level and indifferent to the binary name.
